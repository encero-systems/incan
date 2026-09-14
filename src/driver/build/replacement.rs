//! The replacement build: session inputs for the direct-execution backend and the report it produces.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::backend::replacement::source_profile::{module_is_held_to_source_profile, source_profile_refusal};
use crate::backend::replacement::{
    ReplacementExecutionGraph, execute_prevalidated_free_function, prepare_free_function_execution_in_graph,
};
use crate::backend::selection::{
    SemanticModuleProvenance, digest_output, finalize_receipt_with_semantic_module, select_backend,
};
use crate::driver::build::backend_selection::{
    REPLACEMENT_EXECUTION_REPORT_SCHEMA_VERSION, backend_shadow_comparison, default_backend_receipt_path,
    refuse_replacement_profile, replacement_profile_cli_error, replacement_refusal_source,
    resolve_available_replacement_execution, write_backend_receipt,
};
use crate::driver::build::{BuildCommandOptions, elapsed_ms};
use crate::driver::build_report::BuildReportOptions;
use crate::driver::cargo_policy::CargoPolicy;
use crate::driver::error::{CliError, CliResult};
use crate::driver::modules::collect_modules_detailed_with_session;
use crate::driver::project::resolve_project_root;
use crate::driver::session::CompilationSession;
use crate::frontend::{diagnostics, typechecker};
use crate::provider::ProviderPlan;

/// Checked entry facts the direct replacement executor may consume from one compilation session.
///
/// This private bundle is the replacement CLI's authority boundary: no later stage may lex, parse, re-resolve, or
/// typecheck the source again. `TypeCheckInfo` is retained only as Body IR's transitional lowering bridge; semantic
/// provenance comes from the sibling portable snapshot produced by that same analysis pass.
struct ReplacementSessionInputs {
    /// Checked relationships shared with CodeGraph, projected from this analysis's semantic snapshots.
    dependencies: incan_semantics_core::dependencies::CheckedDependencyGraph,
    /// The cached provider projection used by the same analysis.
    provider_plan: Arc<ProviderPlan>,
    program: crate::frontend::ast::Program,
    module_path: Vec<String>,
    type_info: typechecker::TypeCheckInfo,
    semantic_module: SemanticModuleProvenance,
    /// Every non-entry module the one analysis already checked, in collection order.
    ///
    /// The session collects and analyzes the whole root source graph and previously kept only the entrypoint, which
    /// is all the same-module #988 profile can execute. #1260 executes a call that leaves the entry module, so the
    /// modules that call may reach have to survive the same analysis rather than be re-collected or re-checked
    /// later: re-analysis would produce a second checker authority, and identities minted by two analyses cannot be
    /// compared.
    ///
    /// The entrypoint is deliberately not repeated here. It stays in the fields above so existing readers keep
    /// working unchanged, and the execution graph is assembled with the entry module as its primary.
    reachable_modules: Vec<ReplacementModuleInputs>,
}

/// One checked non-entry module retained from the replacement session's single analysis.
pub(crate) struct ReplacementModuleInputs {
    pub(crate) program: crate::frontend::ast::Program,
    pub(crate) module_path: Vec<String>,
    pub(crate) type_info: typechecker::TypeCheckInfo,
    /// The file this module was collected from, so a refusal raised in it can name its own source.
    pub(crate) file_path: PathBuf,
}

