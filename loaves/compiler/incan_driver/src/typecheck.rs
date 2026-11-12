//! Typecheck orchestration over a collected module graph, and the C-binding verification that runs with it.
//!
//! This is the one place the driver calls the typechecker for a whole program; single-module callers go through
//! the session. Nothing here renders: diagnostics come back as values.

use std::path::Path;
use std::sync::Arc;

use crate::backend::c_abi::{CAbiVerificationPlan, ClangToolchain, verify_checked_c_binding};
use crate::diagnostics::{
    CliDiagnostic, CliDiagnosticFailure, CliDiagnosticSource, parser_warning_diagnostics, render_module_warnings,
    typecheck_diagnostic_phase, typecheck_warning_diagnostics,
};
use crate::modules::{
    import_cycle_peers, imported_module_dep_indexes_for, module_key_index, register_module_path_segments,
};
use crate::unit::UnitDescription;
use incan_frontend::loaf_index::LoafIndex;
use incan_frontend::module::{ExportedSymbol, exported_symbols};
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::typechecker::{
    CBindingDescriptor, DependencyInterfacePreparationCache, ModuleDependencyInterface, TypeCheckInfo,
};
use incan_frontend::{diagnostics, typechecker};
use incan_lang::lang::standard_packages::StandardPackageNamespacePolicy;
use incan_semantics_core::SymbolOrigin;

/// Products retained from one dependency-safe typechecking pass.
pub struct TypecheckedModules {
    /// One result per input module, in input order.
    pub type_infos: Vec<TypeCheckInfo>,
    /// Non-fatal parser and typechecker diagnostics, for reports that expose them.
    ///
    /// Ordered by module, then by parser-before-typechecker within each module, so a machine-readable report is
    /// byte-stable across runs of the same source.
    pub warnings: Vec<CliDiagnostic>,
}

