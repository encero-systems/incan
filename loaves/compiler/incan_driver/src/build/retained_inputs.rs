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
use crate::session::CompilationSession;
use incan_frontend::library_manifest::visit_provider_artifact_inputs;
use incan_provider::{FeatureSelection, ProviderPlan};
use oven_rustc::plan::OvenDirectRustcPlanSelection;

use super::{NativeSdkCommandContext, OvenPreparedProject};

/// One existing effective manifest and its workspace topology, kept alive by the serial executable owner.
pub(super) struct RetainedProject {
    pub(super) manifest: ProjectManifest,
    workspace: Option<WorkspaceGraph>,
}

impl RetainedProject {
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
    sessions: HashMap<SessionPurpose, RetainedSession>,
}

/// Preparation authority required by an entry in the existing retained-session cache.
#[derive(Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub(super) enum SessionPurpose {
    /// Parser/vocabulary collection without native execution preparation.
    Collection,
    /// Full Oven artifact preparation for actual compilation and execution.
    Artifacts,
}

/// The existing compilation context and its provider projections for one reconciled dependency generation.
struct RetainedSession {
    identity: String,
    inputs: ModelInputs,
    session: Arc<CompilationSession>,
    executable: Option<RetainedExecutableDependencies>,
    native_admission: Option<ObservedNativeAdmission>,
    test_generation: Arc<super::test_generation::RetainedTestGeneration>,
}

/// Original native dependency owners shared by build and test within one reconciled lock/session generation.
struct ObservedNativeAdmission {
    context: Arc<NativeSdkCommandContext>,
    inputs: ModelInputs,
}

/// Exact command configuration and existing typed dependency declarations, compared without another encoding.
#[derive(PartialEq, Eq)]
pub(super) struct ExecutableDependencyRequest {
    pub(super) configuration: String,
    pub(super) dependencies: [Vec<DependencySpec>; 3],
}

/// Original native admission and identity projection for one exact dependency request in the retained session.
pub(super) struct ExecutableDependencyInputs {
    key: ExecutableDependencyRequest,
    provider: Arc<ProviderPlan>,
    pub(super) context: Option<Arc<NativeSdkCommandContext>>,
    pub(super) build_inputs: BTreeMap<String, String>,
    observations: ModelInputs,
    plan_observed: bool,
}

impl ExecutableDependencyInputs {
    /// Retain canonical admission, observing its original physical inputs through the existing model observer.
    pub(super) fn new(
        key: ExecutableDependencyRequest,
        provider: Arc<ProviderPlan>,
        context: Option<Arc<NativeSdkCommandContext>>,
        build_inputs: BTreeMap<String, String>,
    ) -> Self {
        let mut observations = ModelInputs::default();
        if let Some(context) = &context {
            context.visit_retained_inputs(&mut |path| observations.observe(path));
        }
        Self {
            key,
            provider,
            context,
            build_inputs,
            observations,
            plan_observed: false,
        }
    }

    /// Match the session's exact provider projection and current native request, excluding only consumer bodies.
    pub(super) fn matches(&self, key: &ExecutableDependencyRequest, provider: &Arc<ProviderPlan>) -> bool {
        let _trace = tracing::trace_span!(target: "incan::invocation", "incan_driver::match_native_dependency_inputs", metadata_checks = self.observations.len()).entered();
        &self.key == key
            && Arc::ptr_eq(&self.provider, provider)
            && self.observations.supports_retention()
            && self.observations.current()
    }

    /// Require observation of a replacement selection instead of treating the previous plan as still retained.
    pub(super) fn require_plan_observation(&mut self) {
        self.plan_observed = false;
    }

