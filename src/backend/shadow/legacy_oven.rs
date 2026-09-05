//! Oven-owned legacy execution for the bounded #1146 shadow comparison.
//!
//! Parity evidence must come from the adopted build and execution boundary, not from an ad-hoc compiler
//! invocation. The legacy route therefore runs the same way an ordinary Incan build does: the emitted Rust is
//! authorized by an [`OvenReceipt`], an immutable direct-`rustc` plan is selected from the bounded
//! [`OvenStore`] against that receipt's reusable build unit, Oven compiles the caller-owned source with no Cargo
//! process, and the produced binary is executed. Every one of those authority facts is carried back in a
//! [`LegacyExecutionAuthority`] and folded into the legacy receipt's output identity.
//!
//! ## Where the build unit comes from
//!
//! An Oven receipt's `build_unit_identity` is derived from build intent, compatibility envelope, and build-unit
//! inputs — never from the generated source. A comparison therefore cannot invent its own build unit and expect a
//! stored plan to match it; it must adopt the intent and build-unit inputs of a project whose plan was already
//! published by an explicit `incan oven bake`. [`LegacyOvenCapability::adopt_baked_project`] does exactly that:
//! it reads and verifies a real Oven receipt, keeps its intent and build-unit inputs, and replaces only the
//! generated-source evidence with this comparison's own program. The source bytes stay caller-owned and are
//! re-authorized every run; the native closure stays store-owned and immutable.
//!
//! Where no such capability is staged, the legacy route is honestly unavailable. It is never approximated by
//! calling a compiler directly, because an unauthorized build would produce a result no receipt can account for.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::backend::ProjectGenerator;
use crate::oven::rustc::{
    OvenStoredDirectRustcRunRequest, bake_stored_direct_rustc_run, select_direct_rustc_plan_for_execution,
};
use crate::oven::store::{OvenStore, OvenStoreLimits};
use crate::oven::{
    DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES, DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES, DEFAULT_OVEN_MAX_PHYSICAL_BYTES,
    OvenGeneratedProjectRequest, OvenReceipt, receipt_generated_project,
};

use super::{
    LegacyExecutionAuthority, LegacyProcessEvidence, LegacyRouteResult, PreparedShadowProfile, ShadowComparisonProfile,
    ShadowLegacyMaterialization, ShadowUnavailable, emit_legacy_rust_with_materialization, observe_legacy_process,
};

/// Receipt evidence key under which the comparison's emitted Rust is authorized.
///
/// Matches the key a normal generated executable build uses, so the stored plan authorizes the same kind of
/// caller-owned root source rather than a comparison-specific shape.
const SOURCE_EVIDENCE_KEY: &str = "generated-root";

/// Project name recorded in the comparison's Oven receipt.
const LEGACY_PROJECT_NAME: &str = "incan-shadow-comparison";

/// Rust crate name for the produced legacy program.
const LEGACY_CRATE_NAME: &str = "incan_shadow_comparison";

/// Per-process suffix for caller-owned staged result-report directories.
///
/// The directory lease is host lifecycle only; the produced report remains entirely source-authored Incan. Atomic
/// creation below means a stale or concurrently created directory can never be reused as result evidence.
static NEXT_RESULT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

/// Everything needed to run one legacy route under Oven authority.
///
/// The intent and build-unit inputs are adopted from an already-baked project so a stored direct-`rustc` plan can
/// be selected; only the generated source is this comparison's own. The compiler is explicit, never resolved from
/// `PATH`, matching Oven's rule that a hidden selector must not decide which compiler produced evidence.
#[derive(Debug, Clone)]
pub struct LegacyOvenCapability {
    store_root: PathBuf,
    rustc: PathBuf,
    baked_receipt: OvenReceipt,
    staged_receipt_paths: Option<Vec<PathBuf>>,
}

