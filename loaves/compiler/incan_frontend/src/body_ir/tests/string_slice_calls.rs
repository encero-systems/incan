//! Checked primitive string slice calls retain private result and canonical operation facts.

use super::{TypeChecker, bir, body_named, build, lexer, parser};
use crate::symbols::ResolvedType;

/// The complete original string operations fixture closes the explicit signed-end slice's result.
#[test]
fn complete_string_operations_retain_closed_slice_result() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_collections_and_strings/string_operations.incn"
    );
    let module = build(source, &["m", "complete_string_slice"])?;
    let body = body_named(&module, "test_string_slice")?;
    assert!(
        !serde_json::to_string(&body.locals)?.contains("\"Unknown\""),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

/// The primitive slice's private result is str while the ordinary method snapshot keeps legacy's Unknown.
#[test]
fn checked_string_slice_preserves_legacy_result_snapshot() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "def main() -> None:\n    text: str = \"héllo\"\n    mid: str = text.__slice__(1, -1)\n    println(mid)\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["m".into(), "string_slice_snapshot".into()]));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let start = source.find("text.__slice__(1, -1)").ok_or("missing slice call")?;
    let key = (start, start + "text.__slice__(1, -1)".len());
    assert_eq!(
        checker.type_info().expressions.expr_types.get(&key),
        Some(&ResolvedType::Unknown)
    );
    assert_eq!(
        checker.type_info().calls.body_ir_call_result_types.get(&key),
        Some(&ResolvedType::Str)
    );
    Ok(())
}

/// Invalid primitive bounds cannot acquire a string slice result from their spelling.
#[test]
fn non_integer_slice_bounds_keep_result_unproven() -> Result<(), Box<dyn std::error::Error>> {
    assert_open_slice("def main() -> None:\n    text: str = \"héllo\"\n    mid = text.__slice__(\"one\", -1)\n")
}

/// The bounded two-argument contract does not guess missing endpoints or discard additional arguments.
#[test]
fn unsupported_slice_arities_keep_result_unproven() -> Result<(), Box<dyn std::error::Error>> {
    for arguments in ["", "1", "1, 2, 3"] {
        let source = format!("def main() -> None:\n    text: str = \"héllo\"\n    mid = text.__slice__({arguments})\n");
        assert_open_slice(&source)?;
    }
    Ok(())
}

/// A source-owned method with the same spelling retains its own integer result and declaration identity.
#[test]
fn source_slice_method_does_not_acquire_primitive_contract() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Slicer:\n    def __slice__(self, start: int, end: int) -> int:\n        return start + end\ndef main() -> None:\n    value = Slicer()\n    println(value.__slice__(1, 2))\n";
    let module = build(source, &["m", "source_slice_method"])?;
    let body = body_named(&module, "main")?;
    let selected = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                ..
            } if target.name == "__slice__" => target.canonical.as_ref(),
            _ => None,
        })
        .ok_or("missing source slice method identity")?;
    assert!(
        matches!(selected.origin, incan_semantics_core::SymbolOrigin::Module(_)),
        "{selected:?}"
    );
    Ok(())
}

/// A synthetic protocol probe preserves the selected primitive member and result while leaving legacy tables alone.
#[test]
fn string_slice_member_survives_call_site_transactions() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def main() -> None:\n    text: str = \"héllo\"\n    mid = text.__slice__(1, -1)\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut facts = checker.type_info().clone();
    let start = source.find("text.__slice__(1, -1)").ok_or("missing slice call")?;
    let span = crate::ast::Span::new(start, start + "text.__slice__(1, -1)".len());
    let key = (span.start, span.end);
    let target = facts
        .calls
        .body_ir_string_slice_members
        .get(&key)
        .ok_or("missing private slice member")?
        .clone();
    let ordinary = facts.expressions.expr_types.clone();
    let saved = facts.take_call_site_facts(span);
    assert!(!facts.calls.body_ir_string_slice_members.contains_key(&key));
    facts.calls.body_ir_string_slice_members.insert(key, target.clone());
    facts.clear_body_ir_call_site_facts(span);
    assert!(!facts.calls.body_ir_string_slice_members.contains_key(&key));
    facts.restore_call_site_facts(span, saved);
    assert_eq!(facts.calls.body_ir_string_slice_members.get(&key), Some(&target));
    assert_eq!(
        facts.calls.body_ir_call_result_types.get(&key),
        Some(&ResolvedType::Str)
    );
    assert_eq!(facts.expressions.expr_types, ordinary);
    Ok(())
}

/// A normally checked but unsupported method shape keeps its result open in Body IR.
fn assert_open_slice(source: &str) -> Result<(), Box<dyn std::error::Error>> {
    let module = build(source, &["m", "unproven_string_slice"])?;
    let body = body_named(&module, "main")?;
    assert!(
        serde_json::to_string(&body.locals)?.contains("\"Unknown\""),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}
