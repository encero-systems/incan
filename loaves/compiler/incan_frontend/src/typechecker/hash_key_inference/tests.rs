//! Cold comparisons against the original syntax-rescanning inference, including ordered forwarding cycles.

use super::{HashKeyCallable, TypeChecker};
use std::collections::{BTreeSet, HashMap};

/// Compare the complete inferred requirement map with a fresh legacy fixed point, then check that propagation never
/// revisits syntax. This covers the pre-body signature influence boundary, not the completed record service.
#[test]
fn influence_facts_match_fresh_legacy_inference() -> Result<(), Box<dyn std::error::Error>> {
    let sources = [
        "def target[T with (Eq, Hash)](value: T) -> None:\n    pass\ndef target[T](value: T, other: int) -> None:\n    keys = {value}\ndef caller[U](value: U) -> None:\n    target(value)\n",
        "def direct[T](key: T) -> None:\n    values = {key}\n",
        "def first[T](key: T) -> None:\n    second(key)\ndef second[U](value: U) -> None:\n    first(value)\n    values = {value}\n",
        "def last[T](value: T) -> None:\n    middle(value)\ndef middle[U](key: U) -> None:\n    first(key)\ndef first[V](item: V) -> None:\n    values = {item}\n",
        "def first[T](key: T) -> None:\n    second(key)\ndef second[U](value: U) -> None:\n    first(value)\n",
        "def target[T](value: List[T]) -> None:\n    keys = set(value)\ndef caller[U](items: List[U]) -> None:\n    target(value=items)\n",
        "def target[T](key: T) -> None:\n    values = {key}\ndef caller[U](value: U) -> None:\n    target(unknown())\n",
        "model Owner:\n    def target[T](self, key: T) -> None:\n        values = {key}\n    def caller[U](self, value: U) -> None:\n        self.target(value)\n",
    ];
    for source in sources {
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker.reset_program_check(&program, false);
        checker.prepare_program_declaration_names(&program);
        checker.collect_declarations_for_check(&program.declarations);
        checker.resolve_forward_type_references(&program.declarations);
        let targets = checker.hash_key_scan_targets(&program.declarations);
        let mut legacy = HashMap::<HashKeyCallable, BTreeSet<String>>::new();
        loop {
            let mut changed = false;
            for target in &targets {
                for parameter in checker.legacy_hashed_type_params(target, &targets, &legacy) {
                    changed |= legacy.entry(target.callable.clone()).or_default().insert(parameter);
                }
            }
            if !changed {
                break;
            }
        }
        let influences = checker.hash_key_influences(&targets);
        let (actual, counters) = super::resolve_influences(&influences);
        assert_eq!(actual, legacy, "{source}");
        assert_eq!(counters.body_scans, targets.len(), "{source}");
        assert!(counters.iterations > 0);
        assert_cold_check(source)?;
    }
    assert_cold_check(include_str!(
        "../../../../../../workspaces/contracts/fixtures/behavior/ownership_numerics_and_bounds/generic_functions_hash_through_dict_and_set_types.incn"
    ))?;
    assert_cold_check(include_str!(
        "../../../../../../workspaces/contracts/fixtures/behavior/cli_values_and_calls/capturing_closures_in_function_types.incn"
    ))?;
    assert_cold_check(include_str!(
        "../../../../../../workspaces/contracts/fixtures/behavior/codegen_functions_and_aliases/decorated_generic_closed_signatures.incn"
    ))?;
    assert_body_local_matrix()?;
    assert_environment_matrix()?;
    assert_imported_influences()?;
    assert_check_work_scope()?;
    measure_influence_scan()?;

    Ok(())
}

/// Compare downstream checked facts and complete ordered diagnostics after independently checking the same source.
/// Existing contract programs carry the language coverage; no expectation in those programs is changed.
fn assert_cold_check(source: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut legacy = TypeChecker::new();
    let mut candidate = TypeChecker::new();
    assert_checkers(&program, &mut candidate, &mut legacy)
}