    /// Observe a newly selected plan once; a retained original plan was reconciled by `matches` for this command.
    ///
    /// Unsupported selection families keep canonical admission. Completion reconciles again to refuse retaining inputs
    /// changed during compilation.
    pub(super) fn observe_plan(&mut self, prepared: &OvenPreparedProject) -> CliResult<bool> {
        let _trace =
            tracing::trace_span!(target: "incan::invocation", "incan_driver::observe_executable_dependency_plan")
                .entered();
        if prepared.runtime_foundation.is_some() || prepared.release_generation_guard.is_some() {
            return Ok(false);
        }
        if self.plan_observed {
            return Ok(true);
        }
        self.observations.observe(&prepared.rustc);
        let supported = prepared
            .plan_selection
            .visit_retained_inputs(&mut |path| self.observations.observe(path))
            .map_err(|error| CliError::failure(error.to_string()))?;
        self.plan_observed = supported && self.observations.supports_retention() && self.observations.current();
        Ok(self.plan_observed)
    }
}

/// One original leased plan under the same build-unit and intent match used by canonical Store selection.
pub(super) struct RetainedExecutableDependencies<I = ExecutableDependencyInputs, S = OvenDirectRustcPlanSelection> {
    pub(super) inputs: I,
    build_unit: String,
    intent: oven_store::OvenBuildIntent,
    pub(super) selection: S,
}

impl RetainedExecutableDependencies {
    /// Move the original leases into the owner only after successful execution and publication.
    pub(super) fn completed(inputs: ExecutableDependencyInputs, prepared: OvenPreparedProject) -> Option<Self> {
        let _trace = tracing::trace_span!(target: "incan::invocation", "incan_driver::retain_executable_dependencies", metadata_checks = inputs.observations.len()).entered();
        if !inputs.observations.supports_retention() || !inputs.observations.current() {
            return None;
        }
        Some(Self {
            inputs,
            build_unit: prepared.receipt.build_unit_identity,
            intent: prepared.receipt.intent,
            selection: prepared.plan_selection,
        })
    }
}

impl<I, S> RetainedExecutableDependencies<I, S> {
    /// Apply the existing selector's identity contract to a freshly generated source receipt.
    pub(super) fn matches_receipt(&self, receipt: &oven_store::OvenReceipt) -> bool {
        self.build_unit == receipt.build_unit_identity && self.intent == receipt.intent
    }
}

impl RetainedExecutableDependencies<ModelInputs> {
    /// Retain the canonical native selection with the ordinary owner's existing metadata observations.
    /// The caller has already reconciled its declared native closure; unsupported plans keep canonical selection.
    pub(super) fn admitted_native_plan(
        receipt: &oven_store::OvenReceipt,
        selection: OvenDirectRustcPlanSelection,
    ) -> CliResult<Self> {
        let mut inputs = ModelInputs::default();
        let supported = selection
            .visit_retained_inputs(&mut |path| inputs.observe(path))
            .map_err(|error| CliError::failure(error.to_string()))?;
        if !supported {
            inputs.disable_retention();
        }
        Ok(Self {
            inputs,
            build_unit: receipt.build_unit_identity.clone(),
            intent: receipt.intent.clone(),
            selection,
        })
    }

    /// Share the original selected plan and its leases between worker requests without copying its authority.
    pub(super) fn into_shared(self) -> RetainedExecutableDependencies<ModelInputs, Arc<OvenDirectRustcPlanSelection>> {
        RetainedExecutableDependencies {
            inputs: self.inputs,
            build_unit: self.build_unit,
            intent: self.intent,
            selection: Arc::new(self.selection),
        }
    }
}