impl LegacyOvenCapability {
    /// Adopt an already-baked project's build unit as the authority for comparison builds.
    ///
    /// `baked_receipt_path` is a persisted Oven receipt — in practice `.incan/oven/receipt.json` from a project
    /// that has been through `incan oven bake`. It is parsed and identity-verified before it can authorize
    /// anything; a receipt that does not verify is refused rather than trusted, exactly as
    /// `incan oven run` treats its own input.
    pub fn adopt_baked_project(
        store_root: impl Into<PathBuf>,
        rustc: impl Into<PathBuf>,
        baked_receipt_path: &Path,
    ) -> Result<Self, ShadowUnavailable> {
        let bytes = std::fs::read(baked_receipt_path).map_err(|error| {
            ShadowUnavailable::new(format!(
                "the legacy route has no Oven authority: cannot read {}: {error}",
                baked_receipt_path.display()
            ))
        })?;
        let baked_receipt: OvenReceipt = serde_json::from_slice(&bytes).map_err(|error| {
            ShadowUnavailable::new(format!(
                "the legacy route's Oven receipt {} could not be parsed: {error}",
                baked_receipt_path.display()
            ))
        })?;
        baked_receipt.verify_identity().map_err(|error| {
            ShadowUnavailable::new(format!(
                "the legacy route's Oven receipt {} failed identity verification: {error}",
                baked_receipt_path.display()
            ))
        })?;
        Ok(Self {
            store_root: store_root.into(),
            rustc: rustc.into(),
            baked_receipt,
            staged_receipt_paths: None,
        })
    }

    /// Build intent adopted for comparison builds.
    #[must_use]
    pub fn intent(&self) -> &crate::oven::OvenBuildIntent {
        &self.baked_receipt.intent
    }

    /// The verified Oven receipt whose build unit this capability adopted.
    ///
    /// Exposed so a report can name the authority a comparison ran under, and so a caller can prove the
    /// capability refuses a receipt that does not verify.
    #[must_use]
    pub fn adopted_receipt(&self) -> &OvenReceipt {
        &self.baked_receipt
    }

    /// Require the caller's source-session provider closure to match this adopted immutable native authority.
    pub(crate) fn require_materialization_compatibility(
        &self,
        materialization: &ShadowLegacyMaterialization,
    ) -> Result<(), ShadowUnavailable> {
        materialization.require_compatible_oven_build_unit_inputs(&self.baked_receipt.sources.build_unit_inputs)
    }

    /// Select this capability's exact staged authority for one source-session provider closure.
    ///
    /// Explicit capabilities retain their one verified receipt. Environment-backed capabilities preserve their
    /// ordered staged receipt list so a comparison can adopt the exact matching closure instead of treating the
    /// historically first receipt as authority for every source profile.
    pub(crate) fn select_for_materialization(
        &self,
        materialization: &ShadowLegacyMaterialization,
    ) -> Result<Self, ShadowUnavailable> {
        match &self.staged_receipt_paths {
            Some(receipt_paths) => Self::select_baked_project_for_materialization(
                &self.store_root,
                &self.rustc,
                receipt_paths,
                materialization,
            ),
            None => {
                self.require_materialization_compatibility(materialization)?;
                Ok(self.clone())
            }
        }
    }

    /// Derive the comparison's own Oven receipt over one emitted Rust file.
    ///
    /// The adopted intent and build-unit inputs are preserved so the reusable build unit — and therefore the
    /// stored plan — stays the baked project's. Only the source evidence is this comparison's, so the receipt
    /// authorizes exactly the bytes that will be compiled.
    fn receipt_for_generated_source(
        &self,
        project_root: &Path,
        generated_source: &Path,
    ) -> Result<OvenReceipt, ShadowUnavailable> {
        let intent = &self.baked_receipt.intent;
        let mut request = OvenGeneratedProjectRequest::new(
            project_root,
            LEGACY_PROJECT_NAME,
            self.baked_receipt.project.version.clone(),
            intent.target.clone(),
            intent.toolchain.clone(),
            intent.profile.clone(),
            intent.features.clone(),
        )
        .with_generated_source(SOURCE_EVIDENCE_KEY, generated_source);
        for (name, value) in &self.baked_receipt.sources.build_unit_inputs {
            request = request.with_build_unit_input(name.clone(), value.clone());
        }
        receipt_generated_project(&request).map_err(|error| {
            ShadowUnavailable::new(format!(
                "the legacy route could not receipt its generated source through Oven: {error}"
            ))
        })
    }