/// Compare two independent checker activations after their selected imports have been prepared. The complete ordered
/// diagnostics are compared directly, including notes and secondary declarations.
fn assert_checkers(
    program: &crate::ast::Program,
    candidate: &mut TypeChecker,
    legacy: &mut TypeChecker,
) -> Result<bool, Box<dyn std::error::Error>> {
    let source = "fresh legacy versus extracted influences";
    let candidate_result = candidate.check_program(program);
    let legacy_result = legacy.legacy_check_program(program);
    assert_eq!(candidate_result, legacy_result, "{source}");
    if program.declarations.iter().any(|declaration| {
        matches!(&declaration.node, crate::ast::Declaration::Function(function) if function.name == "sum_items")
    }) {
        assert!(candidate_result.is_ok());
        assert_eq!(candidate.type_info.body_ir_iterator_sum_items.len(), 1);
    }
    assert_eq!(candidate.warnings, legacy.warnings, "{source}");
    assert_eq!(
        candidate.type_info.declarations.declaration_identities,
        legacy.type_info.declarations.declaration_identities
    );
    assert_eq!(
        candidate.type_info.declarations.hir_bindings_by_span,
        legacy.type_info.declarations.hir_bindings_by_span
    );
    assert_eq!(
        candidate.type_info.declarations.type_alias_targets,
        legacy.type_info.declarations.type_alias_targets
    );
    assert_eq!(candidate.type_info.import_bindings, legacy.type_info.import_bindings);
    assert_eq!(candidate.check_work(), legacy.check_work(), "{source}");
    assert_eq!(
        candidate.type_info.calls.closure_holding_params, legacy.type_info.calls.closure_holding_params,
        "{source}"
    );
    assert_eq!(
        candidate.type_info.calls.closure_returning_callables, legacy.type_info.calls.closure_returning_callables,
        "{source}"
    );

    assert_eq!(
        candidate.hash_key_inference.type_params, legacy.hash_key_inference.type_params,
        "{source}"
    );
    assert_eq!(
        candidate.hash_key_inference.function_bounds, legacy.hash_key_inference.function_bounds,
        "{source}"
    );
    assert_eq!(
        candidate.type_info.declarations.function_bindings_by_span,
        legacy.type_info.declarations.function_bindings_by_span,
        "{source}"
    );
    assert_eq!(
        candidate.type_info.declarations.method_bindings_by_span, legacy.type_info.declarations.method_bindings_by_span,
        "{source}"
    );
    assert_eq!(
        candidate.type_info.expressions.expr_types, legacy.type_info.expressions.expr_types,
        "{source}"
    );
    assert_eq!(
        candidate.type_info.expressions.value_destination_types, legacy.type_info.expressions.value_destination_types,
        "{source}"
    );
    assert_eq!(
        candidate.type_info.semantic_fact_store(&[]).render_snapshot(),
        legacy.type_info.semantic_fact_store(&[]).render_snapshot(),
        "{source}"
    );
    assert_eq!(
        candidate.body_local_observations.len(),
        candidate.check_work.body_local_checks
    );
    assert_eq!(
        candidate
            .body_local_observations
            .iter()
            .map(|(span, _)| span)
            .collect::<BTreeSet<_>>()
            .len(),
        candidate.check_work.body_local_checks,
        "each concrete named body is checked once"
    );
    candidate.observe_body_local(crate::ast::Span::default());
    legacy.observe_body_local(crate::ast::Span::default());
    assert_eq!(
        candidate.body_local_observations, legacy.body_local_observations,
        "complete per-body facts, pending decisions and final settled facts"
    );
    assert_eq!(
        candidate.check_work.body_local_checks,
        candidate.check_work.function_bodies + candidate.check_work.method_bodies
    );
    eprintln!(
        "body-local-oracle\tchecks={}\tobservations={}\terrors={}\twarnings={}\tdefaults={}\tmutation_settlements={}\tdictionary_settlements={}\topen_value_settlements={}\tdifferences=0",
        candidate.check_work.body_local_checks,
        candidate.body_local_observations.len(),
        candidate_result.as_ref().err().map_or(0, Vec::len),
        candidate.warnings.len(),
        candidate.check_work.callable_defaults,
        candidate.check_work.mutation_settlements,
        candidate.check_work.dictionary_settlements,
        candidate.check_work.open_value_settlements,
    );
    compare_environment(candidate, legacy)?;
    Ok(candidate_result.is_ok())
}