impl<S> RetainedExecutableDependencies<ModelInputs, S> {
    /// Reconcile original plan inputs before reuse and after successful execution, preserving the same leases.
    /// This observation is not dependency authority: the existing native producer reconciles that independently.
    pub(super) fn native_inputs_current(&self) -> bool {
        let _trace = tracing::trace_span!(target: "incan::invocation", "incan_driver::match_retained_native_plan", metadata_checks = self.inputs.len()).entered();
        self.inputs.supports_retention() && self.inputs.current()
    }
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
                sessions: HashMap::new(),
            },
        );
        Ok(project)
    }

    /// Retain the ordinary dependency context inside the same observed project that owns the canonical lock.
    ///
    /// The caller supplies reconciled dependency, lock, compiler and selection identity, excluding the consumer's
    /// own Incan bodies. Physical provider inputs reuse the existing replacement-sensitive observations. A changed
    /// or failed generation drops the old session before discovery; source modules are never retained here.
    /// Preparation mode keys this single cache so collection and artifact sessions cannot authorize each other or
    /// evict one another merely because commands alternate. The caller's identity also binds that mode.
    pub(super) fn compilation_session(
        &mut self,
        root: &Path,
        entry: &Path,
        identity: String,
        features: &FeatureSelection,
        sdk_profile: Option<&str>,
        purpose: SessionPurpose,
    ) -> CliResult<Arc<CompilationSession>> {
        let project = self.project(root)?;
        if let Some(retained) = self.projects.get(root).and_then(|record| record.sessions.get(&purpose))
            && retained.identity == identity
            && retained.inputs.supports_retention()
            && retained.inputs.current()
        {
            tracing::trace!(target: "incan::invocation", decision = "reuse_dependency_session", input = %root.display());
            return Ok(Arc::clone(&retained.session));
        }
        if let Some(record) = self.projects.get_mut(root) {
            record.sessions.remove(&purpose);
        }
        tracing::trace!(target: "incan::invocation", decision = "reload_dependency_session", input = %root.display());
        let session = Arc::new(match purpose {
            SessionPurpose::Collection => {
                CompilationSession::discover_for_collection_with_selections(entry, features, sdk_profile)?
            }
            SessionPurpose::Artifacts => CompilationSession::discover_for_oven(entry, features, sdk_profile)?,
        });
        let mut inputs = ModelInputs::default();
        if let Some(inventory) = &session.sdk_inventory {
            inputs.observe(&inventory.root.join(incan_provider::SDK_INVENTORY_FILE));
        }
        let mut roots = std::collections::BTreeSet::new();
        for artifact in session
            .provider_plan
            .records()
            .filter_map(|record| record.artifact.as_ref())
        {
            inputs.observe(&artifact.manifest_path);
            roots.insert(&artifact.crate_root);
        }
        for artifact in session.provider_plan.public_artifacts() {
            inputs.observe(&artifact.artifact.manifest_path);
            roots.insert(&artifact.artifact.crate_root);
        }
        for root in roots {
            visit_provider_artifact_inputs(root, &mut |path| inputs.observe(path))
                .map_err(|error| CliError::failure(error.to_string()))?;
        }
        self.ensure_current_project(root, &project)?;
        if inputs.supports_retention() && !inputs.current() {
            return Err(CliError::failure("provider inputs changed during session observation"));
        }
        let record = self
            .projects
            .get_mut(root)
            .ok_or_else(|| CliError::failure("observed project disappeared during session discovery"))?;
        record.sessions.insert(
            purpose,
            RetainedSession {
                identity,
                inputs,
                session: Arc::clone(&session),
                executable: None,
                native_admission: None,
                test_generation: Arc::default(),
            },
        );
        Ok(session)
    }

    /// Borrow generated test sources only from the exact, already reconciled artifact session.
    pub(super) fn test_generation(
        &self,
        root: &Path,
        session: &Arc<CompilationSession>,
    ) -> Option<Arc<super::test_generation::RetainedTestGeneration>> {
        self.projects
            .get(root)?
            .sessions
            .get(&SessionPurpose::Artifacts)
            .filter(|retained| Arc::ptr_eq(&retained.session, session))
            .map(|retained| Arc::clone(&retained.test_generation))
    }

    /// Transfer the prior plan out of the current session; failures cannot leave a partially reused plan installed.
    pub(super) fn take_executable_dependencies(&mut self, root: &Path) -> Option<RetainedExecutableDependencies> {
        let _trace =
            tracing::trace_span!(target: "incan::invocation", "incan_driver::take_retained_native_dependencies")
                .entered();
        self.projects
            .get_mut(root)?
            .sessions
            .get_mut(&SessionPurpose::Artifacts)?
            .executable
            .take()
    }

    /// Borrow native owners only from the exact artifact session reconciled by the current request.
    /// Changed physical inputs discard the old admission before canonical discovery. Original leases stay alive
    /// while consumers hold the returned context; unsupported observations never authorize a retained hit.
    pub(super) fn native_dependency_context(
        &mut self,
        root: &Path,
        session: &Arc<CompilationSession>,
    ) -> CliResult<Option<Arc<NativeSdkCommandContext>>> {
        let Some(retained) = self
            .projects
            .get_mut(root)
            .and_then(|project| project.sessions.get_mut(&SessionPurpose::Artifacts))
            .filter(|retained| Arc::ptr_eq(&retained.session, session))
        else {
            return NativeSdkCommandContext::discover();
        };
        let _trace =
            tracing::trace_span!(target: "incan::invocation", "incan_driver::shared_native_admission").entered();
        if let Some(admitted) = &retained.native_admission
            && admitted.inputs.supports_retention()
            && admitted.inputs.current()
        {
            tracing::trace!(target: "incan::invocation", decision = "reuse_native_admission");
            return Ok(Some(Arc::clone(&admitted.context)));
        }
        retained.native_admission = None;
        let Some(context) = NativeSdkCommandContext::discover()? else {
            return Ok(None);
        };
        let mut inputs = ModelInputs::default();
        context.visit_retained_inputs(&mut |path| inputs.observe(path));
        context.verify()?;
        if inputs.supports_retention() {
            if !inputs.current() {
                return Err(CliError::failure("native dependency inputs changed during observation"));
            }
            retained.native_admission = Some(ObservedNativeAdmission {
                context: Arc::clone(&context),
                inputs,
            });
        }
        tracing::trace!(target: "incan::invocation", decision = "admit_native_dependencies");
        Ok(Some(context))
    }

    /// Return a completed original plan to the same observed dependency generation, without a second cache.
    pub(super) fn remember_executable_dependencies(
        &mut self,
        root: &Path,
        dependencies: RetainedExecutableDependencies,
    ) {
        if let Some(session) = self
            .projects
            .get_mut(root)
            .and_then(|record| record.sessions.get_mut(&SessionPurpose::Artifacts))
        {
            session.executable = Some(dependencies);
        }
    }

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

