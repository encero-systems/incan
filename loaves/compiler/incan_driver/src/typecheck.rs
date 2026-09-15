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
#[cfg(test)]
use crate::error::{CliError, CliResult};
use crate::modules::{imported_module_deps_for_with_provider_plan, module_key_index, register_module_path_segments};
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::typechecker::stdlib_loader::StdlibAstCache;
use incan_frontend::typechecker::{CBindingDescriptor, TypeCheckInfo};
use incan_frontend::{diagnostics, typechecker};
use incan_provider::ProviderPlan;
use oven_model::manifest::ProjectManifest;
/// Typecheck all collected modules in dependency-safe order using shared CLI diagnostics formatting.
///
/// This helper centralizes the per-module checker setup used by `build` and `check` paths so warning/error rendering
/// stays consistent across command flows.
#[cfg(test)]
pub fn typecheck_modules_with_import_graph(
    modules: &[ParsedModule],
    manifest: Option<&ProjectManifest>,
    provider_plan: &Arc<ProviderPlan>,
    #[cfg(feature = "rust_inspect")] rust_inspect_manifest_dir: Option<&Path>,
) -> CliResult<()> {
    typecheck_modules_with_import_graph_detailed_for_c_abi_target(
        modules,
        manifest,
        provider_plan,
        None,
        #[cfg(feature = "rust_inspect")]
        rust_inspect_manifest_dir,
    )
    .map(|_warnings| ())
    .map_err(|failure| CliError::failure(failure.render_human()))
}

/// Typecheck modules while selecting one already-validated C ABI target for this invocation.
///
/// Returns the non-fatal parser and typechecker diagnostics collected along the way, so callers that emit a
/// machine-readable report can include them; callers that only care about acceptance can discard them.
pub fn typecheck_modules_with_import_graph_detailed_for_c_abi_target(
    modules: &[ParsedModule],
    manifest: Option<&ProjectManifest>,
    provider_plan: &Arc<ProviderPlan>,
    c_abi_plan: Option<&CAbiVerificationPlan>,
    #[cfg(feature = "rust_inspect")] rust_inspect_manifest_dir: Option<&Path>,
) -> Result<Vec<CliDiagnostic>, CliDiagnosticFailure> {
    typecheck_modules_with_import_graph_artifacts(
        modules,
        manifest,
        provider_plan,
        c_abi_plan,
        #[cfg(feature = "rust_inspect")]
        rust_inspect_manifest_dir,
    )
    .map(|artifacts| artifacts.warnings)
}

/// Products retained from one dependency-safe typechecking pass.
pub struct TypecheckModuleArtifacts {
    pub type_infos: Vec<TypeCheckInfo>,
    pub stdlib_cache: StdlibAstCache,
    /// Non-fatal parser and typechecker diagnostics, for reports that expose them.
    ///
    /// Ordered by module, then by parser-before-typechecker within each module, so a machine-readable report is
    /// byte-stable across runs of the same source.
    warnings: Vec<CliDiagnostic>,
}