/// Collect and analyze the replacement entrypoint once through the project-selected compilation session.
///
/// The entry AST, Body-IR lowering bridge, and semantic provenance are extracted as one product. This is deliberately
/// the only constructor for [`ReplacementSessionInputs`], making an independent replacement CLI typecheck impossible
/// without crossing this explicit boundary.
fn replacement_session_inputs(
    entrypoint: &Path,
    compilation_session: &CompilationSession,
) -> CliResult<ReplacementSessionInputs> {
    let modules = collect_modules_detailed_with_session(entrypoint.to_path_buf(), compilation_session)
        .map_err(|failure| CliError::failure(failure.render_human()))?;
    let analysis = compilation_session
        .analyze_modules(
            &modules,
            #[cfg(feature = "rust_inspect")]
            None,
        )
        .map_err(|failure| CliError::failure(failure.render_human()))?;
    let entry_module = modules
        .iter()
        .find(|module| module.file_path == entrypoint)
        .ok_or_else(|| {
            CliError::failure(format!(
                "replacement session did not collect entrypoint {}",
                entrypoint.display()
            ))
        })?;
    let entry_analysis = analysis
        .module_analysis_for_path(&entry_module.file_path)
        .ok_or_else(|| {
            CliError::failure(format!(
                "replacement session did not retain checked analysis for entrypoint {}",
                entrypoint.display()
            ))
        })?;
    let semantic_snapshot = entry_analysis.semantic_snapshot();
    let semantic_snapshot_rendering = semantic_snapshot.render_snapshot();
    let source_identity = digest_output(&[entry_module.source.as_str()]);

    let reachable_modules = modules
        .iter()
        .filter(|module| module.file_path != entry_module.file_path)
        .filter_map(|module| {
            analysis
                .module_analysis_for_path(&module.file_path)
                .map(|checked| ReplacementModuleInputs {
                    program: module.ast.clone(),
                    module_path: module.path_segments.clone(),
                    type_info: checked.type_info().clone(),
                    file_path: module.file_path.clone(),
                })
        })
        .collect();

    Ok(ReplacementSessionInputs {
        dependencies: incan_semantics_core::dependencies::CheckedDependencyGraph::from_fact_stores(
            analysis.semantic_snapshots().values().map(|snapshot| &snapshot.facts),
        ),
        provider_plan: compilation_session.provider_plan_for_modules(&modules)?,
        program: entry_module.ast.clone(),
        module_path: entry_module.path_segments.clone(),
        type_info: entry_analysis.type_info().clone(),
        reachable_modules,
        semantic_module: SemanticModuleProvenance::new(
            semantic_snapshot.hir.id.to_string(),
            semantic_snapshot.hir.path.clone(),
            source_identity,
            digest_output(&[semantic_snapshot_rendering.as_str()]),
        ),
    })
}

