//! Typecheck orchestration over a collected module graph, and the C-binding verification that runs with it.
//!
//! This is the one place the driver calls the typechecker for a whole program; single-module callers go through
//! the session. Nothing here renders: diagnostics come back as values.

use std::path::Path;
use std::sync::Arc;

use crate::backend::c_abi::{CAbiVerificationPlan, ClangToolchain, verify_checked_c_binding};
use crate::diagnostics::{
    CliDiagnostic, CliDiagnosticFailure, parser_warning_diagnostics, render_module_warnings,
    typecheck_diagnostic_phase, typecheck_warning_diagnostics,
};
use crate::modules::{imported_module_deps_for, module_key_index, register_module_path_segments};
use crate::unit::UnitDescription;
use incan_frontend::loaf_index::LoafIndex;
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::typechecker::{CBindingDescriptor, TypeCheckInfo};
use incan_frontend::{diagnostics, typechecker};
use incan_lang::lang::standard_packages::StandardPackageNamespacePolicy;

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
) -> Result<TypecheckedModules, CliDiagnosticFailure> {
    let _trace = tracing::trace_span!(target: "incan::invocation", "incan_driver::typecheck_modules").entered();
    let declared = unit
        .rust_crates
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    let module_idx_by_key = module_key_index(modules);
    let mut diagnostics_out = Vec::new();
    let mut warnings_out = Vec::new();
    let mut type_infos = Vec::with_capacity(modules.len());

    for (idx, module) in modules.iter().enumerate() {
        let deps_for_module = imported_module_deps_for(modules, idx, &module_idx_by_key, own_standard_package);

        // Parser warnings were already rendered at parse time; collect them here so both warning classes reach
        // machine-readable reports from one deterministic sweep.
        warnings_out.extend(parser_warning_diagnostics(module));

        let mut checker = typechecker::TypeChecker::new();
        checker.set_declared_crate_names(declared.clone());
        checker.set_current_module_path(Some(module.path_segments.clone()));
        checker.set_current_package_identity(package.map(str::to_owned));
        register_module_path_segments(&mut checker, modules);
        checker.set_loaves(Arc::clone(loaves));
        if let Some(policy) = own_standard_package {
            checker.set_own_standard_package(policy);
        }

        // A standard package checks its complete source before publishing its public checked facade.
        let check_result = if own_standard_package.is_some() {
            checker.check_with_imports_allow_private(&module.ast, &deps_for_module)
        } else {
            checker.check_with_imports(&module.ast, &deps_for_module)
        };
        // Warnings accumulate on the checker whether or not checking succeeded, so surface them from both arms:
        // a module that also has an error should not hide its warnings from the user or from the report.
        render_module_warnings(
            module.file_path.to_string_lossy().as_ref(),
            &module.source,
            checker.warnings(),
        );
        warnings_out.extend(typecheck_warning_diagnostics(module, checker.warnings()));

        visit(module, &checker);
        match check_result {
            Ok(()) => type_infos.push(checker.type_info().clone()),
            Err(errs) => {
                diagnostics_out.extend(errs.into_iter().map(|error| CliDiagnostic {
                    file_path: module.file_path.to_string_lossy().to_string(),
                    source: module.source.clone(),
                    phase: typecheck_diagnostic_phase(module, error.span),
                    error,
                }));
            }
        }
    }

    if diagnostics_out.is_empty() {
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
            warnings: warnings_out,
        })
    }
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
