//! Generic bounds on the `std.serde.json` protocol traits carry both the stdlib trait, which the protocol's methods
//! dispatch through, and the Rust serde capability the trait forwards to its adopters, under every import spelling
//! and in every position a bound is written (#1820).

use super::*;

/// The Rust serde capability a `Serialize` bound carries beside the stdlib trait, spelled from the crate root.
const SERDE_SERIALIZE: &str = "::serde::Serialize";

/// The Rust serde capability a `Deserialize` bound carries beside the stdlib trait, spelled from the crate root.
const SERDE_DESERIALIZE_OWNED: &str = "::serde::de::DeserializeOwned";

/// The stdlib `Serialize` trait as its method dispatch names it outside an SDK provider build.
const STDLIB_SERIALIZE_PATH: &str = "crate::__incan_std::serde::json::Serialize";

/// One spelling of `std.serde.json.Serialize`: the program that imports it and bounds `encode` and `stringify`, and
/// the bound as written.
struct SerializeSpelling {
    source: &'static str,
    written: &'static str,
}

/// The bare import, an alias and the module-qualified name, each bounding a function that calls `to_json` through the
/// bound and one that passes the bounded value to `json_stringify`.
const SERIALIZE_SPELLINGS: &[SerializeSpelling] = &[
    SerializeSpelling {
        source: r#"
from std.serde.json import Serialize

@derive(Serialize)
model Payload:
  value: int

def encode[T with Serialize](value: T) -> str:
  return value.to_json()

def stringify[T with Serialize](value: T) -> str:
  return json_stringify(value)

def main() -> str:
  return encode(Payload(value=1)) + stringify(Payload(value=1))
"#,
        written: "Serialize",
    },
    SerializeSpelling {
        source: r#"
from std.serde.json import Serialize as JsonSerialize

@derive(JsonSerialize)
model Payload:
  value: int

def encode[T with JsonSerialize](value: T) -> str:
  return value.to_json()

def stringify[T with JsonSerialize](value: T) -> str:
  return json_stringify(value)

def main() -> str:
  return encode(Payload(value=1)) + stringify(Payload(value=1))
"#,
        written: "JsonSerialize",
    },
    SerializeSpelling {
        source: r#"
from std.serde import json

@derive(json)
model Payload:
  value: int

def encode[T with json.Serialize](value: T) -> str:
  return value.to_json()

def stringify[T with json.Serialize](value: T) -> str:
  return json_stringify(value)

def main() -> str:
  return encode(Payload(value=1)) + stringify(Payload(value=1))
"#,
        written: "json.Serialize",
    },
];

/// Return the trait paths of each type parameter in `type_params`, in declaration order.
fn bound_paths(type_params: &[IrTypeParam]) -> Vec<Vec<String>> {
    type_params
        .iter()
        .map(|type_param| type_param.bounds.iter().map(|bound| bound.trait_path.clone()).collect())
        .collect()
}

/// Return the type parameters of the named model or class.
fn lowered_struct_type_params<'a>(ir: &'a IrProgram, name: &str) -> Result<&'a [IrTypeParam], String> {
    ir.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Struct(owner) if owner.name == name => Some(owner.type_params.as_slice()),
            _ => None,
        })
        .ok_or_else(|| format!("missing model or class `{name}`"))
}

/// Return the trait path the named function's trailing `return receiver.method(...)` dispatches through.
fn returned_dispatch_trait_path(ir: &IrProgram, function: &str) -> Result<String, String> {
    let function = lowered_function(ir, function)?;
    let Some(IrStmt {
        kind: IrStmtKind::Return(Some(returned)),
        ..
    }) = function.body.last()
    else {
        return Err(format!(
            "expected `{}` to end in a return, got {:?}",
            function.name, function.body
        ));
    };
    match &returned.kind {
        IrExprKind::MethodCall {
            dispatch: Some(IrMethodDispatch::Trait(dispatch) | IrMethodDispatch::SourceProjection(dispatch)),
            ..
        } => Ok(dispatch.trait_path.clone()),
        other => Err(format!(
            "expected a trait-dispatched call in `{}`, got {other:?}",
            function.name
        )),
    }
}

