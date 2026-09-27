//! The `std.serde.json` traits under each spelling, and what providing one requires of a type's members and type
//! arguments: module-qualified spellings through a module (`serde.json.Serialize`, #1887), module-qualified derives
//! (`@derive(json.Serialize)`, #1885), the JSON form of a declaration's members (#1886) and a newtype's underlying
//! value and bounded type arguments (#1867).

use super::*;

/// #1887: a model another module derives with `@derive(json)` satisfies a `serde.json.Serialize` or
/// `serde.json.Deserialize` bound spelled after `from std import serde`, though the consuming module imports neither
/// `json` nor the traits by name: the derived adoption is compared by the trait's module and name, not by how the
/// declaring module spelled it.
#[test]
fn a_model_derived_in_another_module_satisfies_a_module_qualified_json_bound_issue1887() -> Result<(), String> {
    let models_source = r#"
from std.serde import json


@derive(json)
pub model Payload:
    pub value: int
"#;
    let source = r#"
from std import serde
from models import Payload


def encode[T with serde.json.Serialize](value: T) -> str:
    return value.to_json()


def decode[T with serde.json.Deserialize](text: str) -> Result[T, str]:
    return T.from_json(text)


def main() -> None:
    println(encode(Payload(value=1)))
    match decode[Payload]("{\"value\":4}"):
        Ok(payload) => println(payload.value)
        Err(error) => println(error)
"#;
    let models_ast = parse_program(models_source, "models module");
    let ast = parse_program(source, "consumer");
    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&ast, &[("models", &models_ast)])
        .map_err(|errs| format!("the derived model must satisfy the module-qualified bounds: {errs:?}"))
}

/// #1887: after `from std import serde`, `serde.json.Serialize` and `serde.json.Deserialize` name the
/// `std.serde.json` traits in every position, as `json.Serialize` does after `from std.serde import json`: a model
/// adopts the trait after `with`, a bound accepts a model that derives or adopts it, and a parameter and a return typed
/// by it accept such a model. The spelling used to be unknown after `with`, to refuse a derived model as a bound, and
/// to refuse the model as a returned value. Each program is checked with and without the direct trait import beside
/// the module import.
#[test]
fn serde_json_traits_through_the_serde_module_name_the_stdlib_traits_issue1887() -> Result<(), String> {
    let with_direct_import = r#"
from std import serde
from std.serde.json import Deserialize, Serialize

@derive(Serialize, Deserialize)
model Payload:
  value: int

model Adopter with serde.json.Serialize:
  value: int

def encode[T with serde.json.Serialize](value: T) -> str:
  return value.to_json()

def decode[T with serde.json.Deserialize](text: str) -> Result[T, str]:
  return T.from_json(text)

def describe(value: serde.json.Serialize) -> str:
  return json_stringify(value)

def make() -> serde.json.Serialize:
  return Payload(value=5)

def main() -> None:
  println(encode(Payload(value=1)))
  println(encode(Adopter(value=2)))
  println(describe(Payload(value=3)))
  println(make().to_json())
  println(Adopter(value=4).to_json())
  match decode[Payload]("{\"value\":6}"):
    Ok(payload) => println(payload.value)
    Err(error) => println(error)
"#;
    let module_import_only = r#"
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
"#;
    for (shape, source) in [
        ("beside the direct trait import", with_direct_import),
        ("with the module import alone", module_import_only),
    ] {
        check_str(source).map_err(|errs| format!("`serde.json.Serialize` {shape} should typecheck: {errs:?}"))?;
    }
    Ok(())
}

