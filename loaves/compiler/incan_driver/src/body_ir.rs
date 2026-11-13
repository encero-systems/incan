//! Shared checked source-to-Body-IR construction for compilation, inspection and editor analysis.

use crate::session::{CompilationAnalysis, CompilationSession};
use crate::unit::UnitKind;
use incan_frontend::body_ir::ProviderOperationCatalog;
use incan_frontend::loaf_index::LoafIndex;
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::typechecker::TypeCheckInfo;
use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};
use std::collections::{BTreeSet, HashSet};
use std::thread;

/// Maximum source components lowered at once inside one compiler invocation.
///
/// Each worker owns a cloned dependency context and a compiler-sized stack. Four workers bound those temporary
/// graphs and stacks conservatively inside the 6 GiB lowering reservation while still using independent cores.
const MAX_BODY_IR_WORKERS: usize = 4;

/// One strongly connected source component and the completed source modules it imports.
struct AuthoredComponent {
    members: Vec<usize>,
    dependencies: Vec<usize>,
}

/// Build authored modules in collection order against one provider catalog and the preceding dependency context.
/// The unit kind supplies publication identity; no caller may rebuild a module with a different library contract.
///
/// A member of an import cycle is also lowered against the declarations of the cycle's members that follow it, so it
/// sees every other member's models whatever order the cycle's bodies are lowered in. Those declarations leave the
/// context once the member is lowered; each member's complete module takes their place in turn.
pub fn lower_authored_modules(
    modules: &[ParsedModule],
    analysis: &CompilationAnalysis,
    session: &CompilationSession,
    bodies: &mut Vec<BodyIrModule>,
) -> Result<(), String> {
    let authored_start = bodies.len();
    let provider_operations = ProviderOperationCatalog::from_loaves(session.loaves())?;
    let unit = session.unit();
    let library_package = (unit.kind == UnitKind::Library).then_some(unit.package.as_str());
    let cycle_peers = crate::modules::import_cycle_peers(modules, session.own_standard_package());
    let type_infos = modules
        .iter()
        .map(|module| {
            analysis
                .type_info_for_path(&module.file_path)
                .ok_or("module analysis is missing")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let namespace_bodies = modules
        .iter()
        .map(|module| analysis.namespace_body_for_path(&module.file_path))
        .collect::<Vec<_>>();
    let components = authored_components(modules, session.own_standard_package(), &cycle_peers);
    let waves = component_waves(&components, modules.len())?;
    let mut authored = (0..modules.len()).map(|_| None).collect::<Vec<Option<BodyIrModule>>>();
    for wave in waves {
        let lowered = lower_component_wave(
            &wave,
            &components,
            modules,
            &type_infos,
            &namespace_bodies,
            &cycle_peers,
            bodies,
            &authored,
            &provider_operations,
            &unit.package,
            library_package,
            unit.kind == UnitKind::Test,
        )?;
        for (index, module) in lowered {
            if authored[index].replace(module).is_some() {
                return Err("Body IR source component produced a module twice".to_string());
            }
        }
    }
    let lowered = authored
        .into_iter()
        .map(|module| module.ok_or("Body IR source component omitted a module"))
        .collect::<Result<Vec<_>, _>>()?;
    bodies.extend(lowered);
    let selected = modules
        .iter()
        .filter_map(|module| analysis.type_info_for_path(&module.file_path))
        .flat_map(incan_frontend::body_ir::selected_body_identities)
        .collect::<HashSet<_>>();
    let (context, authored_bodies) = bodies.split_at_mut(authored_start);
    incan_frontend::body_ir::settle_shared_reads_with_selected(authored_bodies, context, &selected);
    Ok(())
}

/// Group source modules into import-cycle components and retain their external dependency closures.
fn authored_components(
    modules: &[ParsedModule],
    own_standard_package: Option<&'static incan_lang::lang::standard_packages::StandardPackageNamespacePolicy>,
    cycle_peers: &[Vec<usize>],
) -> Vec<AuthoredComponent> {
    let module_idx_by_key = crate::modules::module_key_index(modules);
    let mut claimed = vec![false; modules.len()];
    let mut components = Vec::new();
    for index in 0..modules.len() {
        if claimed[index] {
            continue;
        }
        let mut members = Vec::with_capacity(cycle_peers[index].len() + 1);
        members.push(index);
        members.extend(cycle_peers[index].iter().copied());
        members.sort_unstable();
        members.dedup();
        for &member in &members {
            claimed[member] = true;
        }
        let member_set = members.iter().copied().collect::<BTreeSet<_>>();
        let dependencies = members
            .iter()
            .flat_map(|&member| {
                crate::modules::imported_module_dep_indexes_for(
                    modules,
                    member,
                    &module_idx_by_key,
                    own_standard_package,
                )
            })
            .filter(|dependency| !member_set.contains(dependency))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        components.push(AuthoredComponent { members, dependencies });
    }
    components
}

/// Place source components in dependency-ready waves while retaining their stable collected order within each wave.
///
/// Parsed module order is an output-order contract, not a topological-order contract. Standard packages can collect a
/// consumer before its imported provider, so readiness is derived from the component graph itself.
fn component_waves(components: &[AuthoredComponent], module_count: usize) -> Result<Vec<Vec<usize>>, String> {
    let mut component_by_module = vec![usize::MAX; module_count];
    for (component_index, component) in components.iter().enumerate() {
        for &member in &component.members {
            component_by_module[member] = component_index;
        }
    }
    let dependencies = components
        .iter()
        .map(|component| {
            component
                .dependencies
                .iter()
                .map(|dependency| component_by_module[*dependency])
                .collect::<BTreeSet<_>>()
        })
        .collect::<Vec<_>>();
    let mut complete = vec![false; components.len()];
    let mut waves = Vec::new();
    while complete.iter().any(|done| !done) {
        let ready = dependencies
            .iter()
            .enumerate()
            .filter(|(index, required)| !complete[*index] && required.iter().all(|dependency| complete[*dependency]))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if ready.is_empty() {
            return Err("Body IR source component graph remains cyclic after import-cycle grouping".to_string());
        }
        for &index in &ready {
            complete[index] = true;
        }
        waves.push(ready);
    }
    Ok(waves)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_waves_do_not_require_dependencies_to_precede_consumers() -> Result<(), String> {
        let components = vec![
            AuthoredComponent {
                members: vec![0],
                dependencies: vec![1],
            },
            AuthoredComponent {
                members: vec![1],
                dependencies: Vec::new(),
            },
        ];
        assert_eq!(component_waves(&components, 2)?, vec![vec![1], vec![0]]);
        Ok(())
    }
}

/// Lower one dependency-ready wave with bounded scoped threads and return results in component order.
#[allow(
    clippy::too_many_arguments,
    reason = "the worker boundary carries one immutable compilation context"
)]
fn lower_component_wave(
    wave: &[usize],
    components: &[AuthoredComponent],
    modules: &[ParsedModule],
    type_infos: &[&TypeCheckInfo],
    namespace_bodies: &[Option<&BodyIrModule>],
    cycle_peers: &[Vec<usize>],
    dependency_bodies: &[BodyIrModule],
    authored: &[Option<BodyIrModule>],
    provider_operations: &ProviderOperationCatalog,
    registry_package: &str,
    library_package: Option<&str>,
    include_inline_tests: bool,
) -> Result<Vec<(usize, BodyIrModule)>, String> {
    let available = thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    let workers = available.min(MAX_BODY_IR_WORKERS).max(1);
    let mut lowered = Vec::new();
    for batch in wave.chunks(workers) {
        let mut completed = thread::scope(|scope| -> Result<Vec<Vec<(usize, BodyIrModule)>>, String> {
            let mut handles = Vec::with_capacity(batch.len());
            for &component_index in batch {
                let builder = thread::Builder::new()
                    .name(format!("incan-body-ir-{component_index}"))
                    .stack_size(incan_frontend::compiler_stack::compiler_stack_bytes());
                let handle = builder
                    .spawn_scoped(scope, move || {
                        lower_component(
                            &components[component_index],
                            modules,
                            type_infos,
                            namespace_bodies,
                            cycle_peers,
                            dependency_bodies,
                            authored,
                            provider_operations,
                            registry_package,
                            library_package,
                            include_inline_tests,
                        )
                    })
                    .map_err(|error| format!("cannot start Body IR worker: {error}"))?;
                handles.push(handle);
            }
            let mut results = Vec::with_capacity(handles.len());
            for handle in handles {
                match handle.join() {
                    Ok(result) => results.push(result?),
                    Err(payload) => std::panic::resume_unwind(payload),
                }
            }
            Ok(results)
        })?;
        for result in &mut completed {
            lowered.append(result);
        }
    }
    Ok(lowered)
}

