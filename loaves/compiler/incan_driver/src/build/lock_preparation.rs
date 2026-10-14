//! Canonical explicit lock preparation in the existing executable owner.
//!
//! Resolution and publication stay in `crate::lock`. Incan's `lock_preparation` policy selects fallback, reuse and
//! retention; this host observes physical inputs and executes those operations with the original session.

mod policy;

use super::retained_inputs::SessionPurpose;
use crate::error::{CliError, CliResult};
use incan_provider::FeatureSelection;
use oven_model::lock::CargoFeatureSelection;
use oven_model::manifest::ProjectManifest;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Existing lock selections transported to the same owner as build, check and tests.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockPreparationRequest {
    /// Absolute discovery directory; the owner inherits the command's original working directory.
    pub directory: PathBuf,
    /// Optional explicit source entrypoint, normalized by the client before transport.
    pub entry: Option<PathBuf>,
    /// Existing package feature selection.
    pub features: FeatureSelection,
    /// Existing provider profile override.
    pub sdk_profile: Option<String>,
    /// Existing Rust feature selection; workspace semantics remain in the canonical collector.
    pub cargo_features: CargoFeatureSelection,
}

/// Successful canonical publication retained inside its original artifact session.
pub(super) struct CompletedLockPreparation {
    request: String,
    source: String,
    test_discovery: Option<oven_model::observed_inputs::InputObservations>,
    publication: crate::lock::PublishedLockFile,
}

impl CompletedLockPreparation {
    /// Report only the original discovery observer's physical result; Incan combines it with semantic inputs.
    fn discovery_current(&self) -> bool {
        self.test_discovery
            .as_ref()
            .is_some_and(|inputs| inputs.supports_retention() && inputs.current())
    }

    /// Report whether the original writer-owned bytes still exist unchanged, including restoration after damage.
    fn publication_current(&self) -> bool {
        super::file_freshness::digest_file(&self.publication.canonical_path)
            .is_ok_and(|digest| digest == self.publication.content_digest)
    }
}

/// Retain only source entries covered by the existing project reconciler, resolving aliases before containment.
fn entries_belong_to_project(root: &Path, entries: &[PathBuf]) -> bool {
    entries
        .iter()
        .all(|entry| std::fs::canonicalize(entry).is_ok_and(|path| path.starts_with(root)))
}

/// Capture the original retained slot's observations without interpreting whether they permit reuse.
fn reuse_facts(request: &str, source: Option<&str>, retained: Option<&CompletedLockPreparation>) -> serde_json::Value {
    serde_json::json!({
        "session_available": true, "request": request, "source": source,
        "retained_request": retained.map(|value| &value.request),
        "retained_source": retained.map(|value| &value.source),
        "discovery_current": retained.is_some_and(CompletedLockPreparation::discovery_current),
        "publication_current": retained.is_some_and(CompletedLockPreparation::publication_current),
    })
}

/// Capture physical post-publication evidence; the Incan policy owns generation and retention decisions.
fn retention_facts(
    source_before: Option<&str>,
    source_after: Option<&str>,
    discovery: Option<&oven_model::observed_inputs::InputObservations>,
    publication: &crate::lock::PublishedLockFile,
    session_bound: bool,
) -> serde_json::Value {
    serde_json::json!({
        "source_before": source_before, "source_after": source_after,
        "discovery_current": discovery.is_some_and(|inputs| inputs.supports_retention() && inputs.current()),
        "publication_current": super::file_freshness::digest_file(&publication.canonical_path)
            .is_ok_and(|digest| digest == publication.content_digest),
        "session_bound": session_bound,
    })
}

