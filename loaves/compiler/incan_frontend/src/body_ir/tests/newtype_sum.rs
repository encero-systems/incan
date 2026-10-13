//! Canonical numeric newtype iterator terminal selection and post-exhaustion reconstruction.

use super::build;

const SOURCE: &str = r#"type Money = newtype int

type Positive = newtype int:
    def from_underlying(value: int) -> Result[Self, ValidationError]:
        if value <= 0:
            return Err(ValidationError("positive required"))
        return Ok(Positive(value))

def raw(items: list[Money]) -> Money:
    return items.iter().sum()

def checked(items: list[Positive]) -> Positive:
    return items.iter().sum()
"#;

/// A source `sum` member requires its own retained dispatch and must not acquire the automatic numeric bridge.
#[test]
fn numeric_newtype_sum_with_source_member_stays_outside_automatic_profile() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"type Money = newtype int:
    def sum(self) -> Money:
        return self

def total(items: list[Money]) -> Money:
    return items.iter().sum()
"#,
        &["m", "numeric_newtype_source_sum"],
    )?;
    let body = super::body_named(&module, "total")?.render_snapshot();
    assert!(!body.contains("newtype_sum_add"), "{body}");
    Ok(())
}

/// The accepted terminal retains an integer payload projection and reconstructs only after its consumer call.
#[test]
fn numeric_newtype_sum_retains_projection_and_final_reconstruction() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(SOURCE, &["m", "numeric_newtype_sum"])?;
    let raw = super::body_named(&module, "raw")?.render_snapshot();
    let checked = super::body_named(&module, "checked")?.render_snapshot();
    assert!(raw.contains("newtype_sum_add"), "{raw}");
    assert!(!raw.contains("newtype_sum_extract"), "{raw}");
    assert!(checked.contains("newtype_sum_add"), "{checked}");
    assert!(checked.contains("newtype_sum_extract"), "{checked}");
    assert!(!checked.contains("newtype_hook_extract"), "{checked}");
    assert!(
        checked.contains("validated newtype construction failed: Positive::from_underlying"),
        "{checked}"
    );
    let consumer = raw.find("newtype_sum_add").ok_or("missing numeric sum consumer")?;
    let reconstruction = raw
        .rfind("constructor(Money)")
        .ok_or("missing final Money construction")?;
    assert!(consumer < reconstruction, "{raw}");
    let body = super::body_named(&module, "raw")?;
    let loop_index = body
        .block
        .stmts
        .iter()
        .position(|statement| matches!(statement.kind, super::bir::StatementKind::Loop { .. }))
        .ok_or("missing sum loop")?;
    let super::bir::StatementKind::Loop { body: loop_body } = &body.block.stmts[loop_index].kind else {
        return Err("missing typed sum loop".into());
    };
    let super::bir::StatementKind::IterNext {
        iterator: super::bir::Operand::Place(read),
        ..
    } = &loop_body.stmts[0].kind
    else {
        return Err("missing typed sum poll".into());
    };
    let iterator = read.place.local_id().ok_or("missing local sum iterator")?;
    assert!(
        body.block.stmts[..loop_index]
            .iter()
            .any(|statement| matches!(&statement.kind,
        super::bir::StatementKind::Assign { place, rvalue: super::bir::Rvalue::Use(super::bir::Operand::Place(source)) }
        if place.local_id() == Some(iterator) && source.fact == super::bir::OwnershipFact::Move))
    );
    assert!(
        body.block.stmts[loop_index + 1..]
            .iter()
            .any(|statement| matches!(statement.kind,
        super::bir::StatementKind::Drop { local } if local == iterator))
    );
    Ok(())
}

/// Removing an owner's canonical binding cannot authorize a projection from a numerical-looking field.
#[test]
fn numeric_newtype_sum_requires_canonical_owner() -> Result<(), Box<dyn std::error::Error>> {
    let tokens = super::lexer::lex(SOURCE).map_err(|errors| format!("{errors:?}"))?;
    let program = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["m".to_owned(), "numeric_sum_missing_owner".to_owned()];
    let mut checker = super::TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let mut facts = checker.type_info().clone();
    assert!(facts.declarations.named_type_identities.remove("Money").is_some());
    let module = super::build_body_ir_module_v0(&program, &path, &facts);
    let raw = super::body_named(&module, "raw")?.render_snapshot();
    assert!(!raw.contains("newtype_sum_add"), "{raw}");
    assert!(raw.contains("unsupported"), "{raw}");
    Ok(())
}

/// Independent missing checked selection and mismatched hook provenance each retain a named refusal.
#[test]
fn numeric_newtype_sum_requires_complete_selection() -> Result<(), Box<dyn std::error::Error>> {
    let tokens = super::lexer::lex(SOURCE).map_err(|errors| format!("{errors:?}"))?;
    let program = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["m".to_owned(), "numeric_sum_missing_selection".to_owned()];
    let mut checker = super::TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    for corruption in 0..3 {
        let mut facts = checker.type_info().clone();
        match corruption {
            0 => facts.body_ir_iterator_sum_items.clear(),
            1 => {
                let hook = facts
                    .declarations
                    .newtype_construction
                    .get_mut("Positive")
                    .and_then(|facts| facts.checked_constructor_identity.as_mut())
                    .ok_or("missing checked hook identity")?;
                hook.kind = super::SemanticSourceTargetKind::Function;
            }
            _ => {
                facts
                    .declarations
                    .newtype_construction
                    .get_mut("Positive")
                    .ok_or("missing checked construction facts")?
                    .underlying = crate::symbols::ResolvedType::Float;
            }
        }
        let module = super::build_body_ir_module_v0(&program, &path, &facts);
        let checked = super::body_named(&module, "checked")?.render_snapshot();
        assert!(!checked.contains("newtype_sum_add"), "{corruption}: {checked}");
        assert!(
            checked.contains("numeric-newtype sum without complete checked local selection"),
            "{corruption}: {checked}"
        );
    }
    Ok(())
}
