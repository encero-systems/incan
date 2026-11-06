//! Body IR-only identities for checked integer helpers and inferred generic Clone.

use super::{bir, body_named, build};
use incan_semantics_core::SymbolOrigin;

/// Every checked integer helper names the canonical numeric registry operation, independently of legacy references.
#[test]
fn integer_helpers_retain_builtin_identities() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/ownership_numerics_and_bounds/integer_overflow_helpers_and_explicit_resizes.incn"
        ),
        &["m", "integer_helper_identities"],
    )?;
    let body = body_named(&module, "main")?;
    for name in [
        "checked_add",
        "checked_sub",
        "checked_mul",
        "checked_pow",
        "wrapping_add",
        "wrapping_sub",
        "wrapping_mul",
        "wrapping_pow",
        "saturating_add",
        "saturating_sub",
        "saturating_mul",
        "saturating_pow",
    ] {
        let target = body
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                bir::StatementKind::Call {
                    callee: bir::Callee::Method(target),
                    ..
                } if target.name == name => Some(target),
                _ => None,
            })
            .ok_or_else(|| format!("missing checked method {name}"))?;
        let identity = target
            .canonical
            .as_ref()
            .ok_or_else(|| format!("missing identity for {name}"))?;
        assert_eq!(identity.origin, SymbolOrigin::Builtin);
        assert_eq!(identity.declaration_name, format!("u8.{name}"));
    }
    Ok(())
}

/// A generic parameter's inferred Clone is retained as a builtin operation without changing the checker tables.
#[test]
fn inferred_clone_retains_builtin_identity() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "pub def duplicate[T](value: T) -> T:\n    return value.clone()\n",
        &["m", "inferred_clone_identity"],
    )?;
    let body = body_named(&module, "duplicate")?;
    let target = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                ..
            } => Some(target),
            _ => None,
        })
        .ok_or("missing generic Clone call")?;
    let identity = target.canonical.as_ref().ok_or("missing generic Clone identity")?;
    assert_eq!(identity.origin, SymbolOrigin::Builtin);
    assert_eq!(identity.declaration_name, "Clone.clone");
    Ok(())
}

/// A source method sharing an integer helper's spelling keeps its checked source declaration.
#[test]
fn source_helper_spelling_keeps_source_identity() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "model Counter:\n    value: int\n    def checked_add(self, amount: int) -> int:\n        return self.value + amount\n\ndef main() -> None:\n    counter = Counter(value=2)\n    println(counter.checked_add(3))\n",
        &["m", "source_helper_spelling"],
    )?;
    let body = body_named(&module, "main")?;
    let target = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                ..
            } if target.name == "checked_add" => Some(target),
            _ => None,
        })
        .ok_or("missing source checked_add call")?;
    let identity = target.canonical.as_ref().ok_or("missing source checked_add identity")?;
    assert!(matches!(identity.origin, SymbolOrigin::Module(_)));
    assert_eq!(identity.declaration_name, "checked_add");
    Ok(())
}
