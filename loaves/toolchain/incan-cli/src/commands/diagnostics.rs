//! Stable diagnostic CLI surfaces.
//!
//! `incan check` and `incan explain` expose the same compiler diagnostics as the legacy debug flags, but with stable
//! JSON output and catalog-backed explanations for tooling consumers.

use std::env;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use serde::Serialize;

use crate::{CliError, CliResult, ExitCode};
use incan_driver::backend::c_abi::CAbiVerificationPlan;
use incan_frontend::diagnostics::{self, DIAGNOSTIC_SCHEMA_VERSION, StableDiagnostic};
use incan_provider::FeatureSelection;

use incan_driver::diagnostics::{CliDiagnostic, CliDiagnosticFailure};
use incan_driver::modules::collect_modules_detailed_with_session;
use incan_driver::session::CompilationSession;
use incan_driver::typecheck::typecheck_modules_with_import_graph_detailed_for_c_abi_target;

/// Output format for stable diagnostics commands.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticOutputFormat {
    Text,
    Json,
}

/// Schema version for the `incan check` report envelope.
///
/// Deliberately separate from [`DIAGNOSTIC_SCHEMA_VERSION`], which versions the `StableDiagnostic` payload itself
/// and is also stamped onto `incan explain` output and the backend execution receipt. Version 2 changed only this
/// envelope — `diagnostics` now carries non-fatal warnings alongside errors, so `ok` means "no error-severity
/// diagnostics" rather than "no diagnostics at all". Bumping the shared constant instead would have falsely
/// signaled a payload or receipt contract change to consumers of those other surfaces.
pub(crate) const CHECK_REPORT_SCHEMA_VERSION: u32 = 2;

/// One complete typecheck result, retained independently from CLI rendering so workspace orchestration can emit one
/// coherent machine-readable report for several members.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DiagnosticReport {
    schema_version: u32,
    /// Whether typechecking found no error-severity diagnostics.
    ///
    /// Warnings do not clear this flag: a successful check with warnings reports `ok: true` and a non-empty
    /// `diagnostics` array, mirroring the process exit code.
    ok: bool,
    diagnostics: Vec<StableDiagnostic>,
    #[serde(skip_serializing)]
    human_message: Option<String>,
}

impl DiagnosticReport {
    /// Whether collection and typechecking succeeded without error-severity diagnostics.
    pub(crate) fn ok(&self) -> bool {
        self.ok
    }

    /// Human-readable diagnostics retained from the compiler pipeline when the report is unsuccessful.
    pub(crate) fn human_message(&self) -> Option<&str> {
        self.human_message.as_deref()
    }
}

#[derive(Debug, Serialize)]
struct ExplainReport {
    schema_version: u32,
    found: bool,
    entry: diagnostics::DiagnosticCatalogEntry,
}

/// Run the canonical check pipeline for a file or project entrypoint.
pub fn check_path(path: &Path, format: DiagnosticOutputFormat) -> CliResult<ExitCode> {
    check_path_with_features(path, format, &FeatureSelection::default())
}

/// Run the canonical check pipeline for an explicit Incan package-feature projection.
pub fn check_path_with_features(
    path: &Path,
    format: DiagnosticOutputFormat,
    feature_selection: &FeatureSelection,
) -> CliResult<ExitCode> {
    check_path_with_selections(path, format, feature_selection, None)
}

