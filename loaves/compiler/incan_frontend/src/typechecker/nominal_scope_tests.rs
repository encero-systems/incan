use super::{PublicLibraryTypeIdentity, TypeChecker};
use crate::ast::{Program, Span};
use crate::symbols::{ResolvedType, ScopeKind, Symbol, SymbolKind, VariableInfo};
use incan_semantics_core::{CanonicalSymbolId, body_ir::nominal_carrier_key};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

/// Parse a source module without changing its declarations or depending on a published standard library.
fn program(source: &str) -> Result<Program, Box<dyn std::error::Error>> {
    let tokens = crate::lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok(crate::parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?)
}

/// Compare two interface snapshots at the first differing table entry so a regression names the exact boundary.
fn assert_interface_snapshots_match(
    legacy: &super::ModuleDependencyInterfaceSnapshot,
    shared: &super::ModuleDependencyInterfaceSnapshot,
) {
    assert_eq!(legacy.version, shared.version);
    assert_eq!(legacy.tables.len(), shared.tables.len());
    for (legacy_table, shared_table) in legacy.tables.iter().zip(&shared.tables) {
        assert_eq!(legacy_table.name, shared_table.name);
        let compared = legacy_table.entries.len().max(shared_table.entries.len());
        for index in 0..compared {
            assert_eq!(
                legacy_table.entries.get(index),
                shared_table.entries.get(index),
                "dependency interface table {} entry {index}",
                legacy_table.name,
            );
        }
    }
}

/// Reproduce the uncached carrier collection from immutable declaration scopes, independently of the new index.
pub(super) fn fresh_scope(checker: &TypeChecker) -> BTreeMap<String, CanonicalSymbolId> {
    let mut identities = checker.checked_nominal_bindings();
    let mut visited = HashSet::new();
    let carriers = identities
        .values()
        .chain(
            checker
                .declaration_nominal_scopes
                .values()
                .filter(|scope| visited.insert(Arc::as_ptr(scope)))
                .flat_map(|scope| scope.values()),
        )
        .filter_map(|identity| nominal_carrier_key(identity).map(|key| (key, identity.clone())))
        .collect::<Vec<_>>();
    for (key, identity) in carriers {
        identities.entry(key).or_insert(identity);
    }
    identities
}

/// Compare mutation-owned lookup indexes with arena lookup, and prove unchanged requests share one computation.
fn assert_current_inventory(checker: &TypeChecker) {
    let expected = checker
        .symbols
        .all_symbols()
        .iter()
        .enumerate()
        .filter(|(id, symbol)| symbol.scope == 0 && checker.symbols.is_active_lookup_binding(*id))
        .map(|(id, _)| id)
        .collect::<std::collections::BTreeSet<_>>();
    let actual = checker
        .symbols
        .active_module_symbols()
        .map(|(id, _)| id)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(actual, expected);
    let scope = checker.checked_nominal_type_identities();
    assert_eq!(scope.as_ref(), &fresh_scope(checker));
    assert!(Arc::ptr_eq(&scope, &checker.checked_nominal_type_identities()));
}

/// Changing an alias target invalidates facts even when its source lookup binding and inventory length stay fixed.
fn assert_alias_target_revision(checker: &mut TypeChecker) -> Result<(), Box<dyn std::error::Error>> {
    let previous = checker.type_aliases.get("Alias").cloned().ok_or("missing alias")?;
    let binding = checker.symbols.lookup("Alias");
    let before = checker.checked_nominal_type_identities();
    let alias = checker.type_aliases.get_mut("Alias").ok_or("missing alias")?;
    let identity = alias
        .nominal_identities
        .values_mut()
        .next()
        .ok_or("missing alias owner")?;
    identity.declaration_name = "Changed".to_string();
    assert_eq!(checker.symbols.lookup("Alias"), binding);
    assert!(checker.checked_body_nominal_scope().is_none());
    let after = checker.checked_nominal_type_identities();
    assert!(!Arc::ptr_eq(&before, &after));
    assert_current_inventory(checker);
    checker.type_aliases.insert("Alias".to_string(), previous);
    assert_current_inventory(checker);
    Ok(())
}

