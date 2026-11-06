//! Inspect direct argument facts and profile refusal independently of full native execution qualification.

use super::bir;

mod aliases_and_views;
mod conditional;
mod input_projection;

/// Inspect the checked projection directly: admitted source must have no representation gap, while a deferred
/// profile retains its source diagnostic. Removing identity after checking deliberately tests the next boundary.
fn staged(source: &str, remove_call_proof: bool) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = crate::typechecker::TypeChecker::new();
    let checked = checker.check_program(&program);
    if let Err(errors) = &checked {
        assert!(
            errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{errors:?}"
        );
    }
    let mut facts = checker.into_type_info();
    if remove_call_proof {
        for binding in facts.declarations.function_bindings_by_span.values_mut() {
            if binding
                .identity
                .as_ref()
                .is_some_and(|identity| identity.declaration_name == "main")
            {
                binding.identity = None;
            }
        }
    }
    let providers = super::super::ProviderOperationCatalog::new();
    let context = super::super::BodyIrContext {
        modules: &[],
        provider_operations: &providers,
        registry_package: None,
        library_package: None,
    };
    let module = super::super::build_module_seed(&program, &[], &facts, &context);
    if checked.is_ok() && !remove_call_proof {
        for body in &module.bodies {
            assert!(
                body.first_representation_gap().is_none(),
                "{}: {:?}",
                body.name,
                body.first_representation_gap()
            );
        }
    }
    Ok(module)
}

/// Positive execution profiles must pass source checking as well as materialize complete Body IR.
fn admitted(source: &str) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    crate::typechecker::TypeChecker::new()
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    staged(source, false)
}