/// Typecheck a collected graph in dependency-safe order against the program's Loaves, retaining one result for every
/// input module, and verify its checked C bindings against `c_abi_plan` (the host when absent). A library unit passes
/// its `package`, the package identity of every declaration it checks (DD-0007). `visit` sees each module's checker
/// once it is checked, cleanly or not: publication reads the same pass and only uses one that succeeds, and an editor
/// reads the facts of a module with errors too.
///
/// Ordering is intentional: session analysis also needs an identity-keyed representation for synthetic modules that
/// share a source file path.
pub fn typecheck_modules(
    modules: &[ParsedModule],
    unit: &UnitDescription,
    loaves: &Arc<LoafIndex>,
    own_standard_package: Option<&'static StandardPackageNamespacePolicy>,
    package: Option<&str>,
    c_abi_plan: Option<&CAbiVerificationPlan>,
    visit: &mut dyn FnMut(&ParsedModule, &typechecker::TypeChecker),
    body_reuse: &std::collections::BTreeMap<
        std::path::PathBuf,
        incan_frontend::declaration_interface::ValidatedCallableBodies,
    >,
    graph_body_records: &incan_frontend::checker_records::GraphBodyRecords,
) -> Result<TypecheckedModules, CliDiagnosticFailure> {
    let _trace = tracing::trace_span!(target: "incan::invocation", "incan_driver::typecheck_modules").entered();
    let declared = unit
        .rust_crates
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    let module_idx_by_key = module_key_index(modules);
    let dependency_exports = modules
        .iter()
        .map(|module| Arc::<[ExportedSymbol]>::from(exported_symbols(&module.ast)))
        .collect::<Vec<_>>();
    let mut diagnostics_out = Vec::new();
    let mut warnings_out = Vec::new();
    let mut type_infos = Vec::with_capacity(modules.len());
    let mut dependency_interfaces = vec![None::<Arc<ModuleDependencyInterface>>; modules.len()];
    let cycle_peers = import_cycle_peers(modules, own_standard_package);
    let related_sources = modules
        .iter()
        .map(|module| {
            let origin = package
                .map(|library| SymbolOrigin::Package {
                    library: library.to_string(),
                    module_path: module.path_segments.clone(),
                })
                .unwrap_or_else(|| SymbolOrigin::Module(module.path_segments.clone()));
            (
                origin,
                CliDiagnosticSource {
                    file_path: module.file_path.to_string_lossy().to_string(),
                    source: module.source.clone(),
                },
            )
        })
        .collect();
    let memory_trace = std::env::var_os("INCAN_TRACE_TYPECHECK_MEMORY").is_some();
    let mut not_prepared = unit
        .not_prepared_loaves
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    not_prepared.extend(
        unit.loaves
            .iter()
            .filter(|loaf| unit.not_prepared_loaves.contains(&loaf.name))
            .flat_map(|loaf| [loaf.key.clone(), loaf.name.clone()]),
    );
    let dependency_preparations = DependencyInterfacePreparationCache::default();
    let configured_checker = |module: &ParsedModule| {
        let mut checker = typechecker::TypeChecker::new();
        checker.set_declared_crate_names(declared.clone());
        checker.set_current_module_path(Some(module.path_segments.clone()));
        checker.set_current_package_identity(package.map(str::to_owned));
        checker.set_record_package_identity(unit.record_package_identity());
        register_module_path_segments(&mut checker, modules);
        checker.set_loaves(Arc::clone(loaves));
        checker.set_dependency_interface_preparation_cache(dependency_preparations.clone());
        checker.set_not_prepared_loaves(not_prepared.clone());
        if let Some(policy) = own_standard_package {
            checker.set_own_standard_package(policy);
        }
        if let Some(products) = body_reuse.get(&module.file_path) {
            checker.set_callable_body_reuse(products.clone());
        }
        checker.set_graph_body_records(graph_body_records.clone());
        checker
    };

    for (idx, module) in modules.iter().enumerate() {
        let dep_indexes = imported_module_dep_indexes_for(modules, idx, &module_idx_by_key, own_standard_package);
        let deps_for_module = dep_indexes
            .iter()
            .map(|dep_idx| (modules[*dep_idx].name.as_str(), &modules[*dep_idx].ast))
            .collect::<Vec<_>>();
        let exports_for_module = dep_indexes
            .iter()
            .map(|dep_idx| {
                (
                    modules[*dep_idx].name.clone(),
                    Arc::clone(&dependency_exports[*dep_idx]),
                )
            })
            .collect();
        let interfaces_for_module = dep_indexes
            .iter()
            .map(|dep_idx| dependency_interfaces[*dep_idx].as_ref().map(Arc::clone))
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default();
        let _module_trace = tracing::trace_span!(
            target: "incan::invocation",
            "incan_driver::typecheck_module",
            module = %module.file_path.display(),
            declarations = module.ast.declarations.len(),
            dependencies = deps_for_module.len(),
        )
        .entered();

        // Parser warnings were already rendered at parse time; collect them here so both warning classes reach
        // machine-readable reports from one deterministic sweep.
        warnings_out.extend(parser_warning_diagnostics(module));

        let check_result = {
            let mut checker = configured_checker(module);
            checker.select_dependency_interfaces(interfaces_for_module);

            // A standard package checks its complete source before publishing its public checked facade.
            let checked = if own_standard_package.is_some() {
                checker.check_standard_package_with_imports_and_exports(
                    &module.ast,
                    &deps_for_module,
                    exports_for_module,
                )
            } else {
                checker.check_with_imports_and_exports(&module.ast, &deps_for_module, exports_for_module)
            };
            trace_typecheck_memory(
                memory_trace,
                "checked",
                module.file_path.as_path(),
                deps_for_module.len(),
            );
            // Warnings accumulate on the checker whether or not checking succeeded, so surface them from both arms:
            // a module that also has an error should not hide its warnings from the user or from the report.
            render_module_warnings(
                module.file_path.to_string_lossy().as_ref(),
                &module.source,
                checker.warnings(),
            );
            warnings_out.extend(typecheck_warning_diagnostics(module, checker.warnings()));

            visit(module, &checker);
            checked.map(|()| {
                let interface = if dependency_interfaces[idx].is_none() && cycle_peers[idx].is_empty() {
                    Some(checker.seal_current_module_dependency_interface(
                        &module.name,
                        &module.ast,
                        Arc::clone(&dependency_exports[idx]),
                        own_standard_package.is_none(),
                    ))
                } else {
                    None
                };
                (checker.into_type_info(), interface)
            })
        };
        trace_typecheck_memory(
            memory_trace,
            "released",
            module.file_path.as_path(),
            deps_for_module.len(),
        );
        match check_result {
            Ok((type_info, interface)) => {
                type_infos.push(type_info);
                if let Some(interface) = interface {
                    dependency_interfaces[idx] = Some(interface);
                }
                if dependency_interfaces[idx].is_none() {
                    let mut component_indexes = Vec::with_capacity(cycle_peers[idx].len() + 1);
                    component_indexes.push(idx);
                    component_indexes.extend(cycle_peers[idx].iter().copied());
                    component_indexes.sort_unstable();
                    let external_interfaces = dep_indexes
                        .iter()
                        .filter(|dep_idx| !component_indexes.contains(dep_idx))
                        .filter_map(|dep_idx| dependency_interfaces[*dep_idx].as_ref().map(Arc::clone))
                        .collect::<Vec<_>>();
                    let mut builder = configured_checker(&modules[component_indexes[0]]);
                    builder.install_dependency_interfaces_directly(&external_interfaces);
                    let component = component_indexes
                        .iter()
                        .map(|component_idx| {
                            (
                                modules[*component_idx].name.as_str(),
                                &modules[*component_idx].ast,
                                Arc::clone(&dependency_exports[*component_idx]),
                            )
                        })
                        .collect::<Vec<_>>();
                    let built = builder.build_dependency_interface_component(
                        &component,
                        &deps_for_module,
                        own_standard_package.is_none(),
                    );
                    debug_assert_eq!(built.len(), component_indexes.len());
                    for (component_idx, interface) in component_indexes.into_iter().zip(built) {
                        dependency_interfaces[component_idx] = Some(interface);
                    }
                }
            }
            Err(errs) => diagnostics_out.extend(errs.into_iter().map(|error| CliDiagnostic {
                file_path: module.file_path.to_string_lossy().to_string(),
                source: module.source.clone(),
                phase: typecheck_diagnostic_phase(module, &error),
                error,
            })),
        }
    }

    if diagnostics_out.is_empty() {
        debug_assert!(dependency_interfaces.iter().all(Option::is_some));
        verify_checked_c_bindings(modules, unit, &mut type_infos, c_abi_plan, &mut diagnostics_out);
    }

    if diagnostics_out.is_empty() {
        Ok(TypecheckedModules {
            type_infos,
            warnings: warnings_out,
        })
    } else {
        Err(CliDiagnosticFailure {
            diagnostics: diagnostics_out,
            related_sources,
            warnings: warnings_out,
        })
    }
}