    /// Open the bounded Oven store this capability selects plans from.
    fn open_store(&self) -> OvenStore {
        OvenStore::new(
            &self.store_root,
            OvenStoreLimits::new(
                DEFAULT_OVEN_MAX_PHYSICAL_BYTES,
                DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES,
                DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES,
            ),
        )
    }
}

/// Run the legacy route for one profile through Oven and observe what the produced program did.
///
/// The steps mirror `incan oven run`: receipt the caller-owned source, select an immutable plan authorized by
/// that receipt's build unit, compile without a Cargo process, then execute. A failure at any step before
/// execution is [`ShadowUnavailable`] — the program was never observed, so there is nothing to compare, and a
/// build failure must never be promoted into a claim about program meaning.
pub(crate) fn observe_legacy_route(
    profile: &ShadowComparisonProfile,
    prepared: &PreparedShadowProfile,
    materialization: &ShadowLegacyMaterialization,
    capability: &LegacyOvenCapability,
    workspace: &Path,
) -> Result<LegacyRouteResult, ShadowUnavailable> {
    materialization.require_profile_source(profile)?;
    capability.require_materialization_compatibility(materialization)?;
    observe_legacy_route_with_result_report_setup(profile, prepared, materialization, capability, workspace, |_| Ok(()))
}