/// #1820: `T with Serialize` lowers to the stdlib trait as written plus `serde::Serialize` under the bare import, an
/// alias and the module-qualified name alike. The trait is what `value.to_json()` dispatches through; the serde
/// capability is what `json_stringify(value)` compiles against. The bare spelling used to lower to `serde::Serialize`
/// alone, which does not provide `to_json`, and the other two spellings to the trait alone, which does not provide
/// the serde capability.
#[test]
fn json_protocol_bounds_carry_the_trait_and_its_serde_capability_issue1820() -> Result<(), String> {
    for spelling in SERIALIZE_SPELLINGS {
        let ir = lower_checked_source(spelling.source)?;
        let expected = vec![vec![spelling.written.to_string(), SERDE_SERIALIZE.to_string()]];
        for function in ["encode", "stringify"] {
            assert_eq!(
                bound_paths(&lowered_function(&ir, function)?.type_params),
                expected,
                "`T with {}` on `{function}` must name the stdlib trait and the serde capability",
                spelling.written
            );
        }
        assert_eq!(
            returned_dispatch_trait_path(&ir, "encode")?,
            STDLIB_SERIALIZE_PATH,
            "the dispatch under `{}` names the stdlib trait the bound resolves to",
            spelling.written
        );
    }
    Ok(())
}

/// #1820: a `Deserialize` bound carries the stdlib trait, which `T.from_json(text)` dispatches through, and
/// `serde::de::DeserializeOwned`; the bare spelling used to lower to the serde capability alone.
#[test]
fn deserialize_bound_carries_the_trait_and_its_serde_capability_issue1820() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
from std.serde.json import Deserialize

@derive(Deserialize)
model Payload:
  value: int

def decode[T with Deserialize](text: str) -> Result[T, str]:
  return T.from_json(text)

def main() -> None:
  match decode[Payload]("{\"value\":1}"):
    Ok(payload) => println(payload.value)
    Err(error) => println(error)
"#,
    )?;
    assert_eq!(
        bound_paths(&lowered_function(&ir, "decode")?.type_params),
        vec![vec!["Deserialize".to_string(), SERDE_DESERIALIZE_OWNED.to_string()]]
    );
    Ok(())
}

/// #1820: the bound carries the serde capability on a generic model, a generic class and a trait-typed parameter
/// too. A model deriving `Serialize` over its bounded parameter, and a class passing a field of that parameter to
/// `json_stringify`, compile only when the parameter is `serde::Serialize`; both built before the stdlib trait was
/// added to the bound, and must keep building with it.
#[test]
fn json_protocol_bounds_carry_the_serde_capability_on_owners_and_trait_parameters_issue1820() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
from std.serde.json import Deserialize, Serialize

@derive(Serialize, Deserialize)
model Payload:
  value: int

@derive(Serialize)
model Envelope[T with Serialize]:
  payload: T

@derive(Serialize, Deserialize)
model Pair[T with (Serialize, Deserialize)]:
  first: T

class Recorder[T with Serialize]:
  items: list[T]

  def dump(self) -> str:
    return json_stringify(self.items)

def describe(value: Serialize) -> str:
  return value.to_json()

def main() -> None:
  envelope = Envelope(payload=Payload(value=1))
  println(envelope.to_json())
  recorder = Recorder(items=[Payload(value=2)])
  println(recorder.dump())
  println(describe(Payload(value=3)))
"#,
    )?;
    let serialize = vec![vec!["Serialize".to_string(), SERDE_SERIALIZE.to_string()]];
    assert_eq!(bound_paths(lowered_struct_type_params(&ir, "Envelope")?), serialize);
    assert_eq!(bound_paths(lowered_struct_type_params(&ir, "Recorder")?), serialize);
    assert_eq!(
        bound_paths(lowered_struct_type_params(&ir, "Pair")?),
        vec![vec![
            "Serialize".to_string(),
            SERDE_SERIALIZE.to_string(),
            "Deserialize".to_string(),
            SERDE_DESERIALIZE_OWNED.to_string(),
        ]]
    );
    assert_eq!(
        bound_paths(&lowered_function(&ir, "describe")?.type_params),
        serialize,
        "the hidden parameter of a trait-typed argument is bounded like a written one"
    );
    Ok(())
}

/// Return the trait names of the impl blocks lowered for `type_name`, in declaration order.
fn implemented_traits(ir: &IrProgram, type_name: &str) -> Vec<String> {
    ir.declarations
        .iter()
        .filter_map(|decl| match &decl.kind {
            IrDeclKind::Impl(impl_block) if impl_block.target_type == type_name => impl_block.trait_name.clone(),
            _ => None,
        })
        .collect()
}

