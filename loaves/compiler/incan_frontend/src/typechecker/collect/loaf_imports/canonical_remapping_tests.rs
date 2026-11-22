//! A compiled Loaf's canonical type remapping is a fact of the Loaf: the checkers of one module graph evaluate it once.

use std::sync::Arc;

use super::rust_reexport_chain_tests::{TestResult, reexport_chain};
use crate::ast::Program;
use crate::loaf_index::LoafIndex;
use crate::typechecker::{DependencyInterfacePreparationCache, TypeChecker};

/// A project module that imports the Rust items through the module re-export, by name and through the namespace.
const PROVIDER: &str = "from pub::facade.store import RecordKey, read_key\nfrom pub::facade import store\n\npub def fresh(value: int) -> RecordKey:\n    return RecordKey(value=value)\n\npub def total() -> int:\n    first = RecordKey(value=5)\n    second = store.RecordKey(value=6)\n    return read_key(first) + store.read_key(second)\n";

/// A project module that reads the provider's sealed interface and the package-root re-export of the same items.
const CONSUMER: &str = "from provider import fresh, total\nfrom pub::rootfacade import RecordKey, read_key\nimport pub::rootfacade as rf\n\ndef main() -> int:\n    first = fresh(3)\n    second = rf.RecordKey(value=6)\n    return read_key(first) + rf.read_key(second) + total()\n";

fn parse(source: &str) -> TestResult<Program> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    Ok(crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?)
}

/// A fresh checker for `module` against `loaves` with the build's preparation `cache`, as module-graph checking
/// configures each module's checker.
fn checker(module: &str, loaves: &Arc<LoafIndex>, cache: &DependencyInterfacePreparationCache) -> TypeChecker {
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec![module.to_owned()]));
    checker.register_dependency_module_path_segments("provider", vec!["provider".to_owned()]);
    checker.set_loaves(Arc::clone(loaves));
    checker.set_dependency_interface_preparation_cache(cache.clone());
    checker
}

/// The rendered Body IR of one checked module.
fn body_ir(program: &Program, checker: TypeChecker) -> String {
    let facts = checker.into_type_info();
    let providers = crate::body_ir::ProviderOperationCatalog::new();
    let context = crate::body_ir::BodyIrContext {
        modules: crate::body_ir::ContextModules::default(),
        provider_operations: &providers,
        registry_package: None,
        library_package: None,
        include_inline_tests: false,
    };
    format!(
        "{:?}",
        crate::body_ir::build_body_ir_module(program, &[], &facts, &context)
    )
}

/// Check the provider, seal its interface, and check the consumer against that interface, giving each checker the
/// preparation cache `cache_for` returns for it. Returns both modules' Body IR.
fn check_graph(
    loaves: &Arc<LoafIndex>,
    cache_for: &dyn Fn() -> DependencyInterfacePreparationCache,
) -> TestResult<[String; 2]> {
    let provider = parse(PROVIDER)?;
    let consumer = parse(CONSUMER)?;
    let mut first = checker("provider", loaves, &cache_for());
    first
        .check_program(&provider)
        .map_err(|errors| format!("provider: {errors:?}"))?;
    let exports: Arc<[_]> = crate::module::exported_symbols(&provider).into();
    let interface = first.seal_current_module_dependency_interface("provider", &provider, exports, true);
    let mut second = checker("consumer", loaves, &cache_for());
    second.select_dependency_interfaces(vec![interface]);
    second
        .check_with_imports(&consumer, &[("provider", &provider)])
        .map_err(|errors| format!("consumer: {errors:?}"))?;
    Ok([body_ir(&provider, first), body_ir(&consumer, second)])
}

/// One module graph's checkers share one evaluation of each compiled Loaf's canonical type remapping, however many
/// import statements, qualified members and installed interfaces reach that Loaf; the checked modules are the same as
/// with a private evaluation per checker.
#[test]
fn a_loafs_canonical_type_remapping_is_evaluated_once_per_module_graph() -> TestResult {
    let root = tempfile::tempdir()?;
    let loaves = Arc::new(LoafIndex::new(reexport_chain(root.path())?)?);
    let shared = DependencyInterfacePreparationCache::default();
    let checked = check_graph(&loaves, &|| shared.clone())?;
    let evaluations = shared.canonical_type_remapping_evaluations();
    assert!(!evaluations.is_empty(), "the graph reached no compiled Loaf");
    assert!(
        evaluations.values().all(|&count| count == 1),
        "canonical type remappings evaluated more than once per Loaf: {evaluations:?}"
    );
    assert_eq!(
        checked,
        check_graph(&loaves, &DependencyInterfacePreparationCache::default)?
    );
    Ok(())
}