/// Lower one source component serially against immutable external dependencies, preserving cycle peer declarations.
#[allow(
    clippy::too_many_arguments,
    reason = "the worker boundary carries one immutable compilation context"
)]
fn lower_component(
    component: &AuthoredComponent,
    modules: &[ParsedModule],
    type_infos: &[&TypeCheckInfo],
    namespace_bodies: &[Option<&BodyIrModule>],
    cycle_peers: &[Vec<usize>],
    dependency_bodies: &[BodyIrModule],
    authored: &[Option<BodyIrModule>],
    provider_operations: &ProviderOperationCatalog,
    registry_package: &str,
    library_package: Option<&str>,
    include_inline_tests: bool,
) -> Result<Vec<(usize, BodyIrModule)>, String> {
    let mut context_modules = Vec::with_capacity(dependency_bodies.len() + component.dependencies.len());
    context_modules.extend_from_slice(dependency_bodies);
    for &dependency in &component.dependencies {
        context_modules.push(
            authored[dependency]
                .as_ref()
                .ok_or("Body IR source dependency is incomplete")?
                .clone(),
        );
    }
    let context_len = context_modules.len();
    for &index in &component.members {
        let module = &modules[index];
        if let Some(restored) = namespace_bodies[index] {
            let mut restored = restored.clone();
            incan_frontend::body_ir::refresh_dependency_call_targets(&mut restored, &context_modules);
            context_modules.push(restored);
            continue;
        }
        let completed = context_modules.len();
        for &peer in cycle_peers[index].iter().filter(|&&peer| peer > index) {
            context_modules.push(incan_frontend::body_ir::build_cycle_declarations(
                &modules[peer].ast,
                &modules[peer].path_segments,
                type_infos[peer],
                library_package,
            ));
        }
        let context = incan_frontend::body_ir::BodyIrContext {
            modules: &context_modules,
            provider_operations,
            registry_package: Some(registry_package),
            library_package,
            include_inline_tests,
        };
        if let Some(directory) = std::env::var_os("INCAN_BODY_IR_FREE_BASELINE") {
            incan_frontend::body_ir::verify_free_function_inputs(
                &module.ast,
                &module.path_segments,
                type_infos[index],
                &context,
                std::path::Path::new(&directory),
            )
            .map_err(|error| format!("Body IR free-function input baseline: {error}"))?;
        }
        let _invocation =
            tracing::trace_span!(target: "incan::invocation", "incan_driver::build_body_ir_module", module = index)
                .entered();
        let lowered = incan_frontend::body_ir::build_body_ir_module_unsettled(
            &module.ast,
            &module.path_segments,
            type_infos[index],
            &context,
        );
        context_modules.truncate(completed);
        context_modules.push(lowered);
    }
    let lowered = context_modules.split_off(context_len);
    Ok(component.members.iter().copied().zip(lowered).collect())
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