/// Measure source scanning separately from fact propagation on a reverse-ordered forwarding chain. These samples are
/// a local transfer benchmark, not record-service edit reuse or native build timings.
fn measure_influence_scan() -> Result<(), Box<dyn std::error::Error>> {
    let mut source = String::new();
    let members = 64;
    for member in 0..members {
        source.push_str(&format!("def step{member}[T](value: T) -> None:\n"));
        if member + 1 == members {
            source.push_str("    keys = {value}\n");
        } else {
            source.push_str(&format!("    step{}(value)\n", member + 1));
        }
    }
    let tokens = crate::lexer::lex(&source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.reset_program_check(&program, false);
    checker.prepare_program_declaration_names(&program);
    checker.collect_declarations_for_check(&program.declarations);
    checker.resolve_forward_type_references(&program.declarations);
    let targets = checker.hash_key_scan_targets(&program.declarations);
    for sample in 0..20 {
        let started = std::time::Instant::now();
        let mut legacy = HashMap::<HashKeyCallable, BTreeSet<String>>::new();
        let mut scans = 0;
        loop {
            let mut changed = false;
            for target in &targets {
                scans += 1;
                for parameter in checker.legacy_hashed_type_params(target, &targets, &legacy) {
                    changed |= legacy.entry(target.callable.clone()).or_default().insert(parameter);
                }
            }
            if !changed {
                break;
            }
        }
        let legacy_ns = started.elapsed().as_nanos();
        let started = std::time::Instant::now();
        let influences = checker.hash_key_influences(&targets);
        let (actual, counters) = super::resolve_influences(&influences);
        let candidate_ns = started.elapsed().as_nanos();
        assert_eq!(actual, legacy);
        assert_eq!(counters.body_scans, members);
        assert_eq!(scans, members * counters.iterations);
        eprintln!(
            "hash-influence-sample\t{sample}\t{legacy_ns}\t{candidate_ns}\t{scans}\t{}\t{}\t{}",
            counters.body_scans, counters.iterations, counters.member_visits
        );
    }
    Ok(())
}

/// Unreachable invalid bodies still enter checking, while callable defaults are checked once in their declaring
/// scope. This is a legacy-scope baseline; record-service edit reuse has not been implemented by this extraction.
fn assert_check_work_scope() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def first(value: int = 1) -> int:\n    return value\ndef unused(value: int = 2) -> int:\n    return \"wrong\"\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_err());
    let work = checker.check_work();
    assert_eq!(work.function_declarations, 2);
    assert_eq!(work.function_bodies, 2);
    assert_eq!(work.callable_defaults, 2);
    assert_eq!(work.mutation_settlements, 1);
    assert_eq!(work.dictionary_settlements, 1);
    assert_eq!(work.open_value_settlements, 1);
    eprintln!(
        "checker-scope-counts\t{}\t{}\t{}\t{}\t{}\t{}",
        work.function_declarations,
        work.function_bodies,
        work.callable_defaults,
        work.mutation_settlements,
        work.dictionary_settlements,
        work.open_value_settlements
    );
    Ok(())
}

/// Source-imported aliases and hash requirements keep their accepted declaring identity. The imported body is only a
/// signature influence input here; ordinary consumer checking must not call that body's local checker.
fn assert_imported_influences() -> Result<(), Box<dyn std::error::Error>> {
    let dependency = "pub model Item:\n    value: int\npub type Alias = Item\npub def target[T](value: T) -> None:\n    keys = {value}\npub def touch(mut items: list[int]) -> None:\n    pass\n";
    let source =
        "from provider import Alias, target\ndef caller[T](item: Alias, value: T) -> None:\n    target(value)\n";
    let parse = |text: &str| -> Result<crate::ast::Program, Box<dyn std::error::Error>> {
        let tokens = crate::lexer::lex(text).map_err(|errors| format!("{errors:?}"))?;
        Ok(crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?)
    };
    let dependency = parse(dependency)?;
    let program = parse(source)?;
    let mut legacy = TypeChecker::new();
    let mut candidate = TypeChecker::new();
    legacy.import_module(&dependency, "provider");
    candidate.import_module(&dependency, "provider");
    assert_checkers(&program, &mut candidate, &mut legacy)?;
    assert_eq!(candidate.check_work().function_bodies, 1);
    compare_environment(&candidate, &legacy)?;
    let program = parse("from provider import touch\ndef caller() -> None:\n    items = [1]\n    touch(items)\n")?;
    let mut legacy = TypeChecker::new();
    let mut candidate = TypeChecker::new();
    legacy.import_module(&dependency, "provider");
    candidate.import_module(&dependency, "provider");
    assert_checkers(&program, &mut candidate, &mut legacy)?;
    Ok(())
}