/// Typecheck a collected graph in dependency-safe order and retain one result for every input module plus the
/// source-backed stdlib metadata lowering needs.
///
/// Ordering is intentional: session analysis also needs an identity-keyed representation for synthetic modules that
/// share a source file path.
pub fn typecheck_modules_with_import_graph_artifacts(
    modules: &[ParsedModule],
    manifest: Option<&ProjectManifest>,
    provider_plan: &Arc<ProviderPlan>,
    c_abi_plan: Option<&CAbiVerificationPlan>,
    #[cfg(feature = "rust_inspect")] rust_inspect_manifest_dir: Option<&Path>,
) -> Result<TypecheckModuleArtifacts, CliDiagnosticFailure> {
    let declared = manifest.map(|m| m.declared_rust_crate_names());
    let module_idx_by_key = module_key_index(modules);
    let mut diagnostics_out = Vec::new();
    let mut warnings_out = Vec::new();
    let mut type_infos = Vec::with_capacity(modules.len());
    let mut stdlib_cache = StdlibAstCache::new();

    for (idx, module) in modules.iter().enumerate() {
        let deps_for_module =
            imported_module_deps_for_with_provider_plan(modules, idx, &module_idx_by_key, provider_plan);

        // Parser warnings were already rendered at parse time; collect them here so both warning classes reach
        // machine-readable reports from one deterministic sweep.
        warnings_out.extend(parser_warning_diagnostics(module));

        let mut checker = typechecker::TypeChecker::new();
        checker.stdlib_cache = stdlib_cache.clone();
        if let Some(names) = declared.clone() {
            checker.set_declared_crate_names(names);
        }
        checker.set_current_module_path(Some(module.path_segments.clone()));
        register_module_path_segments(&mut checker, modules);
        checker.set_provider_plan(Arc::clone(provider_plan));
        #[cfg(feature = "rust_inspect")]
        if let Some(rust_inspect_manifest_dir) = rust_inspect_manifest_dir {
            checker.set_rust_inspect_manifest_dir(rust_inspect_manifest_dir.to_path_buf());
        }

        // A provider producer checks its complete source package before publishing the public checked facade.
        let check_result = if provider_plan.bootstrap_sdk_namespace_roots().next().is_some() {
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

        match check_result {
            Ok(()) => {
                type_infos.push(checker.type_info().clone());
                stdlib_cache = checker.stdlib_cache.clone();
            }
            Err(errs) => {
                stdlib_cache = checker.stdlib_cache.clone();
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
        verify_checked_c_bindings(modules, manifest, &mut type_infos, c_abi_plan, &mut diagnostics_out);
    }

    if diagnostics_out.is_empty() {
        Ok(TypecheckModuleArtifacts {
            type_infos,
            stdlib_cache,
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
    manifest: Option<&ProjectManifest>,
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
        let verification_binding = resolve_package_owned_c_binding_header(manifest, &binding);
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

/// Resolve a manifest-declared package header before passing its spelling to Clang.
///
/// Checked bindings keep the authored, package-relative header spelling in their public descriptor and lock identity.
/// The verifier alone needs a concrete file location. Restricting this translation to a header declared under
/// `[interop.c]` prevents an arbitrary relative binding path from becoming an ambient include-directory search.
fn resolve_package_owned_c_binding_header(
    manifest: Option<&ProjectManifest>,
    binding: &CBindingDescriptor,
) -> CBindingDescriptor {
    let Some(manifest) = manifest else {
        return binding.clone();
    };
    if Path::new(&binding.header).is_absolute() {
        return binding.clone();
    }
    let declared = manifest.interop_c().is_some_and(|interop| {
        interop.targets.iter().any(|target| {
            target.headers.iter().any(|header| header == &binding.header)
                || target
                    .shims
                    .iter()
                    .flat_map(|shim| &shim.headers)
                    .any(|header| header == &binding.header)
        })
    });
    if !declared {
        return binding.clone();
    }
    let mut resolved = binding.clone();
    resolved.header = manifest
        .project_root()
        .join(&binding.header)
        .to_string_lossy()
        .into_owned();
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;
    use incan_core::lang::c_abi::LinkCapabilityId;
    use incan_frontend::ast::Span;
    use incan_provider::test_support::parsed_module_for_test;

    #[test]
    fn package_declared_c_header_is_resolved_only_for_verification() -> Result<(), Box<dyn std::error::Error>> {
        let manifest = ProjectManifest::from_str(
            "[project]\nname = \"c_header_fixture\"\n\n[interop.c]\nschema = 1\n\n[[interop.c.targets]]\ntarget = \"aarch64-apple-darwin\"\nheaders = [\"interop/include/bridge.h\"]\n",
            Path::new("/workspace/c_header_fixture/loaf.toml"),
        )?;
        let binding = CBindingDescriptor {
            span: Span::new(0, 0),
            class_name: "Bridge".to_string(),
            header: "interop/include/bridge.h".to_string(),
            system_library: "bridge".to_string(),
            link_capability: LinkCapabilityId::SystemLibrary,
            resources: Vec::new(),
            symbols: Vec::new(),
            enums: Vec::new(),
            structs: Vec::new(),
        };

        let resolved = resolve_package_owned_c_binding_header(Some(&manifest), &binding);

        assert_eq!(binding.header, "interop/include/bridge.h");
        assert_eq!(resolved.header, "/workspace/c_header_fixture/interop/include/bridge.h");
        Ok(())
    }

    #[test]
    fn typecheck_modules_with_import_graph_accepts_valid_program() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
def main() -> None:
    pass
"#,
        )?;

        typecheck_modules_with_import_graph(
            &[module],
            None,
            &Arc::new(ProviderPlan::default()),
            #[cfg(feature = "rust_inspect")]
            None,
        )?;

        Ok(())
    }

    #[test]
    fn typecheck_modules_with_import_graph_reports_errors() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
def main() -> None:
    missing_symbol()
"#,
        )?;

        let result = typecheck_modules_with_import_graph(
            &[module],
            None,
            &Arc::new(ProviderPlan::default()),
            #[cfg(feature = "rust_inspect")]
            None,
        );
        assert!(result.is_err(), "expected unresolved symbol to fail typecheck");

        Ok(())
    }
}
