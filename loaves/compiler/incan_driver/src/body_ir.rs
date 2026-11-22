//! Shared checked source-to-Body-IR construction for compilation, inspection and editor analysis.

use crate::session::{CompilationAnalysis, CompilationSession};
use crate::unit::UnitKind;
use incan_frontend::body_ir::{ContextModules, ProviderOperationCatalog};
use incan_frontend::loaf_index::LoafIndex;
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::typechecker::TypeCheckInfo;
use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};
use std::borrow::Cow;
use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;
use std::thread;

/// Maximum source components lowered at once inside one compiler invocation.
///
/// Each worker reads the dependency context in place and owns its component's modules and a compiler-sized stack. Four
/// workers bound those temporary graphs and stacks conservatively inside the 6 GiB lowering reservation while still
/// using independent cores.
const MAX_BODY_IR_WORKERS: usize = 4;

/// One strongly connected source component and the completed source modules it imports.
struct AuthoredComponent<'graph> {
    members: Vec<usize>,
    dependencies: Cow<'graph, [usize]>,
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
    let namespaces = modules
        .iter()
        .map(|module| analysis.namespace_body_by_path.get(&module.file_path).cloned())
        .collect();
    lower_with_namespaces(modules, analysis, session, bodies, namespaces).map(|_| ())
}

/// Lower the active compilation, transferring its admitted namespace bodies into the output.
/// Other consumers may still inspect the checked facts and lowering witnesses in `analysis`.
pub fn lower_compilation_modules(
    modules: &[ParsedModule],
    analysis: &mut CompilationAnalysis,
    session: &CompilationSession,
    bodies: &mut Vec<BodyIrModule>,
) -> Result<BTreeSet<std::path::PathBuf>, String> {
    let retained = std::mem::take(&mut analysis.namespace_body_by_path);
    let namespaces = modules
        .iter()
        .map(|module| retained.get(&module.file_path).cloned())
        .collect();
    // A test unit may deliberately name one source twice. Each module keeps a handle, while the ordinary
    // one-source/one-module case transfers the sole owner into its worker.
    drop(retained);
    lower_with_namespaces(modules, analysis, session, bodies, namespaces)
}

fn lower_with_namespaces(
    modules: &[ParsedModule],
    analysis: &CompilationAnalysis,
    session: &CompilationSession,
    bodies: &mut Vec<BodyIrModule>,
    mut namespace_bodies: Vec<Option<Arc<BodyIrModule>>>,
) -> Result<BTreeSet<std::path::PathBuf>, String> {
    let authored_start = bodies.len();
    let restored = namespace_bodies.iter().map(Option::is_some).collect::<Vec<_>>();
    let provider_operations = ProviderOperationCatalog::from_loaves(session.loaves())?;
    let unit = session.unit();
    let library_package = (unit.kind == UnitKind::Library).then_some(unit.package.as_str());
    let imports = &analysis.imports;
    let cycle_peers = imports.cycle_peers();
    let type_infos = modules
        .iter()
        .map(|module| {
            analysis
                .type_info_for_path(&module.file_path)
                .ok_or("module analysis is missing")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let components = authored_components(imports, cycle_peers);
    let waves = component_waves(&components, modules.len())?;
    let mut authored = (0..modules.len()).map(|_| None).collect::<Vec<Option<BodyIrModule>>>();
    let mut rebound = BTreeSet::new();
    for wave in waves {
        let lowered = lower_component_wave(
            &wave,
            &components,
            modules,
            &type_infos,
            &mut namespace_bodies,
            cycle_peers,
            bodies,
            &authored,
            &provider_operations,
            &unit.package,
            library_package,
            unit.kind == UnitKind::Test,
        )?;
        for LoweredSource {
            index,
            mut module,
            changed_calls,
        } in lowered
        {
            if changed_calls {
                rebound.insert(modules[index].file_path.clone());
            }
            // The namespace codec has already restored shared nominal scopes. Only newly built bodies require
            // interning.
            if !restored[index] {
                incan_semantics_core::body_ir::share_equal_identity_maps(&mut module.bodies);
            }
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
    incan_frontend::body_ir::settle_shared_reads_with_namespaces(authored_bodies, context, &selected, &restored);
    Ok(rebound)
}

/// Group source modules into import-cycle components and retain their external dependency closures.
fn authored_components<'graph>(
    imports: &'graph crate::modules::SourceModuleImports,
    cycle_peers: &[Vec<usize>],
) -> Vec<AuthoredComponent<'graph>> {
    let mut claimed = vec![false; cycle_peers.len()];
    let mut components = Vec::new();
    for index in 0..cycle_peers.len() {
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
        let dependencies = if members.len() == 1 {
            Cow::Borrowed(imports.dependencies(index))
        } else {
            let member_set = members.iter().copied().collect::<BTreeSet<_>>();
            Cow::Owned(
                members
                    .iter()
                    .flat_map(|&member| imports.dependencies(member).iter().copied())
                    .filter(|dependency| !member_set.contains(dependency))
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            )
        };
        components.push(AuthoredComponent { members, dependencies });
    }
    components
}

/// Place source components in dependency-ready waves while retaining their stable collected order within each wave.
///
/// Parsed module order is an output-order contract, not a topological-order contract. Standard packages can collect a
/// consumer before its imported provider, so readiness is derived from the component graph itself.
fn component_waves(components: &[AuthoredComponent<'_>], module_count: usize) -> Result<Vec<Vec<usize>>, String> {
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
                dependencies: Cow::Borrowed(&[1]),
            },
            AuthoredComponent {
                members: vec![1],
                dependencies: Cow::Borrowed(&[]),
            },
        ];
        assert_eq!(component_waves(&components, 2)?, vec![vec![1], vec![0]]);
        Ok(())
    }

    #[test]
    fn a_component_reads_the_modules_it_imports_where_they_are_kept() -> Result<(), String> {
        let authored = vec![
            Some(BodyIrModule::declarations_only("pub::app::models".to_string())),
            None,
        ];
        let component = AuthoredComponent {
            members: vec![1],
            dependencies: Cow::Borrowed(&[0]),
        };
        let imported = imported_modules(&component, &authored)?;
        let read: &BodyIrModule = &imported[0];
        let kept = authored[0].as_ref().ok_or("the imported module is missing")?;
        assert!(
            std::ptr::eq(read, kept),
            "a component must read an imported module in place, not a copy of it"
        );
        Ok(())
    }
}

