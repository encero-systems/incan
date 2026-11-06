//! Canonical explicit lock preparation in the existing executable owner.
//!
//! Resolution and publication stay in `crate::lock`; this host adapter retains their successful evidence beside the
//! original session. It adds no resolver, source detector or persisted cache.

mod policy;

use super::retained_inputs::SessionPurpose;
use crate::error::{CliError, CliResult};
use crate::session::CompilationSession;
use incan_provider::FeatureSelection;
use oven_model::lock::CargoFeatureSelection;
use oven_model::manifest::ProjectManifest;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    test_discovery: Vec<oven_model::observed_inputs::InputObservations>,
    members: Vec<(PathBuf, Arc<CompilationSession>)>,
    publication: crate::lock::PublishedLockFile,
}

impl CompletedLockPreparation {
    /// Report only the original discovery observer's physical result; Incan combines it with semantic inputs.
    fn discovery_current(&self) -> bool {
        !self.test_discovery.is_empty()
            && self
                .test_discovery
                .iter()
                .all(|inputs| inputs.supports_retention() && inputs.current())
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
    discovery: &[oven_model::observed_inputs::InputObservations],
    publication: &crate::lock::PublishedLockFile,
    session_bound: bool,
) -> serde_json::Value {
    serde_json::json!({
        "source_before": source_before, "source_after": source_after,
        "discovery_current": !discovery.is_empty() && discovery.iter().all(|inputs| inputs.supports_retention() && inputs.current()),
        "publication_current": super::file_freshness::digest_file(&publication.canonical_path)
            .is_ok_and(|digest| digest == publication.content_digest),
        "session_bound": session_bound,
    })
}

/// Compare original owner generations after every member has been reconciled for this request.
fn same_member_sessions(
    previous: &[(PathBuf, Arc<CompilationSession>)],
    current: &[(PathBuf, Arc<CompilationSession>)],
) -> bool {
    previous.len() == current.len()
        && previous
            .iter()
            .zip(current)
            .all(|((left_root, left), (right_root, right))| left_root == right_root && Arc::ptr_eq(left, right))
}

/// Bind every selected member's source and local dependency graph using the existing reconciler.
fn source_identity(roots: &[PathBuf], authority: &mut super::ProjectSourceAuthorityDigester) -> CliResult<String> {
    let identities = roots
        .iter()
        .map(|root| authority.digest(root).map(|digest| (root, digest)))
        .collect::<CliResult<Vec<_>>>()?;
    serde_json::to_vec(&identities)
        .map(|bytes| oven_store::digest_bytes(&bytes))
        .map_err(|error| CliError::failure(error.to_string()))
}

/// Match the canonical workspace lock's scope: member location and transient package flags do not select its graph.
/// An explicit entry remains bound to its invoking member; standalone requests retain their ordinary selections.
fn selection_identity(
    request: &LockPreparationRequest,
    project_root: &Path,
    workspace: Option<&oven_model::workspace::WorkspaceGraph>,
) -> CliResult<String> {
    let bytes = match workspace {
        Some(workspace) => serde_json::to_string(&(
            workspace.root(),
            request.entry.as_ref().map(|entry| (project_root, entry)),
            &request.cargo_features,
            &request.sdk_profile,
        )),
        None => serde_json::to_string(request),
    };
    bytes.map_err(|error| CliError::failure(error.to_string()))
}

/// Retain the canonical publication only when every contributing member and its original session remain current.
/// Unsupported source ownership keeps ordinary collection; no partial workspace generation can authorize reuse.
pub(super) fn execute_request(
    request: &LockPreparationRequest,
    authority: &mut super::ProjectSourceAuthorityDigester,
) -> CliResult<()> {
    let _trace = tracing::trace_span!(target: "incan::invocation", "incan_driver::prepare_project_lock").entered();
    // ---- Request authority: paths, original project model and whole workspace ----
    if !request.directory.is_absolute() || request.entry.as_ref().is_some_and(|path| !path.is_absolute()) {
        return Err(CliError::failure("owner requires absolute lock request paths"));
    }
    let manifest = ProjectManifest::discover(&request.directory)
        .map_err(|error| CliError::failure(error.to_string()))?
        .ok_or_else(|| CliError::failure("No loaf.toml found (run `incan init`)"))?;
    crate::cargo_policy::enforce_project_toolchain_constraint(&manifest)?;
    let root = std::fs::canonicalize(manifest.project_root()).map_err(|error| CliError::failure(error.to_string()))?;
    let canonical = || {
        crate::lock::resolution::prepare_sdk_and_publish_project_lock(
            &manifest,
            request.entry.as_deref(),
            &request.cargo_features,
            &request.features,
            request.sdk_profile.as_deref(),
        )
        .map(|_| ())
    };
    authority.begin_request();
    let project = match authority.retained_inputs.project(&root) {
        Ok(project) => project,
        Err(_) => return canonical(),
    };
    let workspace = project.workspace();
    let manifests = match workspace {
        Some(workspace) => workspace
            .members()
            .map(|member| workspace.effective_member_manifest(member))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| CliError::failure(error.to_string()))?,
        None => vec![project.manifest.clone()],
    };
    let roots = manifests
        .iter()
        .map(|manifest| manifest.project_root().to_path_buf())
        .collect::<Vec<_>>();
    let declared_features = FeatureSelection::default();
    let features = if workspace.is_some() {
        &declared_features
    } else {
        &request.features
    };
    let _inventory = super::native_sdk::prepare_or_discover_sdk_inventory()?;
    let mut members = Vec::new();
    for manifest in &manifests {
        crate::cargo_policy::enforce_project_toolchain_constraint(manifest)?;
        let member_root = manifest.project_root();
        let entry = request.entry.as_deref().filter(|_| member_root == root);
        let entries = crate::lock::workspace::project_lock_entry_paths(manifest, entry);
        match policy::decide(
            "route",
            &serde_json::json!({
                "paths_absolute": true, "workspace": workspace.is_some(),
                "entries_owned": entries_belong_to_project(member_root, &entries),
                "has_entry": !entries.is_empty(),
            }),
        )? {
            policy::Action::CanonicalPrepared => return canonical(),
            policy::Action::MissingEntry => {
                return Err(CliError::failure(
                    "incan lock requires a FILE argument or at least one [project.scripts] entry",
                ));
            }
            policy::Action::Observe => {}
            _ => return Err(CliError::failure("lock policy returned an invalid route operation")),
        }
        let Some(entry) = entries.first() else {
            return canonical();
        };
        let session = match authority.compilation_session(
            member_root,
            entry,
            features,
            request.sdk_profile.as_deref(),
            SessionPurpose::Artifacts,
        ) {
            Ok(session) => session,
            Err(reason) => {
                tracing::trace!(target: "incan::invocation", decision = "lock_session_unretained", reason = %reason.message);
                return canonical();
            }
        };
        members.push((member_root.to_path_buf(), session));
    }
    let Some(session) = members
        .iter()
        .find(|(member, _)| member == &root)
        .map(|(_, session)| Arc::clone(session))
    else {
        return canonical();
    };
    // ---- Reuse: every member, test-discovery surface and original session must agree ----
    let source = source_identity(&roots, authority).ok();
    let selection = selection_identity(request, &root, workspace)?;
    // Every workspace member reconciles the same ordered session set. Keep one publication in its first original
    // member session so changing the command's working directory cannot create a second workspace result.
    let (publication_root, publication_session) = members
        .first()
        .map(|(root, session)| (root.clone(), Arc::clone(session)))
        .ok_or_else(|| CliError::failure("lock preparation lost its admitted member session"))?;
    authority.retained_inputs.ensure_current_project(&root, &project)?;
    let retained = authority
        .retained_inputs
        .lock_preparation(&publication_root, &publication_session)
        .and_then(|slot| slot.as_ref())
        .filter(|retained| same_member_sessions(&retained.members, &members));
    let facts = reuse_facts(&selection, source.as_deref(), retained);
    let action = policy::decide("reuse", &facts)?;
    authority.begin_request();
    let current = source_identity(&roots, authority).ok();
    authority.retained_inputs.ensure_current_project(&root, &project)?;
    let slot = authority
        .retained_inputs
        .lock_preparation(&publication_root, &publication_session);
    let retained = slot
        .as_deref()
        .and_then(Option::as_ref)
        .filter(|retained| same_member_sessions(&retained.members, &members));
    if reuse_facts(&selection, current.as_deref(), retained) != facts {
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
    if let Some(slot) = slot {
        *slot = None;
    }
    // ---- Publication: preserve the canonical member feature and root-lock policies ----
    let (context, publication) = match workspace {
        Some(workspace) => crate::lock::resolution::collect_and_publish_workspace_lock_with_sessions(
            workspace,
            &root,
            request.entry.as_deref(),
            &request.cargo_features,
            request.sdk_profile.as_deref(),
            &members,
        )?,
        None => crate::lock::resolution::collect_and_publish_project_lock_with_session(
            &manifest,
            request.entry.as_deref(),
            &request.cargo_features,
            &request.features,
            request.sdk_profile.as_deref(),
            Some(&session),
        )?,
    };
    authority.begin_request();
    let current = source_identity(&roots, authority).ok();
    let model_current = authority
        .retained_inputs
        .ensure_current_project(&root, &project)
        .is_ok();
    let facts = retention_facts(
        source.as_deref(),
        current.as_deref(),
        &context.test_discovery,
        &publication,
        model_current
            && authority
                .retained_inputs
                .lock_preparation(&publication_root, &publication_session)
                .is_some(),
    );
    let action = policy::decide("retain", &facts)?;
    authority.begin_request();
    let current = source_identity(&roots, authority).ok();
    let model_current = authority
        .retained_inputs
        .ensure_current_project(&root, &project)
        .is_ok();
    let slot = authority
        .retained_inputs
        .lock_preparation(&publication_root, &publication_session);
    if retention_facts(
        source.as_deref(),
        current.as_deref(),
        &context.test_discovery,
        &publication,
        model_current && slot.is_some(),
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
    *slot = Some(CompletedLockPreparation {
        request: selection,
        source,
        test_discovery: context.test_discovery,
        members,
        publication,
    });
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

    /// Unchanged publication bytes cannot hide an edited sibling test or an unsupported member observation.
    #[cfg(unix)]
    #[test]
    fn sibling_test_discovery_invalidates_workspace_publication() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = std::fs::canonicalize(directory.path())?;
        let first = root.join("a");
        let second = root.join("b");
        for member in [&first, &second] {
            std::fs::create_dir(member)?;
            std::fs::write(member.join("loaf.toml"), "[project]\nname = 'probe'\n")?;
            std::fs::write(member.join("test_value.incn"), "def test_value() -> None: pass")?;
        }
        let path = root.join("oven.lock");
        std::fs::write(&path, "original publication")?;
        let mut completed = CompletedLockPreparation {
            request: "request".to_string(),
            source: "sources".to_string(),
            members: Vec::new(),
            test_discovery: vec![observe_test_inputs(&first), observe_test_inputs(&second)],
            publication: PublishedLockFile {
                canonical_path: path,
                content_digest: oven_store::digest_bytes(b"original publication"),
            },
        };
        assert!(completed.discovery_current());
        std::fs::write(second.join("test_value.incn"), "def test_other() -> None: pass")?;
        assert!(!completed.discovery_current());
        completed.test_discovery = vec![observe_test_inputs(&first), observe_test_inputs(&second)];
        assert!(completed.discovery_current());
        completed
            .test_discovery
            .last_mut()
            .ok_or("missing sibling observation")?
            .disable_retention();
        assert!(!completed.discovery_current());
        completed.test_discovery.clear();
        assert!(!completed.discovery_current());
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
            test_discovery: vec![oven_model::observed_inputs::InputObservations::default()],
            members: Vec::new(),
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