/// Emit one opt-in memory snapshot around a module check so retained checker state and allocator-held pages can be
/// distinguished without affecting ordinary compiler output.
fn trace_typecheck_memory(enabled: bool, stage: &str, module: &Path, dependencies: usize) {
    if !enabled {
        return;
    }
    let footprint = incan_platform::current_process_footprint()
        .map(|bytes| bytes.to_string())
        .unwrap_or_else(|error| format!("unavailable:{error}"));
    let live = incan_platform::current_process_live_allocated()
        .map(|bytes| bytes.to_string())
        .unwrap_or_else(|error| format!("unavailable:{error}"));
    println!(
        "typecheck memory: stage={stage} module={} dependencies={dependencies} footprint={footprint} live={live}",
        module.display()
    );
}

/// Verify every checked C descriptor against the host or explicitly selected target before any code generation.
///
/// The typechecker has already established the source contract. This phase only checks those explicit facts against
/// the declared header for the target; it neither imports arbitrary headers nor performs library discovery.
fn verify_checked_c_bindings(
    modules: &[ParsedModule],
    unit: &UnitDescription,
    type_infos: &mut [TypeCheckInfo],
    selected_plan: Option<&CAbiVerificationPlan>,
    diagnostics_out: &mut Vec<CliDiagnostic>,
) {
    let mut bindings = Vec::new();
    for (module_index, (module, type_info)) in modules.iter().zip(type_infos.iter()).enumerate() {
        let mut module_bindings = type_info.c_abi.bindings.values().cloned().collect::<Vec<_>>();
        module_bindings.sort_by(|left, right| left.class_name.cmp(&right.class_name));
        bindings.extend(
            module_bindings
                .into_iter()
                .map(|binding| (module_index, module, binding)),
        );
    }
    if bindings.is_empty() {
        return;
    }
    let Some(plan) = selected_plan.cloned().or_else(CAbiVerificationPlan::host) else {
        for (_, module, binding) in bindings {
            diagnostics_out.push(CliDiagnostic {
                file_path: module.file_path.to_string_lossy().to_string(),
                source: module.source.clone(),
                phase: diagnostics::DiagnosticPhase::Typecheck,
                error: diagnostics::CompileError::type_error(
                    "checked C bindings currently require a Linux x86-64 or macOS arm64 host verification target"
                        .to_string(),
                    binding.span,
                ),
            });
        }
        return;
    };
    let toolchain = match ClangToolchain::discover(&plan) {
        Ok(toolchain) => toolchain,
        Err(error) => {
            for (_, module, binding) in bindings {
                diagnostics_out.push(CliDiagnostic {
                    file_path: module.file_path.to_string_lossy().to_string(),
                    source: module.source.clone(),
                    phase: diagnostics::DiagnosticPhase::Typecheck,
                    error: diagnostics::CompileError::type_error(error.to_string(), binding.span),
                });
            }
            return;
        }
    };
    for (module_index, module, binding) in bindings {
        let verification_binding = resolve_package_owned_c_binding_header(unit, &binding);
        match verify_checked_c_binding(&toolchain, &plan, &verification_binding) {
            Ok(receipt) => {
                type_infos[module_index]
                    .c_abi
                    .verified_bindings
                    .insert(binding.class_name.clone());
                let enum_values = &mut type_infos[module_index].c_abi.enum_values;
                for ((enumeration, variant), value) in receipt.enum_values() {
                    enum_values.insert(
                        (binding.class_name.clone(), enumeration.clone(), variant.clone()),
                        *value,
                    );
                }
            }
            Err(error) => {
                diagnostics_out.push(CliDiagnostic {
                    file_path: module.file_path.to_string_lossy().to_string(),
                    source: module.source.clone(),
                    phase: diagnostics::DiagnosticPhase::Typecheck,
                    error: diagnostics::CompileError::type_error(error.to_string(), binding.span),
                });
            }
        }
    }
}

/// Resolve a package-declared header before passing its spelling to Clang.
///
/// Checked bindings keep the authored, package-relative header spelling in their public descriptor. The verifier alone
/// needs a concrete file location. Restricting this translation to a header the package declares (the unit's
/// `c_headers`) prevents an arbitrary relative binding path from becoming an ambient include-directory search.
fn resolve_package_owned_c_binding_header(unit: &UnitDescription, binding: &CBindingDescriptor) -> CBindingDescriptor {
    if Path::new(&binding.header).is_absolute() || !unit.c_headers.contains(&binding.header) {
        return binding.clone();
    }
    let mut resolved = binding.clone();
    resolved.header = unit.project_root.join(&binding.header).to_string_lossy().into_owned();
    resolved
}