/// Run the canonical check pipeline for explicit package-feature and transient SDK-profile selections.
pub fn check_path_with_selections(
    path: &Path,
    format: DiagnosticOutputFormat,
    feature_selection: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> CliResult<ExitCode> {
    let report = check_path_report_with_selections(path, feature_selection, sdk_profile_override)?;
    render_check_report(&report, format)
}

/// Run the canonical check pipeline with an explicit declared target for checked C ABI verification only.
pub(crate) fn check_path_with_interop_target_selection(
    path: &Path,
    format: DiagnosticOutputFormat,
    feature_selection: &FeatureSelection,
    sdk_profile_override: Option<&str>,
    interop_target: Option<&str>,
) -> CliResult<ExitCode> {
    let report =
        check_path_report_with_interop_target_selection(path, feature_selection, sdk_profile_override, interop_target)?;
    render_check_report(&report, format)
}

/// Run the canonical check pipeline for one package-feature and SDK-profile projection without rendering it.
///
/// This is the single-project compiler boundary used by both `incan check` and RFC 077 workspace command fan-out. It
/// deliberately retains stable diagnostics rather than printing them as soon as a member fails.
pub(crate) fn check_path_report_with_selections(
    path: &Path,
    feature_selection: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> CliResult<DiagnosticReport> {
    check_path_report_with_interop_target_selection(path, feature_selection, sdk_profile_override, None)
}

/// Run one canonical package-feature, SDK-profile, and declared C ABI target projection without rendering it.
pub(crate) fn check_path_report_with_interop_target_selection(
    path: &Path,
    feature_selection: &FeatureSelection,
    sdk_profile_override: Option<&str>,
    interop_target: Option<&str>,
) -> CliResult<DiagnosticReport> {
    let normalized_path = normalize_input_path(path)?;
    let compilation_session =
        match CompilationSession::discover_for_check(&normalized_path, feature_selection, sdk_profile_override) {
            Ok(session) => session,
            Err(error) => {
                let failure = CliDiagnosticFailure::single(
                    normalized_path.to_string_lossy(),
                    "",
                    diagnostics::CompileError::new(error.to_string(), incan_frontend::ast::Span::default()),
                    diagnostics::DiagnosticPhase::Import,
                );
                return Ok(diagnostic_report_from_failure(failure));
            }
        };
    let manifest = compilation_session.manifest.clone();
    let c_abi_plan = interop_target
        .map(|target| select_interop_c_abi_verification_plan(manifest.as_ref(), target))
        .transpose()?;
    let modules = match collect_modules_detailed_with_session(normalized_path.clone(), &compilation_session) {
        Ok(modules) => modules,
        Err(failure) => return Ok(diagnostic_report_from_failure(failure)),
    };
    let provider_plan = compilation_session.provider_plan_for_modules(&modules)?;
    #[cfg(feature = "rust_inspect")]
    let rust_inspect_manifest_dir = compilation_session.prepare_check_rust_inspection(&normalized_path, &modules)?;

    let typecheck = typecheck_modules_with_import_graph_detailed_for_c_abi_target(
        &modules,
        manifest.as_ref(),
        &provider_plan,
        c_abi_plan.as_ref(),
        #[cfg(feature = "rust_inspect")]
        rust_inspect_manifest_dir
            .as_ref()
            .map(|workspace| workspace.manifest_dir()),
    );

    match typecheck {
        Ok(warnings) => Ok(DiagnosticReport {
            schema_version: CHECK_REPORT_SCHEMA_VERSION,
            ok: true,
            diagnostics: stable_diagnostics_from(&warnings),
            human_message: None,
        }),
        Err(failure) => Ok(diagnostic_report_from_failure(failure)),
    }
}

/// Select exactly one Oven interop C ABI profile for an invocation that names `--interop-target`.
fn select_interop_c_abi_verification_plan(
    manifest: Option<&oven_model::manifest::ProjectManifest>,
    requested_target: &str,
) -> CliResult<CAbiVerificationPlan> {
    let Some(manifest) = manifest else {
        return Err(CliError::failure(format!(
            "`--interop-target {requested_target}` requires a project manifest with an [interop.c] declaration"
        )));
    };
    let Some(interop) = manifest.interop_c() else {
        return Err(CliError::failure(format!(
            "`--interop-target {requested_target}` requires an [interop.c] declaration in loaf.toml"
        )));
    };
    let Some(interop_target) = interop.targets.iter().find(|target| target.target == requested_target) else {
        return Err(CliError::failure(format!(
            "`--interop-target {requested_target}` is not declared by [[interop.c.targets]] in loaf.toml"
        )));
    };
    CAbiVerificationPlan::from_interop_target(interop_target).map_err(CliError::failure)
}

/// Print a catalog-backed diagnostic explanation.
pub fn explain_diagnostic(code: &str, format: DiagnosticOutputFormat) -> CliResult<ExitCode> {
    if let Some(entry) = diagnostics::explain(code) {
        match format {
            DiagnosticOutputFormat::Text => {
                println!("{}", format_explain_text(entry));
                Ok(ExitCode::SUCCESS)
            }
            DiagnosticOutputFormat::Json => {
                let report = ExplainReport {
                    schema_version: DIAGNOSTIC_SCHEMA_VERSION,
                    found: true,
                    entry: *entry,
                };
                print_json(&report)?;
                Ok(ExitCode::SUCCESS)
            }
        }
    } else {
        let unknown = diagnostics::explain("INCAN-U0001")
            .ok_or_else(|| CliError::failure("internal error: missing INCAN-U0001 diagnostic catalog entry"))?;
        match format {
            DiagnosticOutputFormat::Text => Err(CliError::failure(format!(
                "Unknown diagnostic code `{code}`.\n\n{}",
                format_explain_text(unknown)
            ))),
            DiagnosticOutputFormat::Json => {
                let report = ExplainReport {
                    schema_version: DIAGNOSTIC_SCHEMA_VERSION,
                    found: false,
                    entry: *unknown,
                };
                print_json(&report)?;
                Err(CliError::new("", ExitCode::FAILURE))
            }
        }
    }
}

/// Render one already-evaluated check result in either human text or the stable JSON report shape.
fn render_check_report(report: &DiagnosticReport, format: DiagnosticOutputFormat) -> CliResult<ExitCode> {
    match format {
        DiagnosticOutputFormat::Text => {
            if report.ok {
                println!("✓ Type check passed!");
                Ok(ExitCode::SUCCESS)
            } else {
                Err(CliError::failure(render_check_report_human(report)))
            }
        }
        DiagnosticOutputFormat::Json => {
            print_json(report)?;
            if report.ok {
                Ok(ExitCode::SUCCESS)
            } else {
                Err(CliError::new("", ExitCode::FAILURE))
            }
        }
    }
}

/// Convert source-aware compiler failures into the stable report consumed by both single-project and workspace paths.
/// Project collected CLI diagnostics into their stable machine-readable form, preserving input order.
///
/// Shared by the success and failure paths so warnings and errors are projected identically — the only thing that
/// distinguishes them in the payload is the `severity` each one already carries.
pub(crate) fn stable_diagnostics_from(collected: &[CliDiagnostic]) -> Vec<StableDiagnostic> {
    collected
        .iter()
        .map(|diagnostic| {
            diagnostics::stable_diagnostic(
                &diagnostic.file_path,
                &diagnostic.source,
                &diagnostic.error,
                diagnostic.phase,
            )
        })
        .collect()
}

/// Build the machine-readable check report for a failed collection or typecheck pass.
///
/// Reports the failure's errors together with any warnings gathered before it, so a file that has both does not
/// hide the warning behind the error. The human message stays errors-only: warnings were already printed to
/// stderr as they were produced.
fn diagnostic_report_from_failure(failure: CliDiagnosticFailure) -> DiagnosticReport {
    let human_message = failure.render_human();
    // Errors first, then any warnings gathered before the failure, so a file with both reports both.
    let mut diagnostics = stable_diagnostics_from(&failure.diagnostics);
    diagnostics.extend(stable_diagnostics_from(&failure.warnings));
    DiagnosticReport {
        schema_version: CHECK_REPORT_SCHEMA_VERSION,
        ok: false,
        diagnostics,
        human_message: Some(human_message),
    }
}

/// Render diagnostics for the human CLI without rebuilding source context in the workspace orchestration layer.
fn render_check_report_human(report: &DiagnosticReport) -> String {
    report
        .human_message
        .clone()
        .unwrap_or_else(|| "type check failed without diagnostics".to_string())
}

/// Pretty-print a serializable diagnostics payload to stdout.
fn print_json<T: Serialize>(value: &T) -> CliResult<()> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|error| CliError::failure(format!("failed to serialize diagnostic JSON: {error}")))?;
    println!("{json}");
    Ok(())
}

