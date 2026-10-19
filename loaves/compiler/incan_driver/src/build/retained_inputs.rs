//! Owner-held project models and canonical locks, guarded by existing replacement-sensitive observations.
//!
//! Existing model loaders report their filesystem inputs. This module retains their results rather than introducing
//! another parser, resolver or persisted cache. Final source identities remain command-local.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{fs, io};

use oven_model::lock::IncanLock;
use oven_model::manifest::{
    DependencySpec, INTERNAL_MANIFEST_OVERRIDE_ENV, INTERNAL_PROJECT_ROOT_OVERRIDE_ENV, ProjectManifest,
};
pub(super) use oven_model::observed_inputs::InputObservations as ModelInputs;
use oven_model::workspace::WorkspaceGraph;

use super::file_freshness::{FileStamp, stat_file};
use super::source_authority::project_lock_authority;
use crate::error::{CliError, CliResult};
use crate::project::effective_project_manifest_with_workspace;
use incan_frontend::library_manifest::visit_provider_artifact_inputs;



/// One existing effective manifest and its workspace topology, kept alive by the serial executable owner.
pub(super) struct RetainedProject {
    pub(super) manifest: ProjectManifest,
    workspace: Option<WorkspaceGraph>,
}

impl RetainedProject {
    /// Borrow the same observed workspace topology used to compute effective member declarations.
    pub(super) fn workspace(&self) -> Option<&WorkspaceGraph> {
        self.workspace.as_ref()
    }

    /// Borrow the original member declaration for policies that precede effective dependency inheritance.
    pub(super) fn authored_manifest(&self) -> &ProjectManifest {
        self.workspace
            .as_ref()
            .and_then(|workspace| {
                workspace
                    .member_for_root(self.manifest.project_root())
                    .and_then(|member| workspace.member_manifest(member))
            })
            .unwrap_or(&self.manifest)
    }
}

/// A guarded model generation, including nested-command discovery configuration.
struct ObservedProject {
    inputs: ModelInputs,
    overrides: [Option<OsString>; 2],
    project: Arc<RetainedProject>,
}

/// Retained workspace selection is scoped to the same nested-command configuration as ordinary discovery.
fn discovery_overrides() -> [Option<OsString>; 2] {
    [
        std::env::var_os(INTERNAL_PROJECT_ROOT_OVERRIDE_ENV),
        std::env::var_os(INTERNAL_MANIFEST_OVERRIDE_ENV),
    ]
}

/// A parsed lock and its already-computed member projections for one observed file generation.
struct RetainedLock {
    stamp: Option<FileStamp>,
    lock: IncanLock,
    authorities: HashMap<PathBuf, Option<String>>,
}

/// The existing lock model retained by one serial executable owner, never across compiler generations.
#[derive(Default)]
pub(super) struct RetainedProjectInputs {
    /// One compiled-vocabulary runtime for this compiler owner; current bytes authenticate every module lookup.
    pub(super) vocab_runtime: Arc<std::sync::Mutex<Option<incan_frontend::vocab_desugar_pass::WasmDesugarerRuntime>>>,
    locks: HashMap<PathBuf, RetainedLock>,
    projects: HashMap<PathBuf, ObservedProject>,
    #[cfg(test)]
    loads: usize,
    #[cfg(test)]
    projections: usize,
    #[cfg(test)]
    project_loads: usize,
}

impl RetainedProjectInputs {
    /// Verify the original model generations without refreshing them after authored source collection.
    ///
    /// Cold publication retains this owner across emission. Unsupported observations cannot certify a generation;
    /// ordinary retained-session loading continues to use its existing conservative fallback independently.
    pub(super) fn ensure_original_models_current(&self) -> CliResult<()> {
        let overrides = discovery_overrides();
        if self.projects.values().all(|record| {
            record.inputs.supports_retention() && record.inputs.current() && record.overrides == overrides
        }) {
            Ok(())
        } else {
            Err(CliError::failure(
                "original project model inputs changed during cold compilation",
            ))
        }
    }

    /// Refuse a changed model generation before publishing the source authority computed with it.
    pub(super) fn ensure_current_project(&mut self, root: &Path, project: &Arc<RetainedProject>) -> CliResult<()> {
        if self.projects.get(root).is_some_and(|record| {
            !record.inputs.supports_retention() || Arc::ptr_eq(&record.project, project) && record.inputs.current()
        }) {
            return Ok(());
        }
        self.projects.remove(root);
        Err(CliError::failure(
            "project model inputs changed during source reconciliation",
        ))
    }

