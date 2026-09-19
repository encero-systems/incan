//! `incan oven harvest`: one compatibility-publisher observation of a checked manifest's registry closure, written
//! as incan.pub fact proposals.
//!
//! This is the explicit RFC 119 harvest: a maintainer runs it once per exact selection on the publisher's machine.
//! It renders the manifest's registry `[rust-dependencies]` into a minimal Cargo package, runs the same publisher
//! `legacy-cargo prepare` runs (`prepare_direct_rustc_plan`, under a scratch store that is discarded afterwards),
//! binds the captured units to their staged registry sources, and hands the capture to `harvest_registry_units`.
//! Nothing here writes into a registry; `incan-pub add-fact` admits each proposal in its own reviewable step.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use oven_cargo_compat::{
    HarvestEvidenceInputs, HarvestPublisherIdentity, HarvestReport, OvenLegacyCargoDirectDependencyClosure,
    OvenLegacyCargoPrepareRequest, OvenLegacyCargoPublicationKind, ambient_harvest_hazards,
    bind_legacy_cargo_selected_registry_sources, harvest_notes_for_checkout, harvest_registry_units,
    legacy_cargo_resolved_registry_sources, prepare_direct_rustc_plan, proposal_directory_names,
    stage_locked_loaf_fixture, write_harvest_report,
};
use oven_model::loaf_registry::enclosing_checkout_head_commit;
use oven_model::manifest::{DependencySource, DependencySpec, ProjectManifest};
use oven_rustc::loaf::{OvenLoafEnvelope, direct_rustc_compiler_closure_identity};
use oven_rustc::rustc::rustc_identity;
use oven_store::store::OvenStore;
use oven_store::{OvenGeneratedProjectRequest, receipt_generated_project};
use serde::Serialize;

use super::loaf_bake::loaf_envelope_default_limits;
use super::{CliError, CliResult, ExitCode, OvenHarvestCommandOptions, OvenOutputFormat, oven_error, print_json};

/// The receipt source-evidence key naming the rendered `src/main.rs`, as the Loaf publisher spells it.
const HARVEST_SOURCE_EVIDENCE_KEY: &str = "generated-root";

/// Stable terminal and JSON evidence emitted by one harvest.
#[derive(Debug, Serialize)]
pub(crate) struct OvenHarvestSummary {
    /// The manifest whose registry dependencies selected the closure.
    pub(crate) manifest: PathBuf,
    /// Exact target the facts bind.
    pub(crate) target: String,
    /// `rustc -vV` identity the facts bind.
    pub(crate) toolchain: String,
    /// Profile the facts bind.
    pub(crate) profile: String,
    /// `sha256:` identity of the receipt the publisher ran under (`evidence.receipt` of every proposal).
    pub(crate) receipt: String,
    /// `sha256:` identity of the bounded compiler/sysroot closure (`evidence.rustc_identity`).
    pub(crate) rustc_identity: String,
    /// Hazard tokens recorded on every proposal; a non-empty list makes every proposal inadmissible.
    pub(crate) hazards: Vec<String>,
    /// The publish note every proposal carries, when the manifest sits inside a git checkout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) notes: Option<String>,
    /// Directory the report was written under.
    pub(crate) output: PathBuf,
    /// `<name>-<version>-<profile>` directory of every proposal, in report order.
    pub(crate) proposals: Vec<String>,
    /// Every refusal, in report order.
    pub(crate) refusals: Vec<oven_cargo_compat::HarvestRefusal>,
    /// Harvest always runs Cargo once; it is the observation.
    pub(crate) cargo_process_started: bool,
}

