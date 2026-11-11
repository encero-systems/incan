use super::*;

/// Prove the settled read boundary feeds scalar function seeds without another body's facts. The expected module
/// is constructed independently; empty declaration/layout inputs are justified by this fixture, not a general producer.
pub(super) fn assert_settled_function_seeds() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def chosen(value: int) -> int:\n    if value > 0:\n        return value + 1\n    return value - 1\ndef sibling(value: str) -> str:\n    return value\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let providers = ProviderOperationCatalog::new();
    let context = BodyIrContext {
        modules: &[],
        provider_operations: &providers,
        registry_package: None,
        library_package: None,
        include_inline_tests: false,
    };
    let expected = super::build_module_seed(&program, &[], checker.type_info(), &context);
    let mut actual = expected.clone();
    actual.bodies.clear();
    let identity = super::body_ir_module_identity(&[]);
    let mut declaration_reads = 0;
    for declaration in &program.declarations {
        let crate::ast::Declaration::Function(function) = &declaration.node else {
            return Err("the seed fixture must contain only scalar functions".into());
        };
        let checked = checker
            .checked_body(declaration.span)
            .ok_or("missing settled function")?;
        assert!(!checked.is_refused());
        let binding = checker
            .declaration_view()
            .function(declaration.span)
            .ok_or("missing signature")?;
        declaration_reads += 1;
        let declaration_owner = checker
            .checked_declaration(declaration.span)
            .ok_or("missing checked declaration owner")?;
        let references = checked.reference_inputs(declaration_owner)?;
        let mut info = TypeCheckInfo {
            expressions: checked.expressions().clone(),
            references,
            calls: checked.calls().clone(),
            protocols: checked.protocols().clone(),
            rust: checked.rust().clone(),
            traits: checked.traits().clone(),
            c_abi: checked.c_abi().clone(),
            ..TypeCheckInfo::default()
        };
        info.declarations
            .function_bindings_by_span
            .insert(declaration.span, binding.clone());
        if let Some(canonical) = &binding.identity {
            info.declarations
                .declaration_identities
                .insert(declaration.span, canonical.clone());
        }
        let facts = super::BodyIrLoweringFacts {
            published_methods: &[],
            model_constants: &Default::default(),
            local_enum_declarations: &Default::default(),
            published_callable_returns: &Default::default(),
            published_callable_defaults: &Default::default(),
            published_newtype_returns: &Default::default(),
            published_storage_types: &Default::default(),
            published_constants: &Default::default(),
            type_info: &info,
            runtime_type_conflicts: &Default::default(),
            retain_nominal_carriers: true,
            function_default_sources: &Default::default(),
            local_function_declarations: &Default::default(),
            local_nongeneric_functions: &Default::default(),
            local_receiver_methods: &[],
            local_trait_partials: &[],
            local_nominal_declarations: &Default::default(),
            local_fieldless_enum_declarations: &Default::default(),
            local_value_enum_declarations: &Default::default(),
            module_identity: &identity,
            registry_package: None,
            provider_operations: &providers,
        };
        let mut body = super::lower_function_body(function, declaration.span, &facts);
        super::representation_facts::retain(&mut body);
        actual.bodies.push(body);
    }
    let first = program.declarations.first().ok_or("missing first owner")?;
    let second = program.declarations.get(1).ok_or("missing second owner")?;
    let body = checker.checked_body(first.span).ok_or("missing first body")?;
    let other = checker
        .checked_declaration(second.span)
        .ok_or("missing second declaration")?;
    assert!(matches!(
        body.reference_inputs(other),
        Err(CheckedBodyInputError::DifferentOwner)
    ));
    assert!(matches!(
        body.reference_inputs(body),
        Err(CheckedBodyInputError::WrongOwnerRole)
    ));
    assert_refused_reference_inputs()?;
    assert_eq!(actual.bodies.len(), 2);
    assert_same_module(&expected, &actual)?;
    eprintln!(
        "settled-input-fixture\tbodies={}\tselected_signatures={declaration_reads}",
        actual.bodies.len()
    );
    Ok(())
}