/// Execute the first #988 replacement-backend profile directly from typed Body IR.
///
/// This intentionally has no `ProjectGenerator`, Oven, or generated-Rust path. It accepts only one source module
/// containing free functions and executes its zero-argument `main` body through the replacement executor. A
/// requested replacement build therefore either records a replacement receipt over a real Body-IR result or fails
/// visibly at the original Incan span; it can never reach `IrCodegen` as an implicit compatibility fallback.
///
/// The pipeline constructs one [`CompilationSession`], collects and analyzes the selected module graph through it,
/// applies the module-profile gate to its projected entry AST, and lowers only from the resulting checked facts. The
/// session owns parsing, vocab desugaring, feature projection, and typechecking; this CLI path must never derive a
/// second authority for the same source.
pub(crate) fn build_replacement_file_report(
    file_path: &str,
    options: BuildCommandOptions,
    report_options: &BuildReportOptions,
) -> CliResult<serde_json::Value> {
    if report_options.enabled() && report_options.output_path.is_none() {
        return Err(CliError::failure(
            "replacement execution keeps stdout and stderr for the program; use --report-output <file> with --report json",
        ));
    }
    reject_normal_cargo_controls(&options.cargo_policy, options.generated_cargo_target_dir.as_ref())?;
    let start = Instant::now();
    let entrypoint = if Path::new(file_path).is_absolute() {
        PathBuf::from(file_path)
    } else {
        env::current_dir()
            .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))?
            .join(file_path)
    };
    let compilation_session = CompilationSession::discover_for_collection_with_selections(
        &entrypoint,
        &options.package_features,
        options.sdk_profile.as_deref(),
    )?;
    let session_inputs = replacement_session_inputs(&entrypoint, &compilation_session)?;
    let selection = select_backend(
        options.backend.requested,
        options.backend.explicit,
        options.backend.shadow,
        session_inputs.semantic_module.source_identity(),
        options.backend.fallback_policy,
    );
    // Every module the call can reach has to satisfy the profile, not only the entrypoint. Allowing a local import
    // means an unsupported declaration is now reachable from a file the entry never names, and refusing it here keeps
    // the boundary a property of the executed graph rather than of whichever file happened to be the entrypoint.
    // A refusal carries a span, and a span only means something beside the file it was measured in. Reporting every
    // refusal against the entrypoint was harmless while only the entrypoint could raise one; now that a reachable
    // module can, the pair has to travel together or the diagnostic points at the wrong file.
    let profile_error = source_profile_refusal(&session_inputs.program)
        .map(|error| (error, entrypoint.clone()))
        .or_else(|| {
            session_inputs
                .reachable_modules
                .iter()
                .filter(|module| module_is_held_to_source_profile(&module.module_path))
                .find_map(|module| {
                    source_profile_refusal(&module.program).map(|error| (error, module.file_path.clone()))
                })
        });
    if let Some((error, source_path)) = profile_error {
        return refuse_replacement_profile(&selection, error, &source_path);
    }
    let required_packages = replacement_package_requirements(&session_inputs);
    let published = crate::frontend::executable_resolution::resolve_executable_requirements(
        &session_inputs.provider_plan,
        &required_packages,
    )
    .map_err(|error| CliError::failure(error.to_string()))?;
    // Imported type context comes from selected executable fragments. Local source is lowered once from the
    // session's existing checked facts; neither dependency source nor a second typecheck participates.
    let body_ir = crate::frontend::body_ir::build_body_ir_module_v0_with_executable_context(
        &session_inputs.program,
        &session_inputs.module_path,
        &session_inputs.type_info,
        &published.modules,
    );
    let mut reachable_body_ir: Vec<_> = session_inputs
        .reachable_modules
        .iter()
        .map(|module| {
            crate::frontend::body_ir::build_body_ir_module_v0_with_executable_context(
                &module.program,
                &module.module_path,
                &module.type_info,
                &published.modules,
            )
        })
        .collect();
    let package_versions = published.package_versions;
    let decoded_package_declarations = published.decoded_declarations;
    let package_payload_bytes_read = published.payload_bytes_read;
    let package_content_bytes_verified = published.content_bytes_verified;
    for module in &published.modules {
        for body in &module.bodies {
            if let Err(error) = crate::backend::replacement::validate_direct_body_profile(body) {
                return Err(package_execution_requirement_error(
                    body,
                    &package_versions,
                    error.to_string(),
                ));
            }
        }
    }
    reachable_body_ir.extend(published.modules);
    let execution_graph = match ReplacementExecutionGraph::new(&body_ir, reachable_body_ir.iter()) {
        Ok(graph) => graph,
        Err(error) => return refuse_replacement_profile(&selection, error, &entrypoint),
    };
    let execution_plan = match prepare_free_function_execution_in_graph(execution_graph, "main", &[], None) {
        Ok(plan) => plan,
        Err(error) => {
            if let Some(owner) = error.measured_module()
                && let Some(module) = reachable_body_ir.iter().find(|module| module.module_id.path() == owner)
                && let Some(body) = module.bodies.first().filter(|body| {
                    matches!(
                        body.canonical.as_ref().map(|identity| &identity.origin),
                        Some(incan_semantics_core::SymbolOrigin::Package { .. })
                    )
                })
            {
                return Err(package_execution_requirement_error(
                    body,
                    &package_versions,
                    error.to_string(),
                ));
            }
            let source =
                replacement_refusal_source(&error, &entrypoint, &session_inputs.reachable_modules).to_path_buf();
            return refuse_replacement_profile(&selection, error, &source);
        }
    };
    let executed = resolve_available_replacement_execution(&selection)?;
    let execution = execute_prevalidated_free_function(execution_plan).map_err(|error| {
        let source = replacement_refusal_source(&error, &entrypoint, &session_inputs.reachable_modules);
        replacement_profile_cli_error(error, source)
    })?;
    let result_type = execution.value.scalar_type_name().ok_or_else(|| {
        CliError::failure("replacement execution produced a non-scalar value after scalar-result validation")
    })?;
    let shadow_comparison = backend_shadow_comparison(&selection);
    let backend_receipt = finalize_receipt_with_semantic_module(
        &selection,
        executed,
        execution.output_identity.clone(),
        shadow_comparison,
        diagnostics::DIAGNOSTIC_SCHEMA_VERSION,
        Some(session_inputs.semantic_module.clone()),
    )
    .map_err(|error| CliError::failure(error.to_string()))?;
    let project_root = resolve_project_root(&entrypoint);
    write_backend_receipt(&backend_receipt, &default_backend_receipt_path(&project_root))?;
    Ok(serde_json::json!({
        "schema_version": REPLACEMENT_EXECUTION_REPORT_SCHEMA_VERSION,
        "compiler_version": crate::version::INCAN_VERSION,
        "status": "success",
        "mode": "executable",
        "entrypoint": entrypoint,
        "backend": backend_receipt,
        "semantic_module": session_inputs.semantic_module,
        "replacement_execution": {
            "package_declarations_decoded": decoded_package_declarations,
            "package_payload_bytes_read": package_payload_bytes_read,
            "package_content_bytes_verified": package_content_bytes_verified,
            "result": execution.value.observable_text(),
            "result_type": result_type,
            "output_identity": execution.output_identity,
            "emitted_output": execution.emitted_output(),
            "stdout_bytes": execution.output.stdout(),
            "stderr_bytes": execution.output.stderr(),
            "body_snapshot": execution.body_snapshot,
            "ownership_reads": execution.ownership_evidence(),
            "runtime_requirements": execution.runtime_requirement_evidence(),
            "task_lifecycle": execution.task_lifecycle_evidence(),
        },
        "timings_ms": { "total": elapsed_ms(start) },
    }))
}

