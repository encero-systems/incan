//! Canonical explicit lock preparation in the existing executable owner.
//!
//! Resolution and publication stay in `crate::lock`; this host adapter retains their successful evidence beside the
//! original session. It adds no resolver, source detector or persisted cache.

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
    /// Exact reconciled inputs and writer-owned bytes must still match; a changed lock never authorizes reuse.
    fn matches(&self, request: &str, source: &str) -> bool {
        self.request == request
            && self.source == source
            && self
                .test_discovery
                .as_ref()
                .is_some_and(|inputs| inputs.supports_retention() && inputs.current())
            && super::file_freshness::digest_file(&self.publication.canonical_path)
                .is_ok_and(|digest| digest == self.publication.content_digest)
    }
}

/// Retain only source entries covered by the existing project reconciler, resolving aliases before containment.
fn entries_belong_to_project(root: &Path, entries: &[PathBuf]) -> bool {
    entries
        .iter()
        .all(|entry| std::fs::canonicalize(entry).is_ok_and(|path| path.starts_with(root)))
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
    // The current source reconciler owns standalone project trees. Preserve canonical collection for workspace
    // members and external entrypoints rather than rejecting an existing lock command or retaining partial inputs.
    if workspace.is_some() || !entries_owned {
        crate::lock::resolution::prepare_sdk_and_publish_project_lock(
            &manifest,
            request.entry.as_deref(),
            &request.cargo_features,
            &request.features,
            request.sdk_profile.as_deref(),
        )?;
        return Ok(());
    }
    let entry = entries.into_iter().next().ok_or_else(|| {
        CliError::failure("incan lock requires a FILE argument or at least one [project.scripts] entry")
    })?;
    // ---- Reuse: reconcile the original session and successful publication ----
    let _inventory = super::native_sdk::prepare_or_discover_sdk_inventory()?;
    authority.begin_request();
    let session = match authority.compilation_session(
        &root,
        &entry,
        &request.features,
        request.sdk_profile.as_deref(),
        SessionPurpose::Artifacts,
    ) {
        Ok(session) => session,
        Err(reason) => {
            // Session identity also reconciles source topology. Canonical discovery owns unsupported layouts
            // and their diagnostics; an acceleration failure must neither reject nor bless such a request.
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
    let selection = serde_json::to_string(request).map_err(|error| CliError::failure(error.to_string()))?;
    if let Some(slot) = authority.retained_inputs.lock_preparation(&root, &session) {
        if entries_owned
            && slot.as_ref().is_some_and(|retained| {
                source
                    .as_deref()
                    .is_some_and(|source| retained.matches(&selection, source))
            })
        {
            tracing::trace!(target: "incan::invocation", decision = "reuse_lock_preparation", input = %root.display());
            return Ok(());
        }
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
    let Some(source) = source else {
        return Ok(());
    };
    authority.begin_request();
    let Ok(current) = authority.digest(&root) else {
        return Ok(());
    };
    let completed = CompletedLockPreparation {
        request: selection,
        test_discovery: context.test_discovery,
        source,
        publication,
    };
    // A newly published semantic generation or racing input is not retained as the earlier generation. The next
    // request reconciles it normally. Explicit external entrypoints also keep canonical collection.
    if current == completed.source
        && entries_owned
        && completed.matches(&completed.request, &current)
        && let Some(slot) = authority.retained_inputs.lock_preparation(&root, &session)
    {
        *slot = Some(completed);
    }
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

    /// Original publication bytes, source identity and command selection independently gate a retained hit.
    #[test]
    fn changed_lock_or_request_cannot_reuse_preparation() -> Result<(), Box<dyn std::error::Error>> {
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
        assert!(completed.matches("original request", "original source"));
        assert!(!completed.matches("changed request", "original source"));
        assert!(!completed.matches("original request", "changed source"));
        std::fs::write(&path, "modified publication")?;
        assert!(!completed.matches("original request", "original source"));
        std::fs::write(&path, "original publication")?;
        assert!(completed.matches("original request", "original source"));
        std::fs::remove_file(&path)?;
        assert!(!completed.matches("original request", "original source"));
        Ok(())
    }
}