/// Run the existing collector once per unchanged standalone session and retain only stable successful publications.
/// Workspace collection keeps its canonical whole-workspace path until member sessions can be retained together.
pub(super) fn execute_request(
    request: &LockPreparationRequest,
    authority: &mut super::ProjectSourceAuthorityDigester,
) -> CliResult<()> {
    let _trace = tracing::trace_span!(target: "incan::invocation", "incan_driver::prepare_project_lock").entered();
    // ---- Request authority: paths, project and source ownership ----
    if !request.directory.is_absolute() || request.entry.as_ref().is_some_and(|path| !path.is_absolute()) {
        return Err(CliError::failure("owner requires absolute lock request paths"));
    }
    let manifest = ProjectManifest::discover(&request.directory)
        .map_err(|error| CliError::failure(error.to_string()))?
        .ok_or_else(|| CliError::failure("No loaf.toml found (run `incan init`)"))?;
    crate::cargo_policy::enforce_project_toolchain_constraint(&manifest)?;
    let root = std::fs::canonicalize(manifest.project_root()).map_err(|error| CliError::failure(error.to_string()))?;
    let workspace =
        oven_model::workspace::WorkspaceGraph::discover(&root).map_err(|error| CliError::failure(error.to_string()))?;
    let entries = crate::lock::workspace::project_lock_entry_paths(&manifest, request.entry.as_deref());
    let entries_owned = entries_belong_to_project(&root, &entries);
    match policy::decide(
        "route",
        &serde_json::json!({
            "paths_absolute": true, "workspace": workspace.is_some(),
            "entries_owned": entries_owned, "has_entry": !entries.is_empty(),
        }),
    )? {
        policy::Action::CanonicalPrepared => {
            crate::lock::resolution::prepare_sdk_and_publish_project_lock(
                &manifest,
                request.entry.as_deref(),
                &request.cargo_features,
                &request.features,
                request.sdk_profile.as_deref(),
            )?;
            return Ok(());
        }
        policy::Action::MissingEntry => {
            return Err(CliError::failure(
                "incan lock requires a FILE argument or at least one [project.scripts] entry",
            ));
        }
        policy::Action::Observe => {}
        _ => return Err(CliError::failure("lock policy returned an invalid route operation")),
    }
    let entry = entries.into_iter().next().ok_or_else(|| {
        CliError::failure("incan lock requires a FILE argument or at least one [project.scripts] entry")
    })?;
    // ---- Reuse: reconcile the original session and successful publication ----
    let _inventory = super::native_sdk::prepare_or_discover_sdk_inventory()?;
    authority.begin_request();
    let selection = serde_json::to_string(request).map_err(|error| CliError::failure(error.to_string()))?;
    let session = match authority.compilation_session(
        &root,
        &entry,
        &request.features,
        request.sdk_profile.as_deref(),
        SessionPurpose::Artifacts,
    ) {
        Ok(session) => session,
        Err(reason) => {
            match policy::decide(
                "reuse",
                &serde_json::json!({
                    "session_available": false, "request": selection, "source": null,
                    "retained_request": null, "retained_source": null,
                    "discovery_current": false, "publication_current": false,
                }),
            )? {
                policy::Action::CanonicalCollect => {}
                _ => {
                    return Err(CliError::failure(
                        "lock policy returned an invalid unavailable-session operation",
                    ));
                }
            }
            tracing::trace!(target: "incan::invocation", decision = "lock_session_unretained", reason = %reason.message);
            crate::lock::resolution::collect_and_publish_project_lock(
                &manifest,
                request.entry.as_deref(),
                &request.cargo_features,
                &request.features,
                request.sdk_profile.as_deref(),
            )?;
            return Ok(());
        }
    };
    // Source reconciliation is an acceleration boundary, not a new restriction on canonical lock inputs.
    // For example, the collector accepts linked modules that the build source digester cannot retain safely.
    let source = authority.digest(&root).ok();
    let retained = authority
        .retained_inputs
        .lock_preparation(&root, &session)
        .and_then(|slot| slot.as_ref());
    let facts = reuse_facts(&selection, source.as_deref(), retained);
    let action = policy::decide("reuse", &facts)?;
    // The external policy can authorize only the observations it received. Reconcile through the original
    // observer after the exchange as well, refusing a changed request rather than applying a stale answer.
    authority.begin_request();
    let current = authority.digest(&root).ok();
    let slot = authority.retained_inputs.lock_preparation(&root, &session);
    if reuse_facts(&selection, current.as_deref(), slot.as_deref().and_then(Option::as_ref)) != facts {
        if let Some(slot) = slot {
            *slot = None;
        }
        return Err(CliError::failure(
            "lock inputs changed during policy evaluation; retry the command",
        ));
    }
    match action {
        policy::Action::Reuse => {
            tracing::trace!(target: "incan::invocation", decision = "reuse_lock_preparation", input = %root.display());
            return Ok(());
        }
        policy::Action::Publish => {}
        _ => {
            return Err(CliError::failure(
                "lock policy returned an invalid publication operation",
            ));
        }
    }
    if let Some(slot) = authority.retained_inputs.lock_preparation(&root, &session) {
        *slot = None;
    }
    // ---- Publication: retain only an unchanged, successful generation ----
    let (context, publication) = crate::lock::resolution::collect_and_publish_project_lock_with_session(
        &manifest,
        request.entry.as_deref(),
        &request.cargo_features,
        &request.features,
        request.sdk_profile.as_deref(),
        Some(&session),
    )?;
    authority.begin_request();
    let current = authority.digest(&root).ok();
    let facts = retention_facts(
        source.as_deref(),
        current.as_deref(),
        context.test_discovery.as_ref(),
        &publication,
        authority.retained_inputs.lock_preparation(&root, &session).is_some(),
    );
    let action = policy::decide("retain", &facts)?;
    authority.begin_request();
    let current = authority.digest(&root).ok();
    let slot = authority.retained_inputs.lock_preparation(&root, &session);
    if retention_facts(
        source.as_deref(),
        current.as_deref(),
        context.test_discovery.as_ref(),
        &publication,
        slot.is_some(),
    ) != facts
    {
        return Err(CliError::failure(
            "lock publication inputs changed during policy evaluation; retry the command",
        ));
    }
    match action {
        policy::Action::Complete => return Ok(()),
        policy::Action::Retain => {}
        _ => return Err(CliError::failure("lock policy returned an invalid retention operation")),
    }
    let source = source.ok_or_else(|| CliError::failure("lock policy retained unavailable source evidence"))?;
    let slot = slot.ok_or_else(|| CliError::failure("lock policy retained an unbound session"))?;
    let completed = CompletedLockPreparation {
        request: selection,
        test_discovery: context.test_discovery,
        source,
        publication,
    };
    *slot = Some(completed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::CompletedLockPreparation;
    use crate::lock::PublishedLockFile;

    /// Observe through the canonical discovery walk rather than constructing a parallel test-file inventory.
    fn observe_test_inputs(root: &std::path::Path) -> oven_model::observed_inputs::InputObservations {
        let mut inputs = oven_model::observed_inputs::InputObservations::default();
        crate::testing::discovery::discover_test_file_candidates_observed(root, Some(&mut inputs));
        inputs
    }

    /// Test edits outside src, inline-test candidates and new/deleted files all invalidate retained collection.
    #[cfg(unix)]
    #[test]
    fn test_discovery_observes_edits_additions_and_removals() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = std::fs::canonicalize(directory.path())?;
        std::fs::write(root.join("loaf.toml"), "[project]\nname = 'probe'\n")?;
        std::fs::create_dir(root.join("tests"))?;
        let test = root.join("tests/test_value.incn");
        std::fs::write(&test, "def test_value() -> None: pass")?;
        // ---- Content edits, including restored modification times ----
        let original = std::fs::metadata(&test)?;
        let inputs = observe_test_inputs(&root);
        assert!(inputs.supports_retention() && inputs.current());
        std::fs::write(&test, "def test_other() -> None: pass")?;
        std::fs::File::options()
            .write(true)
            .open(&test)?
            .set_times(std::fs::FileTimes::new().set_modified(original.modified()?))?;
        assert!(!inputs.current());
        let inputs = observe_test_inputs(&root);
        // ---- New inline candidates and removed named tests ----
        let inline = root.join("ordinary.incn");
        std::fs::write(&inline, "def value() -> int: return 1")?;
        assert!(!inputs.current());
        let inputs = observe_test_inputs(&root);
        std::fs::write(&inline, "module tests:\n    def test_new() -> None: pass\n")?;
        assert!(!inputs.current());
        let inputs = observe_test_inputs(&root);
        std::fs::remove_file(&test)?;
        assert!(!inputs.current());
        Ok(())
    }

    /// Nested project boundaries are inputs, while ignored targets and unrelated runtime bytes are not test sources.
    #[cfg(unix)]
    #[test]
    fn test_discovery_observes_boundaries_without_runtime_data() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = std::fs::canonicalize(directory.path())?;
        std::fs::write(root.join("loaf.toml"), "[project]\nname = 'probe'\n")?;
        std::fs::create_dir(root.join("nested"))?;
        std::fs::create_dir(root.join("target"))?;
        let data = root.join("data.txt");
        std::fs::write(&data, "first")?;
        let inputs = observe_test_inputs(&root);
        std::fs::write(&data, "other")?;
        std::fs::write(root.join("target/test_ignored.incn"), "invalid")?;
        assert!(inputs.current());
        std::fs::write(root.join("nested/loaf.toml"), "[project]\nname = 'nested'\n")?;
        assert!(!inputs.current());
        let inputs = observe_test_inputs(&root);
        std::fs::remove_file(root.join("nested/loaf.toml"))?;
        assert!(!inputs.current());
        Ok(())
    }

    /// Existing discovery still follows linked tests, but an unobserved target never permits lock reuse.
    #[cfg(unix)]
    #[test]
    fn linked_test_discovery_declines_retention() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = std::fs::canonicalize(directory.path())?;
        std::fs::write(root.join("loaf.toml"), "[project]\nname = 'probe'\n")?;
        let target = root.join("source.incn");
        std::fs::write(&target, "def test_value() -> None: pass")?;
        std::os::unix::fs::symlink(&target, root.join("test_link.incn"))?;
        assert!(!observe_test_inputs(&root).supports_retention());
        Ok(())
    }

    /// A missing linked directory may later acquire tests without changing the parent project's metadata.
    #[cfg(unix)]
    #[test]
    fn broken_test_directory_link_declines_retention() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = std::fs::canonicalize(directory.path())?;
        std::fs::write(root.join("loaf.toml"), "[project]\nname = 'probe'\n")?;
        std::os::unix::fs::symlink(root.join("absent"), root.join("aliased_tests"))?;
        assert!(!observe_test_inputs(&root).supports_retention());
        Ok(())
    }

    /// A failed inline-source read is not evidence that the file contains no tests.
    #[test]
    fn unreadable_inline_candidate_declines_retention() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = std::fs::canonicalize(directory.path())?;
        std::fs::write(root.join("loaf.toml"), "[project]\nname = 'probe'\n")?;
        std::fs::write(root.join("module.incn"), [0xff])?;
        assert!(!observe_test_inputs(&root).supports_retention());
        Ok(())
    }

    /// Every collected entry must be observed: a later external script or a disappearing entry declines retention.
    #[test]
    fn all_entrypoints_must_belong_to_observed_project() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = std::fs::canonicalize(directory.path())?.join("project");
        std::fs::create_dir(&root)?;
        let local = root.join("main.incn");
        let external = root.parent().ok_or("fixture root has no parent")?.join("external.incn");
        std::fs::write(&local, "def main() -> None: pass")?;
        std::fs::write(&external, "def main() -> None: pass")?;
        assert!(super::entries_belong_to_project(&root, std::slice::from_ref(&local)));
        assert!(!super::entries_belong_to_project(&root, &[local.clone(), external]));
        std::fs::remove_file(&local)?;
        assert!(!super::entries_belong_to_project(&root, &[local]));
        Ok(())
    }

    /// A lexical child path cannot hide an entrypoint whose contents live outside the observed project.
    #[cfg(unix)]
    #[test]
    fn linked_external_entrypoint_declines_retention() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = std::fs::canonicalize(directory.path())?.join("project");
        std::fs::create_dir(&root)?;
        let external = root.parent().ok_or("fixture root has no parent")?.join("external.incn");
        std::fs::write(&external, "def main() -> None: pass")?;
        let link = root.join("main.incn");
        std::os::unix::fs::symlink(&external, &link)?;
        assert!(!super::entries_belong_to_project(&root, &[link]));
        Ok(())
    }

    /// Original publication bytes are observed without replacing the source and request decisions owned by Incan.
    #[test]
    fn changed_lock_bytes_are_observed_before_policy() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let path = project.path().join("oven.lock");
        std::fs::write(&path, "original publication")?;
        let completed = CompletedLockPreparation {
            request: "original request".to_string(),
            test_discovery: Some(oven_model::observed_inputs::InputObservations::default()),
            source: "original source".to_string(),
            publication: PublishedLockFile {
                canonical_path: path.clone(),
                content_digest: oven_store::digest_bytes(b"original publication"),
            },
        };
        assert!(completed.publication_current());
        std::fs::write(&path, "modified publication")?;
        assert!(!completed.publication_current());
        std::fs::write(&path, "original publication")?;
        assert!(completed.publication_current());
        std::fs::remove_file(&path)?;
        assert!(!completed.publication_current());
        Ok(())
    }
}
