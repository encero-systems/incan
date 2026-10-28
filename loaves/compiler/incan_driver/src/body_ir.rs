//! Shared checked source-to-Body-IR construction for compilation, inspection and editor analysis.

use crate::session::{CompilationAnalysis, CompilationSession};
use crate::unit::UnitKind;
use incan_frontend::loaf_index::LoafIndex;
use incan_frontend::parsed_module::ParsedModule;
use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};
use std::collections::BTreeSet;

/// Build authored modules in collection order against one provider catalog and the preceding dependency context.
/// The unit kind supplies publication identity; no caller may rebuild a module with a different library contract.
pub fn lower_authored_modules(
    modules: &[ParsedModule],
    analysis: &CompilationAnalysis,
    session: &CompilationSession,
    bodies: &mut Vec<BodyIrModule>,
) -> Result<(), String> {
    let provider_operations = incan_frontend::body_ir::ProviderOperationCatalog::from_loaves(session.loaves())?;
    let unit = session.unit();
    for module in modules {
        let type_info = analysis
            .type_info_for_path(&module.file_path)
            .ok_or("module analysis is missing")?;
        bodies.push(incan_frontend::body_ir::build_body_ir_module(
            &module.ast,
            &module.path_segments,
            type_info,
            &incan_frontend::body_ir::BodyIrContext {
                modules: bodies,
                provider_operations: &provider_operations,
                registry_package: Some(&unit.package),
                library_package: (unit.kind == UnitKind::Library).then_some(unit.package.as_str()),
            },
        ));
    }
    Ok(())
}

/// Construct authored source bodies through compilation's sequential module boundary.
/// Checking-only dependency interfaces have no executable publication to load; their checked identities remain
/// available through the analysis and provider catalog rather than requiring a native build for inspection.
pub fn checked_modules(
    modules: &[ParsedModule],
    analysis: &CompilationAnalysis,
    session: &CompilationSession,
) -> Result<Vec<BodyIrModule>, String> {
    let mut bodies = Vec::new();
    lower_authored_modules(modules, analysis, session, &mut bodies)?;
    Ok(bodies)
}

/// Project exact public package requirements reachable from accepted module facts, including private re-export calls.
/// The current package's declarations compile from this unit and never resolve through a dependency artifact.
/// Trait-wide default inventories are not execution roots: unused adaptor bodies must not instantiate for every
/// adopter. Native comparison bridges add their selected slots from checked Body IR in `complete_dependency_context`.
pub fn required_loaf_identities(
    modules: &[ParsedModule],
    analysis: &CompilationAnalysis,
    package_identity: &str,
) -> Result<BTreeSet<CanonicalSymbolId>, String> {
    let infos = modules
        .iter()
        .map(|module| {
            analysis
                .type_info_for_path(&module.file_path)
                .ok_or("module analysis is missing")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let facts = modules
        .iter()
        .zip(&infos)
        .map(|(module, info)| info.semantic_fact_store(&module.path_segments))
        .collect::<Vec<_>>();
    let roots = infos.iter().copied().flat_map(|info| {
        info.declarations
            .declaration_identities
            .values()
            .chain(info.declarations.member_declaration_identities.values())
            .cloned()
    });
    let requested = incan_semantics_core::dependencies::CheckedDependencyGraph::from_fact_stores(&facts)
        .reachable_from(roots)
        .into_iter()
        .chain(
            infos
                .iter()
                .copied()
                .flat_map(|info| info.calls.body_ir_reexport_calls.values())
                .map(|call| call.canonical.clone()),
        )
        .chain(
            infos
                .iter()
                .copied()
                .flat_map(|info| info.calls.body_ir_async_call_targets.values().cloned()),
        )
        .chain(
            infos
                .iter()
                .copied()
                .flat_map(|info| info.calls.source_display_methods.values().cloned()),
        )
        .chain(
            infos
                .iter()
                .copied()
                .flat_map(|info| info.declarations.body_ir_trait_adoptions.values())
                .flatten()
                .flat_map(|adoption| adoption.methods.iter().flatten().cloned()),
        )
        .chain(
            infos
                .iter()
                .copied()
                .flat_map(|info| info.expressions.validated_newtype_coercions.values())
                .flat_map(|coercion| &coercion.steps)
                .flat_map(|step| step.owner_identity.iter().chain(step.ctor_identity.iter()).cloned()),
        )
        .chain(
            infos
                .iter()
                .copied()
                .flat_map(|info| info.declarations.body_ir_native_type_identities.values().cloned()),
        )
        .chain(
            infos
                .iter()
                .copied()
                .flat_map(incan_frontend::body_ir::registry_helper_identities),
        )
        .filter(
            |identity| matches!(&identity.origin, SymbolOrigin::Package { library, .. } if library != package_identity),
        )
        .collect();
    Ok(requested)
}

/// Read the authenticated dependency closure and its reached package identities, allowing only exact checked
/// source identities to satisfy local edges. Native linking consumes the same package set rather than treating
/// every available ambient standard package as reached.
pub fn load_loaf_modules(
    loaves: &LoafIndex,
    requested: &BTreeSet<CanonicalSymbolId>,
    checked_local: &BTreeSet<CanonicalSymbolId>,
) -> Result<incan_frontend::executable_resolution::ResolvedExecutableModules, String> {
    incan_frontend::executable_resolution::resolve_loaf_declarations(loaves, requested, checked_local)
        .map_err(|error| format!("unsupported Body IR Loaf declaration: {error}"))
}