/// #1820: a newtype that derives `Serialize` or `Deserialize` implements the stdlib trait as a model does, so
/// `UserId(7).to_json()` and a `T with Serialize` bound over it have the impl they dispatch through. A generic
/// newtype's impl requires the serde capability of its parameter, which is what the impl's default body needs.
#[test]
fn derived_json_protocol_newtypes_implement_the_stdlib_trait_issue1820() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
from std.serde.json import Deserialize, Serialize

@derive(Serialize, Deserialize)
type UserId = newtype int

@derive(Serialize)
type Boxed[T] = newtype T

def main() -> None:
  println(UserId(7).to_json())
  println(Boxed[int](8).to_json())
"#,
    )?;
    assert_eq!(implemented_traits(&ir, "UserId"), vec!["Serialize", "Deserialize"]);
    let boxed = ir
        .declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Impl(impl_block)
                if impl_block.target_type == "Boxed" && impl_block.trait_name.as_deref() == Some("Serialize") =>
            {
                Some(impl_block)
            }
            _ => None,
        })
        .ok_or("missing the derived Serialize impl for `Boxed`")?;
    assert_eq!(bound_paths(&boxed.type_params), vec![vec![SERDE_SERIALIZE.to_string()]]);
    Ok(())
}

/// #1820: a callable declared to return a `std.serde.json` trait type returns the exact Rust type `impl <stdlib
/// trait> + <serde capability>`, since a single `ImplTrait` bound cannot carry both. A method's return also carries
/// the capture list the emitter writes for its trait-typed returns: the owner's parameters, and `Self` in a trait.
#[test]
fn json_protocol_returns_carry_the_trait_and_its_serde_capability_issue1820() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
from std.serde.json import Serialize

@derive(Serialize)
model Payload:
  value: int

trait Source:
  def emit(self) -> Serialize: ...

model Feed with Source:
  value: int

  def emit(self) -> Serialize:
    return Payload(value=self.value)

model Builder:
  value: int

  def build(self) -> Serialize:
    return Payload(value=self.value)

def make() -> Serialize:
  return Payload(value=1)

def main() -> None:
  println(json_stringify(make()))
  println(make().to_json())
  println(json_stringify(Builder(value=2).build()))
  println(json_stringify(Feed(value=3).emit()))
"#,
    )?;
    let two_bounds = format!("impl {STDLIB_SERIALIZE_PATH} + {SERDE_SERIALIZE}");
    assert_eq!(
        lowered_function(&ir, "make")?.return_type,
        IrType::RustDisplay(two_bounds.clone())
    );
    let method_returns = |owner: &str| -> Vec<IrType> {
        ir.declarations
            .iter()
            .flat_map(|decl| match &decl.kind {
                IrDeclKind::Impl(impl_block) if impl_block.target_type == owner => impl_block.methods.as_slice(),
                IrDeclKind::Trait(trait_decl) if trait_decl.name == owner => trait_decl.methods.as_slice(),
                _ => &[],
            })
            .map(|function| function.return_type.clone())
            .collect()
    };
    let method_two_bounds = IrType::RustDisplay(format!("{two_bounds} + use<>"));
    assert!(
        method_returns("Builder").contains(&method_two_bounds),
        "an inherent method's return carries both bounds and its capture list: {:?}",
        method_returns("Builder")
    );
    assert!(
        method_returns("Feed").contains(&method_two_bounds),
        "a trait impl method's return carries both bounds and its capture list: {:?}",
        method_returns("Feed")
    );
    assert!(
        method_returns("Source").contains(&IrType::RustDisplay(format!("{two_bounds} + use<Self>"))),
        "a trait method's return also captures `Self`: {:?}",
        method_returns("Source")
    );
    Ok(())
}

/// #1820: a module-qualified trait annotation (`json.Serialize`) on a parameter or a declared return lowers like the
/// alias does: the parameter gets a hidden generic bounded by the trait as written plus the serde capability, and the
/// return becomes the two-bound Rust type. The annotation used to lower as a bare trait path in type position, which
/// rustc refuses (E0782).
#[test]
fn module_qualified_trait_annotations_lower_like_the_alias_issue1820() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
from std.serde import json

@derive(json)
model Payload:
  value: int

def describe(value: json.Serialize) -> str:
  return json_stringify(value)

def make() -> json.Serialize:
  return Payload(value=2)

def main() -> None:
  println(describe(Payload(value=1)))
  println(json_stringify(make()))
  println(make().to_json())