/// #1885: a module-qualified derive adopts the trait it names, as the bare import does: `@derive(json.Serialize)` and
/// `@derive(json.Deserialize)` give a model `to_json()` and `from_json()` and satisfy a `json.Serialize` bound, under a
/// module alias on a class, an enum and a newtype too, and through a submodule (`serde.json.Serialize`, and the whole
/// module as `serde.json`). The qualified spelling used to be accepted and to adopt nothing.
#[test]
fn qualified_json_derives_adopt_the_traits_they_name_issue1885() -> Result<(), String> {
    let rows = [
        (
            "`json.Serialize` and `json.Deserialize`",
            r#"
from std.serde import json

@derive(json.Serialize, json.Deserialize)
model Payload:
  value: int

def encode[T with json.Serialize](value: T) -> str:
  return value.to_json()

def main() -> None:
  println(Payload(value=1).to_json())
  println(encode(Payload(value=1)))
  match Payload.from_json("{\"value\":2}"):
    Ok(payload) => println(payload.value)
    Err(error) => println(error)
"#,
        ),
        (
            "a module alias on a class, an enum and a newtype",
            r#"
from std.serde import json as j

@derive(j.Serialize)
class Record:
  value: int

@derive(j.Serialize, j.Deserialize)
enum Color:
  Red
  Blue

@derive(j.Serialize)
type UserId = newtype int

def encode[T with j.Serialize](value: T) -> str:
  return value.to_json()

def main() -> None:
  println(Record(value=1).to_json())
  println(encode(Color.Red))
  println(UserId(3).to_json())
"#,
        ),
        (
            "a submodule of `std.serde`",
            r#"
from std import serde

@derive(serde.json.Serialize)
model Payload:
  value: int

@derive(serde.json)
model Both:
  value: int

def encode[T with serde.json.Serialize](value: T) -> str:
  return value.to_json()

def main() -> None:
  println(encode(Payload(value=1)))
  println(Both(value=2).to_json())
  match Both.from_json("{\"value\":3}"):
    Ok(both) => println(both.value)
    Err(error) => println(error)
"#,
        ),
    ];
    for (shape, source) in rows {
        check_str(source).map_err(|errs| format!("the qualified derive through {shape} should adopt: {errs:?}"))?;
    }
    Ok(())
}

/// #1885: a qualified derive that names no derivable trait of its module is refused as an unknown derive, as the bare
/// spelling of an unknown name is.
#[test]
fn qualified_derive_of_an_unknown_trait_is_refused_issue1885() {
    let source = r#"
from std.serde import json

@derive(json.Nope)
model Payload:
  value: int
"#;
    let errs = check_str_err(source, "a qualified derive of an unknown trait");
    assert!(
        errs.iter()
            .any(|err| err.message.contains("Unknown derive 'json.Nope'")),
        "expected the unknown derive refusal: {errs:?}"
    );
}

