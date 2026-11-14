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

/// A module records its storage's initialization order where it is built: source order, across the collectors that
/// gather statics, computed constants and decorator replacements separately. Readers order by that record alone.
pub(super) fn assert_storage_records_its_initialization_order() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Pair:\n    left: int = 1\nconst PAIRS = [Pair(left=1)]\ndef keep(inner: Callable[int, int]) -> Callable[int, int]:\n    return inner\n@keep\ndef bump(value: int) -> int:\n    return value + 1\nstatic counter: int = 0\nconst LATER = [Pair(left=2)]\ndef read() -> int:\n    return counter\n";
    let (_, _, mut seed) = checked_seed(source)?;
    let recorded = |module: &bir::BodyIrModule| {
        module
            .static_declarations
            .iter()
            .map(|storage| (storage.canonical.declaration_name.clone(), storage.initialization_order))
            .collect::<Vec<_>>()
    };
    let expected = vec![
        ("PAIRS".to_owned(), 0),
        ("bump".to_owned(), 1),
        ("counter".to_owned(), 2),
        ("LATER".to_owned(), 3),
    ];
    assert_eq!(recorded(&seed), expected);
    seed.static_declarations.reverse();
    bir::order_static_initialization(&mut seed.static_declarations);
    assert_eq!(recorded(&seed), expected);
    Ok(())
}
