//! Canonical numeric newtype iterator terminal selection and post-exhaustion reconstruction.

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

/// A source `sum` member requires its own retained dispatch and must not acquire the automatic numeric bridge in
/// either local or project mode.
#[test]
fn numeric_newtype_sum_with_source_member_stays_outside_automatic_profile() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"type Money = newtype int:
    def sum(self) -> Money:
        return self

def total(items: list[Money]) -> Money:
    return items.iter().sum()
"#;
    let tokens = super::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["m".to_owned(), "numeric_newtype_source_sum".to_owned()];
    let mut checker = super::TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    for project in [false, true] {
        let module = if project {
            super::build_body_ir_module_v0_with_executable_context(&program, &path, checker.type_info(), &[])
        } else {
            super::build_body_ir_module_v0(&program, &path, checker.type_info())
        };
        let body = super::body_named(&module, "total")?.render_snapshot();
        assert!(!body.contains("newtype_sum_add"), "project={project}: {body}");
    }
    Ok(())
}

/// Local and project terminals retain an integer payload projection and reconstruct the checked canonical owner only
/// after consumption; iterator moves, cleanup, and validation remain explicit in both profiles.
#[test]
fn numeric_newtype_sum_retains_projection_and_final_reconstruction() -> Result<(), Box<dyn std::error::Error>> {
    let tokens = super::lexer::lex(SOURCE).map_err(|errors| format!("{errors:?}"))?;
    let program = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["m".to_owned(), "numeric_newtype_sum".to_owned()];
    let mut checker = super::TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let facts = checker.type_info();
    let owner = facts
        .declarations
        .named_type_identities
        .get("Money")
        .ok_or("missing checked Money owner")?;
    let hook = facts
        .declarations
        .newtype_construction
        .get("Positive")
        .and_then(|facts| facts.checked_constructor_identity.as_ref())
        .ok_or("missing checked Positive hook")?;
    for project in [false, true] {
        let module = if project {
            super::build_body_ir_module_v0_with_executable_context(&program, &path, facts, &[])
        } else {
            super::build_body_ir_module_v0(&program, &path, facts)
        };
        let body = super::body_named(&module, "raw")?;
        let raw = body.render_snapshot();
        let checked_body = super::body_named(&module, "checked")?;
        let checked = checked_body.render_snapshot();
        assert!(raw.contains("newtype_sum_add"), "project={project}: {raw}");
        assert!(!raw.contains("newtype_sum_extract"), "{raw}");
        assert!(checked.contains("newtype_sum_add"), "project={project}: {checked}");
        assert!(checked.contains("newtype_sum_extract"), "{checked}");
        assert!(!checked.contains("newtype_hook_extract"), "{checked}");
        if !project {
            assert!(
                checked.contains("validated newtype construction failed: Positive::from_underlying"),
                "{checked}"
            );
            let consumer = raw.find("newtype_sum_add").ok_or("missing numeric sum consumer")?;
            let reconstruction = raw
                .rfind("constructor(Money)")
                .ok_or("missing final Money construction")?;
            assert!(consumer < reconstruction, "{raw}");
        }
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
        let (reconstruction_index, target) = body
            .block
            .stmts
            .iter()
            .enumerate()
            .find_map(|(index, statement)| match &statement.kind {
                super::bir::StatementKind::Assign {
                    rvalue: super::bir::Rvalue::Aggregate(super::bir::AggregateKind::Constructor(target), _),
                    ..
                } if target.canonical.as_ref() == Some(owner) => Some((index, target)),
                _ => None,
            })
            .ok_or("missing canonical Money reconstruction")?;
        let declaration = module
            .nominal_declarations
            .iter()
            .find(|declaration| &declaration.canonical == owner)
            .ok_or("missing canonical Money layout")?;
        assert!(loop_index < reconstruction_index);
        assert_eq!(
            target.direct_declaration_id.as_ref(),
            Some(&declaration.direct_declaration_id)
        );
        assert_eq!(target.canonical_field_layout.as_ref(), Some(&declaration.fields));
        assert!(
            checked_body
                .block
                .stmts
                .iter()
                .any(|statement| matches!(&statement.kind,
            super::bir::StatementKind::Call {
                callee: super::bir::Callee::Function(super::bir::CallableTarget::Named(target)), ..
            } if target.canonical.as_ref() == Some(hook))),
            "project={project}: {checked}"
        );
    }
    Ok(())
}

/// Removing an owner's canonical binding cannot authorize a projection from a numerical-looking field in either
/// local or project mode.
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
    for project in [false, true] {
        let module = if project {
            super::build_body_ir_module_v0_with_executable_context(&program, &path, &facts, &[])
        } else {
            super::build_body_ir_module_v0(&program, &path, &facts)
        };
        let raw = super::body_named(&module, "raw")?.render_snapshot();
        assert!(!raw.contains("newtype_sum_add"), "project={project}: {raw}");
        assert!(raw.contains("unsupported"), "project={project}: {raw}");
    }
    Ok(())
}

/// Independent missing checked selection, mismatched hook provenance, and wrong underlying types retain the same
/// named refusal in both local and project modes.
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
        for project in [false, true] {
            let module = if project {
                super::build_body_ir_module_v0_with_executable_context(&program, &path, &facts, &[])
            } else {
                super::build_body_ir_module_v0(&program, &path, &facts)
            };
            let checked = super::body_named(&module, "checked")?.render_snapshot();
            assert!(
                !checked.contains("newtype_sum_add"),
                "project={project}, corruption={corruption}: {checked}"
            );
            assert!(
                checked.contains("numeric-newtype sum without complete checked local selection"),
                "project={project}, corruption={corruption}: {checked}"
            );
        }
    }
    Ok(())
}
