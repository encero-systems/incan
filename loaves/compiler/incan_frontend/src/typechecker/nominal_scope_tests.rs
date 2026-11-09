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