/// #1886: a `model`, `class` or `enum` that derives or adopts `Serialize` or `Deserialize` needs the trait of every
/// field or payload type, so a member whose type is a model or class that does not provide it is refused at check
/// time, naming the member and the type inside it that lacks the trait. Such a program used to pass the check and
/// fail in the build.
#[test]
fn a_member_without_the_json_trait_of_its_owner_is_refused_issue1886() -> Result<(), String> {
    let preamble = r#"
from std.serde import json
from std.serde.json import Deserialize, Serialize

model Inner:
  value: int

class Klass:
  value: int

@derive(Deserialize)
model ReadOnly:
  value: int
"#;
    let rows = [
        (
            "@derive(Serialize)\nmodel Outer:\n  inner: Inner\n",
            "Field 'inner' of model 'Outer' has type 'Inner', which does not provide 'Serialize'",
        ),
        (
            "@derive(Serialize)\nclass Outer:\n  inner: Klass\n",
            "Field 'inner' of class 'Outer' has type 'Klass', which does not provide 'Serialize'",
        ),
        (
            "@derive(Serialize)\nmodel Outer:\n  items: list[Inner]\n",
            "Field 'items' of model 'Outer' has type 'List[Inner]', whose 'Inner' does not provide 'Serialize'",
        ),
        (
            "@derive(Serialize)\nmodel Outer:\n  maybe: Option[Inner]\n",
            "Field 'maybe' of model 'Outer' has type 'Option[Inner]', whose 'Inner' does not provide 'Serialize'",
        ),
        (
            "@derive(Deserialize)\nclass Outer:\n  keyed: dict[str, Klass]\n",
            "Field 'keyed' of class 'Outer' has type 'Dict[str, Klass]', whose 'Klass' does not provide 'Deserialize'",
        ),
        (
            "model Outer with Serialize:\n  inner: Inner\n",
            "Field 'inner' of model 'Outer' has type 'Inner', which does not provide 'Serialize'",
        ),
        (
            "@derive(json)\nmodel Outer:\n  inner: Inner\n",
            "Field 'inner' of model 'Outer' has type 'Inner', which does not provide 'Deserialize'",
        ),
        (
            "@derive(Serialize)\nmodel Outer:\n  read_only: ReadOnly\n",
            "Field 'read_only' of model 'Outer' has type 'ReadOnly', which does not provide 'Serialize'",
        ),
        (
            "@derive(Serialize)\nenum Outer:\n  Held(Inner)\n  Empty\n",
            "A payload of variant 'Held' of enum 'Outer' has type 'Inner', which does not provide 'Serialize'",
        ),
        (
            "enum Holder:\n  Held(Inner)\n  Empty\n\n@derive(Serialize)\nmodel Outer:\n  holder: Holder\n",
            "Field 'holder' of model 'Outer' has type 'Holder', whose 'Inner' does not provide 'Serialize'",
        ),
    ];
    let mut mismatches = Vec::new();
    for (declaration, expected) in rows {
        let source = format!("{preamble}\n{declaration}");
        let Err(errs) = check_str(&source) else {
            mismatches.push(format!("`{declaration}` should be refused"));
            continue;
        };
        if !errs.iter().any(|err| err.message == expected) {
            mismatches.push(format!(
                "expected `{expected}` for `{declaration}`, got {:?}",
                errs.iter().map(|err| &err.message).collect::<Vec<_>>()
            ));
        }
    }
    if mismatches.is_empty() {
        Ok(())
    } else {
        Err(mismatches.join("\n"))
    }
}

/// #1886: every member type that provides the trait, or may, stays accepted: a model that adopts it, one that derives
/// it through its module, an enum and a newtype (which serialize through their contents), a generic model bounded by
/// the trait, the owner itself, a type parameter, and a model that does not provide the trait at all.
#[test]
fn members_that_provide_the_json_trait_of_their_owner_are_accepted_issue1886() -> Result<(), String> {
    let source = r#"
from std.serde import json
from std.serde.json import Serialize

model Adopter with Serialize:
  value: int

@derive(json)
model ByModule:
  value: int

enum Kind:
  A
  B(ByModule)

type Id = newtype int

@derive(Serialize)
model Generic[T with Serialize]:
  item: T
  items: list[T]

@derive(Serialize)
model Node:
  children: list[Node]

@derive(Serialize)
model Outer:
  a: Adopter
  b: ByModule
  k: Kind
  i: Id
  g: Generic[ByModule]
  n: Node

model Bare:
  value: int

model Loose:
  bare: Bare

def main() -> None:
  outer = Outer(a=Adopter(value=1), b=ByModule(value=2), k=Kind.A, i=Id(3), g=Generic(item=ByModule(value=4), items=[]), n=Node(children=[]))
  println(outer.to_json())
"#;
    check_str(source).map_err(|errs| format!("members that provide the trait should be accepted: {errs:?}"))
}