/// An authored module and whether a retained body's imported call locations changed for this activation.
struct LoweredSource {
    index: usize,
    module: BodyIrModule,
    changed_calls: bool,
}

/// Lower one dependency-ready wave with bounded scoped threads and return results in component order.
#[allow(
    clippy::too_many_arguments,
    reason = "the worker boundary carries one immutable compilation context"
)]
fn lower_component_wave(
    wave: &[usize],
    components: &[AuthoredComponent<'_>],
    modules: &[ParsedModule],
    type_infos: &[&TypeCheckInfo],
    namespace_bodies: &mut [Option<Arc<BodyIrModule>>],
    cycle_peers: &[Vec<usize>],
    dependency_bodies: &[BodyIrModule],
    authored: &[Option<BodyIrModule>],
    provider_operations: &ProviderOperationCatalog,
    registry_package: &str,
    library_package: Option<&str>,
    include_inline_tests: bool,
) -> Result<Vec<LoweredSource>, String> {
    let available = thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    let workers = available.clamp(1, MAX_BODY_IR_WORKERS);
    let batch_size = wave.len().div_ceil(workers).max(1);
    thread::scope(|scope| {
        let mut handles = Vec::new();
        for batch in wave.chunks(batch_size) {
            let jobs = batch
                .iter()
                .map(|&index| {
                    let component = &components[index];
                    let retained = component
                        .members
                        .iter()
                        .map(|&member| namespace_bodies[member].take())
                        .collect();
                    (component, retained)
                })
                .collect::<Vec<_>>();
            let handle = thread::Builder::new()
                .name(format!("incan-body-ir-{}", batch[0]))
                .stack_size(incan_frontend::compiler_stack::compiler_stack_bytes())
                .spawn_scoped(scope, move || -> Result<Vec<LoweredSource>, String> {
                    let mut result = Vec::new();
                    for (component, retained) in jobs {
                        result.extend(lower_component(
                            component,
                            modules,
                            type_infos,
                            retained,
                            cycle_peers,
                            dependency_bodies,
                            authored,
                            provider_operations,
                            registry_package,
                            library_package,
                            include_inline_tests,
                        )?);
                    }
                    Ok(result)
                })
                .map_err(|error| format!("cannot start Body IR worker: {error}"))?;
            handles.push(handle);
        }
        let mut lowered = Vec::new();
        for handle in handles {
            match handle.join() {
                Ok(result) => lowered.extend(result?),
                Err(payload) => std::panic::resume_unwind(payload),
            }
        }
        Ok(lowered)
    })
}

/// Lower one source component serially against immutable external dependencies, preserving cycle peer declarations.
#[allow(
    clippy::too_many_arguments,
    reason = "the worker boundary carries one immutable compilation context"
)]
fn lower_component(
    component: &AuthoredComponent<'_>,
    modules: &[ParsedModule],
    type_infos: &[&TypeCheckInfo],
    retained: Vec<Option<Arc<BodyIrModule>>>,
    cycle_peers: &[Vec<usize>],
    dependency_bodies: &[BodyIrModule],
    authored: &[Option<BodyIrModule>],
    provider_operations: &ProviderOperationCatalog,
    registry_package: &str,
    library_package: Option<&str>,
    include_inline_tests: bool,
) -> Result<Vec<LoweredSource>, String> {
    // The context is read in place: the dependency Loaves' modules, the imported source modules, this component's
    // completed members and, while a cycle member is lowered, the declarations of the peers that follow it.
    let imported = imported_modules(component, authored)?;
    let mut completed: Vec<BodyIrModule> = Vec::with_capacity(component.members.len());
    let mut changed_calls = Vec::with_capacity(component.members.len());
    for (&index, restored) in component.members.iter().zip(retained) {
        let module = &modules[index];
        if let Some(restored) = restored {
            let mut restored = Arc::unwrap_or_clone(restored);
            let borrowed = imported.iter().copied().chain(&completed).collect::<Vec<_>>();
            changed_calls.push(incan_frontend::body_ir::refresh_dependency_call_targets(
                &mut restored,
                ContextModules::new(dependency_bodies, &borrowed),
            ));
            completed.push(restored);
            continue;
        }
        let peers = cycle_peers[index]
            .iter()
            .filter(|&&peer| peer > index)
            .map(|&peer| {
                incan_frontend::body_ir::build_cycle_declarations(
                    &modules[peer].ast,
                    &modules[peer].path_segments,
                    type_infos[peer],
                    library_package,
                )
            })
            .collect::<Vec<_>>();
        let lowered = {
            let borrowed = imported
                .iter()
                .copied()
                .chain(&completed)
                .chain(&peers)
                .collect::<Vec<_>>();
            let context = incan_frontend::body_ir::BodyIrContext {
                modules: ContextModules::new(dependency_bodies, &borrowed),
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
            let _invocation = tracing::trace_span!(
                target: "incan::invocation",
                "incan_driver::build_body_ir_module",
                module = index,
                file = %module.file_path.display(),
            )
            .entered();
            let mut lowered = incan_frontend::body_ir::build_body_ir_module_unsettled(
                &module.ast,
                &module.path_segments,
                type_infos[index],
                &context,
            );
            // A changed dependency declaration span can invalidate this namespace while leaving an exact scalar body
            // reusable. That retained body still carries the dependency's previous edit-local canonical locator, so a
            // newly assembled module needs the same rebinding as a whole-module restore before native planning.
            incan_frontend::body_ir::refresh_dependency_call_targets(
                &mut lowered,
                ContextModules::new(dependency_bodies, &borrowed),
            );
            lowered
        };
        completed.push(lowered);
        changed_calls.push(false);
    }
    Ok(component
        .members
        .iter()
        .copied()
        .zip(completed)
        .zip(changed_calls)
        .map(|((index, module), changed_calls)| LoweredSource {
            index,
            module,
            changed_calls,
        })
        .collect())
}

/// The completed source modules `component` imports, in import order, read where `authored` keeps them.
fn imported_modules<'a>(
    component: &AuthoredComponent<'_>,
    authored: &'a [Option<BodyIrModule>],
) -> Result<Vec<&'a BodyIrModule>, String> {
    component
        .dependencies
        .iter()
        .map(|&dependency| {
            authored[dependency]
                .as_ref()
                .ok_or_else(|| "Body IR source dependency is incomplete".to_string())
        })
        .collect()
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
    let checked = modules
        .iter()
        .filter(|module| analysis.namespace_body_for_path(&module.file_path).is_none())
        .map(|module| {
            let info = analysis
                .type_info_for_path(&module.file_path)
                .ok_or("module analysis is missing")?;
            Ok::<_, String>((module.path_segments.as_slice(), info))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut requested = checked_package_roots(&checked, package_identity);
    for identity in &analysis.namespace_package_roots {
        if !matches!(&identity.origin, SymbolOrigin::Package { library, .. } if library != package_identity) {
            return Err("restored namespace supplied a non-dependency package root".into());
        }
        requested.insert(identity.clone());
    }
    Ok(requested)
}

/// Project the checked roots before a namespace discards its transient type information. Publication
/// and ordinary dependency resolution use the same semantic facts and compiler-specific requirements.
pub fn checked_package_roots(
    checked: &[(&[String], &TypeCheckInfo)],
    package_identity: &str,
) -> BTreeSet<CanonicalSymbolId> {
    let infos = checked.iter().map(|(_, info)| *info).collect::<Vec<_>>();
    let facts = checked
        .iter()
        .map(|(path, info)| info.semantic_fact_store(path))
        .collect::<Vec<_>>();
    let roots = infos.iter().copied().flat_map(|info| {
        info.declarations
            .declaration_identities
            .values()
            .chain(info.declarations.member_declaration_identities.values())
            .cloned()
    });
    incan_semantics_core::dependencies::CheckedDependencyGraph::from_fact_stores(&facts)
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
        .collect()
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
