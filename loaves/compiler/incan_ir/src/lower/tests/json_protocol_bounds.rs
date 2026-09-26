//! Generic bounds on the `std.serde.json` protocol traits name the stdlib trait under every import spelling, the same
//! trait the protocol's methods dispatch through (#1820).

use super::*;

/// One spelling of `std.serde.json.Serialize`: the program that imports and bounds it, and the bound as written.
struct SerializeSpelling {
    source: &'static str,
    written: &'static str,
}

/// The bare import, an alias and the module-qualified name, each bounding `encode` and calling `to_json` through it.
const SERIALIZE_SPELLINGS: &[SerializeSpelling] = &[
    SerializeSpelling {
        source: r#"
from std.serde.json import Serialize

@derive(Serialize)
model Payload:
  value: int

def encode[T with Serialize](value: T) -> str:
  return value.to_json()

def main() -> str:
  return encode(Payload(value=1))
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

def main() -> str:
  return encode(Payload(value=1))
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

def main() -> str:
  return encode(Payload(value=1))
"#,
        written: "json.Serialize",
    },
];

/// Return the trait paths bounding the named function's only type parameter.
fn single_type_param_bound_paths(ir: &IrProgram, function: &str) -> Result<Vec<String>, String> {
    let function = lowered_function(ir, function)?;
    let [type_param] = function.type_params.as_slice() else {
        return Err(format!(
            "expected one type parameter on `{}`, got {:?}",
            function.name, function.type_params
        ));
    };
    Ok(type_param.bounds.iter().map(|bound| bound.trait_path.clone()).collect())
}

/// #1820: `T with Serialize` over `from std.serde.json import Serialize` lowers to the imported stdlib trait, the trait
/// `value.to_json()` dispatches through, exactly as the alias and the module-qualified spelling do. The bare spelling
/// used to reach the trait-bound registry and lower to `serde::Serialize`, which does not provide `to_json`, so the
/// generated function called a method its only bound did not supply.
#[test]
fn json_protocol_bounds_name_the_dispatched_trait_issue1820() -> Result<(), String> {
    for spelling in SERIALIZE_SPELLINGS {
        let ir = lower_checked_source(spelling.source)?;
        assert_eq!(
            single_type_param_bound_paths(&ir, "encode")?,
            vec![spelling.written.to_string()],
            "`T with {}` must lower as written, naming the imported stdlib trait",
            spelling.written
        );

        let encode = lowered_function(&ir, "encode")?;
        let Some(IrStmt {
            kind: IrStmtKind::Return(Some(returned)),
            ..
        }) = encode.body.last()
        else {
            return Err(format!("expected `encode` to end in a return, got {:?}", encode.body));
        };
        let IrExprKind::MethodCall {
            dispatch: Some(IrMethodDispatch::Trait(dispatch) | IrMethodDispatch::SourceProjection(dispatch)),
            ..
        } = &returned.kind
        else {
            return Err(format!(
                "expected a trait-dispatched `to_json` call under `{}`, got {:?}",
                spelling.written, returned.kind
            ));
        };
        assert_eq!(
            dispatch.trait_path, "crate::__incan_std::serde::json::Serialize",
            "the dispatch under `{}` names the stdlib trait the bound resolves to",
            spelling.written
        );
    }
    Ok(())
}

/// #1820: the bare `Deserialize` import bounds a parameter by the stdlib trait too, not by `serde::de::DeserializeOwned`,
/// so `T.from_json(text)` through the bound has a bound that provides it.
#[test]
fn bare_deserialize_bound_names_the_stdlib_trait_issue1820() -> Result<(), String> {
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
        single_type_param_bound_paths(&ir, "decode")?,
        vec!["Deserialize".to_string()]
    );
    Ok(())
}