/// #1867: a newtype that derives `Serialize` or `Deserialize` needs the trait of its underlying type, as a model needs
/// it of its fields, and a bounded newtype's type argument must satisfy the bound, written or inferred. `int` for
/// `newtype Boxed[T with Serialize]` used to be accepted although a builtin type does not provide `Serialize`, and so
/// did a model or class of the same shape and a user trait bound; each of these passed the check and failed in the
/// build.
#[test]
fn newtype_json_derives_and_bounded_type_arguments_are_checked_issue1867() -> Result<(), String> {
    let preamble = r#"
from std.serde.json import Deserialize, Serialize

trait Shape:
  def area(self) -> int:
    return 1

model Plain:
  value: int

@derive(Serialize)
type Boxed[T with Serialize] = newtype T

@derive(Serialize)
model Envelope[T with Serialize]:
  payload: T

class Holder[T with Shape]:
  item: T
"#;
    let rows = [
        (
            "@derive(Serialize)\ntype Wrapped = newtype Plain\n",
            "The underlying value of newtype 'Wrapped' has type 'Plain', which does not provide 'Serialize'",
        ),
        (
            "@derive(Deserialize)\ntype Wrapped = newtype Plain\n",
            "The underlying value of newtype 'Wrapped' has type 'Plain', which does not provide 'Deserialize'",
        ),
        (
            "@derive(Serialize)\ntype Many = newtype list[Plain]\n",
            "The underlying value of newtype 'Many' has type 'List[Plain]', whose 'Plain' does not provide 'Serialize'",
        ),
        (
            "def main() -> None:\n  println(Boxed[int](1).to_json())\n",
            "Call to 'Boxed' violates generic bound: type parameter 'T' requires 'Serialize' but got 'int'",
        ),
        (
            "def main() -> None:\n  println(Boxed(1).to_json())\n",
            "Call to 'Boxed' violates generic bound: type parameter 'T' requires 'Serialize' but got 'int'",
        ),
        (
            "def main() -> None:\n  println(Envelope(payload=1).to_json())\n",
            "Call to 'Envelope' violates generic bound: type parameter 'T' requires 'Serialize' but got 'int'",
        ),
        (
            "def main() -> None:\n  println(Envelope[str](payload=\"x\").to_json())\n",
            "Call to 'Envelope' violates generic bound: type parameter 'T' requires 'Serialize' but got 'str'",
        ),
        (
            "def main() -> None:\n  held = Holder(item=5)\n",
            "Call to 'Holder' violates generic bound: type parameter 'T' requires 'Shape' but got 'int'",
        ),
    ];
    let mut mismatches = Vec::new();
    for (declaration, expected) in rows {
        let source = format!("{preamble}\n{declaration}");
        let Err(errs) = check_str(&source) else {
            mismatches.push(format!("`{declaration}` should be refused"));
            continue;
        };
        let matching = errs.iter().filter(|err| err.message == expected).count();
        if matching != 1 {
            mismatches.push(format!(
                "expected `{expected}` once for `{declaration}`, got {:?}",
                errs.iter().map(|err| &err.message).collect::<Vec<_>>()
            ));
        }
    }
    if !mismatches.is_empty() {
        return Err(mismatches.join("\n"));
    }

    let accepted = r#"
from std.serde.json import Serialize

trait Shape:
  def area(self) -> int:
    return 1

model Square with Shape:
  side: int

@derive(Serialize)
model Plain:
  value: int

@derive(Serialize)
type Wrapped = newtype Plain

@derive(Serialize)
type Boxed[T with Serialize] = newtype T

@derive(Serialize)
type Loose[T] = newtype T

type Framed[T with Shape] = newtype T

def frame[T with Shape](value: T) -> Framed[T]:
  return Framed(value)

def main() -> None:
  println(Wrapped(Plain(value=1)).to_json())
  println(Boxed(Plain(value=2)).to_json())
  println(Boxed[Plain](Plain(value=3)).to_json())
  println(Loose(4).to_json())
  framed = frame(Square(side=5))
  explicit = Framed[Square](Square(side=6))
"#;
    check_str(accepted)
        .map_err(|errs| format!("satisfied bounds and serializable underlying types stay accepted: {errs:?}"))
}