#[cfg(test)]
mod tests {
    use super::{ExecutableDependencyInputs, ExecutableDependencyRequest, ModelInputs, RetainedProjectInputs};
    use incan_frontend::library_manifest::visit_provider_artifact_inputs;
    use oven_model::lock::{CargoFeatureSelection, IncanLock};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    /// A changed dependency request or provider projection cannot reuse the earlier native admission.
    #[test]
    fn retained_native_inputs_require_exact_request_and_provider_generation() {
        let provider = Arc::new(incan_provider::ProviderPlan::default());
        let request = || ExecutableDependencyRequest {
            configuration: "request".to_string(),
            dependencies: Default::default(),
        };
        let inputs = ExecutableDependencyInputs::new(request(), Arc::clone(&provider), None, Default::default());
        assert!(inputs.matches(&request(), &provider));
        let mut changed = request();
        changed.configuration = "changed configuration".to_string();
        assert!(!inputs.matches(&changed, &provider));
        for group in 0..3 {
            let mut changed = request();
            changed.dependencies[group].push(oven_model::manifest::DependencySpec {
                crate_name: "new_dependency".to_string(),
                version: Some("1.0".to_string()),
                features: Vec::new(),
                default_features: true,
                source: oven_model::manifest::DependencySource::Registry,
                optional: false,
                package: None,
            });
            assert!(!inputs.matches(&changed, &provider));
        }
        assert!(!inputs.matches(&request(), &Arc::new(incan_provider::ProviderPlan::default())));
    }