/// Check admission, repeated imports, lexical shadow/restore and same-size canonical replacements against fresh
/// facts.
#[test]
fn nominal_scope_inventory_matches_fresh_inputs() -> Result<(), Box<dyn std::error::Error>> {
    // ---- Both constructors initialize the same semantic context ----
    for checker in [TypeChecker::new(), TypeChecker::default()] {
        assert_eq!(checker.current_yield_context, super::YieldContext::Disallowed);
        assert!(checker.errors.is_empty());
        assert!(checker.warnings.is_empty());
        assert!(checker.loop_stack.is_empty());
        assert_eq!(checker.symbols.current_scope_index(), 0);
        assert!(checker.symbols.lookup("int").is_some());
        assert!(checker.symbols.lookup("len").is_some());
    }

    // ---- Imported declarations and repeated admission ----
    let first = program("pub model First:\n    value: int\n")?;
    let second = program("pub model Second:\n    value: int\n")?;
    let exports = Arc::from(crate::module::exported_symbols(&first));
    let mut first_builder = TypeChecker::default();
    first_builder.set_current_module_path(Some(vec!["first".to_string()]));
    first_builder
        .check_program(&first)
        .map_err(|errors| format!("{errors:?}"))?;
    let first_interface =
        first_builder.seal_current_module_dependency_interface("first", &first, Arc::clone(&exports), true);
    let mut replay_builder = TypeChecker::default();
    replay_builder.set_current_module_path(Some(vec!["first".to_string()]));
    replay_builder
        .check_program(&first)
        .map_err(|errors| format!("{errors:?}"))?;
    let replayed_first_interface =
        replay_builder.seal_current_module_dependency_interface("first", &first, exports, true);
    assert_eq!(first_interface.snapshot, replayed_first_interface.snapshot);
    assert_eq!(
        first_interface.snapshot.render(),
        replayed_first_interface.snapshot.render()
    );
    let consumer = program("from first import First\n\ndef read(value: First) -> int:\n    return 1\n")?;
    let dependencies = [("first", &first)];
    let mut legacy_consumer = TypeChecker::default();
    legacy_consumer.register_dependency_module_path_segments("first", vec!["first".to_string()]);
    legacy_consumer
        .check_with_imports(&consumer, &dependencies)
        .map_err(|errors| format!("{errors:?}"))?;
    let legacy_consumer_snapshot = legacy_consumer.dependency_interface_snapshot();
    let mut shared_consumer = TypeChecker::default();
    shared_consumer.register_dependency_module_path_segments("first", vec!["first".to_string()]);
    shared_consumer.select_dependency_interfaces(vec![Arc::clone(&first_interface)]);
    shared_consumer
        .check_with_imports(&consumer, &dependencies)
        .map_err(|errors| format!("{errors:?}"))?;
    assert_eq!(
        legacy_consumer_snapshot,
        shared_consumer.dependency_interface_snapshot()
    );
    let mut checker = TypeChecker::default();
    for (name, dependency) in [("first", &first), ("second", &second), ("first", &first)] {
        checker.import_module_all(dependency, name);
        assert_current_inventory(&checker);
    }
    let legacy_interface = checker.dependency_interface_snapshot();
    let mut replay = TypeChecker::default();
    for (name, dependency) in [("first", &first), ("second", &second), ("first", &first)] {
        replay.import_module_all(dependency, name);
    }
    let replayed_interface = replay.dependency_interface_snapshot();
    assert_eq!(legacy_interface, replayed_interface);
    assert_eq!(legacy_interface.render(), replayed_interface.render());
    assert!(legacy_interface.render().contains("[symbol_rows]"));
    assert_eq!(checker.retained_nominal_carriers.len(), 2);
    let module = program("model Local:\n    value: int\ntype Alias = Local\n")?;
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    checker.reset_program_check(&module, true);
    checker.collect_program_check_declarations(&module);
    checker.prepare_program_body_facts();
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    assert_eq!(
        checker.checked_body_nominal_scope().as_deref(),
        Some(&fresh_scope(&checker))
    );
    assert_alias_target_revision(&mut checker)?;

    // ---- Lexical alias shadowing and restoration ----
    checker.symbols.enter_scope(ScopeKind::Block);
    checker.symbols.define(Symbol {
        name: "Alias".to_string(),
        kind: SymbolKind::Variable(VariableInfo {
            ty: ResolvedType::Int,
            is_mutable: false,
            is_used: false,
        }),
        span: Span::new(100, 110),
        scope: 0,
    });
    assert!(checker.checked_body_nominal_scope().is_none());
    assert_current_inventory(&checker);
    checker.symbols.exit_scope();
    assert_current_inventory(&checker);
    assert_eq!(
        checker.checked_body_nominal_scope().as_deref(),
        Some(&fresh_scope(&checker))
    );

    // ---- Equal-size public owner replacement and removal ----
    let local = checker
        .symbols
        .lookup("Local")
        .and_then(|id| checker.symbols.identity_of(id))
        .cloned()
        .ok_or("missing Local identity")?;
    checker.public_library_type_identities.insert(
        "foreign".to_string(),
        PublicLibraryTypeIdentity::new("demo", &["Local".to_string()]).with_canonical(Some(local.clone())),
    );
    checker.prepare_program_body_facts();
    let mut replacement = local;
    replacement.declaration_name = "Different".to_string();
    checker
        .public_library_type_identities
        .get_mut("foreign")
        .ok_or("missing public binding")?
        .canonical = Some(replacement);
    assert!(checker.checked_body_nominal_scope().is_none());
    assert_current_inventory(&checker);

    checker.prepare_program_body_facts();
    checker.public_library_type_identities.remove("foreign");
    checker.public_library_type_identities.insert(
        "unproven".to_string(),
        PublicLibraryTypeIdentity::new("demo", &["Unproven".to_string()]),
    );
    assert!(checker.checked_body_nominal_scope().is_none());
    assert_current_inventory(&checker);
    Ok(())
}