/// Observe a checked manifest's registry closure once and write its harvest proposals.
pub fn oven_harvest(options: OvenHarvestCommandOptions) -> CliResult<ExitCode> {
    // ---- Inputs ----
    if !options.cargo.is_file() || !options.rustc.is_file() {
        return Err(CliError::failure(
            "harvest requires regular --cargo and --rustc executables".to_string(),
        ));
    }
    let manifest_path = resolve_manifest_path(&options.project)?;
    let manifest_text = fs::read_to_string(&manifest_path).map_err(|error| {
        CliError::failure(format!(
            "could not read checked manifest {}: {error}",
            manifest_path.display()
        ))
    })?;
    let manifest = ProjectManifest::from_str(&manifest_text, &manifest_path)
        .map_err(|error| CliError::failure(format!("checked manifest is invalid: {error}")))?;
    let toolchain = rustc_identity(&options.rustc).map_err(oven_error)?;
    let compiler_closure =
        direct_rustc_compiler_closure_identity(&options.rustc, &options.target).map_err(oven_error)?;
    let ambient_hazards = ambient_harvest_hazards();
    let notes = manifest_path
        .parent()
        .and_then(enclosing_checkout_head_commit)
        .map(|head| harvest_notes_for_checkout(&head));
    fs::create_dir_all(&options.output).map_err(|error| {
        CliError::failure(format!(
            "could not create harvest output {}: {error}",
            options.output.display()
        ))
    })?;

    // ---- One Cargo package for the manifest's registry closure, in scratch ----
    let scratch = tempfile::Builder::new()
        .prefix(".incan-oven-harvest-")
        .tempdir_in(&options.output)
        .map_err(|error| CliError::failure(format!("could not allocate harvest scratch: {error}")))?;
    let package_root = scratch.path().join("package");
    render_harvest_package(&manifest, &package_root).map_err(CliError::failure)?;
    if let Some(lock) = options.cargo_lock.as_deref() {
        stage_locked_loaf_fixture(&options.cargo, &package_root, lock).map_err(oven_error)?;
    }
    let (project_name, project_version) = manifest_identity(&manifest);
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            &package_root,
            project_name,
            project_version,
            &options.target,
            &toolchain,
            &options.profile,
            Vec::new(),
        )
        .with_generated_source(HARVEST_SOURCE_EVIDENCE_KEY, package_root.join("src/main.rs")),
    )
    .map_err(oven_error)?;

    // ---- The same publisher observation `legacy-cargo prepare` performs, under a scratch store ----
    let store = OvenStore::new(
        scratch.path().join("store"),
        loaf_envelope_default_limits(OvenLoafEnvelope::Release),
    );
    let prepared = prepare_direct_rustc_plan(&OvenLegacyCargoPrepareRequest {
        compiler: incan_oven_facet::compiler_identity(),
        provider_hooks: incan_oven_facet::provider_hooks(),
        store: &store,
        receipt: receipt.clone(),
        generated_project: package_root.clone(),
        cargo: options.cargo.clone(),
        rustc: options.rustc.clone(),
        sdk_inventory: None,
        compiler_loaf_root: None,
        domain: format!("harvest-{}", options.profile),
        publication_kind: OvenLegacyCargoPublicationKind::Executable,
        source_evidence_key: HARVEST_SOURCE_EVIDENCE_KEY.to_string(),
        compile_environment: BTreeMap::new(),
        inspection_packages: None,
        direct_dependency_closure: OvenLegacyCargoDirectDependencyClosure::CheckedDeclared,
        provider_compilations: &[],
        compact_debug_info: true,
        source_compiler_vocab_support: false,
        base_loaf: None,
    })
    .map_err(oven_error)?;
    let mut capture = prepared.selected_units.clone().ok_or_else(|| {
        CliError::failure(
            "the publisher produced no selected-unit capture; harvest needs the stable rustc trace".to_string(),
        )
    })?;
    let authority_dir = scratch.path().join("registry-sources");
    fs::create_dir_all(&authority_dir)
        .map_err(|error| CliError::failure(format!("could not create harvest source staging: {error}")))?;
    let sources =
        legacy_cargo_resolved_registry_sources(&options.cargo, &package_root.join("Cargo.toml"), &[], &authority_dir)
            .map_err(oven_error)?;
    bind_legacy_cargo_selected_registry_sources(&mut capture, &sources).map_err(oven_error)?;

    // ---- Harvest and write, copying OUT_DIR members from the published plan before the store is discarded ----
    let evidence = HarvestEvidenceInputs::from_prepare_result(
        &prepared,
        HarvestPublisherIdentity::new(&receipt.identity, &compiler_closure, ambient_hazards, notes.clone()),
    );
    let report = harvest_registry_units(&capture, &evidence, &options.profile).map_err(oven_error)?;
    let hazards = report.hazards.clone();
    let (entry, _lease) = store.select(&prepared.plan_identity).map_err(oven_error)?;
    write_harvest_report(&report, &options.output, &entry.materialized_root()).map_err(oven_error)?;
    let summary = OvenHarvestSummary {
        manifest: manifest_path,
        target: options.target,
        toolchain,
        profile: options.profile,
        receipt: receipt.identity,
        rustc_identity: compiler_closure,
        hazards,
        notes,
        output: options.output,
        proposals: proposal_directory_names(&report).map_err(oven_error)?,
        refusals: report.refusals.clone(),
        cargo_process_started: true,
    };
    print_harvest_summary(&summary, &report, options.format)?;
    Ok(ExitCode::SUCCESS)
}