/// Select package dependencies through the same checked declaration relationships used by CodeGraph.
///
/// Alias and facade spellings have already been resolved by the one session analysis. Layout fields, variants and
/// defaults participate through the shared semantic graph; Body IR is lowered once after selecting imported context.
fn replacement_package_requirements(
    inputs: &ReplacementSessionInputs,
) -> BTreeSet<incan_semantics_core::CanonicalSymbolId> {
    use incan_semantics_core::{SemanticSourceTargetKind, SymbolOrigin};
    let roots = inputs
        .type_info
        .declarations
        .declaration_identities
        .values()
        .filter(|identity| identity.kind == SemanticSourceTargetKind::Function && identity.declaration_name == "main")
        .cloned();
    inputs
        .dependencies
        .reachable_from(roots)
        .into_iter()
        .filter(|identity| matches!(identity.origin, SymbolOrigin::Package { .. }))
        .collect()
}

/// Render a package preflight refusal before program effects or receipt publication.
fn package_execution_requirement_error(
    body: &incan_semantics_core::body_ir::Body,
    versions: &BTreeMap<String, String>,
    reason: String,
) -> CliError {
    let library = match body.canonical.as_ref().map(|identity| &identity.origin) {
        Some(incan_semantics_core::SymbolOrigin::Package { library, .. }) => library.as_str(),
        _ => "<unresolved>",
    };
    let version = versions.get(library).map(String::as_str).unwrap_or("<unresolved>");
    CliError::failure(format!(
        "package `{library}` version {version} cannot satisfy the executable representation requirement for `{}`: {reason}",
        body.name
    ))
}