/// Prove that sealing an imported decorator receiver and its decorated declaration preserves the exact dependency
/// layer needed by a downstream consumer.
#[test]
fn dependency_interfaces_preserve_imported_static_decorator_bindings() -> Result<(), Box<dyn std::error::Error>> {
    let registry = program(
        "@derive(Clone)\npub class ProbeRegistry:\n    @staticmethod\n    pub def new() -> Self:\n        return ProbeRegistry()\n\n    pub def add[F](mut self, name: str, value: int) -> (F) -> F:\n        return (func) => func\n\npub static PROBE_REGISTRY: ProbeRegistry = ProbeRegistry.new()\n",
    )?;
    let decorated = program(
        "from probe_registry import PROBE_REGISTRY\n\n@PROBE_REGISTRY.add(\"decorated\", 1)\npub def decorated(value: int) -> int:\n    return value\n",
    )?;
    let consumer =
        program("from probe_decorated import decorated\n\ndef main() -> None:\n    println(decorated(7))\n")?;

    let registry_exports = Arc::from(crate::module::exported_symbols(&registry));
    let mut registry_checker = TypeChecker::default();
    registry_checker.set_current_module_path(Some(vec!["probe_registry".to_string()]));
    registry_checker
        .check_program(&registry)
        .map_err(|errors| format!("{errors:?}"))?;
    let registry_interface =
        registry_checker.seal_current_module_dependency_interface("probe_registry", &registry, registry_exports, true);

    let decorated_exports = Arc::from(crate::module::exported_symbols(&decorated));
    let decorated_dependencies = [("probe_registry", &registry)];
    let mut legacy_decorated_checker = TypeChecker::default();
    legacy_decorated_checker.set_current_module_path(Some(vec!["probe_decorated".to_string()]));
    legacy_decorated_checker
        .register_dependency_module_path_segments("probe_registry", vec!["probe_registry".to_string()]);
    legacy_decorated_checker
        .check_with_imports(&decorated, &decorated_dependencies)
        .map_err(|errors| format!("{errors:?}"))?;
    let legacy_decorated_snapshot = legacy_decorated_checker.dependency_interface_snapshot();
    let mut decorated_checker = TypeChecker::default();
    decorated_checker.set_current_module_path(Some(vec!["probe_decorated".to_string()]));
    decorated_checker.register_dependency_module_path_segments("probe_registry", vec!["probe_registry".to_string()]);
    decorated_checker.select_dependency_interfaces(vec![Arc::clone(&registry_interface)]);
    decorated_checker
        .check_with_imports(&decorated, &decorated_dependencies)
        .map_err(|errors| format!("{errors:?}"))?;
    assert_interface_snapshots_match(
        &legacy_decorated_snapshot,
        &decorated_checker.dependency_interface_snapshot(),
    );
    let decorated_interface = decorated_checker.seal_current_module_dependency_interface(
        "probe_decorated",
        &decorated,
        decorated_exports,
        true,
    );

    let mut consumer_checker = TypeChecker::default();
    consumer_checker.set_current_module_path(Some(vec!["main".to_string()]));
    consumer_checker.register_dependency_module_path_segments("probe_registry", vec!["probe_registry".to_string()]);
    consumer_checker.register_dependency_module_path_segments("probe_decorated", vec!["probe_decorated".to_string()]);
    consumer_checker.select_dependency_interfaces(vec![registry_interface, decorated_interface]);
    let consumer_dependencies = [("probe_registry", &registry), ("probe_decorated", &decorated)];
    let mut legacy_consumer_checker = TypeChecker::default();
    legacy_consumer_checker.set_current_module_path(Some(vec!["main".to_string()]));
    legacy_consumer_checker
        .register_dependency_module_path_segments("probe_registry", vec!["probe_registry".to_string()]);
    legacy_consumer_checker
        .register_dependency_module_path_segments("probe_decorated", vec!["probe_decorated".to_string()]);
    legacy_consumer_checker
        .check_with_imports(&consumer, &consumer_dependencies)
        .map_err(|errors| format!("{errors:?}"))?;
    let legacy_consumer_snapshot = legacy_consumer_checker.dependency_interface_snapshot();
    consumer_checker
        .check_with_imports(&consumer, &consumer_dependencies)
        .map_err(|errors| format!("{errors:?}"))?;
    assert_interface_snapshots_match(
        &legacy_consumer_snapshot,
        &consumer_checker.dependency_interface_snapshot(),
    );
    Ok(())
}

