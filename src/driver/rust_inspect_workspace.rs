//! The rust-inspect workspace: a Cargo workspace regenerated under the project so the inspector can see the exact
//! Rust items a program imports, fingerprinted so an unchanged query surface never rebuilds it.
//!
//! Everything here is about producing and prewarming that workspace. Reading Rust metadata out of it is the
//! `rust_inspect` crate's job.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

#[cfg(feature = "rust_inspect")]
use crate::backend::ProjectGenerator;
use crate::dependency_resolver::ResolvedDependencies;
use crate::driver::error::{CliError, CliResult};
use crate::frontend::ast::{ImportKind, Program};
use crate::frontend::parsed_module::ParsedModule;
use crate::library_manifest::digest_provider_artifact;
use crate::manifest::{DependencySource, DependencySpec, ProjectManifest};
use crate::provider::inventory::{normalize_sdk_artifact_projections, normalize_sdk_dependency_rebindings};
use crate::provider::requirements::ProjectRequirements;
use crate::provider::{SdkArtifactProjection, SdkDependencyRebinding};
#[cfg(feature = "rust_inspect")]
use crate::rust_inspect::Inspector;
#[cfg(feature = "rust_inspect")]
use crate::rust_inspect::InspectorConfig;
const RUST_INSPECT_WORKSPACE_FINGERPRINT_FILE: &str = ".incan_rust_inspect_fingerprint";

#[cfg(feature = "rust_inspect")]
const RUST_INSPECT_WORKSPACE_FINGERPRINT_PREFIX: &str = "v1:";

#[cfg(feature = "rust_inspect")]
const RUST_INSPECT_OUT_DIRS_FINGERPRINT_FILE: &str = ".incan_rust_inspect_out_dirs_fingerprint";

/// Counts how many times each rust-inspect stub workspace is fully regenerated instead of skipped via fingerprint.
///
/// Full lib tests run in parallel and other tests can legitimately create unrelated rust-inspect workspaces, so this
/// instrumentation is keyed by generated workspace path instead of using one process-wide counter.
#[cfg(all(test, feature = "rust_inspect"))]
static TEST_RUST_INSPECT_WORKSPACE_GENERATIONS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::BTreeMap<PathBuf, u64>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::BTreeMap::new()));

/// Records a full rust-inspect workspace regeneration for the generated workspace path under test.
#[cfg(all(test, feature = "rust_inspect"))]
fn record_test_rust_inspect_workspace_generation(workspace_dir: &Path) {
    let mut counts = TEST_RUST_INSPECT_WORKSPACE_GENERATIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *counts.entry(workspace_dir.to_path_buf()).or_default() += 1;
}

/// Returns the number of full rust-inspect workspace regenerations recorded for a generated workspace path.
#[cfg(all(test, feature = "rust_inspect"))]
fn test_rust_inspect_workspace_generations(workspace_dir: &Path) -> u64 {
    let counts = TEST_RUST_INSPECT_WORKSPACE_GENERATIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    counts.get(workspace_dir).copied().unwrap_or(0)
}

/// Trim, sort and dedupe the stdlib feature list so the workspace fingerprint does not change with spelling order.
#[cfg(feature = "rust_inspect")]
fn normalized_stdlib_features_for_rust_inspect_fingerprint(features: &[String]) -> Vec<String> {
    let mut normalized: Vec<String> = features
        .iter()
        .map(|feature| feature.trim().to_string())
        .filter(|feature| !feature.is_empty())
        .collect();
    normalized.sort();
    normalized.dedup();
    normalized
}