    /// Borrow the existing project model while all inputs to discovery and parsing still match.
    ///
    /// Changed or failed generations are removed before rebuilding through the ordinary model and inheritance
    /// boundaries. Symlinked or unsupported inputs use ordinary loading each time; they never authorize retention.
    pub(super) fn project(&mut self, root: &Path) -> CliResult<Arc<RetainedProject>> {
        let overrides = discovery_overrides();
        if let Some(record) = self.projects.get(root)
            && record.inputs.supports_retention()
            && record.overrides == overrides
            && record.inputs.current()
        {
            tracing::trace!(target: "incan::invocation", input = %root.display(), decision = "reuse_project_models");
            return Ok(Arc::clone(&record.project));
        }
        self.projects.remove(root);
        #[cfg(test)]
        {
            self.project_loads += 1;
        }
        let _load = tracing::trace_span!(target: "incan::invocation", "incan_driver::load_project_models", input = %root.display()).entered();
        let mut inputs = ModelInputs::default();
        let mut observe = |path: &Path| inputs.observe(path);
        let manifest =
            ProjectManifest::load_observed(&root.join(oven_model::manifest::LOAF_MANIFEST_FILENAME), &mut observe)
                .map_err(|error| CliError::failure(error.to_string()))?;
        let workspace = WorkspaceGraph::discover_observed(root, &mut observe)
            .map_err(|error| CliError::failure(error.to_string()))?;
        let manifest = effective_project_manifest_with_workspace(manifest, workspace.as_ref())?;
        if inputs.supports_retention() && (!inputs.current() || overrides != discovery_overrides()) {
            return Err(CliError::failure("project model inputs changed during discovery"));
        }
        let project = Arc::new(RetainedProject { manifest, workspace });
        self.projects.insert(
            root.to_path_buf(),
            ObservedProject {
                inputs,
                overrides,
                project: Arc::clone(&project),
            },
        );
        Ok(project)
    }

    /// Retain the ordinary dependency context inside the same observed project that owns the canonical lock.
    ///
    /// The caller supplies reconciled dependency, lock, compiler and selection identity, excluding the consumer's
    /// own Incan bodies. Physical provider inputs reuse the existing replacement-sensitive observations. A changed
    /// or failed generation drops the old session before discovery; source modules are never retained here.
    /// Select the canonical lock from the same observed workspace used for the effective manifest.
    pub(super) fn lock_path(&mut self, root: &Path) -> CliResult<PathBuf> {
        let project = self.project(root)?;
        Ok(project
            .workspace
            .as_ref()
            .map_or(root, WorkspaceGraph::root)
            .join(oven_model::lock::LOCK_FILENAME))
    }

    /// Borrow a current typed lock; changed or failed reads discard the previous generation before parsing.
    ///
    /// Missing locks are checked again on every request. Symlinks and platforms without replacement-sensitive
    /// observations keep the existing loader behavior but do not retain a reusable record. Before/after checks reject
    /// inputs changing during a parse; this is the same synchronous observation boundary as source reconciliation.
    fn lock(&mut self, path: &Path) -> CliResult<Option<&mut RetainedLock>> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.locks.remove(path);
                tracing::trace!(target: "incan::invocation", input = %path.display(), decision = "lock_absent");
                return Ok(None);
            }
            Err(error) => {
                self.locks.remove(path);
                return Err(CliError::failure(format!(
                    "cannot observe canonical lock {}: {error}",
                    path.display()
                )));
            }
        };
        let before = stat_file(path).ok();
        let reusable = !metadata.file_type().is_symlink() && before.as_ref().is_some_and(FileStamp::supports_retention);
        let current = reusable && self.locks.get(path).is_some_and(|record| record.stamp == before);
        if current {
            tracing::trace!(target: "incan::invocation", input = %path.display(), decision = "reuse_typed_lock", lock_parses = 0_u64);
        } else {
            self.locks.remove(path);
            #[cfg(test)]
            {
                self.loads += 1;
            }
            let _parse = tracing::trace_span!(target: "incan::invocation", "incan_driver::load_typed_lock", input = %path.display()).entered();
            let lock = IncanLock::load(path).map_err(|error| CliError::failure(error.to_string()))?;
            // Unsupported observations authorize this invocation's parsed value only. The next lookup reloads it.
            if let Some(stamp) = before.as_ref()
                && stat_file(path).ok().as_ref() != Some(stamp)
            {
                return Err(CliError::failure("canonical lock changed while being loaded"));
            }
            self.locks.insert(
                path.to_path_buf(),
                RetainedLock {
                    stamp: before,
                    lock,
                    authorities: HashMap::new(),
                },
            );
        }
        Ok(self.locks.get_mut(path))
    }

    /// Read the derived fingerprint from the same current typed lock used for semantic authority.
    pub(super) fn fingerprint(&mut self, path: &Path) -> CliResult<Option<String>> {
        Ok(self.lock(path)?.map(|record| record.lock.deps_fingerprint.clone()))
    }

    /// Reuse the existing member-scoped lock projection until the observed canonical file changes.
    pub(super) fn authority(&mut self, path: &Path, project_root: &Path) -> CliResult<Option<String>> {
        let Some(record) = self.lock(path)? else {
            return Ok(None);
        };
        if let Some(authority) = record.authorities.get(project_root) {
            return Ok(authority.clone());
        }
        let _projection = tracing::trace_span!(target: "incan::invocation", "incan_driver::project_lock_authority", input = %path.display()).entered();
        let authority = project_lock_authority(&record.lock, path, project_root)?;
        record.authorities.insert(project_root.to_path_buf(), authority.clone());
        #[cfg(test)]
        {
            self.projections += 1;
        }
        Ok(authority)
    }
}

