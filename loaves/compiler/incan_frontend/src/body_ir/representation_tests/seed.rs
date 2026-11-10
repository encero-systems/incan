use super::*;

/// Prepare identical checked inputs for the old module pass and the independent native operations. This whole
/// seed construction exists only in the verification harness and is never a record producer implementation.
pub(super) fn checked_seed(
    source: &str,
) -> Result<(crate::ast::Program, TypeCheckInfo, bir::BodyIrModule), Box<dyn std::error::Error>> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let facts = checker.into_type_info();
    let providers = ProviderOperationCatalog::new();
    let context = BodyIrContext {
        modules: &[],
        provider_operations: &providers,
        registry_package: None,
        library_package: None,
        include_inline_tests: false,
    };
    let seed = super::build_module_seed(&program, &[], &facts, &context);
    Ok((program, facts, seed))
}
