//! Generated Rust for a static collection read whose argument is read again (#1793), a static dict's `get` returned
//! as `Option[V]`, and a `str` or `FrozenStr` const stored at a destination of the other string type (#1794).

use incan_ir::expr::{IrStaticReferenceKind, VarAccess, VarRefKind};
use incan_ir::{IrExpr, IrExprKind, IrType};

use crate::codegen::IrCodegen;
use crate::conversions::{Conversion, ConversionContext, determine_conversion};
use crate::test_support::parse_program_result;

/// Check, lower and emit one program, returning the generated Rust with all whitespace and the trailing commas of
/// wrapped argument lists removed, so assertions do not depend on how the pretty-printer wraps a line.
fn generated_rust_without_whitespace(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let program = parse_program_result(source)?;
    let code = IrCodegen::new()
        .try_generate(&program)
        .map_err(|error| format!("generation failed: {error:?}"))?;
    Ok(code.split_whitespace().collect::<String>().replace(",)", ")"))
}

/// #1793: the lookup key of a static dict's `get`, `insert` and membership test is copied into the temporary the
/// storage access reads, so the parameter the arms read afterwards is still there; at its last read it is moved in,
/// and a literal key is stored as an owned `str`. The `get` of a static dict hands back the value itself, so
/// returning it as `Option[int]` checks and builds.
#[test]
fn static_collection_arguments_stay_readable_issue1793() -> Result<(), Box<dyn std::error::Error>> {
    let code = generated_rust_without_whitespace(
        r#"
static counts: dict[str, int] = {}


def lookup(name: str) -> str:
    match counts.get(name):
        Some(value) => return f"{name}={value}"
        None => return f"{name} missing"


def record(name: str) -> bool:
    counts.insert(name, 2)
    return name in counts


def direct(name: str) -> Option[int]:
    return counts.get(name)


def main() -> None:
    counts.insert("a", 1)
    println(lookup("a"))
    println(record("b"))
    println(direct("b").unwrap_or(0))
"#,
    )?;
    assert_eq!(
        code.matches("let__incan_static_arg_0=name.clone();").count(),
        2,
        "`lookup` and the `insert` in `record` copy the reused key: {code}"
    );
    assert!(
        code.contains("let__incan_static_arg_0=name;"),
        "a key at its last read is moved into the temporary: {code}"
    );
    assert!(
        code.contains(".get(<_asAsRef<str>>::as_ref(&__incan_static_arg_0)).copied()"),
        "the lookup reads the temporary and copies the value out: {code}"
    );
    assert!(
        code.contains(".insert(__incan_static_arg_0.to_string(),__incan_static_arg_1)"),
        "a literal key is stored as the static's owned `str`: {code}"
    );
    assert!(
        code.contains("->Option<i64>"),
        "`direct` returns the looked-up value as `Option[int]`: {code}"
    );
    Ok(())
}

/// #1794: a `const` declared `str` is a `'static` string that carries `FrozenStr`, so every `FrozenStr` destination
/// wraps it: a plain return, a `FrozenStr | int` return and argument, a `Some(...)` in an `Option[FrozenStr]` return,
/// argument and binding, and an annotated binding.
#[test]
fn str_const_is_wrapped_at_frozen_str_destinations_issue1794() -> Result<(), Box<dyn std::error::Error>> {
    let code = generated_rust_without_whitespace(
        r#"
const NAME: str = "policy"


def frozen_or_int(flag: bool) -> FrozenStr | int:
    if flag:
        return NAME
    return 7


def maybe_frozen(flag: bool) -> Option[FrozenStr]:
    if flag:
        return Some(NAME)
    return None


def plain_frozen() -> FrozenStr:
    return NAME


def union_arg(value: FrozenStr | int) -> bool:
    return isinstance(value, str)


def option_arg(value: Option[FrozenStr]) -> bool:
    return value is not None


def main() -> None:
    println(isinstance(frozen_or_int(true), str))
    println(isinstance(maybe_frozen(true), str))
    println(plain_frozen())
    println(union_arg(NAME))
    println(option_arg(Some(NAME)))
    bound: FrozenStr = NAME
    wrapped: Option[FrozenStr] = Some(NAME)
    println(bound)
    println(wrapped is not None)
"#,
    )?;
    let wrapped_name = "incan_std_core::frozen::FrozenStr::new(NAME)";
    assert_eq!(
        code.matches(wrapped_name).count(),
        7,
        "every `FrozenStr` destination wraps the const: {code}"
    );
    assert!(
        code.contains(&format!("::V0({wrapped_name})")),
        "the union member is the wrapped const: {code}"
    );
    assert!(
        code.contains(&format!("Some({wrapped_name})")),
        "the `Some` payload is the wrapped const: {code}"
    );
    assert!(
        !code.contains("NAME.to_string()") && !code.contains("returnNAME;"),
        "the const never reaches a `FrozenStr` destination unwrapped or as an owned string: {code}"
    );
    Ok(())
}

