//! Checked single-field model validation and its retained canonical-layout boundary.

use super::build;

/// A sole supplied model field uses a distinct aggregate extraction, preserving fail-fast callers and locals.
#[test]
fn single_field_newtype_ingress_retains_aggregate_extraction() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/snapshots_newtypes_and_serde/newtype_implicit_coercion.incn"
        ),
        &["m", "single_field_ingress"],
    )?;
    let main = super::body_named(&module, "main")?;
    let snapshot = main.render_snapshot();
    assert!(snapshot.contains("newtype_field_hook_extract"), "{snapshot}");
    assert!(snapshot.contains("newtype_hook_extract"), "{snapshot}");
    assert!(!snapshot.contains("unsupported("), "{snapshot}");
    Ok(())
}

/// A constructor with two checked fields still refuses until supplied-field collection is represented.
#[test]
fn multiple_checked_model_fields_remain_refused() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"type Positive = newtype int:
    def from_underlying(value: int) -> Result[Self, ValidationError]:
        return Ok(Positive(value))

model Pair:
    left: Positive
    right: Positive

def main() -> None:
    pair = Pair(left=1, right=2)
    println(pair.left.0)
"#,
        &["m", "multiple_checked_fields"],
    )?;
    let main = super::body_named(&module, "main")?;
    let snapshot = main.render_snapshot();
    assert!(
        snapshot.contains("checked-newtype aggregate-field validation"),
        "{snapshot}"
    );
    assert!(!snapshot.contains("newtype_field_hook_extract"), "{snapshot}");
    Ok(())
}

/// A checked coercion alone cannot authorize field extraction after its canonical model layout is missing.
#[test]
fn single_field_newtype_ingress_requires_canonical_field_layout() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_newtypes_and_serde/newtype_implicit_coercion.incn"
    );
    let tokens = super::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["m".to_owned(), "missing_checked_field".to_owned()];
    let mut checker = super::TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let mut facts = checker.type_info().clone();
    assert!(!facts.declarations.member_declaration_identities.is_empty());
    facts.declarations.member_declaration_identities.clear();
    let module = super::build_body_ir_module_v0(&program, &path, &facts);
    let snapshot = super::body_named(&module, "main")?.render_snapshot();
    assert!(
        snapshot.contains("checked-newtype aggregate-field validation"),
        "{snapshot}"
    );
    assert!(!snapshot.contains("newtype_field_hook_extract"), "{snapshot}");
    Ok(())
}