/// Prove that an imported generic callable keeps accepting explicit call-site type arguments after its decorator
/// signature has crossed the immutable dependency layer.
#[test]
fn dependency_interfaces_preserve_decorated_generic_calls() -> Result<(), Box<dyn std::error::Error>> {
    let provider = program(
        "def register[F]() -> (F) -> F:\n    return (func) => func\n\n@register()\npub def from_csv[T]() -> str:\n    return T.__class_name__()\n",
    )?;
    let consumer = program(
        "from provider import from_csv\n\n@derive(Clone)\nmodel Row:\n    value: int\n\ndef main() -> None:\n    println(from_csv[Row]())\n",
    )?;
    let exports = Arc::from(crate::module::exported_symbols(&provider));
    let mut provider_checker = TypeChecker::default();
    provider_checker.set_current_module_path(Some(vec!["provider".to_string()]));
    provider_checker
        .check_program(&provider)
        .map_err(|errors| format!("{errors:?}"))?;
    let provider_interface =
        provider_checker.seal_current_module_dependency_interface("provider", &provider, exports, true);

    let dependencies = [("provider", &provider)];
    let mut legacy = TypeChecker::default();
    legacy.register_dependency_module_path_segments("provider", vec!["provider".to_string()]);
    legacy
        .check_with_imports(&consumer, &dependencies)
        .map_err(|errors| format!("legacy: {errors:?}"))?;

    let mut shared = TypeChecker::default();
    shared.register_dependency_module_path_segments("provider", vec!["provider".to_string()]);
    shared.select_dependency_interfaces(vec![provider_interface]);
    shared
        .check_with_imports(&consumer, &dependencies)
        .map_err(|errors| format!("shared: {errors:?}"))?;
    assert_interface_snapshots_match(
        &legacy.dependency_interface_snapshot(),
        &shared.dependency_interface_snapshot(),
    );
    Ok(())
}