/// The reverse direction: a `FrozenStr` const at a `str` destination (a `Some(...)` in an `Option[str]` return,
/// argument and binding, a `str | int` return and argument, a plain return) is converted to an owned string.
#[test]
fn frozen_str_const_is_converted_at_str_destinations_issue1794() -> Result<(), Box<dyn std::error::Error>> {
    let code = generated_rust_without_whitespace(
        r#"
const FROZEN: FrozenStr = "frozen"


def maybe_text(flag: bool) -> Option[str]:
    if flag:
        return Some(FROZEN)
    return None


def text_or_int(flag: bool) -> str | int:
    if flag:
        return FROZEN
    return 7


def plain_text() -> str:
    return FROZEN


def union_arg(value: str | int) -> bool:
    return isinstance(value, str)


def option_arg(value: Option[str]) -> bool:
    return value is not None


def main() -> None:
    println(isinstance(maybe_text(true), str))
    println(isinstance(text_or_int(true), str))
    println(plain_text())
    println(union_arg(FROZEN))
    println(option_arg(Some(FROZEN)))
    bound: Option[str] = Some(FROZEN)
    println(bound is not None)
"#,
    )?;
    assert_eq!(
        code.matches("FROZEN.to_string()").count(),
        6,
        "every `str` destination converts the const: {code}"
    );
    assert!(
        !code.contains("Some(FROZEN)") && !code.contains("returnFROZEN;"),
        "the const never reaches a `str` destination as a `FrozenStr`: {code}"
    );
    Ok(())
}

/// #1794: the conversion planner wraps a `'static` string, the storage of a `const` declared `str`, in `FrozenStr` at
/// every Incan destination that expects one, and at no Rust-facing boundary. A value that already is a `FrozenStr`
/// needs nothing, and a `'static` string at a `str` destination keeps its owned-string conversion.
#[test]
fn planner_wraps_a_static_str_at_frozen_str_destinations_issue1794() {
    let const_read = IrExpr::new(
        IrExprKind::StaticRead {
            name: "NAME".to_string(),
            reference_kind: IrStaticReferenceKind::Source,
        },
        IrType::StaticStr,
    );
    let frozen_var = IrExpr::new(
        IrExprKind::Var {
            name: "label".to_string(),
            access: VarAccess::Read,
            ref_kind: VarRefKind::Value,
        },
        IrType::FrozenStr,
    );
    for context in [
        ConversionContext::IncanFunctionArg,
        ConversionContext::IncanFunctionArgInReturn,
        ConversionContext::StructField,
        ConversionContext::CollectionElement,
        ConversionContext::Assignment,
        ConversionContext::ReturnValue,
    ] {
        assert_eq!(
            determine_conversion(&const_read, Some(&IrType::FrozenStr), context),
            Conversion::ToFrozenStr,
            "{context:?} wraps the const"
        );
        assert_ne!(
            determine_conversion(&frozen_var, Some(&IrType::FrozenStr), context),
            Conversion::ToFrozenStr,
            "{context:?} leaves a `FrozenStr` as it is"
        );
        assert_eq!(
            determine_conversion(&const_read, Some(&IrType::String), context),
            Conversion::ToString,
            "{context:?} still makes an owned `str` from the const"
        );
    }
    for context in [
        ConversionContext::ExternalFunctionArg,
        ConversionContext::MethodArg,
        ConversionContext::MatchScrutinee,
    ] {
        assert_ne!(
            determine_conversion(&const_read, Some(&IrType::FrozenStr), context),
            Conversion::ToFrozenStr,
            "{context:?} is a Rust-facing boundary"
        );
    }
    assert_eq!(
        Conversion::ToFrozenStr.apply(quote::quote! { NAME }).to_string(),
        quote::quote! { incan_std_core::frozen::FrozenStr::new(NAME) }.to_string()
    );
}