/// Compare scoped declaration readers with a separately collected legacy checker at each binding transition.
fn assert_environment_matrix() -> Result<(), Box<dyn std::error::Error>> {
    let sources = [
        "def read(value: Alias) -> int:\n    return value.value\ntype Alias = Item\nmodel Item:\n    value: int\n",
        "model Item:\n    value: int\ntype Alias = Item\ndef read(value: Alias) -> int:\n    return value.value\n",
        "def choose(value: int) -> int:\n    return value\ndef choose(value: str) -> str:\n    return value\ndef caller() -> int:\n    return choose(1)\n",
        "def invalid_default(value: int = value) -> int:\n    return value\n",
        "def defaults(value: int = outer) -> int:\n    return value\nconst outer: int = 3\n",
        "def direct[T](key: T) -> None:\n    keys = {key}\ndef forward[U](key: U) -> None:\n    direct(key)\n",
    ];
    for source in sources {
        assert_environment_source(source)?;
        assert_cold_check(source)?;
    }
    for source in [
        include_str!(
            "../../../../../../workspaces/contracts/fixtures/behavior/snapshots_traits_and_generics/trait_supertraits.incn"
        ),
        include_str!(
            "../../../../../../workspaces/contracts/fixtures/behavior/snapshots_functions_and_projections/user_defined_decorators.incn"
        ),
        include_str!(
            "../../../../../../workspaces/contracts/fixtures/behavior/codegen_functions_and_aliases/decorated_type_bound_overloads.incn"
        ),
        include_str!(
            "../../../../../../workspaces/contracts/fixtures/behavior/cli_values_and_calls/capturing_closures_in_function_types.incn"
        ),
        include_str!(
            "../../../../../../workspaces/contracts/fixtures/behavior/codegen_functions_and_aliases/decorated_generic_closed_signatures.incn"
        ),
    ] {
        assert_environment_source(source)?;
        assert_cold_check(source)?;
    }
    Ok(())
}

/// Advance independent collectors in source order; defaults and decorators keep their existing checking boundary.
fn assert_environment_source(source: &str) -> Result<(), Box<dyn std::error::Error>> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut candidate = TypeChecker::new();
    let mut legacy = TypeChecker::new();
    candidate.reset_program_check(&program, false);
    legacy.reset_program_check(&program, false);
    candidate.collect_program_check_declarations(&program);
    legacy.legacy_collect_program_declarations(&program);
    candidate.record_program_header_facts();
    candidate.record_source_declaration_identities(&program);
    candidate.visibility_point = super::super::VisibilityPoint::Collected;
    candidate.prepare_program_body_facts();
    legacy.prepare_program_body_facts();
    let source_identities = candidate.type_info.declarations.source_identities.clone();
    compare_environment(&candidate, &legacy)?;
    candidate.validate_source_type_names = true;
    legacy.validate_source_type_names = true;
    for constants in [true, false] {
        for declaration in &program.declarations {
            if matches!(declaration.node, crate::ast::Declaration::Const(_)) != constants {
                continue;
            }
            candidate.visibility_point = super::super::VisibilityPoint::BeforeDeclaration(declaration.span);
            compare_environment(&candidate, &legacy)?;
            candidate.check_environment_declaration(declaration);
            assert_eq!(candidate.type_info.declarations.source_identities, source_identities);
            legacy.check_declaration(declaration);
            assert_eq!(
                candidate.check_environment().visibility_point(),
                super::super::VisibilityPoint::AfterDeclaration(declaration.span)
            );
            compare_environment(&candidate, &legacy)?;
        }
    }
    eprintln!(
        "environment-oracle\t{}\t{}",
        program.declarations.len(),
        candidate.check_work().function_bodies
    );
    Ok(())
}