"#,
    )?;
    assert_eq!(
        bound_paths(&lowered_function(&ir, "describe")?.type_params),
        vec![vec!["json.Serialize".to_string(), SERDE_SERIALIZE.to_string()]]
    );
    assert_eq!(
        lowered_function(&ir, "make")?.return_type,
        IrType::RustDisplay(format!("impl {STDLIB_SERIALIZE_PATH} + {SERDE_SERIALIZE}"))
    );
    Ok(())
}

/// Return the derives of the named model or class.
fn lowered_struct_derives<'a>(ir: &'a IrProgram, name: &str) -> Result<&'a [String], String> {
    ir.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Struct(owner) if owner.name == name => Some(owner.derives.as_slice()),
            _ => None,
        })
        .ok_or_else(|| format!("missing model or class `{name}`"))
}

/// #1887: after `from std import serde`, `serde.json.Serialize` lowers like `json.Serialize`. A bound and a parameter
/// carry the trait as written plus the serde capability, a return is the two-bound Rust type, `value.to_json()`
/// dispatches through the stdlib trait, and adopting the trait with `with` implements it and derives the capability.
/// The qualifier used to resolve only when its module was one imported name, so each position lowered without the
/// capability.
#[test]
fn serde_json_spelling_through_the_serde_module_lowers_like_json_serialize_issue1887() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
from std import serde

model Adopter with serde.json.Serialize:
  value: int

def encode[T with serde.json.Serialize](value: T) -> str:
  return value.to_json()

def describe(value: serde.json.Serialize) -> str:
  return json_stringify(value)

def make() -> serde.json.Serialize:
  return Adopter(value=3)

def main() -> None:
  println(encode(Adopter(value=1)))
  println(describe(Adopter(value=2)))
  println(make().to_json())
"#,
    )?;
    let expected = vec![vec!["serde.json.Serialize".to_string(), SERDE_SERIALIZE.to_string()]];
    assert_eq!(bound_paths(&lowered_function(&ir, "encode")?.type_params), expected);
    assert_eq!(
        bound_paths(&lowered_function(&ir, "describe")?.type_params),
        expected,
        "the hidden parameter of a trait-typed argument is bounded like a written one"
    );
    assert_eq!(
        lowered_function(&ir, "make")?.return_type,
        IrType::RustDisplay(format!("impl {STDLIB_SERIALIZE_PATH} + {SERDE_SERIALIZE}"))
    );
    assert_eq!(returned_dispatch_trait_path(&ir, "encode")?, STDLIB_SERIALIZE_PATH);
    assert_eq!(implemented_traits(&ir, "Adopter"), vec!["serde.json.Serialize"]);
    let derives = lowered_struct_derives(&ir, "Adopter")?;
    assert!(
        derives.iter().any(|derive| derive == "serde::Serialize"),
        "adopting the trait derives its serde capability: {derives:?}"
    );
    Ok(())
}

/// #1885: a module-qualified derive lowers as the bare one does: a model, a class, an enum and a newtype deriving
/// `json.Serialize` or `json.Deserialize` implement the stdlib trait and derive its serde capability, and so does one
/// deriving through a submodule (`serde.json.Serialize`). The qualified spelling used to lower to neither.
#[test]
fn qualified_json_derives_implement_the_stdlib_traits_issue1885() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
from std.serde import json

@derive(json.Serialize, json.Deserialize)
model Payload:
  value: int

@derive(json.Serialize)
class Record:
  value: int

@derive(json.Serialize)
enum Color:
  Red
  Blue

@derive(json.Serialize)
type UserId = newtype int

def main() -> None:
  println(Payload(value=1).to_json())
  println(Record(value=2).to_json())
  println(Color.Red.to_json())
  println(UserId(3).to_json())
"#,
    )?;
    assert_eq!(
        implemented_traits(&ir, "Payload"),
        vec!["json.Serialize", "json.Deserialize"]
    );
    for owner in ["Record", "Color", "UserId"] {
        assert_eq!(implemented_traits(&ir, owner), vec!["json.Serialize"], "`{owner}`");
    }
    let payload = lowered_struct_derives(&ir, "Payload")?;
    for capability in ["serde::Serialize", "serde::Deserialize"] {
        assert!(
            payload.iter().any(|derive| derive == capability),
            "the qualified derive derives `{capability}`: {payload:?}"
        );
    }
    let record = lowered_struct_derives(&ir, "Record")?;
    assert!(
        record.iter().any(|derive| derive == "serde::Serialize"),
        "a class's qualified derive derives the capability: {record:?}"
    );

    let through_submodule = lower_checked_source(
        r#"
from std import serde

@derive(serde.json.Serialize)
model Payload:
  value: int

def main() -> None:
  println(Payload(value=1).to_json())
"#,
    )?;
    assert_eq!(
        implemented_traits(&through_submodule, "Payload"),
        vec!["serde.json.Serialize"]
    );
    let derives = lowered_struct_derives(&through_submodule, "Payload")?;
    assert!(
        derives.iter().any(|derive| derive == "serde::Serialize"),
        "a derive through a submodule derives the capability: {derives:?}"
    );
    Ok(())
}