/// Run the legacy route after the host has leased, but before generated source uses, its private report paths.
///
/// Production supplies an empty setup. Keeping this lifecycle seam private lets module tests deterministically make
/// the source-authored `write` or `rename` fail without introducing a Rust result transport or a public option.
fn observe_legacy_route_with_result_report_setup<F>(
    profile: &ShadowComparisonProfile,
    prepared: &PreparedShadowProfile,
    materialization: &ShadowLegacyMaterialization,
    capability: &LegacyOvenCapability,
    workspace: &Path,
    setup_result_report: F,
) -> Result<LegacyRouteResult, ShadowUnavailable>
where
    F: FnOnce(&ResultReportLease) -> Result<(), ShadowUnavailable>,
{
    // The host only leases a unique caller-owned directory. The transport itself remains source-authored: the
    // generated Incan entrypoint writes `result.next` and atomically replaces `result` after calling the observed
    // function. The directory is removed after the report bytes have been copied into process evidence.
    let result_directory = ResultReportLease::new(workspace)?;
    setup_result_report(&result_directory)?;
    let result_path = result_directory.result_path();
    let program = profile.legacy_program_source(prepared.result_kind, result_path, &prepared.wrapper_identifiers)?;

    let project_root = workspace.join("oven-project");
    let source_path = materialize_legacy_program(&program, materialization, &project_root)?;
    let output_path = project_root.join(LEGACY_CRATE_NAME);

    let receipt = capability.receipt_for_generated_source(&project_root, &source_path)?;
    let store = capability.open_store();
    let selected = select_direct_rustc_plan_for_execution(&store, &receipt)
        .map_err(|error| {
            ShadowUnavailable::new(format!(
                "the legacy route could not select an Oven direct-rustc plan: {error}"
            ))
        })?
        .ok_or_else(|| {
            ShadowUnavailable::new(format!(
                "no immutable Oven direct-rustc plan is staged for build unit {}; bake the adopted project once \
                 with `incan oven bake` before a legacy comparison route can run",
                receipt.build_unit_identity
            ))
        })?;
    let plan_identity = selected.manifest.identity.clone();

    let bake = bake_stored_direct_rustc_run(&OvenStoredDirectRustcRunRequest {
        store: &store,
        plan_identity: plan_identity.clone(),
        receipt: receipt.clone(),
        rustc: capability.rustc.clone(),
        source: source_path.clone(),
        output: output_path.clone(),
        crate_name: LEGACY_CRATE_NAME.to_string(),
        edition: crate::backend::project::cargo_toml::DEFAULT_GENERATED_RUST_EDITION.to_string(),
        source_evidence_key: SOURCE_EVIDENCE_KEY.to_string(),
    })
    .map_err(|error| {
        ShadowUnavailable::new(format!(
            "Oven did not build the legacy program, so it was never observed: {error}"
        ))
    })?;

    let authority = LegacyExecutionAuthority {
        oven_receipt_identity: receipt.identity.clone(),
        oven_build_unit_identity: receipt.build_unit_identity.clone(),
        direct_rustc_plan_identity: plan_identity,
        output_digest: bake.output_digest.clone(),
        cargo_process_started: bake.cargo_process_started,
    };
    if authority.cargo_process_started {
        return Err(ShadowUnavailable::new(
            "a Cargo process participated in the legacy build, so the result is not Oven-owned execution evidence"
                .to_string(),
        ));
    }

    let mut command = Command::new(&bake.output);
    clear_inherited_cargo_environment(&mut command);
    let run = command.output().map_err(|error| {
        ShadowUnavailable::new(format!(
            "the legacy route could not run the Oven-produced program {}: {error}",
            bake.output.display()
        ))
    })?;
    let mut process = LegacyProcessEvidence {
        exit_code: run.status.code(),
        stdout: run.stdout,
        stderr: run.stderr,
        result_report: None,
    };
    let observed = if process.exit_code == Some(0) {
        match std::fs::read(result_path) {
            Ok(report) => {
                process.result_report = Some(report);
                observe_legacy_process(
                    profile.profile_kind(),
                    &profile.profile_identity(),
                    &authority,
                    &process,
                    prepared.result_kind,
                )
            }
            Err(error) => Err(ShadowUnavailable::new(format!(
                "the legacy process exited successfully but its source-authored result report {} was unavailable: \
                 {error}",
                result_path.display()
            ))),
        }
    } else {
        observe_legacy_process(
            profile.profile_kind(),
            &profile.profile_identity(),
            &authority,
            &process,
            prepared.result_kind,
        )
    };
    let (observation, unavailable_reason) = match observed {
        Ok(observation) => (Some(observation), None),
        Err(unavailable) => (None, Some(unavailable.reason)),
    };
    Ok(LegacyRouteResult {
        observation,
        authority,
        process,
        unavailable_reason,
    })
}

#[cfg(all(test, feature = "cli"))]
#[derive(Debug, Clone, Copy)]
/// The source-authored publication step a deterministic test prevents from completing.
pub(super) enum ForcedResultTransportFailure {
    Write,
    Rename,
}

#[cfg(all(test, feature = "cli"))]
/// Execute the real authored transport after making exactly one leased publication path unusable.
pub(super) fn observe_legacy_route_with_forced_transport_failure(
    profile: &ShadowComparisonProfile,
    prepared: &PreparedShadowProfile,
    materialization: &ShadowLegacyMaterialization,
    capability: &LegacyOvenCapability,
    workspace: &Path,
    failure: ForcedResultTransportFailure,
) -> Result<LegacyRouteResult, ShadowUnavailable> {
    materialization.require_profile_source(profile)?;
    capability.require_materialization_compatibility(materialization)?;
    observe_legacy_route_with_result_report_setup(profile, prepared, materialization, capability, workspace, |lease| {
        lease.force_transport_failure(failure)
    })
}

/// Lease one unique caller-owned directory for an atomic source-authored result report.
///
/// `create_dir` is the uniqueness authority: no existing directory is reused, and cleanup removes only the exact
/// directory this lease created after the report bytes have been retained in [`LegacyProcessEvidence`].
struct ResultReportLease {
    directory: PathBuf,
    result_path: PathBuf,
}