/// Fold one dependency spec into the workspace fingerprint — crate name, version, features, the default-features
/// and optional flags, package rename, and source — with NUL separators and tagged absences so two specs cannot
/// collide by concatenation.
#[cfg(feature = "rust_inspect")]
fn hash_dependency_spec_for_rust_inspect(hasher: &mut Sha256, spec: &DependencySpec) {
    use crate::manifest::GitReference;

    hasher.update(spec.crate_name.as_bytes());
    hasher.update(b"\0");
    match &spec.version {
        Some(v) => {
            hasher.update(b"ver\0");
            hasher.update(v.as_bytes());
            hasher.update(b"\0");
        }
        None => hasher.update(b"nover\0"),
    }
    let mut feats = spec.features.clone();
    feats.sort();
    for f in feats {
        hasher.update(f.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update([if spec.default_features { 1 } else { 0 }]);
    hasher.update([if spec.optional { 1 } else { 0 }]);
    match &spec.package {
        Some(p) => {
            hasher.update(b"pkg\0");
            hasher.update(p.as_bytes());
            hasher.update(b"\0");
        }
        None => hasher.update(b"nopkg\0"),
    }
    match &spec.source {
        DependencySource::Registry => hasher.update(b"src_reg\0"),
        DependencySource::Git { url, reference } => {
            hasher.update(b"src_git\0");
            hasher.update(url.as_bytes());
            hasher.update(b"\0");
            match reference {
                GitReference::Branch(s) => {
                    hasher.update(b"git_br\0");
                    hasher.update(s.as_bytes());
                    hasher.update(b"\0");
                }
                GitReference::Tag(s) => {
                    hasher.update(b"git_tag\0");
                    hasher.update(s.as_bytes());
                    hasher.update(b"\0");
                }
                GitReference::Rev(s) => {
                    hasher.update(b"git_rev\0");
                    hasher.update(s.as_bytes());
                    hasher.update(b"\0");
                }
            }
        }
        DependencySource::Path { path } => {
            hasher.update(b"src_path\0");
            hasher.update(path.as_os_str().as_encoded_bytes());
            hasher.update(b"\0");
        }
    }
    hasher.update(b"|dep|\0");
}

/// Stable fingerprint for inputs that define one generated rust-inspect Cargo workspace.
#[cfg(feature = "rust_inspect")]
#[allow(clippy::too_many_arguments)]
fn rust_inspect_workspace_fingerprint(
    project_name: &str,
    cargo_package_name: &str,
    rust_edition: Option<&str>,
    resolved: &ResolvedDependencies,
    stdlib_features: &[String],
    sdk_dependency_rebindings: &[SdkDependencyRebinding],
    sdk_path_dependencies: &[DependencySpec],
    sdk_artifact_projections: &[SdkArtifactProjection],
    cargo_lock_payload: Option<&str>,
    cargo_lock_projection_root: Option<&str>,
    clear_cargo_lock: bool,
    cargo_target_dir: &Path,
    rust_derive_probe_paths: &[String],
) -> String {
    let mut hasher = Sha256::new();
    // Version 5 also fingerprints semantic derive probes emitted into the generated inspection root.
    // This prevents `cargo metadata --locked` from rejecting an otherwise exact dependency lock solely because the
    // inspectable local root was previously emitted with the compiler's version.
    hasher.update(b"incan_rust_inspect_workspace/5\0");
    hasher.update(project_name.as_bytes());
    hasher.update(b"\0");
    hasher.update(cargo_package_name.as_bytes());
    hasher.update(b"\0");
    if clear_cargo_lock {
        hasher.update(b"clear_cargo_lock\0");
    }
    hasher.update(b"cargo_target_dir\0");
    hasher.update(cargo_target_dir.as_os_str().as_encoded_bytes());
    hasher.update(b"\0");
    if let Some(root) = cargo_lock_projection_root {
        hasher.update(b"lock_projection\0");
        hasher.update(root.as_bytes());
        hasher.update(b"\0");
    }
    match rust_edition {
        Some(e) => {
            hasher.update(b"ed\0");
            hasher.update(e.as_bytes());
            hasher.update(b"\0");
        }
        None => hasher.update(b"noed\0"),
    }
    // Matches `ProjectGenerator::new(..., is_binary: true)` + `set_include_dev_dependencies(true)` for this workspace.
    hasher.update(b"layout_bin_devdeps\0");
    hasher.update(b"derive_probes\0");
    for path in rust_derive_probe_paths {
        hasher.update(path.as_bytes());
        hasher.update(b"\0");
    }

    let stdlib = normalized_stdlib_features_for_rust_inspect_fingerprint(stdlib_features);
    for f in &stdlib {
        hasher.update(f.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update(b"|\0");

    let mut projections = sdk_artifact_projections.to_vec();
    normalize_sdk_artifact_projections(&mut projections);
    hasher.update(b"sdk_artifact_projections\0");
    for projection in projections {
        hasher.update(projection.artifact.crate_root.as_os_str().as_encoded_bytes());
        hasher.update(b"\0");
        match digest_provider_artifact(&projection.artifact.crate_root) {
            Ok(digest) => hasher.update(digest.as_bytes()),
            Err(error) => hasher.update(error.to_string().as_bytes()),
        }
        hasher.update(b"\0");
    }
    hasher.update(b"|\0");

    let mut rebindings = sdk_dependency_rebindings.to_vec();
    normalize_sdk_dependency_rebindings(&mut rebindings);
    hasher.update(b"sdk_dependency_rebindings\0");
    for rebinding in rebindings {
        hasher.update(rebinding.containing_artifact.crate_root.as_os_str().as_encoded_bytes());
        hasher.update(b"\0");
        hasher.update(rebinding.provider_name.as_bytes());
        hasher.update(b"\0");
        hasher.update(rebinding.dependency_key.as_bytes());
        hasher.update(b"\0");
        hasher.update(rebinding.source_crate_root.as_os_str().as_encoded_bytes());
        hasher.update(b"\0");
        hasher.update(rebinding.active_crate_root.as_os_str().as_encoded_bytes());
        hasher.update(b"\0");
        match digest_provider_artifact(&rebinding.active_crate_root) {
            Ok(digest) => hasher.update(digest.as_bytes()),
            Err(error) => hasher.update(error.to_string().as_bytes()),
        }
        hasher.update(b"\0");
    }
    hasher.update(b"|\0");

    let mut sdk_paths = sdk_path_dependencies.to_vec();
    sdk_paths.sort_by(|left, right| {
        (&left.crate_name, left.package.as_deref()).cmp(&(&right.crate_name, right.package.as_deref()))
    });
    hasher.update(b"sdk_path_dependencies\0");
    for dependency in &sdk_paths {
        hash_dependency_spec_for_rust_inspect(&mut hasher, dependency);
        if let DependencySource::Path { path } = &dependency.source {
            match digest_provider_artifact(path) {
                Ok(digest) => hasher.update(digest.as_bytes()),
                Err(error) => hasher.update(error.to_string().as_bytes()),
            }
            hasher.update(b"\0");
        }
    }
    hasher.update(b"|\0");

    let mut deps = resolved.dependencies.clone();
    deps.sort_by(|a, b| a.crate_name.cmp(&b.crate_name));
    for dep in &mut deps {
        *dep = dep.clone().normalized();
    }
    hasher.update(b"deps\0");
    for dep in &deps {
        hash_dependency_spec_for_rust_inspect(&mut hasher, dep);
    }
    hasher.update(b"|\0");

    let mut dev_deps = resolved.dev_dependencies.clone();
    dev_deps.sort_by(|a, b| a.crate_name.cmp(&b.crate_name));
    for dep in &mut dev_deps {
        *dep = dep.clone().normalized();
    }
    hasher.update(b"dev_deps\0");
    for dep in &dev_deps {
        hash_dependency_spec_for_rust_inspect(&mut hasher, dep);
    }
    hasher.update(b"|\0");

    match cargo_lock_payload {
        Some(lock) => {
            hasher.update(b"lock\0");
            hasher.update(lock.as_bytes());
        }
        None => hasher.update(b"nolock\0"),
    }

    format!(
        "{}{}",
        RUST_INSPECT_WORKSPACE_FINGERPRINT_PREFIX,
        hex::encode(hasher.finalize())
    )
}

/// Keep derive probes within the generated inspection root's declared Cargo dependency namespaces.
///
/// SDK path catalogs and dependency rebindings may also describe private or transitive artifact edges, so they are
/// deliberately not namespace authority for source-level `@rust.derive(...)` paths.
#[cfg(feature = "rust_inspect")]
fn declared_rust_derive_probe_paths(
    resolved: &ResolvedDependencies,
    rust_derive_probe_paths: &[String],
) -> Vec<String> {
    let declared_crates = resolved
        .dependencies
        .iter()
        .chain(resolved.dev_dependencies.iter())
        .map(|dependency| dependency.crate_name.replace('-', "_"))
        .collect::<BTreeSet<_>>();

    rust_derive_probe_paths
        .iter()
        .filter(|path| {
            path.split("::")
                .next()
                .is_some_and(|crate_name| declared_crates.contains(crate_name))
        })
        .cloned()
        .collect()
}

/// Return the workspace directory used for Rust inspection metadata.
#[cfg(feature = "rust_inspect")]
fn rust_inspect_workspace_dir(project_root: &Path, project_name: &str, fingerprint: &str) -> PathBuf {
    let mut safe_name = project_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if safe_name.is_empty() {
        safe_name.push_str("project");
    }
    let suffix = fingerprint
        .rsplit_once(':')
        .map(|(_, hash)| hash)
        .unwrap_or(fingerprint)
        .chars()
        .take(16)
        .collect::<String>();
    crate::lockfile::compiler_lock_state_dir(project_root)
        .join("rust_inspect")
        .join(format!("{safe_name}-{suffix}"))
}

#[cfg(feature = "rust_inspect")]
/// Build a deterministic fingerprint for generated build-script metadata prewarm inputs and requested Rust paths.
fn rust_inspect_out_dirs_fingerprint(
    manifest_dir: &Path,
    target_dir: &Path,
    query_paths: &[String],
) -> CliResult<String> {
    let mut hasher = Sha256::new();
    hasher.update(b"incan_rust_inspect_out_dirs/1\0");
    hasher.update(target_dir.as_os_str().as_encoded_bytes());
    hasher.update(b"\0");
    for relative in ["Cargo.toml", "Cargo.lock", "src/main.rs"] {
        let path = manifest_dir.join(relative);
        match fs::read(&path) {
            Ok(bytes) => {
                hasher.update(relative.as_bytes());
                hasher.update(b"\0");
                hasher.update(bytes);
                hasher.update(b"\0");
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound && relative == "Cargo.lock" => {}
            Err(err) => {
                return Err(CliError::failure(format!(
                    "Failed to fingerprint rust-inspect out-dir prewarm input {}: {err}",
                    path.display()
                )));
            }
        }
    }
    let mut sorted_paths = query_paths.to_vec();
    sorted_paths.sort();
    sorted_paths.dedup();
    for query_path in sorted_paths {
        hasher.update(query_path.as_bytes());
        hasher.update(b"\0");
    }
    Ok(format!(
        "{}{}",
        RUST_INSPECT_OUT_DIRS_FINGERPRINT_FILE,
        hex::encode(hasher.finalize())
    ))
}

#[cfg(feature = "rust_inspect")]
/// Return whether the stored out-dir prewarm stamp matches the current fingerprint and the shared target still exists.
fn rust_inspect_out_dirs_stamp_matches(stamp_path: &Path, fingerprint: &str, target_dir: &Path) -> bool {
    target_dir.is_dir()
        && fs::read_to_string(stamp_path)
            .map(|existing| existing.trim() == fingerprint)
            .unwrap_or(false)
}

#[cfg(feature = "rust_inspect")]
/// Write a Cargo config that points the generated rust-inspect workspace at the shared target directory.
fn write_rust_inspect_cargo_config(manifest_dir: &Path, target_dir: &Path) -> CliResult<()> {
    let cargo_dir = manifest_dir.join(".cargo");
    fs::create_dir_all(&cargo_dir).map_err(|err| {
        CliError::failure(format!(
            "Failed to create rust-inspect Cargo config directory {}: {err}",
            cargo_dir.display()
        ))
    })?;
    let escaped_target_dir = target_dir.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    fs::write(
        cargo_dir.join("config.toml"),
        format!("[build]\ntarget-dir = \"{escaped_target_dir}\"\n"),
    )
    .map_err(|err| {
        CliError::failure(format!(
            "Failed to write rust-inspect Cargo config {}: {err}",
            cargo_dir.join("config.toml").display()
        ))
    })
}

#[cfg(feature = "rust_inspect")]
/// Point one generated rust-inspect workspace at its selected Cargo target.
pub(crate) fn configure_rust_inspect_cargo_target(manifest_dir: &Path, target_dir: &Path) -> CliResult<()> {
    write_rust_inspect_cargo_config(manifest_dir, target_dir)
}

#[cfg(feature = "rust_inspect")]
/// Detect Cargo's stale-lockfile failure so prewarm can retry with an offline lock refresh instead of silently
/// skipping.
fn rust_inspect_locked_prewarm_needs_lock_update(stderr: &str) -> bool {
    stderr.contains("--locked was passed")
        && stderr.contains("lock file")
        && (stderr.contains("cannot update") || stderr.contains("needs to be updated"))
}

#[cfg(feature = "rust_inspect")]
/// Run the Cargo command that warms generated build-script output for rust-inspect metadata extraction.
fn run_rust_inspect_out_dirs_prewarm_command(
    manifest_dir: &Path,
    target_dir: &Path,
    mode: RustInspectPrewarmCargoMode,
) -> CliResult<std::process::Output> {
    let mut command = crate::backend::project::runner::cargo_command();
    crate::backend::project::runner::configure_cargo_target(&mut command, target_dir);
    command.arg("check");
    command.arg("--manifest-path");
    command.arg(manifest_dir.join("Cargo.toml"));
    match mode {
        RustInspectPrewarmCargoMode::Locked if manifest_dir.join("Cargo.lock").is_file() => {
            command.arg("--locked");
        }
        RustInspectPrewarmCargoMode::Offline => {
            command.arg("--offline");
        }
        RustInspectPrewarmCargoMode::Locked => {}
    }
    command
        .env_remove("SSL_CERT_FILE")
        .env_remove("SSL_CERT_DIR")
        .env_remove("CURL_CA_BUNDLE")
        .env_remove("REQUESTS_CA_BUNDLE")
        .env_remove("CARGO_HTTP_CAINFO")
        .output()
        .map_err(|err| CliError::failure(format!("Failed to run rust-inspect build-script prewarm: {err}")))
}

#[cfg(feature = "rust_inspect")]
#[derive(Debug, Clone, Copy)]
enum RustInspectPrewarmCargoMode {
    Locked,
    Offline,
}

#[cfg(feature = "rust_inspect")]
/// Prewarm generated build-script output directories for rust-inspect lookups and stamp successful runs for reuse.
fn prewarm_rust_inspect_out_dirs(manifest_dir: &Path, target_dir: &Path, query_paths: &[String]) -> CliResult<()> {
    write_rust_inspect_cargo_config(manifest_dir, target_dir)?;
    let fingerprint = rust_inspect_out_dirs_fingerprint(manifest_dir, target_dir, query_paths)?;
    let stamp_path = manifest_dir.join(RUST_INSPECT_OUT_DIRS_FINGERPRINT_FILE);
    if rust_inspect_out_dirs_stamp_matches(&stamp_path, &fingerprint, target_dir) {
        return Ok(());
    }

    eprintln!(
        "rust-inspect build-script prewarm: checking generated metadata workspace into {}",
        target_dir.display()
    );
    let mut output =
        run_rust_inspect_out_dirs_prewarm_command(manifest_dir, target_dir, RustInspectPrewarmCargoMode::Locked)?;
    if !output.status.success()
        && rust_inspect_locked_prewarm_needs_lock_update(String::from_utf8_lossy(&output.stderr).as_ref())
    {
        eprintln!("rust-inspect build-script prewarm: generated Cargo.lock is stale; retrying offline lock refresh");
        output =
            run_rust_inspect_out_dirs_prewarm_command(manifest_dir, target_dir, RustInspectPrewarmCargoMode::Offline)?;
    }

    if !output.status.success() {
        return Err(CliError::failure(format!(
            "rust-inspect build-script prewarm failed with status {}:\n{}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    fs::write(&stamp_path, &fingerprint).map_err(|err| {
        CliError::failure(format!(
            "Failed to write rust-inspect out-dir prewarm fingerprint {}: {err}",
            stamp_path.display()
        ))
    })?;
    Ok(())
}

/// Generate the rust-inspect workspace that semantic Rust extraction should query for this project.
///
/// The generated workspace intentionally uses the Rust import spelling for dependency keys, while preserving the
/// published Cargo package name separately when the two differ.
///
/// When the same inputs are seen again (for example across multiple `incan test` cases in one package), regeneration is
/// skipped if the namespaced workspace fingerprint matches the computed digest and expected artifacts exist.
#[cfg(all(test, feature = "rust_inspect"))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn ensure_rust_inspect_workspace(
    project_root: &Path,
    project_name: &str,
    rust_edition: Option<String>,
    resolved: &ResolvedDependencies,
    project_requirements: &ProjectRequirements,
    cargo_lock_payload: Option<String>,
    cargo_target_dir: &Path,
    cargo_policy_flags: &[String],
) -> CliResult<PathBuf> {
    ensure_rust_inspect_workspace_with_cargo_package_name(
        project_root,
        project_name,
        project_name,
        rust_edition,
        resolved,
        project_requirements,
        cargo_lock_payload,
        None,
        false,
        cargo_target_dir,
        cargo_policy_flags,
        &[],
    )
}

/// Generate a rust-inspect workspace whose Cargo package identity matches the canonical lock owner.
#[cfg(feature = "rust_inspect")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn ensure_rust_inspect_workspace_with_cargo_package_name(
    project_root: &Path,
    project_name: &str,
    cargo_package_name: &str,
    rust_edition: Option<String>,
    resolved: &ResolvedDependencies,
    project_requirements: &ProjectRequirements,
    cargo_lock_payload: Option<String>,
    cargo_lock_projection_root: Option<&str>,
    clear_cargo_lock: bool,
    cargo_target_dir: &Path,
    cargo_policy_flags: &[String],
    rust_derive_probe_paths: &[String],
) -> CliResult<PathBuf> {
    let project_manifest =
        ProjectManifest::discover(project_root).map_err(|error| CliError::failure(error.to_string()))?;
    let cargo_lock_payload =
        project_rust_inspect_lock_projection(cargo_lock_payload, cargo_package_name, project_manifest.as_ref())?;
    let rust_derive_probe_paths = declared_rust_derive_probe_paths(resolved, rust_derive_probe_paths);
    let fingerprint = rust_inspect_workspace_fingerprint(
        project_name,
        cargo_package_name,
        rust_edition.as_deref(),
        resolved,
        &project_requirements.stdlib_features,
        &project_requirements.sdk_dependency_rebindings,
        &project_requirements.sdk_path_dependencies,
        &project_requirements.sdk_artifact_projections,
        cargo_lock_payload.as_deref(),
        cargo_lock_projection_root,
        clear_cargo_lock,
        cargo_target_dir,
        rust_derive_probe_paths.as_slice(),
    );
    let rust_inspect_manifest_dir = rust_inspect_workspace_dir(project_root, project_name, &fingerprint);
    let fingerprint_path = rust_inspect_manifest_dir.join(RUST_INSPECT_WORKSPACE_FINGERPRINT_FILE);
    let cargo_toml_path = rust_inspect_manifest_dir.join("Cargo.toml");
    let main_rs_path = rust_inspect_manifest_dir.join("src").join("main.rs");

    let fingerprint_matches = match fs::read_to_string(&fingerprint_path) {
        Ok(existing) => existing.trim() == fingerprint.as_str(),
        Err(_) => false,
    };

    if cargo_toml_path.is_file()
        && main_rs_path.is_file()
        && fingerprint_matches
        && project_requirements.sdk_artifact_projections.is_empty()
    {
        return Ok(rust_inspect_manifest_dir);
    }

    let mut generator = ProjectGenerator::new(&rust_inspect_manifest_dir, project_name, true);
    generator.set_package_name(Some(cargo_package_name.to_string()));
    if let Some(project) = project_manifest.as_ref().and_then(|manifest| manifest.project.as_ref()) {
        generator.set_package_metadata(project.version.clone(), project.license.clone());
    }
    generator.set_dependencies(resolved.dependencies.clone());
    generator.set_dev_dependencies(resolved.dev_dependencies.clone());
    generator.set_include_dev_dependencies(true);
    generator.set_stdlib_features(project_requirements.stdlib_features.clone());
    generator.set_sdk_dependency_rebindings(project_requirements.sdk_dependency_rebindings.clone());
    generator.set_sdk_path_dependencies(project_requirements.sdk_path_dependencies.clone());
    generator.set_sdk_artifact_projections(project_requirements.sdk_artifact_projections.clone());
    generator.set_rust_edition(rust_edition);
    generator.set_cargo_lock_payload(cargo_lock_payload);
    generator.set_cargo_lock_projection_root(cargo_lock_projection_root.map(ToOwned::to_owned));
    generator.set_clear_cargo_lock(clear_cargo_lock);
    generator.set_cargo_policy_flags(cargo_policy_flags.to_vec());
    let mut referenced_crates = std::collections::BTreeSet::new();
    for dep in resolved.dependencies.iter().chain(resolved.dev_dependencies.iter()) {
        referenced_crates.insert(dep.crate_name.replace('-', "_"));
    }
    let mut rust_inspect_stub = String::new();
    for crate_name in referenced_crates {
        rust_inspect_stub.push_str(format!("use {crate_name} as _;\n").as_str());
    }
    for (index, derive_path) in rust_derive_probe_paths.iter().enumerate() {
        rust_inspect_stub.push_str(format!("#[derive({derive_path})]\nstruct __IncanDeriveProbe{index};\n").as_str());
    }
    rust_inspect_stub.push_str("fn main() {}");

    #[cfg(all(test, feature = "rust_inspect"))]
    record_test_rust_inspect_workspace_generation(&rust_inspect_manifest_dir);

    generator.generate(rust_inspect_stub.as_str()).map_err(|e| {
        CliError::failure(format!(
            "Failed to generate rust-inspect lock project at {}: {e}",
            rust_inspect_manifest_dir.display()
        ))
    })?;
    generator.materialize_cargo_lock_projection().map_err(|error| {
        CliError::failure(format!(
            "Failed to project rust-inspect Cargo.lock at {}: {error}",
            rust_inspect_manifest_dir.display()
        ))
    })?;

    if let Err(err) = fs::write(&fingerprint_path, &fingerprint) {
        return Err(CliError::failure(format!(
            "Failed to write rust-inspect workspace fingerprint {}: {err}",
            fingerprint_path.display()
        )));
    }

    Ok(rust_inspect_manifest_dir)
}

/// Project a canonical dependency lock into the generated rust-inspect root without re-resolving dependencies.
///
/// A Cargo lock records the local package's version alongside registry packages. Incan's dependency fingerprint
/// intentionally excludes that local version because it cannot alter the resolved third-party graph. When a project
/// changes only its own version, passing the old root record to Cargo with `--locked` spuriously rejects the source
/// inspection projection. Rewrite only that source-less root record to the current manifest version; registry package
/// entries, checksums, and dependency edges remain selected from the canonical lock. This stays a projection, never
/// a lock publication or dependency resolver.
#[cfg(feature = "rust_inspect")]
fn project_rust_inspect_lock_projection(
    cargo_lock_payload: Option<String>,
    cargo_package_name: &str,
    manifest: Option<&ProjectManifest>,
) -> CliResult<Option<String>> {
    let Some(payload) = cargo_lock_payload else {
        return Ok(None);
    };
    let Some(version) = manifest
        .and_then(|manifest| manifest.project.as_ref())
        .and_then(|project| project.version.as_deref())
    else {
        return Ok(Some(payload));
    };
    let mut lock = toml::from_str::<toml::Value>(&payload).map_err(|error| {
        CliError::failure(format!(
            "failed to parse canonical Cargo.lock for rust-inspect projection: {error}"
        ))
    })?;
    let Some(packages) = lock.get_mut("package").and_then(toml::Value::as_array_mut) else {
        // Modern semantic Incan locks intentionally have no Cargo package graph until the explicit Oven publisher
        // creates one. They are not a Cargo projection and must remain untouched here.
        return Ok(Some(payload));
    };
    let mut matching_roots = packages
        .iter_mut()
        .filter_map(|package| {
            let table = package.as_table_mut()?;
            let name = table.get("name")?.as_str()?;
            (name == cargo_package_name && !table.contains_key("source")).then_some(table)
        })
        .collect::<Vec<_>>();
    let [root] = matching_roots.as_mut_slice() else {
        // A provider-only or workspace lock can legitimately omit the generated inspection package. It is still an
        // exact external dependency authority, so leave it intact rather than manufacturing a local root record.
        return Ok(Some(payload));
    };
    root.insert("version".to_string(), toml::Value::String(version.to_string()));
    toml::to_string_pretty(&lock).map(Some).map_err(|error| {
        CliError::failure(format!(
            "failed to serialize rust-inspect Cargo.lock projection: {error}"
        ))
    })
}

/// Collect canonical rust-inspect query paths from parsed `rust::` imports.
#[cfg(feature = "rust_inspect")]
pub(crate) fn collect_rust_inspect_query_paths(modules: &[ParsedModule]) -> Vec<String> {
    collect_rust_inspect_query_paths_from_programs(modules.iter().map(|module| &module.ast))
}

/// Collect canonical rust-inspect query paths from parsed programs.
///
/// Loaf publication also parses compiler-owned provider source that is metadata-only in the consumer module
/// graph. Keeping the import walk program-based lets that explicit publisher preserve Rust ownership signatures
/// without making normal Oven consumers re-emit provider source.
#[cfg(feature = "rust_inspect")]
pub(crate) fn collect_rust_inspect_query_paths_from_programs<'a>(
    programs: impl IntoIterator<Item = &'a Program>,
) -> Vec<String> {
    /// Read a presence-and-truthiness flag (`1`, `true`, `on`) from the environment.
    fn env_flag_enabled(name: &str) -> bool {
        std::env::var_os(name).is_some_and(|value| {
            let value = value.to_string_lossy();
            matches!(value.as_ref(), "1" | "true" | "TRUE" | "on" | "ON")
        })
    }

    // Default policy: prewarm explicit non-stdlib `from rust::... import Item` imports. These are the exact paths
    // semantic/codegen hot paths may query later, including Rust types with uppercase names.
    //
    // We still avoid crate/module imports and `incan_stdlib::*` by default. Full eager prewarm can force broad
    // rust-analyzer walks and persist negative module lookups that are not safe metadata items.
    // Set `INCAN_RUST_INSPECT_PREWARM_ALL=1` to restore full eager prewarm for debugging/regressions.
    let prewarm_all = env_flag_enabled("INCAN_RUST_INSPECT_PREWARM_ALL");
    let mut paths: BTreeSet<String> = BTreeSet::new();
    for program in programs {
        for decl in &program.declarations {
            let crate::frontend::ast::Declaration::Import(import) = &decl.node else {
                continue;
            };
            match &import.kind {
                ImportKind::RustCrate { crate_name, path, .. } if prewarm_all => {
                    let mut segments = Vec::with_capacity(path.len() + 1);
                    segments.push(crate_name.replace('-', "_"));
                    segments.extend(path.iter().cloned());
                    if !segments.is_empty() {
                        paths.insert(segments.join("::"));
                    }
                }
                ImportKind::RustCrate { .. } => {}
                ImportKind::RustFrom {
                    crate_name,
                    path,
                    items,
                    ..
                } => {
                    let mut base = Vec::with_capacity(path.len() + 1);
                    base.push(crate_name.replace('-', "_"));
                    base.extend(path.iter().cloned());
                    let base = base.join("::");
                    if base.is_empty() {
                        continue;
                    }
                    if !prewarm_all && base.starts_with("incan_stdlib::") {
                        continue;
                    }
                    let primitive_ns = matches!(base.as_str(), "std::primitive" | "core::primitive");
                    for item in items {
                        if !primitive_ns {
                            paths.insert(format!("{base}::{}", item.name));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    paths.into_iter().collect()
}

/// Collect exact Rust derive paths used by concrete Incan type declarations.
///
/// The explicit Cargo-backed inspection bootstrap emits one synthetic Rust type per path. Expanding that type records
/// a candidate trait and associated-type contract for ABI selection; native rustc remains authoritative for the real
/// Incan-authored declaration. This collector admits direct `rust::` item imports used by `@derive(...)` or
/// `@rust.derive(...)` plus syntactically valid explicit Rust macro paths. The preparation boundary later limits those
/// paths to declared dependency namespaces before generating the probe workspace.
#[cfg(feature = "rust_inspect")]
pub(crate) fn collect_rust_inspect_derive_probe_paths(modules: &[ParsedModule]) -> Vec<String> {
    use crate::frontend::ast::{Declaration, Decorator, DecoratorArg, Expr, Literal};

    /// Return decorators only for declarations that can emit a concrete Rust type.
    fn declaration_decorators(declaration: &Declaration) -> Option<&[crate::frontend::ast::Spanned<Decorator>]> {
        match declaration {
            Declaration::Model(item) => Some(&item.decorators),
            Declaration::Class(item) => Some(&item.decorators),
            Declaration::Newtype(item) => Some(&item.decorators),
            Declaration::Enum(item) => Some(&item.decorators),
            Declaration::Import(_)
            | Declaration::Const(_)
            | Declaration::Static(_)
            | Declaration::Trait(_)
            | Declaration::Alias(_)
            | Declaration::Partial(_)
            | Declaration::TypeAlias(_)
            | Declaration::Function(_)
            | Declaration::TestModule(_)
            | Declaration::VocabBlock(_)
            | Declaration::Capability(_)
            | Declaration::Docstring(_) => None,
        }
    }

    /// Validate a canonical Rust item path, including raw-identifier segments, before generating a derive probe.
    fn is_valid_rust_path(path: &str) -> bool {
        path.split("::").all(|segment| {
            let segment = segment.strip_prefix("r#").unwrap_or(segment);
            let mut chars = segment.chars();
            chars
                .next()
                .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
                && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        })
    }

    let mut probes = BTreeSet::new();
    for module in modules {
        let mut imported_paths = HashMap::new();
        for declaration in &module.ast.declarations {
            let Declaration::Import(import) = &declaration.node else {
                continue;
            };
            let ImportKind::RustFrom {
                crate_name,
                path,
                items,
                ..
            } = &import.kind
            else {
                continue;
            };
            let mut base = Vec::with_capacity(path.len() + 1);
            base.push(crate_name.replace('-', "_"));
            base.extend(path.iter().cloned());
            for item in items {
                let binding = item.alias.as_ref().unwrap_or(&item.name);
                let mut segments = base.clone();
                segments.push(item.name.clone());
                imported_paths.insert(binding.clone(), segments.join("::"));
            }
        }
        for declaration in &module.ast.declarations {
            let Some(decorators) = declaration_decorators(&declaration.node) else {
                continue;
            };
            for decorator in decorators {
                let Some(decorator_id) = incan_core::lang::decorators::from_segments(&decorator.node.path.segments)
                else {
                    continue;
                };
                for argument in &decorator.node.args {
                    let DecoratorArg::Positional(argument) = argument else {
                        continue;
                    };
                    match (decorator_id, &argument.node) {
                        (incan_core::lang::decorators::DecoratorId::Derive, Expr::Ident(name))
                        | (incan_core::lang::decorators::DecoratorId::RustDerive, Expr::Ident(name)) => {
                            if let Some(path) = imported_paths.get(name) {
                                probes.insert(path.clone());
                            }
                        }
                        (
                            incan_core::lang::decorators::DecoratorId::RustDerive,
                            Expr::Literal(Literal::String(path)),
                        ) if path.contains("::") && is_valid_rust_path(path) => {
                            probes.insert(path.clone());
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    probes.into_iter().collect()
}

/// Return whether rust-inspect prewarm should run for the supplied environment value.
#[cfg(feature = "rust_inspect")]
fn parse_rust_inspect_prewarm_env(raw: Option<&str>) -> bool {
    let Some(raw) = raw else {
        return false;
    };
    matches!(raw.trim(), "1" | "true" | "TRUE" | "on" | "ON" | "yes" | "YES")
}

/// Return whether Rust inspection prewarming is enabled.
#[cfg(feature = "rust_inspect")]
fn rust_inspect_prewarm_enabled() -> bool {
    parse_rust_inspect_prewarm_env(std::env::var("INCAN_RUST_INSPECT_PREWARM").ok().as_deref())
}

/// Return whether rust-inspect should eagerly run Cargo to materialize every generated build-script `OUT_DIR`.
#[cfg(feature = "rust_inspect")]
fn parse_rust_inspect_eager_out_dirs_prewarm_env(raw: Option<&str>) -> bool {
    raw.is_some_and(|raw| matches!(raw.trim(), "1" | "true" | "TRUE" | "on" | "ON" | "yes" | "YES"))
}

/// Return whether rust-inspect should eagerly run Cargo to materialize every generated build-script `OUT_DIR`.
#[cfg(feature = "rust_inspect")]
fn rust_inspect_eager_out_dirs_prewarm_enabled() -> bool {
    parse_rust_inspect_eager_out_dirs_prewarm_env(
        std::env::var("INCAN_RUST_INSPECT_EAGER_OUT_DIRS_PREWARM")
            .ok()
            .as_deref(),
    )
}

/// Surface rust-inspect preparation progress from explicit CLI prewarm phases.
#[cfg(feature = "rust_inspect")]
fn print_rust_inspect_prewarm_progress(message: String) {
    if message.starts_with("rust-inspect prewarm") {
        eprintln!("{message}");
    }
}

/// Marker understood by `rust_inspect` that selects its build-system-neutral `rust-project.json` loader.
///
/// Mark one compiler-authored Rust inspection projection for receipt-bound direct-Rustc loading.
#[cfg(feature = "rust_inspect")]
pub(crate) fn mark_oven_direct_rust_inspection(manifest_dir: &Path) -> CliResult<()> {
    let bootstrap_marker = manifest_dir.join(crate::rust_inspect::OVEN_CARGO_BOOTSTRAP_INSPECTION_MARKER);
    if bootstrap_marker.is_file() {
        fs::remove_file(&bootstrap_marker).map_err(|error| {
            CliError::failure(format!(
                "failed to retire explicit Oven Cargo inspection marker {}: {error}",
                bootstrap_marker.display()
            ))
        })?;
    }
    let marker = manifest_dir.join(crate::rust_inspect::OVEN_DIRECT_INSPECTION_MARKER);
    fs::write(&marker, b"receipt-bound direct-rustc inspection\n").map_err(|error| {
        CliError::failure(format!(
            "failed to mark Oven Rust inspection projection {}: {error}",
            marker.display()
        ))
    })
}

/// Mark the explicit Oven publisher's generated inspection workspace for one Cargo-backed semantic bootstrap.
///
/// The publisher already owns Cargo authority while preparing a new dependency closure. Letting rust-analyzer load
/// real proc-macro output here records generated trait implementations without teaching ordinary build/run paths to
/// invoke Cargo or infer macro behavior from source text.
#[cfg(feature = "rust_inspect")]
pub(crate) fn mark_oven_cargo_bootstrap_rust_inspection(manifest_dir: &Path) -> CliResult<()> {
    let direct_marker = manifest_dir.join(crate::rust_inspect::OVEN_DIRECT_INSPECTION_MARKER);
    if direct_marker.is_file() {
        fs::remove_file(&direct_marker).map_err(|error| {
            CliError::failure(format!(
                "failed to retire direct Oven inspection marker {}: {error}",
                direct_marker.display()
            ))
        })?;
    }
    let marker = manifest_dir.join(crate::rust_inspect::OVEN_CARGO_BOOTSTRAP_INSPECTION_MARKER);
    fs::write(&marker, b"explicit Oven Cargo semantic bootstrap\n").map_err(|error| {
        CliError::failure(format!(
            "failed to mark explicit Oven Cargo inspection projection {}: {error}",
            marker.display()
        ))
    })
}

#[cfg(feature = "rust_inspect")]
/// Return whether this generated manifest carries the direct-Oven rust-inspect marker.
fn oven_direct_rust_inspection_marked(manifest_dir: &Path) -> bool {
    manifest_dir
        .join(crate::rust_inspect::OVEN_DIRECT_INSPECTION_MARKER)
        .is_file()
}

/// Prepare rust-inspect metadata access before typechecking/codegen hot paths.
///
/// Metadata extraction now defaults to lazy lookup because eager rust-analyzer extraction across every imported Rust
/// path can dominate cold downstream builds before the real generated Rust build starts. The Cargo target configuration
/// is still prepared up front so lazy build-script `OUT_DIR` routes share the generated-project target directory. Set
/// `INCAN_RUST_INSPECT_PREWARM=1` to opt into eager metadata prewarm, and set
/// `INCAN_RUST_INSPECT_EAGER_OUT_DIRS_PREWARM=1` only when debugging a suspected out-dir cache regression.
#[cfg(feature = "rust_inspect")]
pub(crate) fn prewarm_rust_inspect_workspace(
    manifest_dir: &Path,
    target_dir: &Path,
    query_paths: &[String],
    force_direct_prewarm: bool,
) -> CliResult<()> {
    if oven_direct_rust_inspection_marked(manifest_dir) {
        // The direct loader consumes the compiler-authored source graph without Cargo. Its only build-script
        // OUT_DIR sources are the sealed plan directories the installer recorded, so ignore the legacy eager-OUT_DIR
        // knob rather than letting a normal Oven command regain a Cargo subprocess through an inspection side path.
        if !query_paths.is_empty() && (force_direct_prewarm || rust_inspect_prewarm_enabled()) {
            let inspector = Inspector::new(InspectorConfig::new(manifest_dir.to_path_buf()));
            inspector
                .prewarm(query_paths.iter().cloned(), &print_rust_inspect_prewarm_progress)
                .map_err(|err| {
                    CliError::failure(format!(
                        "failed to prewarm direct Oven rust-inspect cache from {}: {err}",
                        manifest_dir.display()
                    ))
                })?;
        }
        return Ok(());
    }

    configure_rust_inspect_cargo_target(manifest_dir, target_dir)?;
    if query_paths.is_empty() {
        return Ok(());
    }
    if !rust_inspect_prewarm_enabled() {
        return Ok(());
    }
    if rust_inspect_eager_out_dirs_prewarm_enabled() {
        prewarm_rust_inspect_out_dirs(manifest_dir, target_dir, query_paths)?;
    }
    let inspector = Inspector::new(InspectorConfig::new(manifest_dir.to_path_buf()));
    inspector
        .prewarm(query_paths.iter().cloned(), &print_rust_inspect_prewarm_progress)
        .map_err(|err| {
            CliError::failure(format!(
                "failed to prewarm rust-inspect cache from {}: {err}",
                manifest_dir.display()
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::test_support::parsed_module_for_test;
    use crate::frontend::library_manifest_index::LibraryArtifactMetadata;
    use crate::library_manifest::LibraryManifest;
    use std::process::Command;

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_prewarm_env_defaults_to_disabled() {
        assert!(!parse_rust_inspect_prewarm_env(None));
        assert!(!parse_rust_inspect_prewarm_env(Some("")));
        assert!(parse_rust_inspect_prewarm_env(Some("1")));
        assert!(parse_rust_inspect_prewarm_env(Some("true")));
        assert!(parse_rust_inspect_prewarm_env(Some("on")));
        assert!(parse_rust_inspect_prewarm_env(Some("YES")));
        assert!(!parse_rust_inspect_prewarm_env(Some("0")));
        assert!(!parse_rust_inspect_prewarm_env(Some("false")));
        assert!(!parse_rust_inspect_prewarm_env(Some(" OFF ")));
        assert!(!parse_rust_inspect_prewarm_env(Some("no")));
        assert!(!parse_rust_inspect_prewarm_env(Some("unexpected")));
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_eager_out_dir_prewarm_env_defaults_to_disabled() {
        assert!(!parse_rust_inspect_eager_out_dirs_prewarm_env(None));
        assert!(!parse_rust_inspect_eager_out_dirs_prewarm_env(Some("")));
        assert!(parse_rust_inspect_eager_out_dirs_prewarm_env(Some("1")));
        assert!(parse_rust_inspect_eager_out_dirs_prewarm_env(Some("true")));
        assert!(parse_rust_inspect_eager_out_dirs_prewarm_env(Some("ON")));
        assert!(parse_rust_inspect_eager_out_dirs_prewarm_env(Some("yes")));
        assert!(!parse_rust_inspect_eager_out_dirs_prewarm_env(Some("0")));
        assert!(!parse_rust_inspect_eager_out_dirs_prewarm_env(Some("false")));
        assert!(!parse_rust_inspect_eager_out_dirs_prewarm_env(Some("unexpected")));
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_query_paths_include_explicit_rust_item_imports() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
from rust::datafusion::execution::context import SessionContext
from rust::datafusion::prelude import CsvReadOptions, read_csv
from rust::incan_stdlib::async::runtime import block_on
from rust::std::fs import metadata
from rust::std::primitive import i64 as RustI64
"#,
        )?;

        let paths = collect_rust_inspect_query_paths(&[module]);

        assert_eq!(
            paths,
            vec![
                "datafusion::execution::context::SessionContext".to_string(),
                "datafusion::prelude::CsvReadOptions".to_string(),
                "datafusion::prelude::read_csv".to_string(),
                "std::fs::metadata".to_string(),
            ]
        );

        let program_paths = collect_rust_inspect_query_paths_from_programs([&parsed_module_for_test(
            r#"
from rust::rustix::fs import flock
"#,
        )?
        .ast]);
        assert_eq!(program_paths, vec!["rustix::fs::flock".to_string()]);
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_derive_probes_include_only_used_direct_rust_imports() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
from rust::provider::prelude import Component as ForeignComponent, UnusedDerive

@derive(ForeignComponent, Clone)
model Velocity:
  x: f32

@rust.derive(ForeignComponent, "provider::derive::ExplicitComponent", "provider::r#async::Component", "provider::derive::Bad;Drop", Clone)
model ExplicitVelocity:
  x: f32
"#,
        )?;

        assert_eq!(
            collect_rust_inspect_derive_probe_paths(&[module]),
            vec![
                "provider::derive::ExplicitComponent".to_string(),
                "provider::prelude::Component".to_string(),
                "provider::r#async::Component".to_string(),
            ],
            "directly imported and explicit-path Rust derives that are actually invoked must become semantic probes"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_derive_probe_aliases_remain_module_scoped() -> Result<(), Box<dyn std::error::Error>> {
        let left = parsed_module_for_test(
            r#"
from rust::left_provider import Component as SharedDerive

@derive(SharedDerive)
model LeftValue:
  value: int
"#,
        )?;
        let right = parsed_module_for_test(
            r#"
from rust::right_provider import Component as SharedDerive

@rust.derive(SharedDerive)
model RightValue:
  value: int
"#,
        )?;

        assert_eq!(
            collect_rust_inspect_derive_probe_paths(&[left, right]),
            vec![
                "left_provider::Component".to_string(),
                "right_provider::Component".to_string(),
            ],
            "the same visible alias in separate modules must retain both module-local Rust identities"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_derive_probes_are_limited_to_declared_dependency_namespaces() {
        let resolved = ResolvedDependencies {
            dependencies: vec![DependencySpec {
                crate_name: "provider".to_string(),
                version: Some("1".to_string()),
                features: Vec::new(),
                default_features: true,
                source: DependencySource::Registry,
                optional: false,
                package: None,
            }],
            dev_dependencies: Vec::new(),
        };
        let probes = super::declared_rust_derive_probe_paths(
            &resolved,
            &["missing::Component".to_string(), "provider::Component".to_string()],
        );
        assert_eq!(probes, vec!["provider::Component"]);
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_workspace_does_not_emit_an_undeclared_derive_probe() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::write(
            tmp.path().join("loaf.toml"),
            "[project]\nname = \"derive_probe\"\nversion = \"0.1.0\"\n",
        )?;
        let requirements = ProjectRequirements {
            sdk_path_dependencies: vec![DependencySpec {
                crate_name: "private_sdk".to_string(),
                version: None,
                features: Vec::new(),
                default_features: true,
                source: DependencySource::Path {
                    path: tmp.path().join("private-sdk"),
                },
                optional: false,
                package: None,
            }],
            sdk_dependency_rebindings: vec![SdkDependencyRebinding {
                containing_artifact: LibraryArtifactMetadata {
                    dependency_key: "compiled_provider".to_string(),
                    manifest_name: "compiled_provider".to_string(),
                    manifest_path: tmp.path().join("compiled-provider.incnlib"),
                    crate_root: tmp.path().join("compiled-provider"),
                    cargo_toml_path: tmp.path().join("compiled-provider/Cargo.toml"),
                    crate_lib_path: tmp.path().join("compiled-provider/src/lib.rs"),
                    kind: crate::frontend::library_manifest_index::LibraryArtifactKind::Materialized,
                },
                source_crate_root: tmp.path().join("old-private-sdk"),
                provider_name: "private_sdk".to_string(),
                dependency_key: "private_sdk".to_string(),
                active_crate_root: tmp.path().join("private-sdk"),
            }],
            ..ProjectRequirements::default()
        };
        let resolved = ResolvedDependencies {
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
        };
        let workspace = ensure_rust_inspect_workspace_with_cargo_package_name(
            tmp.path(),
            "derive_probe",
            "derive_probe",
            Some("2021".to_string()),
            &resolved,
            &requirements,
            None,
            None,
            false,
            &tmp.path().join("cargo-target"),
            &[],
            &["missing::Component".to_string(), "private_sdk::Component".to_string()],
        )?;
        let source = fs::read_to_string(workspace.join("src/main.rs"))?;
        assert!(!source.contains("missing::Component"));
        assert!(!source.contains("private_sdk::Component"));
        assert!(!source.contains("__IncanDeriveProbe"));
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_workspace_fingerprint_is_deterministic() {
        let requirements = ProjectRequirements::default();
        let resolved = ResolvedDependencies {
            dependencies: vec![DependencySpec {
                crate_name: "serde".to_string(),
                version: Some("1".to_string()),
                features: vec!["derive".to_string()],
                default_features: true,
                source: DependencySource::Registry,
                optional: false,
                package: None,
            }],
            dev_dependencies: Vec::new(),
        };
        let fp_a = super::rust_inspect_workspace_fingerprint(
            "probe",
            "probe",
            Some("2021"),
            &resolved,
            &requirements.stdlib_features,
            &requirements.sdk_dependency_rebindings,
            &requirements.sdk_path_dependencies,
            &requirements.sdk_artifact_projections,
            Some("lock-bytes"),
            None,
            false,
            Path::new("/cache/target"),
            &[],
        );
        let fp_b = super::rust_inspect_workspace_fingerprint(
            "probe",
            "probe",
            Some("2021"),
            &resolved,
            &requirements.stdlib_features,
            &requirements.sdk_dependency_rebindings,
            &requirements.sdk_path_dependencies,
            &requirements.sdk_artifact_projections,
            Some("lock-bytes"),
            None,
            false,
            Path::new("/cache/target"),
            &[],
        );
        let workspace_fp = super::rust_inspect_workspace_fingerprint(
            "probe",
            "incan_workspace",
            Some("2021"),
            &resolved,
            &requirements.stdlib_features,
            &requirements.sdk_dependency_rebindings,
            &requirements.sdk_path_dependencies,
            &requirements.sdk_artifact_projections,
            Some("lock-bytes"),
            None,
            false,
            Path::new("/cache/target"),
            &[],
        );
        let target_fp = super::rust_inspect_workspace_fingerprint(
            "probe",
            "probe",
            Some("2021"),
            &resolved,
            &requirements.stdlib_features,
            &requirements.sdk_dependency_rebindings,
            &requirements.sdk_path_dependencies,
            &requirements.sdk_artifact_projections,
            Some("lock-bytes"),
            None,
            false,
            Path::new("/cache/other-target"),
            &[],
        );
        assert_eq!(fp_a, fp_b);
        assert_ne!(fp_a, workspace_fp);
        assert_ne!(fp_a, target_fp);
        assert!(fp_a.starts_with(super::RUST_INSPECT_WORKSPACE_FINGERPRINT_PREFIX));
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_workspace_fingerprint_changes_when_lock_payload_changes() {
        let requirements = ProjectRequirements::default();
        let resolved = ResolvedDependencies {
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
        };
        let fp_one = super::rust_inspect_workspace_fingerprint(
            "p",
            "p",
            None,
            &resolved,
            &requirements.stdlib_features,
            &requirements.sdk_dependency_rebindings,
            &requirements.sdk_path_dependencies,
            &requirements.sdk_artifact_projections,
            Some("lock-a"),
            None,
            false,
            Path::new("/cache/target"),
            &[],
        );
        let fp_two = super::rust_inspect_workspace_fingerprint(
            "p",
            "p",
            None,
            &resolved,
            &requirements.stdlib_features,
            &requirements.sdk_dependency_rebindings,
            &requirements.sdk_path_dependencies,
            &requirements.sdk_artifact_projections,
            Some("lock-b"),
            None,
            false,
            Path::new("/cache/target"),
            &[],
        );
        assert_ne!(fp_one, fp_two);
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_workspace_fingerprint_changes_when_derive_probes_change() {
        let requirements = ProjectRequirements::default();
        let resolved = ResolvedDependencies {
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
        };
        let fingerprint = |probe: &str| {
            super::rust_inspect_workspace_fingerprint(
                "p",
                "p",
                None,
                &resolved,
                &requirements.stdlib_features,
                &requirements.sdk_dependency_rebindings,
                &requirements.sdk_path_dependencies,
                &requirements.sdk_artifact_projections,
                None,
                None,
                false,
                Path::new("/cache/target"),
                &[probe.to_string()],
            )
        };
        assert_ne!(fingerprint("provider::A"), fingerprint("provider::B"));
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_lock_projection_updates_only_the_local_package_version() -> Result<(), Box<dyn std::error::Error>> {
        let manifest = ProjectManifest::from_str(
            "[project]\nname = \"probe\"\nversion = \"1.2.3\"\n",
            Path::new("probe/loaf.toml"),
        )?;
        let payload = r#"version = 4

[[package]]
name = "probe"
version = "0.1.0"
dependencies = ["serde"]

[[package]]
name = "serde"
version = "1.0.219"
source = "registry+https://example.invalid/index"
checksum = "fixture-checksum"
"#;
        let projected =
            super::project_rust_inspect_lock_projection(Some(payload.to_string()), "probe", Some(&manifest))?
                .ok_or("rust-inspect lock projection unexpectedly removed its lock")?;
        let lock = toml::from_str::<toml::Value>(&projected)?;
        let packages = lock
            .get("package")
            .and_then(toml::Value::as_array)
            .ok_or("projected lock omitted its package array")?;
        let local = packages
            .iter()
            .find(|package| {
                package.get("name").and_then(toml::Value::as_str) == Some("probe") && package.get("source").is_none()
            })
            .ok_or("projected lock omitted its local package")?;
        assert_eq!(local.get("version").and_then(toml::Value::as_str), Some("1.2.3"));
        let registry = packages
            .iter()
            .find(|package| package.get("name").and_then(toml::Value::as_str) == Some("serde"))
            .ok_or("projected lock omitted its registry package")?;
        assert_eq!(registry.get("version").and_then(toml::Value::as_str), Some("1.0.219"));
        assert_eq!(
            registry.get("checksum").and_then(toml::Value::as_str),
            Some("fixture-checksum")
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_workspace_without_a_lock_projection_never_launches_cargo() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let requirements = ProjectRequirements::default();
        let resolved = ResolvedDependencies {
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
        };
        let fingerprint = super::rust_inspect_workspace_fingerprint(
            "policy_probe",
            "caller",
            None,
            &resolved,
            &requirements.stdlib_features,
            &requirements.sdk_dependency_rebindings,
            &requirements.sdk_path_dependencies,
            &requirements.sdk_artifact_projections,
            None,
            None,
            false,
            &tmp.path().join("cargo-target"),
            &[],
        );
        let output_dir = super::rust_inspect_workspace_dir(tmp.path(), "policy_probe", &fingerprint);

        let result = ensure_rust_inspect_workspace_with_cargo_package_name(
            tmp.path(),
            "policy_probe",
            "caller",
            None,
            &resolved,
            &requirements,
            None,
            None,
            false,
            &tmp.path().join("cargo-target"),
            &[],
            &[],
        )?;
        assert_eq!(result, output_dir);
        assert_eq!(
            fs::read_to_string(output_dir.join("Cargo.lock")).ok(),
            None,
            "a Rust-inspect workspace without a compatibility projection must not create Cargo.lock"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_fingerprint_tracks_same_path_projection_rebuild_issue911() -> Result<(), Box<dyn std::error::Error>>
    {
        let workspace = tempfile::tempdir()?;
        let artifact = workspace.path().join("compiled");
        fs::create_dir_all(artifact.join("src"))?;
        fs::write(
            artifact.join("Cargo.toml"),
            "[package]\nname = \"issue911_compiled\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(artifact.join("src/lib.rs"), "pub fn value() -> u8 { 1 }\n")?;
        let requirements = ProjectRequirements {
            sdk_artifact_projections: vec![SdkArtifactProjection {
                artifact: LibraryArtifactMetadata::from_crate_root("issue911_compiled", "issue911_compiled", &artifact),
            }],
            ..ProjectRequirements::default()
        };
        let resolved = ResolvedDependencies {
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
        };
        let before = super::rust_inspect_workspace_fingerprint(
            "probe",
            "probe",
            None,
            &resolved,
            &requirements.stdlib_features,
            &requirements.sdk_dependency_rebindings,
            &requirements.sdk_path_dependencies,
            &requirements.sdk_artifact_projections,
            None,
            None,
            false,
            &workspace.path().join("cargo-target"),
            &[],
        );
        fs::write(artifact.join("src/lib.rs"), "pub fn value() -> u8 { 2 }\n")?;
        let after = super::rust_inspect_workspace_fingerprint(
            "probe",
            "probe",
            None,
            &resolved,
            &requirements.stdlib_features,
            &requirements.sdk_dependency_rebindings,
            &requirements.sdk_path_dependencies,
            &requirements.sdk_artifact_projections,
            None,
            None,
            false,
            &workspace.path().join("cargo-target"),
            &[],
        );

        assert_ne!(before, after);
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_helper_materializes_sdk_projection_issue911() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let artifact = workspace.path().join("compiled");
        let absent_sdk = workspace.path().join("sdk-cache-a/runtime");
        let active_sdk = workspace.path().join("sdk-cache-b/runtime");
        for root in [&artifact, &active_sdk] {
            fs::create_dir_all(root.join("src"))?;
        }
        fs::write(
            artifact.join("Cargo.toml"),
            format!(
                "[package]\nname = \"issue911_compiled\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n\n[dependencies.issue911_runtime]\npath = {:?}\ndefault-features = false\n",
                absent_sdk.to_string_lossy()
            ),
        )?;
        fs::write(
            artifact.join("src/lib.rs"),
            "pub fn value() -> u8 { issue911_runtime::value() }\n",
        )?;
        fs::write(
            active_sdk.join("Cargo.toml"),
            "[package]\nname = \"issue911_runtime\"\nversion = \"0.5.0\"\nedition = \"2024\"\n\n[workspace]\n",
        )?;
        fs::write(active_sdk.join("src/lib.rs"), "pub fn value() -> u8 { 3 }\n")?;
        let mut manifest = LibraryManifest::new("issue911_compiled", "0.1.0");
        manifest.contract_metadata.provider.provider_dependencies.push(
            crate::library_manifest::ProviderDependencyMetadata {
                kind: crate::library_manifest::ProviderDependencyKind::PrivateImplementation,
                dependency_key: "issue911_runtime".to_string(),
                provider_name: "issue911_runtime".to_string(),
                provider_version: "0.5.0".to_string(),
                artifact_digest: digest_provider_artifact(&active_sdk)?,
                relative_artifact_path: "../sdk-cache-a/runtime".to_string(),
                requested_features: BTreeSet::new(),
                default_features: false,
                optional: false,
            },
        );
        let manifest_path = artifact.join("issue911_compiled.incnlib");
        manifest.write_to_path(&manifest_path)?;
        let metadata = LibraryArtifactMetadata::from_manifest_path(
            "issue911_compiled",
            "issue911_compiled",
            manifest_path,
            artifact.clone(),
        );
        let requirements = ProjectRequirements {
            sdk_dependency_rebindings: vec![SdkDependencyRebinding {
                containing_artifact: metadata.clone(),
                source_crate_root: absent_sdk.clone(),
                provider_name: "issue911_runtime".to_string(),
                dependency_key: "issue911_runtime".to_string(),
                active_crate_root: active_sdk,
            }],
            sdk_artifact_projections: vec![SdkArtifactProjection { artifact: metadata }],
            ..ProjectRequirements::default()
        };
        let resolved = ResolvedDependencies {
            dependencies: vec![DependencySpec {
                crate_name: "issue911_compiled".to_string(),
                version: None,
                features: Vec::new(),
                default_features: true,
                source: DependencySource::Path { path: artifact },
                optional: false,
                package: None,
            }],
            dev_dependencies: Vec::new(),
        };

        let generated = ensure_rust_inspect_workspace_with_cargo_package_name(
            workspace.path(),
            "issue911_probe",
            "issue911_probe",
            Some("2024".to_string()),
            &resolved,
            &requirements,
            None,
            None,
            false,
            &workspace.path().join("cargo-target"),
            &[],
            &[],
        )?;

        let cargo_manifest = fs::read_to_string(generated.join("Cargo.toml"))?;
        assert!(cargo_manifest.contains(".incan-sdk-rebound"));
        assert!(!cargo_manifest.contains(absent_sdk.to_string_lossy().as_ref()));
        let projection_parent = generated
            .parent()
            .ok_or("rust-inspect workspace has no parent")?
            .join(".incan-sdk-rebound");
        let shadow_root = fs::read_dir(&projection_parent)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.is_dir())
            .ok_or("missing rust-inspect projected artifact")?;
        let rebound_manifest = toml::from_str::<toml::Value>(&fs::read_to_string(shadow_root.join("Cargo.toml"))?)?;
        let runtime_relative = rebound_manifest
            .get("dependencies")
            .and_then(toml::Value::as_table)
            .and_then(|dependencies| dependencies.get("issue911_runtime"))
            .and_then(toml::Value::as_table)
            .and_then(|dependency| dependency.get("path"))
            .and_then(toml::Value::as_str)
            .ok_or("rust-inspect rebound artifact has no runtime path dependency")?;
        let runtime_root = shadow_root.join(runtime_relative);
        let direct = workspace.path().join("oven-direct-rustc");
        fs::create_dir_all(&direct)?;
        let rustc = std::env::var_os(crate::oven::compiler_suite_env::OVEN_COMPILER_SUITE_RUSTC_ENV)
            .or_else(|| std::env::var_os("RUSTC"))
            .unwrap_or_else(|| "rustc".into());
        let compile = |source: &Path,
                       crate_name: &str,
                       output: &Path,
                       externs: &[(&str, &Path)]|
         -> Result<(), Box<dyn std::error::Error>> {
            let mut command = Command::new(&rustc);
            command
                .arg("--edition")
                .arg("2024")
                .arg("--crate-name")
                .arg(crate_name)
                .arg("--crate-type")
                .arg("lib")
                .arg(source)
                .arg("-o")
                .arg(output)
                .env_remove("CARGO")
                .env_remove("CARGO_MANIFEST_DIR")
                .env_remove("CARGO_MANIFEST_PATH");
            for (name, artifact) in externs {
                let parent = artifact.parent().ok_or_else(|| {
                    format!(
                        "rust-inspect direct artifact for `{name}` has no dependency search directory: {}",
                        artifact.display()
                    )
                })?;
                command
                    .arg("-L")
                    .arg(format!("dependency={}", parent.display()))
                    .arg("--extern")
                    .arg(format!("{name}={}", artifact.display()));
            }
            let result = command.output()?;
            if result.status.success() {
                return Ok(());
            }
            Err(format!(
                "rust-inspect direct-Rustc fixture compilation for `{crate_name}` failed:\n{}{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            )
            .into())
        };
        let runtime = direct.join("libissue911_runtime.rlib");
        let compiled = direct.join("libissue911_compiled.rlib");
        let probe = direct.join("libissue911_probe.rlib");
        compile(&runtime_root.join("src/lib.rs"), "issue911_runtime", &runtime, &[])?;
        compile(
            &shadow_root.join("src/lib.rs"),
            "issue911_compiled",
            &compiled,
            &[("issue911_runtime", runtime.as_path())],
        )?;
        compile(
            &generated.join("src/main.rs"),
            "issue911_probe",
            &probe,
            &[
                ("issue911_compiled", compiled.as_path()),
                ("issue911_runtime", runtime.as_path()),
            ],
        )?;
        fs::write(shadow_root.join("src/lib.rs"), "pub fn corrupt() {}\n")?;
        let regenerated = ensure_rust_inspect_workspace_with_cargo_package_name(
            workspace.path(),
            "issue911_probe",
            "issue911_probe",
            Some("2024".to_string()),
            &resolved,
            &requirements,
            None,
            None,
            false,
            &workspace.path().join("cargo-target"),
            &[],
            &[],
        )?;
        assert_eq!(generated, regenerated);
        assert!(fs::read_to_string(shadow_root.join("src/lib.rs"))?.contains("value"));
        assert!(!absent_sdk.exists());
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_workspace_dir_is_namespaced_by_input_fingerprint() {
        let root = Path::new("/workspace");
        let first = super::rust_inspect_workspace_dir(root, "demo", "v1:aaaaaaaaaaaaaaaaaaaaaaaa");
        let second = super::rust_inspect_workspace_dir(root, "demo", "v1:bbbbbbbbbbbbbbbbbbbbbbbb");

        assert_ne!(first, second);
        assert!(first.ends_with(Path::new("target/incan_lock/rust_inspect/demo-aaaaaaaaaaaaaaaa")));
        assert!(second.ends_with(Path::new("target/incan_lock/rust_inspect/demo-bbbbbbbbbbbbbbbb")));
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_out_dirs_fingerprint_tracks_query_surface() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let manifest_dir = tmp.path();
        fs::create_dir_all(manifest_dir.join("src"))?;
        fs::write(
            manifest_dir.join("Cargo.toml"),
            "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        fs::write(manifest_dir.join("src").join("main.rs"), "fn main() {}\n")?;
        let target_dir = manifest_dir.join("target");

        let one = super::rust_inspect_out_dirs_fingerprint(manifest_dir, &target_dir, &["demo::One".to_string()])?;
        let two = super::rust_inspect_out_dirs_fingerprint(manifest_dir, &target_dir, &["demo::Two".to_string()])?;

        assert_ne!(
            one, two,
            "rust-inspect out-dir prewarm must rerun when the inspected ABI query surface changes"
        );
        assert!(one.starts_with(super::RUST_INSPECT_OUT_DIRS_FINGERPRINT_FILE));
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_locked_prewarm_detects_stale_generated_lockfile() {
        let cannot_update = "error: cannot update the lock file /tmp/target/incan_lock/rust_inspect/demo/Cargo.lock because --locked was passed to prevent this";
        assert!(super::rust_inspect_locked_prewarm_needs_lock_update(cannot_update));

        let needs_update = "error: the lock file /tmp/target/incan_lock/rust_inspect/demo/Cargo.lock needs to be updated but --locked was passed to prevent this";
        assert!(super::rust_inspect_locked_prewarm_needs_lock_update(needs_update));

        assert!(!super::rust_inspect_locked_prewarm_needs_lock_update(
            "error: failed to select a version for `demo`"
        ));
        assert!(!super::rust_inspect_locked_prewarm_needs_lock_update(
            "error: package selected but no lock file policy was involved"
        ));
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn ensure_rust_inspect_workspace_uses_rust_safe_dependency_keys() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let requirements = ProjectRequirements::default();
        let resolved = ResolvedDependencies {
            dependencies: vec![DependencySpec {
                crate_name: "datafusion-substrait".to_string(),
                version: Some("53".to_string()),
                features: vec!["protoc".to_string()],
                default_features: true,
                source: DependencySource::Registry,
                optional: false,
                package: None,
            }],
            dev_dependencies: Vec::new(),
        };

        let out_dir = ensure_rust_inspect_workspace(
            tmp.path(),
            "metadata_probe",
            Some("2021".to_string()),
            &resolved,
            &requirements,
            Some("[[package]]\nname = \"metadata_probe\"\n".to_string()),
            &tmp.path().join("cargo-target"),
            &[],
        )?;
        assert_eq!(
            super::test_rust_inspect_workspace_generations(&out_dir),
            1,
            "expected one rust-inspect workspace generation"
        );

        let cargo_toml = fs::read_to_string(out_dir.join("Cargo.toml"))?;
        let cargo_lock = fs::read_to_string(out_dir.join("Cargo.lock"))?;
        let main_rs = fs::read_to_string(out_dir.join("src").join("main.rs"))?;

        assert!(
            cargo_toml.contains("[dependencies.datafusion_substrait]"),
            "expected rust-safe dependency key in generated rust-inspect workspace, got:\n{cargo_toml}"
        );
        assert!(
            cargo_toml.contains("package = \"datafusion-substrait\""),
            "expected original package name preserved in generated rust-inspect workspace, got:\n{cargo_toml}"
        );
        assert!(
            cargo_lock.contains("metadata_probe"),
            "expected rust-inspect workspace to write the provided Cargo.lock payload"
        );
        assert!(
            main_rs.contains("use datafusion_substrait as _;"),
            "expected rust-inspect workspace stub to reference the aliased dependency crate, got:\n{main_rs}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn ensure_rust_inspect_workspace_skips_regeneration_when_unchanged() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let requirements = ProjectRequirements::default();
        let resolved = ResolvedDependencies {
            dependencies: vec![DependencySpec {
                crate_name: "serde".to_string(),
                version: Some("1".to_string()),
                features: Vec::new(),
                default_features: true,
                source: DependencySource::Registry,
                optional: false,
                package: None,
            }],
            dev_dependencies: Vec::new(),
        };
        let lock = Some("[[package]]\nname = \"skip_probe\"\n".to_string());

        let out_dir = ensure_rust_inspect_workspace(
            tmp.path(),
            "skip_probe",
            Some("2021".to_string()),
            &resolved,
            &requirements,
            lock.clone(),
            &tmp.path().join("cargo-target"),
            &[],
        )?;
        assert_eq!(
            super::test_rust_inspect_workspace_generations(&out_dir),
            1,
            "first call should generate the workspace"
        );

        ensure_rust_inspect_workspace(
            tmp.path(),
            "skip_probe",
            Some("2021".to_string()),
            &resolved,
            &requirements,
            lock,
            &tmp.path().join("cargo-target"),
            &[],
        )?;
        assert_eq!(
            super::test_rust_inspect_workspace_generations(&out_dir),
            1,
            "second call with identical inputs should skip regeneration"
        );

        Ok(())
    }
}