/// Return the supertraits of the named trait declaration, as Rust paths.
fn lowered_supertrait_paths(ir: &IrProgram, name: &str) -> Result<Vec<String>, String> {
    ir.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Trait(trait_decl) if trait_decl.name == name => {
                Some(trait_decl.supertraits.iter().map(|(path, _)| path.clone()).collect())
            }
            _ => None,
        })
        .ok_or_else(|| format!("missing trait `{name}`"))
}

/// #1845: a `std.serde.json` supertrait lowers to the stdlib trait as written plus its serde capability, under the
/// bare import, an alias and the module-qualified name, as the same trait in a bound does. The bare spelling used to
/// lower to `serde::Serialize` alone, which does not provide the `to_json()` a default method calls through it.
#[test]
fn json_protocol_supertraits_carry_the_trait_and_its_serde_capability_issue1845() -> Result<(), String> {
    let spellings = [
        ("from std.serde.json import Serialize", "Serialize"),
        ("from std.serde.json import Serialize as JsonSerialize", "JsonSerialize"),
        ("from std.serde import json", "json.Serialize"),
    ];
    for (import, written) in spellings {
        let ir = lower_checked_source(&format!(
            r#"
{import}

trait Loggable with {written}:
  def log(self) -> str:
    return self.to_json()

def main() -> None:
  pass
"#
        ))?;
        assert_eq!(
            lowered_supertrait_paths(&ir, "Loggable")?,
            vec![written.to_string(), SERDE_SERIALIZE.to_string()],
            "`trait Loggable with {written}`"
        );
    }
    Ok(())
}

/// #1845: a trait an adopter reaches more than once is implemented once. A model that derives `Serialize` and adopts
/// a trait whose supertrait is `Serialize` used to get two `impl Serialize` blocks (E0119), and so did a model adopting
/// two traits that share a supertrait, and a model whose derive (or automatic `Clone`) implements a builtin supertrait
/// of a trait it adopts. An adopter that reaches `Serialize` only through the supertrait derives its serde capability,
/// as a direct `with Serialize` adoption does.
#[test]
fn a_trait_reached_twice_is_implemented_once_issue1845() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
from std.serde.json import Serialize

trait Loggable with Serialize:
  def log(self) -> str:
    return self.to_json()

@derive(Serialize)
model Payload with Loggable:
  value: int

class Record with Loggable:
  value: int

trait Named:
  def name(self) -> str:
    return "shared"

trait First with Named:
  def first(self) -> int:
    return 1

trait Second with Named:
  def second(self) -> int:
    return 2

model Both with First, Second:
  value: int

trait Keyed with Eq:
  def key(self) -> int:
    return 1

@derive(Eq)
model Key with Keyed:
  value: int

trait Copyable with Clone:
  def dup(self) -> Self:
    return self.clone()

model Copied with Copyable:
  value: int

def main() -> None:
  println(Payload(value=1).log())
  println(Record(value=2).log())
  println(Both(value=0).first() + Both(value=0).second())
  println(Key(value=3).key())
  println(Copied(value=4).dup().value)
"#,
    )?;
    assert_eq!(implemented_traits(&ir, "Payload"), vec!["Loggable", "Serialize"]);
    assert_eq!(implemented_traits(&ir, "Record"), vec!["Loggable", "Serialize"]);
    assert_eq!(implemented_traits(&ir, "Both"), vec!["First", "Named", "Second"]);
    assert_eq!(
        implemented_traits(&ir, "Key"),
        vec!["Keyed"],
        "a derived `Eq` is not implemented again through the supertrait"
    );
    assert_eq!(
        implemented_traits(&ir, "Copied"),
        vec!["Copyable"],
        "a model's automatic `Clone` is not implemented again through the supertrait"
    );
    let record = lowered_struct_derives(&ir, "Record")?;
    assert!(
        record.iter().any(|derive| derive == "serde::Serialize"),
        "an adopter reaching `Serialize` through a supertrait derives its capability: {record:?}"
    );
    Ok(())
}