impl ResultReportLease {
    /// Reserve a fresh directory below the caller workspace, never adopting a pre-existing candidate.
    fn new(workspace: &Path) -> Result<Self, ShadowUnavailable> {
        let parent = workspace.join("incan-shadow-result-reports-v1");
        std::fs::create_dir_all(&parent).map_err(|error| {
            ShadowUnavailable::new(format!(
                "the legacy route could not create its caller-owned result-report parent {}: {error}",
                parent.display()
            ))
        })?;
        for _ in 0..1024 {
            let sequence = NEXT_RESULT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let directory = parent.join(format!("{}-{sequence}", std::process::id()));
            match std::fs::create_dir(&directory) {
                Ok(()) => {
                    let result_path = directory.join("result");
                    return Ok(Self { directory, result_path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(ShadowUnavailable::new(format!(
                        "the legacy route could not reserve its result-report directory {}: {error}",
                        directory.display()
                    )));
                }
            }
        }
        Err(ShadowUnavailable::new(format!(
            "the legacy route could not reserve a unique result-report directory under {} after 1024 attempts",
            parent.display()
        )))
    }

    /// Return the final path the source-authored wrapper publishes after its write succeeds.
    fn result_path(&self) -> &Path {
        &self.result_path
    }

    #[cfg(all(test, feature = "cli"))]
    /// Create a directory where the selected source operation requires a file, without touching other leases.
    fn force_transport_failure(&self, failure: ForcedResultTransportFailure) -> Result<(), ShadowUnavailable> {
        let path = match failure {
            ForcedResultTransportFailure::Write => self.result_path.with_extension("next"),
            ForcedResultTransportFailure::Rename => self.result_path.clone(),
        };
        std::fs::create_dir(&path).map_err(|error| {
            ShadowUnavailable::new(format!(
                "the shadow test could not force the source-authored result transport failure at {}: {error}",
                path.display()
            ))
        })
    }
}

impl Drop for ResultReportLease {
    /// Remove only this lease's generated directory after its process evidence has been copied.
    fn drop(&mut self) {
        // This path was atomically created by `new`; ignore cleanup failure because the process evidence was already
        // copied and must not be replaced by a post-execution filesystem error.
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// Materialize one legacy root with the caller-owned provider projection used by codegen.
///
/// The generator owns the narrow compatibility facade that maps compiler-generated `crate::__incan_std` paths to
/// linked compiled provider artifacts. The comparison route does not recreate that facade as a Rust string or reopen
/// any provider source. It only receipts the resulting `src/main.rs` and sends those exact bytes to Oven.
pub(crate) fn materialize_legacy_program(
    program: &str,
    materialization: &ShadowLegacyMaterialization,
    project_root: &Path,
) -> Result<PathBuf, ShadowUnavailable> {
    let rust_source = emit_legacy_rust_with_materialization(program, materialization)?;
    let mut generator = ProjectGenerator::new(project_root, LEGACY_PROJECT_NAME, true);
    generator.set_provider_plan(materialization.provider_plan());
    generator.generate(&rust_source).map_err(|error| {
        ShadowUnavailable::new(format!(
            "the legacy route could not materialize its caller-owned generated project at {}: {error}",
            project_root.display()
        ))
    })?;
    Ok(generator.crate_root_path())
}

/// Remove inherited Cargo process variables before running the Oven-produced program.
///
/// Mirrors what `incan oven run` does: an Oven-owned execution must not observe a surrounding Cargo invocation's
/// environment, or its behavior could depend on how the comparison happened to be launched.
fn clear_inherited_cargo_environment(command: &mut Command) {
    let inherited: Vec<String> = std::env::vars_os()
        .filter_map(|(name, _)| name.into_string().ok())
        .filter(|name| name == "CARGO" || name.starts_with("CARGO_"))
        .collect();
    for name in inherited {
        command.env_remove(name);
    }
}

/// Named environment variables that stage a legacy comparison capability.
///
/// Kept as one table so the contract is stated once, and so an operator can see exactly what must be provided
/// before a comparison can run.
pub const CAPABILITY_ENVIRONMENT: &[(&str, &str)] = &[
    (
        INCAN_HOME_ENV,
        "Incan home whose bounded Oven store holds a published direct-rustc plan",
    ),
    (
        RECEIPT_ENV,
        "path list of verified Oven receipts from projects already baked with `incan oven bake`",
    ),
    (RUSTC_ENV, "explicit Rust compiler executable Oven must use"),
];

/// Incan home whose bounded Oven store the comparison selects plans from.
const INCAN_HOME_ENV: &str = "INCAN_SHADOW_OVEN_HOME";

/// Persisted Oven receipt whose build unit the comparison adopts.
const RECEIPT_ENV: &str = "INCAN_SHADOW_OVEN_RECEIPT";

/// Explicit compiler Oven must use for the comparison build.
const RUSTC_ENV: &str = "INCAN_SHADOW_RUSTC";

/// Read one required filesystem path from the staged comparison environment.
fn environment_path(name: &str) -> Result<PathBuf, ShadowUnavailable> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            ShadowUnavailable::new(format!(
                "the legacy comparison route is not staged: {name} is unset; it must name the {}",
                CAPABILITY_ENVIRONMENT
                    .iter()
                    .find(|(variable, _)| *variable == name)
                    .map_or("required input", |(_, description)| *description)
            ))
        })
}

/// Read the ordered receipt candidates staged for source-session compatibility selection.
fn environment_receipt_paths() -> Result<Vec<PathBuf>, ShadowUnavailable> {
    let value = std::env::var_os(RECEIPT_ENV)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ShadowUnavailable::new(format!(
                "the legacy comparison route is not staged: {RECEIPT_ENV} is unset; it must name the {}",
                CAPABILITY_ENVIRONMENT
                    .iter()
                    .find(|(variable, _)| *variable == RECEIPT_ENV)
                    .map_or("required input", |(_, description)| *description)
            ))
        })?;
    let paths = std::env::split_paths(&value).collect::<Vec<_>>();
    if paths.is_empty() {
        return Err(ShadowUnavailable::new(format!(
            "the legacy comparison route is not staged: {RECEIPT_ENV} contains no receipt paths"
        )));
    }
    Ok(paths)
}