    /// Source bodies do not invalidate dependency admission, but replacing a declared native input does.
    #[cfg(unix)]
    #[test]
    fn retained_native_inputs_observe_physical_replacement_without_source_invalidation()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = fs::canonicalize(directory.path())?;
        let native = root.join("native.rlib");
        let source = root.join("main.incn");
        fs::write(&native, "immutable native bytes")?;
        fs::write(&source, "old source body")?;
        let provider = Arc::new(incan_provider::ProviderPlan::default());
        let request = || ExecutableDependencyRequest {
            configuration: "request".to_string(),
            dependencies: Default::default(),
        };
        let mut inputs = ExecutableDependencyInputs::new(request(), Arc::clone(&provider), None, Default::default());
        inputs.observations.observe(&native);
        assert!(inputs.matches(&request(), &provider));
        fs::write(&source, "new source body")?;
        assert!(inputs.matches(&request(), &provider));
        let replacement = root.join("replacement.rlib");
        fs::write(&replacement, "immutable native bytes")?;
        fs::rename(replacement, native)?;
        assert!(!inputs.matches(&request(), &provider));
        Ok(())
    }

    /// Retained provider observations detect nested content and membership changes without watching build outputs.
    #[cfg(unix)]
    #[test]
    fn provider_observations_track_nested_changes_and_ignore_mutable_outputs() -> Result<(), Box<dyn std::error::Error>>
    {
        let artifact = tempfile::tempdir()?;
        let root = fs::canonicalize(artifact.path())?;
        fs::create_dir_all(root.join("src/nested"))?;
        fs::create_dir_all(root.join("target"))?;
        let source = root.join("src/nested/value.rs");
        let output = root.join("target/output");
        fs::write(&source, "pub fn value() -> i32 { 1 }")?;
        fs::write(&output, "old output")?;
        let mut inputs = ModelInputs::default();
        visit_provider_artifact_inputs(&root, &mut |path| inputs.observe(path))?;
        assert!(inputs.supports_retention());
        assert!(inputs.current());
        fs::write(&output, "changed output")?;
        assert!(inputs.current());
        let replacement = source.with_extension("replacement");
        fs::write(&replacement, "pub fn value() -> i32 { 2 }")?;
        fs::rename(&replacement, &source)?;
        assert!(!inputs.current());
        let mut inputs = ModelInputs::default();
        visit_provider_artifact_inputs(&root, &mut |path| inputs.observe(path))?;
        assert!(inputs.current());
        fs::write(root.join("src/nested/added.rs"), "pub fn added() {}")?;
        assert!(!inputs.current());
        Ok(())
    }
    /// Create a conventional project so every test uses real model parsing and canonical, non-linked paths.
    fn write_project(root: &Path, name: &str) -> Result<(), Box<dyn std::error::Error>> {
        fs::create_dir_all(root.join("src"))?;
        fs::write(root.join("loaf.toml"), format!("[project]\nname = {name:?}\n"))?;
        fs::write(root.join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        Ok(())
    }

    /// An unchanged model and unrelated source edits reuse the same existing manifest/workspace generation.
    #[cfg(unix)]
    #[test]
    fn unchanged_project_models_are_retained() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = fs::canonicalize(directory.path())?;
        write_project(&root, "model")?;
        let mut inputs = RetainedProjectInputs::default();
        let first = inputs.project(&root)?;
        fs::write(root.join("src/main.incn"), "def main() -> None:\n    println(1)\n")?;
        let second = inputs.project(&root)?;
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(inputs.project_loads, 1);
        assert_eq!(inputs.lock_path(&root)?, root.join("oven.lock"));
        assert_eq!(inputs.project_loads, 1);
        Ok(())
    }

    /// Invalid changed manifests discard prior models; repair and identical-byte replacement create new generations.
    #[test]
    fn changed_manifest_reloads_or_refuses() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = fs::canonicalize(directory.path())?;
        write_project(&root, "model")?;
        let mut inputs = RetainedProjectInputs::default();
        let first = inputs.project(&root)?;
        let path = root.join("loaf.toml");
        let bytes = fs::read(&path)?;
        fs::write(&path, "invalid manifest [")?;
        assert!(inputs.project(&root).is_err());
        assert!(!inputs.projects.contains_key(&root));
        fs::write(&path, &bytes)?;
        let repaired = inputs.project(&root)?;
        assert!(!Arc::ptr_eq(&first, &repaired));
        let replacement = root.join("replacement.toml");
        fs::write(&replacement, bytes)?;
        fs::rename(replacement, path)?;
        assert!(!Arc::ptr_eq(&repaired, &inputs.project(&root)?));
        Ok(())
    }

    /// A manifest edit during source reconciliation cannot publish identities using the earlier parsed model.
    #[test]
    fn model_edit_during_reconciliation_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = fs::canonicalize(directory.path())?;
        write_project(&root, "before")?;
        let mut inputs = RetainedProjectInputs::default();
        let project = inputs.project(&root)?;
        fs::write(root.join("loaf.toml"), "[project]\nname = \"after\"\n")?;
        assert!(inputs.ensure_current_project(&root, &project).is_err());
        assert!(!inputs.projects.contains_key(&root));
        assert!(inputs.project(&root).is_ok());
        Ok(())
    }

    /// A newly authored ancestor workspace must replace the former nearest-project lock decision.
    #[test]
    fn new_ancestor_workspace_changes_canonical_lock() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = fs::canonicalize(directory.path())?;
        let member = root.join("member");
        write_project(&member, "member")?;
        let mut inputs = RetainedProjectInputs::default();
        assert_eq!(inputs.lock_path(&member)?, member.join("oven.lock"));
        fs::write(root.join("loaf.toml"), "[workspace]\nmembers = [\"member\"]\n")?;
        assert_eq!(inputs.lock_path(&member)?, root.join("oven.lock"));
        fs::remove_file(root.join("loaf.toml"))?;
        assert_eq!(inputs.lock_path(&member)?, member.join("oven.lock"));
        Ok(())
    }

    /// Directory observations discover new glob members and refuse duplicate names rather than using old topology.
    #[test]
    fn changed_workspace_topology_reloads() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = fs::canonicalize(directory.path())?;
        let member = root.join("packages/first");
        write_project(&member, "first")?;
        fs::write(root.join("loaf.toml"), "[workspace]\nmembers = [\"packages/*\"]\n")?;
        let mut inputs = RetainedProjectInputs::default();
        assert_eq!(
            inputs
                .project(&member)?
                .workspace
                .as_ref()
                .ok_or("workspace missing")?
                .members()
                .count(),
            1
        );
        let second = root.join("packages/second");
        write_project(&second, "second")?;
        assert_eq!(
            inputs
                .project(&member)?
                .workspace
                .as_ref()
                .ok_or("workspace missing")?
                .members()
                .count(),
            2
        );
        fs::write(second.join("loaf.toml"), "[project]\nname = \"first\"\n")?;
        assert!(inputs.project(&member).is_err());
        assert!(!inputs.projects.contains_key(&member));
        Ok(())
    }

    /// Local Rust entrypoint existence participates in parsing even when the authored manifest did not change.
    #[test]
    fn local_rust_classification_is_observed() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = fs::canonicalize(directory.path())?;
        write_project(&root, "model")?;
        fs::create_dir_all(root.join("native/src"))?;
        fs::write(
            root.join("loaf.toml"),
            "[project]\nname = \"model\"\n[dependencies]\nhelper = { path = \"native\", loaf = \"helper\", version = \"0.1\" }\n",
        )?;
        let mut inputs = RetainedProjectInputs::default();
        assert!(
            inputs
                .project(&root)?
                .manifest
                .library_dependencies()
                .contains_key("helper")
        );
        fs::write(root.join("native/src/lib.rs"), "pub fn helper() {}")?;
        assert!(
            inputs
                .project(&root)?
                .manifest
                .rust_dependencies()
                .contains_key("helper")
        );
        fs::remove_file(root.join("native/src/lib.rs"))?;
        assert!(
            inputs
                .project(&root)?
                .manifest
                .library_dependencies()
                .contains_key("helper")
        );
        Ok(())
    }

    /// Script/library aliasing is revalidated after a symlink replaces an independently executable source.
    #[cfg(unix)]
    #[test]
    fn script_alias_replacement_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = fs::canonicalize(directory.path())?;
        write_project(&root, "model")?;
        fs::write(
            root.join("loaf.toml"),
            "[project]\nname = \"model\"\n[project.scripts]\ncommand = \"src/command.incn\"\n",
        )?;
        fs::write(root.join("src/lib.incn"), "")?;
        let script = root.join("src/command.incn");
        fs::write(&script, "")?;
        let mut inputs = RetainedProjectInputs::default();
        assert!(inputs.project(&root).is_ok());
        fs::remove_file(&script)?;
        std::os::unix::fs::symlink(root.join("src/lib.incn"), &script)?;
        assert!(inputs.project(&root).is_err());
        assert!(!inputs.projects.contains_key(&root));
        Ok(())
    }

    /// Existing symlinked manifests are loaded normally but cannot authorize reuse across model requests.
    #[cfg(unix)]
    #[test]
    fn linked_manifest_is_not_retained() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = fs::canonicalize(directory.path())?;
        write_project(&root, "model")?;
        let path = root.join("loaf.toml");
        let target = root.join("manifest.toml");
        fs::rename(&path, &target)?;
        std::os::unix::fs::symlink(&target, path)?;
        let mut inputs = RetainedProjectInputs::default();
        let first = inputs.project(&root)?;
        assert!(!Arc::ptr_eq(&first, &inputs.project(&root)?));
        assert_eq!(inputs.project_loads, 2);
        Ok(())
    }

    /// Write through the canonical loader's publication format so these controls exercise real lock admission.
    fn write_lock(path: &Path, fingerprint: &str, feature: &str) -> Result<(), Box<dyn std::error::Error>> {
        IncanLock::new(
            "0.0.0-test",
            fingerprint.to_string(),
            CargoFeatureSelection {
                cargo_features: vec![feature.to_string()],
                ..CargoFeatureSelection::default()
            },
            String::new(),
        )
        .write(path)?;
        Ok(())
    }

    /// Unchanged observations borrow one typed record and one semantic projection across requests.
    #[test]
    fn unchanged_lock_reuses_typed_record_and_projection() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("oven.lock");
        write_lock(&path, "sha256:first", "alpha")?;
        let mut inputs = RetainedProjectInputs::default();
        let authority = inputs.authority(&path, root.path())?;
        assert!(authority.is_some());
        assert_eq!(inputs.fingerprint(&path)?.as_deref(), Some("sha256:first"));
        assert_eq!(inputs.authority(&path, root.path())?, authority);
        #[cfg(unix)]
        assert_eq!((inputs.loads, inputs.projections), (1, 1));
        Ok(())
    }

    /// Derived fingerprint changes reload the file without changing its semantic authority; meaningful changes do.
    #[test]
    fn replacement_reloads_and_keeps_semantic_projection_rules() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("oven.lock");
        let mut inputs = RetainedProjectInputs::default();
        write_lock(&path, "sha256:first", "alpha")?;
        let first = inputs.authority(&path, root.path())?;
        write_lock(&path, "sha256:other", "alpha")?;
        assert_eq!(inputs.fingerprint(&path)?.as_deref(), Some("sha256:other"));
        assert_eq!(inputs.authority(&path, root.path())?, first);
        write_lock(&path, "sha256:other", "bravo")?;
        assert_ne!(inputs.authority(&path, root.path())?, first);
        #[cfg(unix)]
        assert_eq!(inputs.loads, 3);
        Ok(())
    }

    /// A malformed or removed lock cannot fall back to a formerly valid retained value, and repair is admitted.
    #[test]
    fn failed_reload_discards_previous_lock_and_recovers() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("oven.lock");
        let mut inputs = RetainedProjectInputs::default();
        assert!(inputs.fingerprint(&path)?.is_none());
        write_lock(&path, "sha256:first", "alpha")?;
        assert!(inputs.fingerprint(&path)?.is_some());
        fs::write(&path, "not valid toml [")?;
        assert!(inputs.fingerprint(&path).is_err());
        assert!(!inputs.locks.contains_key(&path));
        fs::remove_file(&path)?;
        assert!(inputs.fingerprint(&path)?.is_none());
        fs::create_dir(&path)?;
        assert!(inputs.fingerprint(&path).is_err());
        fs::remove_dir(&path)?;
        write_lock(&path, "sha256:fixed", "alpha")?;
        assert_eq!(inputs.fingerprint(&path)?.as_deref(), Some("sha256:fixed"));
        Ok(())
    }

    /// Command-local graph identities never survive a new owner request, even though guarded inputs may.
    #[test]
    fn next_request_forgets_source_digest_memos() {
        let mut authority = super::super::ProjectSourceAuthorityDigester::default();
        authority
            .project_digests
            .insert(PathBuf::from("project"), "old".to_string());
        authority
            .rust_crate_digests
            .insert(PathBuf::from("crate"), "old".to_string());
        authority
            .rust_source_closure_digests
            .insert(PathBuf::from("closure"), "old".to_string());
        authority.begin_request();
        assert!(authority.project_digests.is_empty());
        assert!(authority.rust_crate_digests.is_empty());
        assert!(authority.rust_source_closure_digests.is_empty());
    }

    /// Retained projections remain member scoped, including the existing absent-member semantic rule.
    #[test]
    fn workspace_members_share_lock_without_sharing_projections() -> Result<(), Box<dyn std::error::Error>> {
        use oven_model::lock::LockedWorkspaceMember;
        let root = tempfile::tempdir()?;
        let path = root.path().join("oven.lock");
        let mut lock = IncanLock::new(
            "0.0.0-test",
            "sha256:first".to_string(),
            CargoFeatureSelection::default(),
            String::new(),
        );
        for member_root in ["member-a", "member-b"] {
            fs::create_dir(root.path().join(member_root))?;
            lock.semantic.workspace_members.push(LockedWorkspaceMember {
                member_root: member_root.to_string(),
                sdk: None,
                packages: Vec::new(),
                feature_edges: Vec::new(),
                providers: Vec::new(),
                oven: None,
            });
        }
        lock.write(&path)?;
        let mut inputs = RetainedProjectInputs::default();
        let first = inputs.authority(&path, &root.path().join("member-a"))?;
        let second = inputs.authority(&path, &root.path().join("member-b"))?;
        assert!(first.is_some() && second.is_some());
        assert_ne!(first, second);
        assert!(inputs.authority(&path, &root.path().join("absent"))?.is_none());
        lock.semantic.workspace_members.pop();
        lock.write(&path)?;
        assert_eq!(inputs.authority(&path, &root.path().join("member-a"))?, first);
        assert!(inputs.authority(&path, &root.path().join("member-b"))?.is_none());
        Ok(())
    }

    /// Size and modification time alone cannot hide an in-place edit from the existing Unix observation stamp.
    #[cfg(unix)]
    #[test]
    fn same_size_edit_with_restored_mtime_reloads() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("oven.lock");
        write_lock(&path, "sha256:first", "alpha")?;
        let mut inputs = RetainedProjectInputs::default();
        let first = inputs.authority(&path, root.path())?;
        let before = fs::metadata(&path)?;
        let content = fs::read_to_string(&path)?.replace("alpha", "bravo");
        fs::write(&path, content)?;
        fs::File::options()
            .write(true)
            .open(&path)?
            .set_times(fs::FileTimes::new().set_modified(before.modified()?))?;
        assert_eq!(fs::metadata(&path)?.len(), before.len());
        assert_eq!(fs::metadata(&path)?.modified()?, before.modified()?);
        assert_ne!(inputs.authority(&path, root.path())?, first);
        assert_eq!(inputs.loads, 2);
        Ok(())
    }

    /// A linked lock uses normal loading each time; changing its target cannot reuse a link's old typed value.
    #[cfg(unix)]
    #[test]
    fn linked_lock_does_not_retain_target_state() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("target.lock");
        let path = root.path().join("oven.lock");
        write_lock(&target, "sha256:first", "alpha")?;
        std::os::unix::fs::symlink(&target, &path)?;
        let mut inputs = RetainedProjectInputs::default();
        assert_eq!(inputs.fingerprint(&path)?.as_deref(), Some("sha256:first"));
        assert_eq!(inputs.fingerprint(&path)?.as_deref(), Some("sha256:first"));
        write_lock(&target, "sha256:other", "alpha")?;
        assert_eq!(inputs.fingerprint(&path)?.as_deref(), Some("sha256:other"));
        assert_eq!(inputs.loads, 3);
        Ok(())
    }
}
