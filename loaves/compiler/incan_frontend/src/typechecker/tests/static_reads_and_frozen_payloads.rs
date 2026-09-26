//! What a read of module static storage answers with (a static dict's `get`), and the string type a `Some(...)` of a
//! `FrozenStr` payload is instantiated at when its destination is `Option[str]` or `Option[FrozenStr]` (#1794).

use super::*;

/// Wrap one type in `Option[...]`.
fn option_of(inner: ResolvedType) -> ResolvedType {
    ResolvedType::Generic(
        collection_types::as_str(CollectionTypeId::Option).to_string(),
        vec![inner],
    )
}

/// The span of the `occurrence`-th (zero-based) appearance of `text` in `source`.
fn span_of(source: &str, text: &str, occurrence: usize) -> Result<Span, String> {
    let start = source
        .match_indices(text)
        .nth(occurrence)
        .map(|(start, _)| start)
        .ok_or_else(|| format!("missing occurrence {occurrence} of `{text}` in the source"))?;
    Ok(Span::new(start, start + text.len()))
}

/// `get` on a module static dict answers with `Option[V]`: the value is copied out of the storage cell, so there is
/// no view into the cell to hand back. That holds for a static, a field of a static, and in a `match`; a local dict
/// keeps its `Option[&V]` view, and a parameter that shadows a static reads as the parameter.
#[test]
fn get_on_a_static_dict_answers_with_the_value_type() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
model Registry:
    counts: dict[str, int]


static counts: dict[str, int] = {}
static registry: Registry = Registry(counts={})


def lookup(name: str) -> Option[int]:
    return counts.get(name)


def nested(name: str) -> Option[int]:
    return registry.counts.get(name)


def bumped(name: str) -> int:
    match counts.get(name):
        Some(value) => return value + 1
        None => return 0


def local(table: dict[str, int], name: str) -> Option[int]:
    return table.get(name).copied()


def shadowed(counts: dict[str, int], name: str) -> Option[int]:
    return counts.get(name).copied()
"#;
    let info = typecheck_info_for_module(source, vec!["main".to_string()], "static dict get")?;
    let owned = option_of(ResolvedType::Int);
    let viewed = option_of(ResolvedType::Ref(Box::new(ResolvedType::Int)));
    for (text, occurrence, expected) in [
        ("counts.get(name)", 0, &owned),
        ("registry.counts.get(name)", 0, &owned),
        ("counts.get(name)", 2, &owned),
        ("table.get(name)", 0, &viewed),
        ("counts.get(name)", 3, &viewed),
    ] {
        let span = span_of(source, text, occurrence)?;
        assert_eq!(
            info.expr_type(span),
            Some(expected),
            "`{text}` (occurrence {occurrence}) must type as `{expected}`"
        );
    }
    Ok(())
}

/// The static dict's `get` already answers with the value, so the `Option[&V]` helper `copied` does not apply to it and
/// the checker refuses it rather than leaving it to the build.
#[test]
fn copied_on_a_static_dict_get_is_refused() {
    let source = r#"
static counts: dict[str, int] = {}


def lookup(name: str) -> Option[int]:
    return counts.get(name).copied()
"#;
    assert!(
        check_str(source).is_err(),
        "`copied` on an owned `Option[int]` must not check"
    );
}

/// #1794: a `FrozenStr` payload (a `const` declared `str` carries `FrozenStr`, as does one declared `FrozenStr`) placed
/// in an `Option[str]` or an `Option[FrozenStr]` instantiates `Some` at the destination's own string type. The checker
/// records that parameter as the call's callable fact, so lowering carries it and the payload is converted at the
/// argument, and it types the call as the instantiation.
#[test]
fn some_frozen_str_payload_is_instantiated_at_the_destination_string_type_issue1794()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
const NAME: str = "policy"
const FROZEN: FrozenStr = "frozen"


def maybe_frozen() -> Option[FrozenStr]:
    return Some(NAME)


def maybe_text() -> Option[str]:
    return Some(FROZEN)


def takes_text(value: Option[str]) -> bool:
    return value is not None


def main() -> None:
    println(takes_text(Some(NAME)))
    bound: Option[FrozenStr] = Some(FROZEN)
    println(bound is not None)
"#;
    let info = typecheck_info_for_module(source, vec!["main".to_string()], "Some of a FrozenStr payload")?;
    for (text, occurrence, destination) in [
        ("Some(NAME)", 0, ResolvedType::FrozenStr),
        ("Some(FROZEN)", 0, ResolvedType::Str),
        ("Some(NAME)", 1, ResolvedType::Str),
        ("Some(FROZEN)", 1, ResolvedType::FrozenStr),
    ] {
        let span = span_of(source, text, occurrence)?;
        assert_eq!(
            info.call_site_callable_params(span).map(<[CallableParam]>::to_vec),
            Some(vec![CallableParam::positional(destination.clone())]),
            "`{text}` (occurrence {occurrence}) must record `{destination}` as the constructor's parameter"
        );
        assert_eq!(info.expr_type(span), Some(&option_of(destination)));
    }
    Ok(())
}

/// The admission is one-way for runtime values: a `str` that is not a `const` is not a `FrozenStr`, so returning one
/// where `FrozenStr` is expected, bare or inside `Some`, stays refused.
#[test]
fn a_runtime_str_is_not_admitted_where_frozen_str_is_expected() {
    for source in [
        r#"
def frozen(value: str) -> FrozenStr:
    return value
"#,
        r#"
def maybe_frozen(value: str) -> Option[FrozenStr]:
    return Some(value)
"#,
    ] {
        assert!(
            check_str(source).is_err(),
            "a runtime `str` must not check as `FrozenStr`:\n{source}"
        );
    }
}