/// Retain both complete native and publication streams when the qualification harness requests evidence. The
/// sequence identifies comparisons only; it is not an evaluation counter or a persistent compiler authority.
pub(super) fn capture_module_baseline(
    expected: &bir::BodyIrModule,
    actual: &bir::BodyIrModule,
    before: &str,
    after: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COMPARISON: AtomicUsize = AtomicUsize::new(0);
    let Some(directory) = std::env::var_os("INCAN_BODY_IR_BASELINE_DIR") else {
        return Ok(());
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory)?;
    let sequence = COMPARISON.fetch_add(1, Ordering::Relaxed);
    std::fs::write(directory.join(format!("{sequence:04}-expected.native.txt")), before)?;
    std::fs::write(directory.join(format!("{sequence:04}-actual.native.txt")), after)?;
    std::fs::write(
        directory.join(format!("{sequence:04}-expected.publication.bin")),
        postcard::to_stdvec(expected)?,
    )?;
    std::fs::write(
        directory.join(format!("{sequence:04}-actual.publication.bin")),
        postcard::to_stdvec(actual)?,
    )?;
    Ok(())
}

/// A refused body remains a semantic predecessor refusal; the native read adapter cannot turn it into executable
/// inputs even when its own declaration's signature and default checks succeeded.
fn assert_refused_reference_inputs() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def invalid(value: int) -> int:\n    return \"wrong\"\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_err());
    let declaration = program.declarations.first().ok_or("missing refused owner")?;
    let body = checker.checked_body(declaration.span).ok_or("missing refused body")?;
    let owner = checker
        .checked_declaration(declaration.span)
        .ok_or("missing refused declaration")?;
    assert!(matches!(
        body.reference_inputs(owner),
        Err(CheckedBodyInputError::RefusedOwner)
    ));
    Ok(())
}

/// Extend the existing representation baseline to nonempty declaration defaults, nominal construction, calls and
/// nested lexical closures. The missing-owner control proves a comparison cannot silently skip an eligible body.
pub(super) fn assert_free_function_input_baseline() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Value:\n    number: int = 1\n    offset: int = 2\ndef increment(value: int) -> int:\n    return value + 1\ndef chosen(value: int = increment(2)) -> int:\n    item = Value(number=value)\n    callback: Callable[int, int] = (number) => number + item.number\n    return callback(value)\ndef sibling() -> int:\n    values = [1, 2]\n    mut total = 0\n    for value in values:\n        total += increment(value)\n    return total\ndef refined() -> int:\n    mut values = []\n    values.append(1)\n    return len(values)\ndef defaults(prefix: int, items: list[float] = []) -> int:\n    return prefix\ndef residual() -> int:\n    callback = partial defaults(prefix=1)\n    return callback()\npreset = partial defaults(prefix=2)\ndef preset_use() -> int:\n    return preset()\ndef constructor_partial() -> int:\n    make = partial Value(number=3)\n    return make().number\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let owners = checker.body_ir_baseline_inputs();
    let mut info = checker.into_type_info();
    let providers = ProviderOperationCatalog::new();
    let context = BodyIrContext {
        modules: &[],
        provider_operations: &providers,
        registry_package: None,
        library_package: None,
        include_inline_tests: false,
    };
    let reports = tempfile::tempdir()?;
    std::fs::write(reports.path().join("interrupted.tmp"), b"{\"equal\":")?;
    assert!(super::verify_free_function_inputs(&program, &[], &info, &context, reports.path()).is_err());
    info.body_ir_baseline_inputs = Some(owners);
    super::verify_free_function_inputs(&program, &[], &info, &context, reports.path())?;
    let mut comparisons = 0;
    for entry in std::fs::read_dir(reports.path())? {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "jsonl") {
            for line in std::fs::read_to_string(path)?.lines() {
                let row: serde_json::Value = serde_json::from_str(line)?;
                assert_eq!(row["equal"], true, "{row}");
                comparisons += 1;
            }
        }
    }
    assert_eq!(comparisons, 16);
    let module = super::build_body_ir_module(&program, &[], &info, &context);
    let expected = module.bodies.first().ok_or("missing comparator control body")?;
    let mut changed = expected.clone();
    changed.native_attributes.push(bir::NativeAttribute {
        path: "selected::attribute".to_owned(),
        arguments: Vec::new(),
    });
    assert_eq!(postcard::to_stdvec(expected)?, postcard::to_stdvec(&changed)?);
    let failure = super::free_function_baseline::record_comparison(expected, &changed, "negative", reports.path())
        .err()
        .ok_or("free-function baseline omitted a native-only field")?;
    assert!(failure.to_string().contains("native_attributes"));
    Ok(())
}