/// Reject controls that only have meaning for the retired Cargo execution backend.
///
/// Lock strictness is deliberately not rejected: it validates compiler-owned `oven.lock` consistency before Oven
/// selection without launching Cargo. Offline is already satisfied because this normal path starts neither Cargo nor
/// a networked dependency resolver.
pub(crate) fn reject_normal_cargo_controls(cargo_policy: &CargoPolicy, target_dir: Option<&PathBuf>) -> CliResult<()> {
    if !cargo_policy.extra_args.is_empty() || target_dir.is_some() {
        return Err(CliError::failure(
            "Oven Alpha normal build and run do not accept Cargo passthrough or target-directory controls; use the supported Oven-native provider/dependency envelope instead",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::build::test_support::replacement_build_options;
    use std::fs;
    use std::path::PathBuf;

    use crate::driver::build::backend_selection::REPLACEMENT_EXECUTION_REPORT_SCHEMA_VERSION;
    use crate::driver::build_report::BuildReportOptions;
    use crate::driver::cargo_policy::CargoPolicy;
    use crate::driver::error::CliError;
    #[cfg(test)]
    use crate::frontend::body_ir::build_body_ir_module_v0;
    use crate::frontend::{body_ir, lexer, parser, typechecker};
    use crate::provider::FeatureSelection;

    #[test]
    fn oven_normal_commands_keep_lock_strictness_but_reject_cargo_backend_controls() {
        assert!(reject_normal_cargo_controls(&CargoPolicy::explicit(true, false, false, Vec::new()), None).is_ok());
        assert!(reject_normal_cargo_controls(&CargoPolicy::explicit(false, true, false, Vec::new()), None).is_ok());
        assert!(reject_normal_cargo_controls(&CargoPolicy::explicit(false, false, true, Vec::new()), None).is_ok());
        assert!(
            reject_normal_cargo_controls(
                &CargoPolicy::explicit(false, false, false, vec!["--timings".to_string()]),
                None,
            )
            .is_err()
        );
        assert!(
            reject_normal_cargo_controls(&CargoPolicy::default(), Some(&PathBuf::from("target/generated-cargo")),)
                .is_err()
        );
    }

    #[test]
    fn the_replacement_build_uses_the_session_selected_package_feature_projection()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let entrypoint = project.path().join("src/main.incn");
        fs::create_dir_all(entrypoint.parent().ok_or("fixture entrypoint has no parent")?)?;
        fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname = \"replacement_features\"\n\n[project.features]\nbeta = []\n",
        )?;
        fs::write(
            &entrypoint,
            "when feature(\"beta\"):\n    def main() -> int:\n        return 7\n",
        )?;

        let mut options = replacement_build_options();
        options.package_features = FeatureSelection::new(["beta"]);
        let report =
            build_replacement_file_report(&entrypoint.to_string_lossy(), options, &BuildReportOptions::default())?;
        assert_eq!(report["replacement_execution"]["result"], "7");
        assert_eq!(report["semantic_module"]["module_path"], "main");
        Ok(())
    }

    #[test]
    fn the_replacement_report_retains_exact_numeric_result_type() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let entrypoint = project.path().join("main.incn");
        fs::write(project.path().join("loaf.toml"), "[project]\nname = \"typed_report\"\n")?;
        fs::write(&entrypoint, "def main() -> f32:\n    return 1.23456789\n")?;

        let report = build_replacement_file_report(
            &entrypoint.to_string_lossy(),
            replacement_build_options(),
            &BuildReportOptions::default(),
        )?;
        assert_eq!(report["schema_version"], REPLACEMENT_EXECUTION_REPORT_SCHEMA_VERSION);
        assert_eq!(report["replacement_execution"]["result"], 1.234_567_9_f32.to_string());
        assert_eq!(report["replacement_execution"]["result_type"], "f32");
        assert!(
            report["replacement_execution"]["output_identity"]
                .as_str()
                .is_some_and(|identity| identity.starts_with("sha256:"))
        );
        Ok(())
    }

    #[test]
    fn the_replacement_build_pipeline_analyzes_its_session_exactly_once() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let entrypoint = project.path().join("src/main.incn");
        fs::create_dir_all(entrypoint.parent().ok_or("fixture entrypoint has no parent")?)?;
        fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname = \"replacement_one_analysis\"\n",
        )?;
        fs::write(
            &entrypoint,
            "def helper() -> int:\n    return 1\n\ndef main() -> int:\n    return helper()\n",
        )?;

        let analysis_scope = crate::driver::session::scoped_compilation_session_analysis_invocations();
        let report = build_replacement_file_report(
            &entrypoint.to_string_lossy(),
            replacement_build_options(),
            &BuildReportOptions::default(),
        )?;

        assert_eq!(report["replacement_execution"]["result"], "1");
        assert_eq!(
            analysis_scope.invocation_count(),
            1,
            "the actual replacement build pipeline must analyze its compilation session exactly once"
        );
        Ok(())
    }

    #[test]
    fn the_replacement_build_never_executes_a_main_behind_an_inactive_feature() -> Result<(), Box<dyn std::error::Error>>
    {
        // The end-to-end consequence, through the real CLI entry point: with no active feature there is no `main`
        // to lower, so the build must refuse rather than execute a body this compilation does not contain.
        let project = tempfile::tempdir()?;
        let entrypoint = project.path().join("main.incn");
        fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname = \"replacement_inactive_feature\"\n\n[project.features]\nbeta = []\n",
        )?;
        fs::write(
            &entrypoint,
            "when feature(\"beta\"):\n    def main() -> int:\n        return 7\n",
        )?;

        let error = build_replacement_file_report(
            &entrypoint.to_string_lossy(),
            replacement_build_options(),
            &BuildReportOptions::default(),
        )
        .err()
        .ok_or("a `main` behind an inactive feature must not produce a successful replacement build")?;
        assert!(
            !error.to_string().contains("7"),
            "no gated body may have been executed: {error}"
        );

        // Same source, same contract, through the direct API the parity corpus and unit tests use: both entry
        // points must agree that nothing was lowered, which is the point of stating the contract at all.
        let tokens =
            lexer::lex(&fs::read_to_string(&entrypoint)?).map_err(|errors| CliError::failure(format!("{errors:?}")))?;
        let parsed = parser::parse(&tokens).map_err(|errors| CliError::failure(format!("{errors:?}")))?;
        let program = body_ir::apply_body_ir_input_contract(parsed, &entrypoint)
            .map_err(|errors| CliError::failure(format!("{errors:?}")))?;
        let module_path = vec!["main".to_string()];
        let mut checker = typechecker::TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        checker
            .check_program(&program)
            .map_err(|errors| CliError::failure(format!("{errors:?}")))?;
        let body_ir = build_body_ir_module_v0(&program, &module_path, checker.type_info());
        assert!(
            body_ir.bodies.is_empty(),
            "the direct API must lower the same nothing the CLI path did: {:?}",
            body_ir.bodies.iter().map(|body| &body.name).collect::<Vec<_>>()
        );
        Ok(())
    }
}