/// Render the summary as text or JSON.
fn print_harvest_summary(
    summary: &OvenHarvestSummary,
    report: &HarvestReport,
    format: OvenOutputFormat,
) -> CliResult<()> {
    match format {
        OvenOutputFormat::Text => {
            println!(
                "Harvested {} proposal(s) and {} refusal(s) for {} {} under {} into {}.",
                report.proposals.len(),
                report.refusals.len(),
                summary.target,
                summary.profile,
                summary.toolchain,
                summary.output.display()
            );
            for (proposal, name) in report.proposals.iter().zip(&summary.proposals) {
                let fact = proposal.rust.facts.first();
                println!(
                    "  proposed {name}: features [{}], cfg [{}], {} generated input(s)",
                    fact.map(|fact| fact.features.join(", ")).unwrap_or_default(),
                    fact.map(|fact| fact.cfg.join(", ")).unwrap_or_default(),
                    fact.map(|fact| fact.out.len()).unwrap_or_default(),
                );
            }
            for refusal in &report.refusals {
                println!(
                    "  refused {} {}: {} ({})",
                    refusal.package,
                    refusal.version,
                    refusal_reason_label(refusal.reason),
                    refusal.detail
                );
            }
            if !summary.hazards.is_empty() {
                println!(
                    "Hazards: {} (admission refuses every proposal of this harvest)",
                    summary.hazards.join(", ")
                );
            }
            println!("Receipt: {}", summary.receipt);
            println!("Compiler closure: {}", summary.rustc_identity);
        }
        OvenOutputFormat::Json => print_json(summary)?,
    }
    Ok(())
}

/// The kebab-case wire spelling of a refusal reason, for terminal output.
fn refusal_reason_label(reason: oven_cargo_compat::HarvestRefusalReason) -> String {
    serde_json::to_value(reason)
        .ok()
        .and_then(|value| value.as_str().map(ToString::to_string))
        .unwrap_or_else(|| format!("{reason:?}"))
}

/// `--project` names the manifest file itself or a directory holding `loaf.toml`.
fn resolve_manifest_path(project: &Path) -> CliResult<PathBuf> {
    let candidate = if project.is_dir() {
        project.join("loaf.toml")
    } else {
        project.to_path_buf()
    };
    if !candidate.is_file() {
        return Err(CliError::failure(format!(
            "harvest needs a checked manifest file; {} is not one",
            candidate.display()
        )));
    }
    Ok(candidate)
}

/// The project identity the receipt records, from `[project]` or a fixed harvest name.
fn manifest_identity(manifest: &ProjectManifest) -> (String, String) {
    let project = manifest.project.as_ref();
    (
        project
            .and_then(|project| project.name.clone())
            .unwrap_or_else(|| "oven_harvest".to_string()),
        project
            .and_then(|project| project.version.clone())
            .unwrap_or_else(|| "0.0.0".to_string()),
    )
}

/// Write `Cargo.toml` and an empty `src/main.rs` whose `[dependencies]` are the manifest's registry dependencies.
///
/// Only registry dependencies can be harvested (a path or git dependency has no registry record to propose), so any
/// other source refuses the whole manifest rather than silently narrowing the closure. Each dependency carries its
/// version requirement, features, default-feature choice and `package` rename exactly as declared, which is what
/// makes Cargo's feature unification for this package the selection the facts bind.
pub(crate) fn render_harvest_package(manifest: &ProjectManifest, package_root: &Path) -> Result<(), String> {
    let mut dependencies = manifest
        .rust_dependencies()
        .iter()
        .map(|(alias, spec)| (alias.clone(), spec))
        .collect::<Vec<_>>();
    dependencies.sort_by(|left, right| left.0.cmp(&right.0));
    if dependencies.is_empty() {
        return Err("checked manifest declares no [rust-dependencies] to harvest".to_string());
    }
    let mut cargo_toml = String::from(
        "[package]\nname = \"oven_harvest\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n\n[dependencies]\n",
    );
    for (alias, spec) in dependencies {
        cargo_toml.push_str(&render_dependency(&alias, spec)?);
        cargo_toml.push('\n');
    }
    let source_root = package_root.join("src");
    fs::create_dir_all(&source_root)
        .map_err(|error| format!("could not create harvest package {}: {error}", source_root.display()))?;
    fs::write(package_root.join("Cargo.toml"), cargo_toml)
        .map_err(|error| format!("could not write harvest Cargo.toml: {error}"))?;
    fs::write(source_root.join("main.rs"), "fn main() {}\n")
        .map_err(|error| format!("could not write harvest main.rs: {error}"))?;
    Ok(())
}