/// Format one catalog entry for the default `incan explain` human output.
fn format_explain_text(entry: &diagnostics::DiagnosticCatalogEntry) -> String {
    let mut text = String::new();
    text.push_str(entry.code);
    text.push_str(": ");
    text.push_str(entry.title);
    text.push('\n');
    text.push_str(entry.summary);
    text.push_str("\n\n");
    text.push_str(entry.explanation);
    if !entry.common_causes.is_empty() {
        text.push_str("\n\nCommon causes:");
        for cause in entry.common_causes {
            text.push_str("\n- ");
            text.push_str(cause);
        }
    }
    if !entry.fixes.is_empty() {
        text.push_str("\n\nFixes:");
        for fix in entry.fixes {
            text.push_str("\n- ");
            text.push_str(fix);
        }
    }
    if let Some(url) = entry.docs_url {
        text.push_str("\n\nDocs: ");
        text.push_str(url);
    }
    text
}

/// Resolve the user-supplied check target relative to the current directory for project discovery.
fn normalize_input_path(path: &Path) -> CliResult<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(env::current_dir()
            .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))?
            .join(path))
    }
}

#[cfg(test)]
mod dev7_checked_provider_metadata_tests {
    use super::*;
    use incan_frontend::library_manifest_index::{LibraryArtifactKind, LibraryManifestIndexEntry};
    use std::fs;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Write only authored inputs for a local package; checking must not require generated files.
    fn write_package(root: &Path, manifest: &str, entry: &str, source: &str) -> TestResult {
        fs::create_dir_all(root.join("src"))?;
        fs::write(root.join("loaf.toml"), manifest)?;
        fs::write(root.join("src").join(entry), source)?;
        Ok(())
    }