impl LegacyOvenCapability {
    /// Resolve a capability from the environment variables listed in [`CAPABILITY_ENVIRONMENT`].
    ///
    /// When the receipt variable contains a platform path list, this constructor retains that ordered list for the
    /// bounded comparison to select the exact source-session-compatible authority before either route executes.
    ///
    /// Returns [`ShadowUnavailable`] naming the first missing variable, so an unstaged environment produces an
    /// actionable non-green reason instead of a silent skip.
    pub fn from_environment() -> Result<Self, ShadowUnavailable> {
        let incan_home = environment_path(INCAN_HOME_ENV)?;
        let receipt_paths = environment_receipt_paths()?;
        let receipt_path = receipt_paths
            .first()
            .ok_or_else(|| ShadowUnavailable::new("the legacy comparison route has no staged Oven receipt"))?;
        let rustc = environment_path(RUSTC_ENV)?;
        let mut capability = Self::adopt_baked_project(
            crate::oven::store::store_root_for_home(&incan_home),
            rustc,
            receipt_path,
        )?;
        capability.staged_receipt_paths = Some(receipt_paths);
        Ok(capability)
    }

    /// Select the staged verified receipt whose immutable inputs exactly match one source-session materialization.
    ///
    /// A corpus may contain source profiles with different feature/provider closures. The environment therefore
    /// admits an ordered platform path list while retaining one Oven store and Rust compiler. Every candidate is
    /// identity-verified, and only an exact build-unit-input match is returned; a broader receipt is never treated
    /// as authority for a narrower source session (or vice versa).
    #[cfg(test)]
    pub(crate) fn from_environment_for_materialization(
        materialization: &ShadowLegacyMaterialization,
    ) -> Result<Self, ShadowUnavailable> {
        let incan_home = environment_path(INCAN_HOME_ENV)?;
        let rustc = environment_path(RUSTC_ENV)?;
        let store_root = crate::oven::store::store_root_for_home(&incan_home);
        let receipt_paths = environment_receipt_paths()?;
        Self::select_baked_project_for_materialization(&store_root, &rustc, &receipt_paths, materialization)
    }