/// Compare every active spelling, canonical target, alias, source signature, generic bound and closure selection.
fn compare_environment(candidate: &TypeChecker, legacy: &TypeChecker) -> Result<(), Box<dyn std::error::Error>> {
    let environment = candidate.check_environment();
    let declarations = candidate.declaration_view();
    let names = |checker: &TypeChecker| {
        checker
            .symbols
            .active_module_symbols()
            .map(|(_, symbol)| symbol.name.clone())
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(names(candidate), names(legacy));
    for symbol in legacy.symbols.all_symbols() {
        let expected = legacy
            .symbols
            .lookup(&symbol.name)
            .and_then(|id| legacy.symbols.get(id));
        let actual = environment.lookup(&symbol.name);
        assert_eq!(
            actual.map(|value| (&value.symbol.name, value.symbol.span)),
            expected.map(|value| (&value.name, value.span))
        );
        let Some(actual) = actual else { continue };
        let expected_id = legacy.symbols.lookup(&symbol.name).ok_or("missing legacy binding")?;
        assert_eq!(actual.canonical, legacy.symbols.identity_of(expected_id));
        assert_eq!(
            environment.import_path(&symbol.name),
            legacy.symbols.import_binding_path(&symbol.name)
        );
        let Some(expected) = expected else { continue };
        compare_binding_kinds(&actual.symbol.kind, &expected.kind)?;
    }
    assert!(environment.lookup("__absent_environment_binding__").is_none());
    for (name, expected) in legacy.type_aliases.iter() {
        let actual = declarations.alias(name).ok_or("missing alias")?;
        assert_eq!(actual.type_params, expected.type_params);
        assert_eq!(actual.target, expected.target);
        assert_eq!(actual.nominal_identities, expected.nominal_identities);
    }
    for (canonical, expected) in &legacy.hash_key_inference.type_params {
        assert_eq!(declarations.hash_parameters(canonical), Some(expected));
    }
    for (span, expected) in &legacy.type_info.declarations.function_bindings_by_span {
        assert_eq!(declarations.function(*span), Some(expected));
    }
    for (span, expected) in &legacy.type_info.declarations.method_bindings_by_span {
        assert_eq!(declarations.method(*span), Some(expected));
    }
    assert_eq!(
        candidate.hash_key_inference.type_params,
        legacy.hash_key_inference.type_params
    );
    assert_eq!(
        candidate.type_info.calls.closure_holding_params,
        legacy.type_info.calls.closure_holding_params
    );
    assert_eq!(
        candidate.type_info.calls.closure_returning_callables,
        legacy.type_info.calls.closure_returning_callables
    );
    assert_eq!(candidate.errors, legacy.errors);
    assert_eq!(candidate.warnings, legacy.warnings);
    Ok(())
}

/// Compare callable fields without ordering HashMaps or substituting display strings for checked types.
fn compare_function(actual: &crate::symbols::FunctionInfo, expected: &crate::symbols::FunctionInfo) {
    assert_eq!(actual.params, expected.params);
    assert_eq!(actual.return_type, expected.return_type);
    assert_eq!(actual.is_async, expected.is_async);
    assert_eq!(actual.type_params, expected.type_params);
    assert_eq!(actual.type_param_bounds, expected.type_param_bounds);
    assert_eq!(actual.type_param_bound_details, expected.type_param_bound_details);
    assert_eq!(actual.emitted_name, expected.emitted_name);
}

/// Decorator transitions may replace a function with a callable value; overload candidates keep all checked fields.
fn compare_binding_kinds(
    actual: &crate::symbols::SymbolKind,
    expected: &crate::symbols::SymbolKind,
) -> Result<(), String> {
    use crate::symbols::SymbolKind;
    assert_eq!(std::mem::discriminant(actual), std::mem::discriminant(expected));
    match (actual, expected) {
        (SymbolKind::Function(actual), SymbolKind::Function(expected)) => compare_function(actual, expected),
        (SymbolKind::FunctionOverloads(actual), SymbolKind::FunctionOverloads(expected)) => {
            assert_eq!(actual.len(), expected.len());
            for (actual, expected) in actual.iter().zip(expected) {
                compare_function(&actual.info, &expected.info);
                assert_eq!(actual.identity, expected.identity);
                assert_eq!(actual.span, expected.span);
            }
        }
        (SymbolKind::Variable(actual), SymbolKind::Variable(expected)) => {
            assert_eq!(actual.ty, expected.ty);
            assert_eq!(actual.is_mutable, expected.is_mutable);
        }
        (SymbolKind::Trait(actual), SymbolKind::Trait(expected)) => {
            assert_eq!(actual.type_params, expected.type_params);
            assert_eq!(actual.supertraits, expected.supertraits);
            assert_eq!(actual.requires, expected.requires);
            assert_eq!(actual.method_aliases, expected.method_aliases);
            compare_methods(&actual.methods, &expected.methods)?;
        }
        (SymbolKind::Type(actual), SymbolKind::Type(expected)) => compare_nominal_methods(actual, expected)?,
        _ => {}
    }
    Ok(())
}

/// Preserve every checked method signature field, including generic constraints and inherited aliases.
fn compare_methods(
    actual: &HashMap<String, crate::symbols::MethodInfo>,
    expected: &HashMap<String, crate::symbols::MethodInfo>,
) -> Result<(), String> {
    assert_eq!(actual.len(), expected.len());
    for (name, expected) in expected {
        let actual = actual
            .get(name)
            .ok_or_else(|| format!("missing checked method {name}"))?;
        compare_method(actual, expected);
    }
    Ok(())
}

/// Compare a method candidate independently of its name slot, including receiver, constraints and alias target.
fn compare_method(actual: &crate::symbols::MethodInfo, expected: &crate::symbols::MethodInfo) {
    assert_eq!(actual.identity, expected.identity);
    assert_eq!(actual.type_params, expected.type_params);
    assert_eq!(actual.type_param_bounds, expected.type_param_bounds);
    assert_eq!(actual.type_param_bound_details, expected.type_param_bound_details);
    assert_eq!(actual.trait_target, expected.trait_target);
    assert_eq!(actual.receiver, expected.receiver);
    assert_eq!(actual.params, expected.params);
    assert_eq!(actual.return_type, expected.return_type);
    assert_eq!(actual.is_async, expected.is_async);
    assert_eq!(actual.has_body, expected.has_body);
    assert_eq!(actual.alias_of, expected.alias_of);
}

/// Same-name methods retain every checked overload, rather than comparing only the primary candidate.
fn compare_overloads(
    actual: &HashMap<String, Vec<crate::symbols::MethodInfo>>,
    expected: &HashMap<String, Vec<crate::symbols::MethodInfo>>,
) -> Result<(), String> {
    assert_eq!(actual.len(), expected.len());
    for (name, expected) in expected {
        let actual = actual
            .get(name)
            .ok_or_else(|| format!("missing method overload set {name}"))?;
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(expected) {
            compare_method(actual, expected);
        }
    }
    Ok(())
}

/// Compare nominal method tables through their checked declarations, with no source-text re-resolution.
fn compare_nominal_methods(
    actual: &crate::symbols::TypeInfo,
    expected: &crate::symbols::TypeInfo,
) -> Result<(), String> {
    use crate::symbols::TypeInfo;
    assert_eq!(std::mem::discriminant(actual), std::mem::discriminant(expected));
    match (actual, expected) {
        (TypeInfo::Model(actual), TypeInfo::Model(expected)) => {
            assert_eq!(actual.type_params, expected.type_params);
            compare_methods(&actual.methods, &expected.methods)?;
            compare_overloads(&actual.method_overloads, &expected.method_overloads)?;
        }
        (TypeInfo::Class(actual), TypeInfo::Class(expected)) => {
            assert_eq!(actual.type_params, expected.type_params);
            compare_methods(&actual.methods, &expected.methods)?;
            compare_overloads(&actual.method_overloads, &expected.method_overloads)?;
        }
        (TypeInfo::Enum(actual), TypeInfo::Enum(expected)) => {
            assert_eq!(actual.type_params, expected.type_params);
            compare_methods(&actual.methods, &expected.methods)?;
            compare_overloads(&actual.method_overloads, &expected.method_overloads)?;
        }
        (TypeInfo::Newtype(actual), TypeInfo::Newtype(expected)) => {
            assert_eq!(actual.type_params, expected.type_params);
            compare_methods(&actual.methods, &expected.methods)?;
            compare_overloads(&actual.method_overloads, &expected.method_overloads)?;
        }
        _ => {}
    }
    Ok(())
}

/// Extend the cold oracle with the G2 effect, deferred-use, lexical inference and diagnostic matrix. Both routes
/// check fresh source, including an unreachable refusal; the oracle never provides input to the local route.
fn assert_body_local_matrix() -> Result<(), Box<dyn std::error::Error>> {
    let variants = [
        "def first(mut items: list[int]) -> None:\n    second(items)\ndef second(mut items: list[int]) -> None:\n    first(items)\n    items.append(1)\ndef caller() -> None:\n    items = [1]\n    first(items)\n",
        "def touch(mut items: list[int]) -> None:\n    pass\ndef caller() -> None:\n    items = [1]\n    touch(items)\n",
        "def touch(mut items: list[int]) -> None:\n    return None\ndef caller() -> None:\n    items = [1]\n    touch(items)\n",
        "def touch(mut items: list[int]) -> None:\n    items.append(2)\ndef caller() -> None:\n    items = [1]\n    touch(items)\n",
        "def touch(mut items: list[int]) -> None:\n    pass\ndef write(mut items: list[int]) -> None:\n    items.append(2)\ndef caller() -> None:\n    mut action = touch\n    action = write\n    items = [1]\n    action(items)\n",
        "def factory[T]() -> list[T]:\n    return []\ndef use() -> list[int]:\n    values = factory()\n    values.append(1)\n    return values\ndef sibling() -> list[str]:\n    values = factory()\n    values.append(\"x\")\n    return values\n",
        "def valid() -> int:\n    return 1\ndef unreachable() -> int:\n    return \"wrong\"\n",
        include_str!(
            "../../../../../../workspaces/contracts/fixtures/behavior/cli_values_and_calls/mut_marked_callable_value_changes_the_argument.incn"
        ),
        include_str!(
            "../../../../../../workspaces/contracts/fixtures/behavior/cli_refusals/refused_kept_dict_lookup_of_a_generator.incn"
        ),
        include_str!(
            "../../../../../../workspaces/contracts/fixtures/behavior/ownership_parameters_and_receivers/dict_lookups_through_a_mutable_parameter.incn"
        ),
        include_str!(
            "../../../../../../workspaces/contracts/fixtures/behavior/cli_values_and_calls/capturing_closures_in_function_types.incn"
        ),
        include_str!(
            "../../../../../../workspaces/contracts/fixtures/behavior/cli_values_and_calls/mut_parameter_changed_through_trait_dispatch_reaches_the_caller.incn"
        ),
        include_str!(
            "../../../../../../workspaces/contracts/fixtures/behavior/cli_refusals_mutation/refused_immutable_argument_to_trait_dispatched_mut_parameter.incn"
        ),
        "def sum_items(values: list[int]) -> int:\n    return values.iter().sum()\n",
        "def numbers() -> Generator[int]:\n    yield 1\ndef probe(streams: dict[str, Generator[int]], key: str) -> bool:\n    match streams.get(key):\n        Some(value) => return true\n        None => return false\n",
    ];
    let read_only_index = variants.len() - 1;
    for (index, source) in variants.into_iter().enumerate() {
        let accepted = assert_cold_check(source)?;
        if matches!(index, 1..=3) {
            assert_eq!(
                accepted,
                index != 3,
                "the mutation edit changes only the caller's refusal"
            );
        }
        if index == read_only_index {
            assert!(
                accepted,
                "matching a generator lookup without keeping its payload is read-only"
            );
        }
    }
    Ok(())
}