/// One `[dependencies]` line in inline-table form.
fn render_dependency(alias: &str, spec: &DependencySpec) -> Result<String, String> {
    if !matches!(spec.source, DependencySource::Registry) {
        return Err(format!(
            "dependency `{alias}` is not a registry dependency; harvest proposes registry records only"
        ));
    }
    let version = spec
        .version
        .as_deref()
        .filter(|version| !version.trim().is_empty())
        .ok_or_else(|| format!("registry dependency `{alias}` declares no version requirement"))?;
    let mut fields = vec![format!("version = {}", toml_string(version))];
    if let Some(package) = spec.package.as_deref() {
        fields.push(format!("package = {}", toml_string(package)));
    }
    if !spec.features.is_empty() {
        let mut features = spec.features.clone();
        features.sort();
        features.dedup();
        fields.push(format!(
            "features = [{}]",
            features
                .iter()
                .map(|feature| toml_string(feature))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !spec.default_features {
        fields.push("default-features = false".to_string());
    }
    Ok(format!("{} = {{ {} }}", toml_key(alias), fields.join(", ")))
}

/// A TOML basic string.
fn toml_string(value: &str) -> String {
    format!("{:?}", value)
}

/// A TOML key: bare when it can be, quoted otherwise.
fn toml_key(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_' || character == '-')
    {
        value.to_string()
    } else {
        toml_string(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn a_checked_manifest_renders_one_cargo_package_over_its_registry_dependencies() -> TestResult {
        let manifest = ProjectManifest::from_str(
            "[project]\nname = \"oven_release_stdlib\"\nversion = \"0.1.0\"\n\n[rust-dependencies]\nserde = { version = \"1\", features = [\"derive\"] }\nmd5 = { package = \"md-5\", version = \"0.10\" }\nbitflags = \"=1.3.2\"\nrand = { version = \"0.8\", default-features = false }\n",
            Path::new("loaf.toml"),
        )?;
        let root = tempfile::tempdir()?;
        render_harvest_package(&manifest, root.path())?;
        let cargo_toml = fs::read_to_string(root.path().join("Cargo.toml"))?;
        assert_eq!(
            cargo_toml,
            "[package]\nname = \"oven_harvest\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n\n[dependencies]\nbitflags = { version = \"=1.3.2\" }\nmd5 = { version = \"0.10\", package = \"md-5\" }\nrand = { version = \"0.8\", default-features = false }\nserde = { version = \"1\", features = [\"derive\"] }\n"
        );
        assert_eq!(fs::read_to_string(root.path().join("src/main.rs"))?, "fn main() {}\n");
        assert_eq!(
            manifest_identity(&manifest),
            ("oven_release_stdlib".to_string(), "0.1.0".to_string())
        );
        let parsed: toml::Value = toml::from_str(&cargo_toml)?;
        assert_eq!(
            parsed["dependencies"]["md5"]["package"].as_str(),
            Some("md-5"),
            "the rendered manifest is valid TOML Cargo can read"
        );
        Ok(())
    }

    #[test]
    fn manifests_without_a_registry_closure_refuse() -> TestResult {
        let empty = ProjectManifest::from_str(
            "[project]\nname = \"empty\"\nversion = \"0.1.0\"\n",
            Path::new("loaf.toml"),
        )?;
        let root = tempfile::tempdir()?;
        assert!(render_harvest_package(&empty, root.path()).is_err());
        let local = ProjectManifest::from_str(
            "[project]\nname = \"local\"\nversion = \"0.1.0\"\n\n[rust-dependencies]\nhelper = { path = \"../helper\" }\n",
            Path::new("loaf.toml"),
        )?;
        let refused = render_harvest_package(&local, root.path());
        assert!(
            refused
                .as_ref()
                .err()
                .is_some_and(|error| error.contains("not a registry dependency")),
            "{refused:?}"
        );
        Ok(())
    }

    #[test]
    fn the_manifest_path_is_the_file_or_the_directory_holding_it() -> TestResult {
        let root = tempfile::tempdir()?;
        assert!(resolve_manifest_path(root.path()).is_err());
        fs::write(root.path().join("loaf.toml"), "[project]\nname = \"x\"\n")?;
        assert_eq!(resolve_manifest_path(root.path())?, root.path().join("loaf.toml"));
        assert_eq!(
            resolve_manifest_path(&root.path().join("loaf.toml"))?,
            root.path().join("loaf.toml")
        );
        Ok(())
    }
}
