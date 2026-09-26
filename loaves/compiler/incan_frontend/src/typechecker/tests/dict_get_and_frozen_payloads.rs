//! What a dict's `get` answers with (the stored value, for every dict), and the string type a `Some(...)` of a
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

/// `get` answers with `Option[V]`, the stored value, on every dict: a module static, a field of a static, a local bound
/// to a static, a parameter (including one that shadows a static) and a local dict; and in a `match`, whose arms bind
/// the value itself. A result bound to an `Option[int]` annotation checks.
#[test]
fn get_on_every_dict_answers_with_the_value_type() -> Result<(), Box<dyn std::error::Error>> {
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


def local(table: dict[str, int], name: str) -> int:
    return table.get(name).unwrap_or(0)


def shadowed(counts: dict[str, int], name: str) -> Option[int]:
    return counts.get(name)


def aliased(name: str) -> Option[int]:
    live = counts
    found: Option[int] = live.get(name)
    return found


def built(name: str) -> Option[int]:
    mut totals: dict[str, int] = {}
    totals[name] = 1
    return totals.get(name)
"#;
    let info = typecheck_info_for_module(source, vec!["main".to_string()], "dict get")?;
    let value = option_of(ResolvedType::Int);
    for (text, occurrence) in [
        ("counts.get(name)", 0),
        ("registry.counts.get(name)", 0),
        ("counts.get(name)", 2),
        ("table.get(name)", 0),
        ("counts.get(name)", 3),
        ("live.get(name)", 0),
        ("totals.get(name)", 0),
    ] {
        let span = span_of(source, text, occurrence)?;
        assert_eq!(
            info.expr_type(span),
            Some(&value),
            "`{text}` (occurrence {occurrence}) must type as `{value}`"
        );
    }
    Ok(())
}

/// `get` already answers with the value, so `.copied()` and `.cloned()` on its result are refused at check time, with a
/// hint to remove them, rather than left to the build; a static dict and a local dict alike.
#[test]
fn copied_and_cloned_on_a_dict_get_are_refused() {
    for (source, method) in [
        (
            "static counts: dict[str, int] = {}\n\ndef lookup(name: str) -> Option[int]:\n    return counts.get(name).copied()\n",
            "copied",
        ),
        (
            "def lookup(table: dict[str, int], name: str) -> Option[int]:\n    return table.get(name).copied()\n",
            "copied",
        ),
        (
            "def lookup(table: dict[str, list[int]], name: str) -> Option[list[int]]:\n    return table.get(name).cloned()\n",
            "cloned",
        ),
    ] {
        let errors = check_str_err(source, "`.copied()` / `.cloned()` after `get` must not check");
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains(&format!("no method '{method}(...)'"))
                    && error
                        .hints
                        .iter()
                        .any(|hint| hint.contains("`get` returns the stored value itself"))),
            "`{method}` after `get` must be refused with the removal hint, got {errors:?}"
        );
    }
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

/// A `dict.get` result that only feeds a `match`, `if let` or `while let` whose bindings are only read — the argument
/// of `len`, `print` or `println`, an f-string interpolation, or not used at all — is recorded as read-only, so it is
/// read in place with no copy of the stored value. A lookup whose result is returned, bound, or whose binding is used
/// any other way keeps its own value, and so does a lookup on a static dict.
#[test]
fn a_dict_get_whose_result_is_only_read_is_recorded_read_only() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
static counts: dict[str, int] = {}


def size[V](table: dict[str, list[V]], key: str) -> int:
    match table.get(key):
        Some(items) => return len(items)
        None => return 0


def shown(table: dict[str, str], key: str) -> bool:
    if let Some(text) = table.get(key):
        println(f"found {text}")
        return true
    return false


def present(table: dict[str, list[int]], key: str) -> bool:
    match table.get(key):
        Some(_) => return true
        None => return false


def kept[V](table: dict[str, V], key: str) -> Option[V]:
    return table.get(key)


def handed(table: dict[str, list[int]], key: str) -> list[int]:
    match table.get(key):
        Some(items) => return items
        None => return []


def static_read(key: str) -> bool:
    match counts.get(key):
        Some(_) => return true
        None => return false
"#;
    let info = typecheck_info_for_module(source, vec!["main".to_string()], "read-only dict lookups")?;
    for (anchor, call, read_only) in [
        (
            "table.get(key):\n        Some(items) => return len(items)",
            "table.get(key)",
            true,
        ),
        ("table.get(key):\n        println", "table.get(key)", true),
        ("table.get(key):\n        Some(_)", "table.get(key)", true),
        ("table.get(key)\n\n\ndef handed", "table.get(key)", false),
        (
            "table.get(key):\n        Some(items) => return items",
            "table.get(key)",
            false,
        ),
        ("counts.get(key)", "counts.get(key)", false),
    ] {
        let start = source
            .find(anchor)
            .ok_or_else(|| format!("missing `{anchor}` in the source"))?;
        let span = Span::new(start, start + call.len());
        assert_eq!(
            info.is_read_only_dict_lookup(span),
            read_only,
            "the lookup at `{anchor}` must be read-only: {read_only}"
        );
    }
    Ok(())
}

/// A lookup that keeps its own value of a Rust type proven unable to be copied is refused at check time, naming the
/// type and the lookup that works instead; a lookup whose result is only read is accepted.
#[cfg(feature = "rust_inspect")]
#[test]
fn a_kept_dict_get_of_a_value_that_cannot_be_copied_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import Lock


def kept(table: dict[str, Lock], key: str) -> Option[Lock]:
    return table.get(key)


def present(table: dict[str, Lock], key: str) -> bool:
    match table.get(key):
        Some(_) => return true
        None => return false
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    let tmp = seeded_rust_inspect_workspace()?;
    let manifest_dir = tmp.path().to_path_buf();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::Lock".to_string(),
                definition_path: Some("demo::Lock".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    type_params: Vec::new(),
                    type_param_defaults: Vec::new(),
                    mutable_reference_type_params: Vec::new(),
                    expanded_derive_traits: Vec::new(),
                    has_const_params: false,
                    alias_target: None,
                    metadata_completeness: Default::default(),
                    methods: vec![],
                    implemented_traits: Vec::new(),
                    fields: vec![],
                    variants: vec![],
                }),
            },
        )
        .map_err(|error| std::io::Error::other(format!("seed rust-inspect lock: {error}")))?;
    let errors = match checker.check_program(&ast) {
        Ok(()) => return Err("a kept lookup of a value that cannot be copied must not check".into()),
        Err(errors) => errors,
    };
    let refusals = errors
        .iter()
        .filter(|error| error.message.contains("cannot be copied"))
        .collect::<Vec<_>>();
    let [refusal] = refusals.as_slice() else {
        return Err(format!("only the kept lookup is refused, got {errors:?}").into());
    };
    assert!(
        refusal.message.contains("Lock"),
        "the refusal names the value type: {refusal:?}"
    );
    assert!(
        refusal
            .hints
            .iter()
            .any(|hint| hint.contains("only reads the value works")),
        "the refusal names the lookup that works: {refusal:?}"
    );
    let kept_start = source.find("table.get(key)").ok_or("missing the kept lookup")?;
    assert_eq!(refusal.span.start, kept_start, "the refusal points at the kept lookup");
    Ok(())
}