    /// Read the command-local source authority without treating it as an executable artifact.
    fn source_identity(entry: &Path, dependency: &str) -> Result<String, Box<dyn std::error::Error>> {
        let session = CompilationSession::discover_for_check(entry, &FeatureSelection::default(), None)?;
        let Some(LibraryManifestIndexEntry::Loaded { manifest, metadata }) =
            session.library_manifest_index.get(dependency)
        else {
            return Err(format!("checked provider `{dependency}` is missing").into());
        };
        assert_eq!(metadata.kind, LibraryArtifactKind::CheckedSource);
        let record = session
            .provider_plan
            .records()
            .find(|record| record.identity.name == manifest.name)
            .ok_or("checked source provider record is missing")?;
        assert!(record.artifact.is_none());
        assert!(record.implementation_facets.is_empty());
        assert!(manifest.contract_metadata.executable_representation.is_none());
        Ok(record.identity.stable_key())
    }

    /// Reject any generated project or provider output in the fresh source-only tree.
    fn assert_no_outputs(root: &Path) -> TestResult {
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                assert_ne!(entry.file_name(), "target", "check generated {}", path.display());
                assert_no_outputs(&path)?;
            } else {
                assert_ne!(path.extension().and_then(|value| value.to_str()), Some("incnlib"));
            }
        }
        Ok(())
    }

    /// A fresh local provider is checked successfully without receiving native execution authority.
    #[test]
    fn dev7_checked_provider_metadata_fresh_dependency() -> TestResult {
        let scratch = tempfile::tempdir()?;
        let root = scratch.path();
        write_package(
            root,
            "[project]\nname='consumer'\nversion='0.1.0'\n[dependencies]\nwidgets={path='deps/widgets'}\n",
            "main.incn",
            "from pub::widgets import answer\n\ndef main() -> None:\n    print(answer())\n",
        )?;
        write_package(
            &root.join("deps/widgets"),
            "[project]\nname='widgets'\nversion='0.1.0'\n",
            "lib.incn",
            "pub def answer() -> int:\n    return 42\n",
        )?;
        let entry = root.join("src/main.incn");
        let report = check_path_report_with_selections(&entry, &FeatureSelection::default(), None)?;
        assert!(report.ok(), "{report:?}");
        source_identity(&entry, "widgets")?;
        let error = CompilationSession::discover_for_oven(&entry, &FeatureSelection::default(), None)
            .err()
            .ok_or("execution accepted unbaked source")?;
        assert!(error.to_string().contains("requires a baked package Loaf"), "{error}");
        assert_no_outputs(root)
    }

    /// Check retains the published public boundary for unsupported defaults, trait bodies and inactive exports.
    #[test]
    fn dev7_checked_provider_metadata_existing_refusals() -> TestResult {
        let cases = [
            ("refused_adopter_relying_on_a_dependency_trait_default", "INCAN-T0001"),
            ("refused_dependency_default_calling_a_builtin", "INCAN-T0001"),
            ("refused_dependency_default_constructing_a_private_model", "INCAN-T0001"),
            ("refused_feature_gated_export_of_dependency", "INCAN-I0103"),
            ("refused_imported_partial_without_carried_default", "INCAN-T0001"),
        ];
        for (name, expected) in cases {
            let scratch = tempfile::tempdir()?;
            let root = scratch.path().join(name);
            incan_test_support::cli_project::copy_fixture_directory(
                &incan_test_support::fixtures_dir()
                    .join("behavior/cli_dependencies")
                    .join(name),
                &root,
            )?;
            assert_no_outputs(&root)?;
            let report =
                check_path_report_with_selections(&root.join("src/main.incn"), &FeatureSelection::default(), None)?;
            assert!(!report.ok(), "{name}: {report:?}");
            let json = serde_json::to_value(&report)?;
            let diagnostics = json["diagnostics"].as_array().ok_or("missing check diagnostics")?;
            assert!(
                diagnostics.iter().any(|diagnostic| diagnostic["code"] == expected),
                "{name}: {report:?}"
            );
            assert!(
                diagnostics.iter().all(|diagnostic| diagnostic["code"] != "INCAN-I0001"),
                "{name}: {report:?}"
            );
            assert_no_outputs(&root)?;
        }
        Ok(())
    }

    /// A transitive source change and dependency-edge edit invalidate metadata; restoring inputs restores identity.
    #[test]
    fn dev7_checked_provider_metadata_source_graph_restoration() -> TestResult {
        let scratch = tempfile::tempdir()?;
        let root = scratch.path();
        let middle = root.join("deps/middle");
        let leaf = root.join("deps/leaf");
        let middle_manifest = "[project]\nname='middle'\nversion='0.1.0'\n[dependencies]\nleaf={path='../leaf'}\n";
        let leaf_source = "pub def answer() -> int:\n    return 42\n";
        write_package(
            root,
            "[project]\nname='consumer'\nversion='0.1.0'\n[dependencies]\nmiddle={path='deps/middle'}\n",
            "main.incn",
            "from pub::middle import value\n\ndef main() -> None:\n    print(value())\n",
        )?;
        write_package(
            &middle,
            middle_manifest,
            "lib.incn",
            "from pub::leaf import answer\n\npub def value() -> int:\n    return answer()\n",
        )?;
        write_package(
            &leaf,
            "[project]\nname='leaf'\nversion='0.1.0'\n",
            "lib.incn",
            leaf_source,
        )?;
        let entry = root.join("src/main.incn");
        let original = source_identity(&entry, "middle")?;
        assert!(check_path_report_with_selections(&entry, &FeatureSelection::default(), None)?.ok());
        fs::write(leaf.join("src/lib.incn"), leaf_source.replace("42", "43"))?;
        assert_ne!(source_identity(&entry, "middle")?, original);
        fs::write(leaf.join("src/lib.incn"), leaf_source)?;
        assert_eq!(source_identity(&entry, "middle")?, original);
        write_package(
            &root.join("deps/other"),
            "[project]\nname='other'\nversion='0.1.0'\n",
            "lib.incn",
            leaf_source,
        )?;
        fs::write(middle.join("loaf.toml"), middle_manifest.replace("../leaf", "../other"))?;
        assert_ne!(source_identity(&entry, "middle")?, original);
        fs::write(middle.join("loaf.toml"), middle_manifest)?;
        assert_eq!(source_identity(&entry, "middle")?, original);
        fs::write(
            leaf.join("src/lib.incn"),
            "pub def answer() -> str:\n    return \"changed\"\n",
        )?;
        assert!(!check_path_report_with_selections(&entry, &FeatureSelection::default(), None)?.ok());
        fs::write(leaf.join("src/lib.incn"), leaf_source)?;
        assert!(check_path_report_with_selections(&entry, &FeatureSelection::default(), None)?.ok());
        assert_eq!(source_identity(&entry, "middle")?, original);
        assert_no_outputs(root)
    }

    /// Selected features propagate through a middle package to its source dependency.
    #[test]
    fn dev7_checked_provider_metadata_transitive_features() -> TestResult {
        let scratch = tempfile::tempdir()?;
        let root = scratch.path();
        write_package(
            root,
            "[project]\nname='consumer'\n[dependencies]\nmiddle={path='deps/middle',features=['alpha']}\n",
            "main.incn",
            "from pub::middle import value\n\ndef main() -> None:\n    print(value())\n",
        )?;
        write_package(
            &root.join("deps/middle"),
            "[project]\nname='middle'\n[project.features]\nalpha=['leaf/alpha']\n[dependencies]\nleaf={path='../leaf'}\n",
            "lib.incn",
            "from pub::leaf import answer\n\npub def value() -> int:\n    return answer()\n",
        )?;
        write_package(
            &root.join("deps/leaf"),
            "[project]\nname='leaf'\n[project.features]\nalpha=[]\n",
            "lib.incn",
            "when feature(\"alpha\"):\n    pub def answer() -> int:\n        return 42\n",
        )?;
        let entry = root.join("src/main.incn");
        let report = check_path_report_with_selections(&entry, &FeatureSelection::default(), None)?;
        assert!(report.ok(), "{report:?}");
        source_identity(&entry, "middle")?;
        assert_no_outputs(root)
    }

    /// Two consumers of one source dependency observe its unified additive feature instance.
    #[test]
    fn dev7_checked_provider_metadata_diamond_features() -> TestResult {
        let scratch = tempfile::tempdir()?;
        let root = scratch.path();
        write_package(
            root,
            "[project]\nname='consumer'\n[dependencies]\nmiddle={path='deps/middle',features=['alpha']}\nright={path='deps/right',features=['beta']}\n",
            "main.incn",
            "from pub::middle import value\n\ndef main() -> None:\n    print(value())\n",
        )?;
        write_package(
            &root.join("deps/middle"),
            "[project]\nname='middle'\n[project.features]\nalpha=['leaf/alpha']\n[dependencies]\nleaf={path='../leaf'}\n",
            "lib.incn",
            "from pub::leaf import beta_value\n\npub def value() -> int:\n    return beta_value()\n",
        )?;
        write_package(
            &root.join("deps/right"),
            "[project]\nname='right'\n[project.features]\nbeta=['leaf/beta']\n[dependencies]\nleaf={path='../leaf'}\n",
            "lib.incn",
            "pub def value() -> int:\n    return 7\n",
        )?;
        write_package(
            &root.join("deps/leaf"),
            "[project]\nname='leaf'\n[project.features]\nalpha=[]\nbeta=[]\n",
            "lib.incn",
            "when feature(\"alpha\"):\n    pub def alpha_value() -> int:\n        return 41\n\nwhen feature(\"beta\"):\n    pub def beta_value() -> int:\n        return 42\n",
        )?;
        let entry = root.join("src/main.incn");
        let report = check_path_report_with_selections(&entry, &FeatureSelection::default(), None)?;
        assert!(report.ok(), "{report:?}");
        source_identity(&entry, "middle")?;
        assert_no_outputs(root)
    }

    /// Public model and newtype reexports retain their exact declaring source identity without exposing private edges.
    #[test]
    fn dev7_checked_provider_metadata_transitive_nominals() -> TestResult {
        let scratch = tempfile::tempdir()?;
        let root = scratch.path();
        let consumer = "from pub::middle import Widget, Tag\n\ndef main() -> None:\n    item = Widget(value=42)\n    tag = Tag(7)\n    print(item.value)\n";
        write_package(
            root,
            "[project]\nname='consumer'\n[dependencies]\nmiddle={path='deps/middle'}\n",
            "main.incn",
            consumer,
        )?;
        write_package(
            &root.join("deps/middle"),
            "[project]\nname='middle'\n[dependencies]\nleaf={path='../leaf'}\n",
            "lib.incn",
            "pub from pub::leaf import Widget, Tag\n",
        )?;
        write_package(
            &root.join("deps/leaf"),
            "[project]\nname='leaf'\n",
            "lib.incn",
            "pub model Widget:\n    value: int\n\npub newtype Tag = int\n",
        )?;
        let entry = root.join("src/main.incn");
        let report = check_path_report_with_selections(&entry, &FeatureSelection::default(), None)?;
        assert!(report.ok(), "{report:?}");
        let identity = source_identity(&entry, "middle")?;
        let session = CompilationSession::discover_for_check(&entry, &FeatureSelection::default(), None)?;
        assert!(session.library_manifest_index.get("leaf").is_none());
        fs::write(&entry, consumer.replace("pub::middle", "pub::leaf"))?;
        assert!(!check_path_report_with_selections(&entry, &FeatureSelection::default(), None)?.ok());
        fs::write(&entry, consumer.replace("pub::middle", "pub::middle::leaf"))?;
        assert!(!check_path_report_with_selections(&entry, &FeatureSelection::default(), None)?.ok());
        fs::write(&entry, consumer)?;
        assert!(check_path_report_with_selections(&entry, &FeatureSelection::default(), None)?.ok());
        assert_eq!(source_identity(&entry, "middle")?, identity);
        assert_no_outputs(root)
    }

    /// A producer's direct model has semantic authority while its provider retains no execution artifact.
    #[test]
    fn dev7_checked_provider_metadata_direct_nominal() -> TestResult {
        let scratch = tempfile::tempdir()?;
        let root = scratch.path();
        write_package(
            root,
            "[project]\nname='consumer'\n[dependencies]\nwidgets={path='deps/widgets'}\n",
            "main.incn",
            "from pub::widgets import Widget\n\ndef main() -> None:\n    item = Widget(value=42)\n    print(item.value)\n",
        )?;
        write_package(
            &root.join("deps/widgets"),
            "[project]\nname='widgets'\n",
            "lib.incn",
            "pub model Widget:\n    value: int\n",
        )?;
        let entry = root.join("src/main.incn");
        let report = check_path_report_with_selections(&entry, &FeatureSelection::default(), None)?;
        assert!(report.ok(), "{report:?}");
        source_identity(&entry, "widgets")?;
        assert_no_outputs(root)
    }

    /// Fresh vocabulary companions refuse at metadata discovery rather than compiling during check.
    #[test]
    fn dev7_checked_provider_metadata_vocab_cache_miss() -> TestResult {
        let scratch = tempfile::tempdir()?;
        let root = scratch.path();
        write_package(
            root,
            "[project]\nname='consumer'\n[dependencies]\nwidgets={path='deps/widgets'}\n",
            "main.incn",
            "from pub::widgets import answer\n\ndef main() -> None:\n    print(answer())\n",
        )?;
        let widgets = root.join("deps/widgets");
        write_package(
            &widgets,
            "[project]\nname='widgets'\n[vocab]\ncrate='companion'\n",
            "lib.incn",
            "pub def answer() -> int:\n    return 42\n",
        )?;
        fs::create_dir_all(widgets.join("companion/src"))?;
        fs::write(
            widgets.join("companion/Cargo.toml"),
            "[package]\nname='dev7_checked_provider_metadata_companion_miss'\nversion='0.1.0'\nedition='2021'\n",
        )?;
        fs::write(widgets.join("companion/src/lib.rs"), "pub fn library_vocab() {}\n")?;
        let report =
            check_path_report_with_selections(&root.join("src/main.incn"), &FeatureSelection::default(), None)?;
        assert!(!report.ok(), "{report:?}");
        assert!(
            report
                .human_message()
                .is_some_and(|message| message.contains("preparation authority is missing")),
            "{report:?}"
        );
        assert_no_outputs(root)
    }

    /// A recursive source-provider graph refuses deterministically before producing artifacts.
    #[test]
    fn dev7_checked_provider_metadata_dependency_cycle() -> TestResult {
        let scratch = tempfile::tempdir()?;
        let root = scratch.path();
        write_package(
            root,
            "[project]\nname='consumer'\n[dependencies]\na={path='deps/a'}\n",
            "main.incn",
            "def main() -> None:\n    pass\n",
        )?;
        write_package(
            &root.join("deps/a"),
            "[project]\nname='a'\n[dependencies]\nb={path='../b'}\n",
            "lib.incn",
            "pub def answer() -> int:\n    return 1\n",
        )?;
        write_package(
            &root.join("deps/b"),
            "[project]\nname='b'\n[dependencies]\na={path='../a'}\n",
            "lib.incn",
            "pub def answer() -> int:\n    return 2\n",
        )?;
        let report =
            check_path_report_with_selections(&root.join("src/main.incn"), &FeatureSelection::default(), None)?;
        assert!(!report.ok(), "{report:?}");
        assert!(
            report.human_message().is_some_and(|message| message.contains("cycle")),
            "{report:?}"
        );
        assert_no_outputs(root)
    }

    /// Missing producer Rust ABI facts refuse explicitly rather than triggering native inspection during check.
    #[cfg(feature = "rust_inspect")]
    #[test]
    fn dev7_checked_provider_metadata_rust_abi_refusal() -> TestResult {
        let scratch = tempfile::tempdir()?;
        let root = scratch.path();
        write_package(
            root,
            "[project]\nname='consumer'\n[dependencies]\nwidgets={path='deps/widgets'}\n",
            "main.incn",
            "from pub::widgets import answer\n\ndef main() -> None:\n    print(answer())\n",
        )?;
        write_package(
            &root.join("deps/widgets"),
            "[project]\nname='widgets'\n",
            "lib.incn",
            "from rust::std::time import Duration\n\npub def answer() -> int:\n    return 42\n",
        )?;
        let report =
            check_path_report_with_selections(&root.join("src/main.incn"), &FeatureSelection::default(), None)?;
        assert!(!report.ok(), "{report:?}");
        assert!(
            report
                .human_message()
                .is_some_and(|message| message.contains("requires Rust ABI metadata")),
            "{report:?}"
        );
        assert_no_outputs(root)
    }
}
