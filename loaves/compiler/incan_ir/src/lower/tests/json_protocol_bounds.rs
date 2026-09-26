//! Generic bounds on the `std.serde.json` protocol traits carry both the stdlib trait, which the protocol's methods
//! dispatch through, and the Rust serde capability the trait forwards to its adopters, under every import spelling
//! and in every position a bound is written (#1820).

use super::*;

/// The Rust serde capability a `Serialize` bound carries beside the stdlib trait.
const SERDE_SERIALIZE: &str = "serde::Serialize";

/// The Rust serde capability a `Deserialize` bound carries beside the stdlib trait.
const SERDE_DESERIALIZE_OWNED: &str = "serde::de::DeserializeOwned";

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
            "crate::__incan_std::serde::json::Serialize",
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