    /// Adopt the first verified receipt whose immutable inputs exactly match the source session.
    fn select_baked_project_for_materialization(
        store_root: &Path,
        rustc: &Path,
        receipt_paths: &[PathBuf],
        materialization: &ShadowLegacyMaterialization,
    ) -> Result<Self, ShadowUnavailable> {
        let mut rejected = Vec::new();
        for receipt_path in receipt_paths {
            let capability = match Self::adopt_baked_project(store_root, rustc, receipt_path) {
                Ok(capability) => capability,
                Err(error) => {
                    rejected.push(format!("{}: {}", receipt_path.display(), error.reason));
                    continue;
                }
            };
            match capability.require_materialization_compatibility(materialization) {
                Ok(()) => return Ok(capability),
                Err(error) => rejected.push(format!("{}: {}", receipt_path.display(), error.reason)),
            }
        }
        Err(ShadowUnavailable::new(format!(
            "the legacy comparison route has no staged receipt matching this source-session provider closure; \
             checked {} candidate(s): {}",
            receipt_paths.len(),
            rejected.join("; ")
        )))
    }
}

#[cfg(test)]
mod receipt_selection_tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use super::*;

    /// Persist one identity-valid receipt carrying the selected provider closure.
    fn write_receipt(
        workspace: &Path,
        name: &str,
        provider_plan_identity: &str,
    ) -> Result<(PathBuf, OvenReceipt), Box<dyn std::error::Error>> {
        let project_root = workspace.join(name);
        std::fs::create_dir_all(&project_root)?;
        let generated_source = project_root.join("main.rs");
        std::fs::write(&generated_source, "fn main() {}\n")?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                &project_root,
                name,
                "0.1.0",
                "fixture-target",
                "fixture-toolchain",
                "debug",
                Vec::new(),
            )
            .with_generated_source(SOURCE_EVIDENCE_KEY, &generated_source)
            .with_build_unit_input("provider-plan", provider_plan_identity),
        )?;
        let receipt_path = project_root.join("receipt.json");
        std::fs::write(&receipt_path, serde_json::to_vec(&receipt)?)?;
        Ok((receipt_path, receipt))
    }

    /// Receipt-list selection skips valid unrelated authority and adopts the later exact source closure.
    #[test]
    fn source_materialization_selects_the_later_exact_receipt() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let (unrelated_path, unrelated) = write_receipt(workspace.path(), "unrelated", "sha256:unrelated")?;
        let (matching_path, matching) = write_receipt(workspace.path(), "matching", "sha256:matching")?;
        let materialization = ShadowLegacyMaterialization::from_provider_plan(
            Arc::new(crate::provider::ProviderPlan::default()),
            BTreeMap::from([("provider-plan".to_string(), "sha256:matching".to_string())]),
            "sha256:source".to_string(),
        );

        let selected = LegacyOvenCapability::select_baked_project_for_materialization(
            &workspace.path().join("store"),
            &workspace.path().join("rustc"),
            &[unrelated_path, matching_path],
            &materialization,
        )?;

        assert_ne!(unrelated.identity, matching.identity);
        assert_eq!(selected.adopted_receipt().identity, matching.identity);
        Ok(())
    }
}
