//! Receipt-bound Oven interop execution inputs.
//!
//! Portable `[interop.c]` lock data describes what a package requires. This module records the separately
//! selected compiler and SDK facts that authorize a native bake. It intentionally contains no ambient discovery,
//! Cargo invocation, or direct filesystem-path hand-off.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::oven::loaf::LoafTemporaryDirectory;
use crate::oven::process::{isolate_process_group, terminate_process_group};
use crate::oven::rustc::{
    OvenRustcArtifactManifest, OvenRustcSupportingArtifact, select_direct_rustc_plan_for_execution,
};
use crate::oven::store::{OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore};
use crate::oven::{OvenBuildIntent, OvenReceipt, digest_bytes, receipt_with_build_unit_input};
use crate::oven_interop::{
    InteropArtifactKind, InteropArtifactOrigin, InteropShimLanguage, InteropTargetPlatform, LockedInteropInput,
    LockedInteropTarget, ToolchainRequirement, ios_target_kind, is_interop_native_library_name,
    locked_interop_target_identity,
};

/// Compatibility version for one selected Oven interop execution receipt.
pub(crate) const OVEN_INTEROP_EXECUTION_RECEIPT_SCHEMA_VERSION: u32 = 1;
/// Compatibility version for the stored native-archive provenance bound to one direct-Rustc plan.
pub(crate) const OVEN_INTEROP_EXECUTION_PROVENANCE_SCHEMA_VERSION: u32 = 3;
/// Receipt input key which makes the immutable final-plan contract explicit.
///
/// A change to the materialized interop plan must select a new immutable plan rather than treating an older
/// receipt-compatible entry as reusable. Normal commands reconstruct this same input from the selected receipt.
pub(crate) const OVEN_INTEROP_PLAN_SCHEMA_INPUT: &str = "oven-interop-plan-schema";
/// Current immutable final-plan materialization contract.
///
/// Version 5 records native directories beneath the loader-safe immutable store layout. A plan baked before the
/// current directory encoding can embed a split ELF `RUNPATH`, so it must not be reused.
const OVEN_INTEROP_PLAN_SCHEMA: &str = "5";
/// Receipt input key that binds a normal consumer to one selected native-execution contract.
pub(crate) const OVEN_INTEROP_EXECUTION_RECEIPT_INPUT: &str = "oven-interop-execution-receipt";
/// Store-owned directory containing static archives baked from declared interop shims or artifacts.
pub(crate) const OVEN_INTEROP_NATIVE_DIRECTORY: &str = "interop-native";
/// Store-owned directory containing locked bundled runtime files for direct execution or a target packager.
pub(crate) const OVEN_INTEROP_BUNDLED_DIRECTORY: &str = "interop-bundled";
/// Store-owned provenance file that records the selected interop execution contract without local paths.
pub(crate) const OVEN_INTEROP_EXECUTION_PROVENANCE_PATH: &str = "provenance/interop-execution.json";
/// Caller-owned adapter manifest emitted beside staged native runtime files.
pub(crate) const OVEN_INTEROP_ADAPTER_MANIFEST_PATH: &str = "incan-interop-adapter.json";
/// Compatibility version for the narrow Android/iOS staging handoff.
const OVEN_INTEROP_ADAPTER_MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Maximum wall-clock time for one direct native compiler or archiver process.
///
/// A declared interop shim is a small bounded source input. Five minutes provides room for a constrained
/// cross-toolchain invocation while ensuring a child compiler, linker, or build helper cannot retain the Oven
/// publisher indefinitely. The isolated process group is terminated on expiry.
const OVEN_INTEROP_BAKE_COMMAND_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// One concrete compiler or SDK capability selected by Oven for a locked interop target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct OvenInteropCapabilitySelection {
    /// Stable capability vocabulary required by the lock.
    pub(crate) capability: String,
    /// Concrete selected version checked against the locked semantic-version requirement.
    pub(crate) version: String,
    /// Content-derived identity of the selected tool or SDK provider.
    pub(crate) identity: String,
}

/// Selected native-execution facts that join one locked target to an immutable Oven artifact plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct OvenInteropExecutionReceipt {
    /// Wire-schema version for this selected-execution contract.
    pub(crate) schema_version: u32,
    /// Content identity of the locked target requirements and declared package inputs.
    pub(crate) locked_target_identity: String,
    /// Exact Rust/C target triple selected for the native shim and consumer link plan.
    pub(crate) target: String,
    /// Selected compiler capability, if required by the locked target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) toolchain: Option<OvenInteropCapabilitySelection>,
    /// Selected SDK capability, if required by the locked target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sdk: Option<OvenInteropCapabilitySelection>,
    /// Content identity of this selected execution contract.
    pub(crate) identity: String,
}

/// One static archive compiled or selected by the explicit interop baker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct OvenInteropBakedArchive {
    /// Locked package-local shim or artifact name used as the Rust native link name.
    pub(crate) name: String,
    /// Store-owned path below [`OVEN_INTEROP_NATIVE_DIRECTORY`].
    pub(crate) relative_path: String,
    /// Content identity of the exact retained archive bytes.
    pub(crate) digest: String,
    /// Declared third-party origin retained beside the exact archive digest, when the package supplied one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) origin: Option<InteropArtifactOrigin>,
}

/// One declared dynamic library or framework file staged for a target-native packaging adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct OvenInteropBakedBundle {
    /// Locked logical artifact name.
    pub(crate) name: String,
    /// Store-owned path below [`OVEN_INTEROP_BUNDLED_DIRECTORY`].
    pub(crate) relative_path: String,
    /// Content identity of the exact retained runtime file.
    pub(crate) digest: String,
    /// Runtime loader name retained for the target adapter.
    pub(crate) runtime_name: String,
    /// Logical adapter placement; never an absolute destination path.
    pub(crate) placement: String,
    /// Declared minimum platform version for the staged runtime.
    pub(crate) minimum_platform: String,
    /// Declared third-party origin retained beside the exact bundled-file digest, when the package supplied one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) origin: Option<InteropArtifactOrigin>,
}

/// Portable provenance copied into the immutable plan beside the baked native archives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct OvenInteropExecutionProvenance {
    /// Wire-schema version of this provenance record.
    pub(crate) schema_version: u32,
    /// Selected toolchain/SDK contract that authorized this native archive set.
    pub(crate) receipt: OvenInteropExecutionReceipt,
    /// Every archive available to direct Rustc through the plan's one native search directory.
    pub(crate) archives: Vec<OvenInteropBakedArchive>,
    /// Every bundled runtime file retained for receipt-bound direct execution or a target-native packaging adapter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) bundles: Vec<OvenInteropBakedBundle>,
    /// Explicit target-native capabilities selected from the locked declaration without a package file hand-off.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) system_capabilities: Vec<String>,
}

/// Supported caller-owned native packaging layouts for an already baked interop plan.
///
/// This selects a fixed staging layout only. It does not invoke Gradle, Xcode, a signing tool, or a platform build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum OvenInteropAdapter {
    /// Copy bundled runtime files to Android's arm64 JNI library directory.
    Android,
    /// Copy bundled runtime files to an iOS framework staging directory.
    Ios,
}

/// Inputs for one receipt-bound, caller-owned native runtime staging operation.
pub(crate) struct OvenInteropAdapterStageRequest<'a> {
    /// Store that owns the final direct-Rustc plan and its active selection lease.
    pub(crate) store: &'a OvenStore,
    /// Current locked target selected from the canonical package lock.
    pub(crate) target: &'a LockedInteropTarget,
    /// Runtime receipt from which the interop plan receipt is deterministically reconstructed.
    pub(crate) base_receipt: &'a OvenReceipt,
    /// Persisted selected compiler/SDK identity that must still match the current lock.
    pub(crate) execution_receipt: &'a OvenInteropExecutionReceipt,
    /// Fixed Android or iOS layout to emit.
    pub(crate) adapter: OvenInteropAdapter,
    /// New caller-owned directory to atomically create. Existing output is never replaced.
    pub(crate) output: &'a Path,
}

/// Evidence returned after atomically staging one adapter layout.
#[derive(Debug, Clone)]
pub(crate) struct OvenInteropAdapterStage {
    /// Reconstructed immutable-plan receipt that selected the source artifacts.
    pub(crate) receipt: OvenReceipt,
    /// Immutable selected direct-Rustc plan identity.
    pub(crate) plan_identity: String,
    /// Caller-owned manifest written relative to the new output root.
    pub(crate) manifest_path: PathBuf,
    /// Number of digest-verified bundled files staged for the adapter.
    pub(crate) bundled_files: usize,
}

/// Portable facts recorded in the caller-owned adapter manifest.
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
struct OvenInteropAdapterManifest {
    schema_version: u32,
    adapter: OvenInteropAdapter,
    target: String,
    locked_target_identity: String,
    execution_receipt_identity: String,
    final_receipt_identity: String,
    plan_identity: String,
    bundles: Vec<OvenInteropStagedBundle>,
    system_capabilities: Vec<String>,
}

/// One staged bundled file described only in output-relative coordinates.
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
struct OvenInteropStagedBundle {
    name: String,
    runtime_name: String,
    declared_placement: String,
    minimum_platform: String,
    digest: String,
    staged_path: String,
}

/// Inputs for one explicit Oven-owned native interop bake.
///
/// The base receipt selects the already sealed compiler runtime closure. The final receipt is derived internally by
/// adding this bake's selected interop execution identity, so the runtime and native archives remain one immutable
/// direct-Rustc plan rather than competing overlays.
pub(crate) struct OvenInteropNativeBakeRequest<'a> {
    /// Bounded store that owns both the base selection lease and the final immutable plan.
    pub(crate) store: &'a OvenStore,
    /// Package root that owns the locked header, artifact, and shim source inputs.
    pub(crate) project_root: &'a Path,
    /// Canonical target-specific inputs loaded from the current package lock.
    pub(crate) target: &'a LockedInteropTarget,
    /// Runtime-only receipt whose build unit deliberately predates this interop selection.
    pub(crate) base_receipt: &'a OvenReceipt,
    /// Selected toolchain/SDK contract that authorizes this native compilation.
    pub(crate) execution_receipt: &'a OvenInteropExecutionReceipt,
    /// Explicit selected C compiler. It is required only when the target declares a C shim.
    pub(crate) c_compiler: Option<&'a Path>,
    /// Explicit selected C++ compiler. It is required only when the target declares a C++ shim.
    pub(crate) cxx_compiler: Option<&'a Path>,
    /// Explicit selected archiver. It is required only when the target declares a shim.
    pub(crate) archiver: Option<&'a Path>,
    /// Explicit selected SDK root used as the compiler sysroot when the target requires an SDK.
    pub(crate) sdk_root: Option<&'a Path>,
    /// Exact regular SDK file whose bytes establish the selected SDK receipt identity.
    ///
    /// This remains an execution-only input: its local path is never recorded in the portable receipt or plan.
    pub(crate) sdk_identity_file: Option<&'a Path>,
}

/// Result of one receipt-bound interop native bake.
#[derive(Debug, Clone)]
pub(crate) struct OvenInteropNativeBake {
    /// Final receipt that normal Oven commands will independently reconstruct from the selected sidecar.
    pub(crate) receipt: OvenReceipt,
    /// Immutable direct-Rustc plan identity published under the final receipt.
    pub(crate) plan_identity: String,
    /// Logical names of the copied static archives and compiled shim archives available to direct Rustc.
    pub(crate) archive_names: Vec<String>,
    /// Logical names of locked bundled runtime files available to direct execution and target-native packaging.
    pub(crate) bundle_names: Vec<String>,
    /// Whether an already verified final plan was reused without starting a native tool.
    pub(crate) reused: bool,
}

/// Derive the portable build-unit inputs needed to select an interop-ready direct-Rustc plan.
pub(crate) fn interop_execution_build_unit_inputs(receipt: &OvenInteropExecutionReceipt) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            OVEN_INTEROP_EXECUTION_RECEIPT_INPUT.to_string(),
            receipt.identity.clone(),
        ),
        (
            OVEN_INTEROP_PLAN_SCHEMA_INPUT.to_string(),
            OVEN_INTEROP_PLAN_SCHEMA.to_string(),
        ),
    ])
}

/// Reconstruct the one immutable plan receipt shared by the native baker and every later adapter consumer.
///
/// Keeping this derivation here prevents a packager handoff from naming a plan that normal Oven execution would not
/// independently select from the same runtime receipt and persisted selected-execution contract.
pub(crate) fn final_interop_plan_receipt(
    base_receipt: &OvenReceipt,
    execution_receipt: &OvenInteropExecutionReceipt,
) -> Result<OvenReceipt, String> {
    let execution_bound_receipt = receipt_with_build_unit_input(
        base_receipt,
        OVEN_INTEROP_EXECUTION_RECEIPT_INPUT,
        execution_receipt.identity.clone(),
    )
    .map_err(|error| format!("could not derive final Oven interop receipt: {error}"))?;
    receipt_with_build_unit_input(
        &execution_bound_receipt,
        OVEN_INTEROP_PLAN_SCHEMA_INPUT,
        OVEN_INTEROP_PLAN_SCHEMA,
    )
    .map_err(|error| format!("could not derive final Oven interop plan receipt: {error}"))
}

/// Return the project-owned, target-specific location of the selected interop execution receipt.
#[must_use]
pub(crate) fn default_interop_execution_receipt_path(project_root: impl AsRef<Path>, target: &str) -> PathBuf {
    let target_identity = digest_bytes(target.as_bytes()).replace(':', "-");
    project_root
        .as_ref()
        .join(".incan")
        .join("oven")
        .join("interop")
        .join(format!("{target_identity}.json"))
}

/// Atomically persist a selected interop execution receipt for normal-command receipt construction.
pub(crate) fn write_interop_execution_receipt(
    receipt: &OvenInteropExecutionReceipt,
    path: impl AsRef<Path>,
) -> Result<(), String> {
    let path = path.as_ref();
    let parent = path
        .parent()
        .ok_or_else(|| format!("selected Oven interop receipt path {} has no parent", path.display()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("selected Oven interop receipt path {} has no file name", path.display()))?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "failed to create selected Oven interop receipt directory {}: {error}",
            parent.display()
        )
    })?;
    let payload = serde_json::to_vec_pretty(receipt)
        .map_err(|error| format!("failed to serialize selected Oven interop receipt: {error}"))?;
    let staged = parent.join(format!(".{}.tmp-{}", file_name.to_string_lossy(), std::process::id()));
    let result = crate::oven::write_receipt_staged(&payload, &staged, path, parent);
    if result.is_err() && staged.exists() {
        let _ = fs::remove_file(&staged);
    }
    result.map_err(|error| {
        format!(
            "failed to write selected Oven interop receipt {}: {error}",
            path.display()
        )
    })
}

/// Load one previously selected interop execution receipt without discovering a compiler, SDK, or native artifact.
pub(crate) fn load_interop_execution_receipt(path: impl AsRef<Path>) -> Result<OvenInteropExecutionReceipt, String> {
    let path = path.as_ref();
    let payload = fs::read(path).map_err(|error| {
        format!(
            "failed to read selected Oven interop receipt {}: {error}",
            path.display()
        )
    })?;
    serde_json::from_slice(&payload)
        .map_err(|error| format!("invalid selected Oven interop receipt {}: {error}", path.display()))
}

/// Require a persisted selected receipt to match the current locked target and its declared input identities.
pub(crate) fn validate_interop_execution_receipt(
    target: &LockedInteropTarget,
    receipt: &OvenInteropExecutionReceipt,
) -> Result<(), String> {
    let expected = receipt_interop_execution(target, receipt.toolchain.clone(), receipt.sdk.clone())?;
    if &expected != receipt {
        return Err(format!(
            "selected Oven interop receipt for target `{}` no longer matches its locked requirements",
            target.target
        ));
    }
    Ok(())
}

/// Combine a selected compiler runtime closure with interop-native archive evidence into one final Rustc plan.
///
/// `base` is only a publisher input. The returned plan keeps its full runtime closure, adds sealed static and bundled
/// native search directories, and includes a small provenance file as a regular digest-verified supporting artifact.
/// Consumers see only the returned plan under the final receipt, preventing a native-only overlay from competing
/// with its runtime.
pub(crate) fn bind_interop_native_archives(
    base: &OvenRustcArtifactManifest,
    final_intent: &OvenBuildIntent,
    receipt: &OvenInteropExecutionReceipt,
    archives: &[OvenInteropBakedArchive],
    bundles: &[OvenInteropBakedBundle],
    system_capabilities: &[String],
) -> Result<(OvenRustcArtifactManifest, Vec<u8>), String> {
    if &base.intent != final_intent {
        return Err("interop native plan base does not match the final direct-Rustc intent".to_string());
    }
    if receipt.target != final_intent.target {
        return Err(format!(
            "selected interop receipt target `{}` does not match final Rust target `{}`",
            receipt.target, final_intent.target
        ));
    }
    let mut seen_names = std::collections::BTreeSet::new();
    let mut seen_paths = std::collections::BTreeSet::new();
    for archive in archives {
        if !is_interop_native_library_name(&archive.name) {
            return Err(format!(
                "interop archive name `{}` is not a safe native library name",
                archive.name
            ));
        }
        let expected_path = format!("{OVEN_INTEROP_NATIVE_DIRECTORY}/lib{}.a", archive.name);
        if archive.relative_path != expected_path {
            return Err(format!(
                "interop archive `{}` must use store-owned path `{expected_path}`",
                archive.name
            ));
        }
        if !archive.digest.starts_with("sha256:") || archive.digest.len() != 71 {
            return Err(format!(
                "interop archive `{}` has no canonical SHA-256 digest",
                archive.name
            ));
        }
        if !seen_names.insert(&archive.name) || !seen_paths.insert(&archive.relative_path) {
            return Err("interop native plan repeats a baked archive name or path".to_string());
        }
    }
    let mut seen_bundle_names = std::collections::BTreeSet::new();
    for bundle in bundles {
        let expected_path = bundled_artifact_relative_path(&bundle.name, &bundle.runtime_name)?;
        if bundle.relative_path != expected_path {
            return Err(format!(
                "interop bundled artifact `{}` must use store-owned path `{expected_path}`",
                bundle.name
            ));
        }
        if !bundle.digest.starts_with("sha256:") || bundle.digest.len() != 71 {
            return Err(format!(
                "interop bundled artifact `{}` has no canonical SHA-256 digest",
                bundle.name
            ));
        }
        if bundle.placement.trim().is_empty() || bundle.minimum_platform.trim().is_empty() {
            return Err(format!(
                "interop bundled artifact `{}` has incomplete target packaging metadata",
                bundle.name
            ));
        }
        if !seen_bundle_names.insert(&bundle.name) || !seen_paths.insert(&bundle.relative_path) {
            return Err("interop native plan repeats a bundled artifact name or materialized path".to_string());
        }
    }
    let mut archives = archives.to_vec();
    archives.sort_by(|left, right| left.name.cmp(&right.name));
    let mut bundles = bundles.to_vec();
    bundles.sort_by(|left, right| left.name.cmp(&right.name));
    let mut system_capabilities = system_capabilities.to_vec();
    system_capabilities.sort();
    system_capabilities.dedup();
    if system_capabilities
        .iter()
        .any(|capability| capability.trim().is_empty())
    {
        return Err("interop native plan has an empty selected system capability".to_string());
    }
    let provenance = OvenInteropExecutionProvenance {
        schema_version: OVEN_INTEROP_EXECUTION_PROVENANCE_SCHEMA_VERSION,
        receipt: receipt.clone(),
        archives: archives.clone(),
        bundles: bundles.clone(),
        system_capabilities,
    };
    let provenance_bytes = serde_json::to_vec_pretty(&provenance)
        .map_err(|error| format!("failed to serialize Oven interop execution provenance: {error}"))?;

    let mut plan = base.clone();
    if !archives.is_empty()
        && !plan
            .native_search_paths
            .iter()
            .any(|path| path == OVEN_INTEROP_NATIVE_DIRECTORY)
    {
        plan.native_search_paths.push(OVEN_INTEROP_NATIVE_DIRECTORY.to_string());
    }
    for archive in &archives {
        if plan
            .supporting_artifacts
            .iter()
            .any(|artifact| artifact.relative_path == archive.relative_path)
        {
            return Err(format!(
                "base direct-Rustc plan already declares interop archive `{}`",
                archive.relative_path
            ));
        }
        plan.supporting_artifacts.push(OvenRustcSupportingArtifact {
            relative_path: archive.relative_path.clone(),
            digest: archive.digest.clone(),
        });
    }
    for bundle in &bundles {
        // A bundled runtime is still selected only through this final receipt and its digest-verified supporting
        // artifact. Giving Rustc its sealed parent directory lets a checked binding link that declared runtime
        // directly; it never turns a mutable package directory into a native search path. The same stored file
        // remains available to Android/iOS adapter staging below.
        let native_search_path = format!("{OVEN_INTEROP_BUNDLED_DIRECTORY}/{}", bundle.name);
        if !plan.native_search_paths.iter().any(|path| path == &native_search_path) {
            plan.native_search_paths.push(native_search_path);
        }
        if plan
            .supporting_artifacts
            .iter()
            .any(|artifact| artifact.relative_path == bundle.relative_path)
        {
            return Err(format!(
                "base direct-Rustc plan already declares interop bundled artifact `{}`",
                bundle.relative_path
            ));
        }
        plan.supporting_artifacts.push(OvenRustcSupportingArtifact {
            relative_path: bundle.relative_path.clone(),
            digest: bundle.digest.clone(),
        });
    }
    if plan
        .supporting_artifacts
        .iter()
        .any(|artifact| artifact.relative_path == OVEN_INTEROP_EXECUTION_PROVENANCE_PATH)
    {
        return Err("base direct-Rustc plan already declares Oven interop provenance".to_string());
    }
    plan.supporting_artifacts.push(OvenRustcSupportingArtifact {
        relative_path: OVEN_INTEROP_EXECUTION_PROVENANCE_PATH.to_string(),
        digest: digest_bytes(&provenance_bytes),
    });
    plan.native_search_paths.sort();
    plan.native_search_paths.dedup();
    plan.supporting_artifacts
        .sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok((plan, provenance_bytes))
}

/// Compile or copy every locked static interop input and publish one final direct-Rustc plan.
///
/// The returned plan is deliberately complete: its verified materializations begin with the selected base runtime
/// closure and then add package-owned static archives, compiled C/C++ shims, and the selected-execution provenance.
/// The base selection lease remains held until publication completes, which prevents policy pruning from replacing a
/// trusted runtime input between validation and final immutable publication.
pub(crate) fn bake_interop_native_plan(
    request: OvenInteropNativeBakeRequest<'_>,
) -> Result<OvenInteropNativeBake, String> {
    validate_interop_bake_request(&request)?;
    let final_receipt = final_interop_plan_receipt(request.base_receipt, request.execution_receipt)?;
    if let Some(selected) = select_direct_rustc_plan_for_execution(request.store, &final_receipt)
        .map_err(|error| format!("could not select an existing Oven interop plan: {error}"))?
    {
        let validated =
            validate_existing_interop_plan(&selected, &final_receipt, request.execution_receipt, request.target)?;
        let mut archive_names = validated
            .provenance
            .archives
            .iter()
            .map(|archive| archive.name.clone())
            .collect::<Vec<_>>();
        archive_names.sort();
        archive_names.dedup();
        let mut bundle_names = validated
            .provenance
            .bundles
            .iter()
            .map(|bundle| bundle.name.clone())
            .collect::<Vec<_>>();
        bundle_names.sort();
        bundle_names.dedup();
        return Ok(OvenInteropNativeBake {
            receipt: final_receipt,
            plan_identity: selected.manifest.identity,
            archive_names,
            bundle_names,
            reused: true,
        });
    }
    let selected = select_direct_rustc_plan_for_execution(request.store, request.base_receipt)
        .map_err(|error| format!("could not select base Oven runtime plan: {error}"))?
        .ok_or_else(|| {
            "Oven interop bake has no compatible base direct-Rustc runtime plan; prepare the sealed runtime Loaf before baking native interop"
                .to_string()
        })?;
    let base_manifest = selected.manifest;
    if base_manifest.kind != OvenArtifactKind::DirectRustcPlan
        || base_manifest.build_unit_identity != request.base_receipt.build_unit_identity
        || base_manifest.intent != request.base_receipt.intent
    {
        return Err("selected base Oven entry is not authorized by the runtime receipt".to_string());
    }
    let base_plan = serde_json::from_slice::<OvenRustcArtifactManifest>(&selected.payload)
        .map_err(|error| format!("selected base Oven runtime payload is not a direct-Rustc plan: {error}"))?;
    let mut materialized_files = base_plan
        .materialized_artifacts(&selected.artifact_root, &request.base_receipt.intent)
        .map_err(|error| format!("selected base Oven runtime plan is invalid: {error}"))?
        .into_iter()
        .map(|artifact| OvenArtifactMaterializedFile {
            source_path: artifact.source_path,
            relative_path: artifact.relative_path,
        })
        .collect::<Vec<_>>();

    // This owned staging directory is deliberately below the store root. It is publisher-owned transient state, cleaned
    // on every return path, and never becomes a normal command's cache or link-path authority.
    fs::create_dir_all(request.store.root()).map_err(|error| {
        format!(
            "could not create Oven interop store root {}: {error}",
            request.store.root().display()
        )
    })?;
    let staging = LoafTemporaryDirectory::create(request.store.root(), ".interop-bake-")
        .map_err(|error| format!("could not create Oven interop bake staging: {error}"))?;
    let native_root = staging.path().join(OVEN_INTEROP_NATIVE_DIRECTORY);
    fs::create_dir_all(&native_root).map_err(|error| {
        format!(
            "could not create Oven interop native staging directory {}: {error}",
            native_root.display()
        )
    })?;
    let bundled_root = staging.path().join(OVEN_INTEROP_BUNDLED_DIRECTORY);
    fs::create_dir_all(&bundled_root).map_err(|error| {
        format!(
            "could not create Oven interop bundled staging directory {}: {error}",
            bundled_root.display()
        )
    })?;

    let mut archives = copy_locked_static_archives(request.project_root, request.target, &native_root)?;
    archives.extend(compile_locked_interop_shims(&request, &native_root)?);
    archives.sort_by(|left, right| left.name.cmp(&right.name));
    if archives.windows(2).any(|pair| pair[0].name == pair[1].name) {
        return Err(
            "locked interop static artifacts and shim outputs must not share a native library name".to_string(),
        );
    }
    let bundles = copy_locked_bundled_artifacts(request.project_root, request.target, &bundled_root)?;
    let system_capabilities = locked_system_capabilities(request.target)?;
    let (final_plan, provenance) = bind_interop_native_archives(
        &base_plan,
        &final_receipt.intent,
        request.execution_receipt,
        &archives,
        &bundles,
        &system_capabilities,
    )?;
    for archive in &archives {
        materialized_files.push(OvenArtifactMaterializedFile {
            source_path: staging.path().join(&archive.relative_path),
            relative_path: archive.relative_path.clone(),
        });
    }
    for bundle in &bundles {
        materialized_files.push(OvenArtifactMaterializedFile {
            source_path: staging.path().join(&bundle.relative_path),
            relative_path: bundle.relative_path.clone(),
        });
    }
    let provenance_path = staging.path().join(OVEN_INTEROP_EXECUTION_PROVENANCE_PATH);
    let provenance_parent = provenance_path.parent().ok_or_else(|| {
        format!(
            "Oven interop provenance path {} has no parent",
            provenance_path.display()
        )
    })?;
    fs::create_dir_all(provenance_parent).map_err(|error| {
        format!(
            "could not create Oven interop provenance directory {}: {error}",
            provenance_parent.display()
        )
    })?;
    fs::write(&provenance_path, provenance).map_err(|error| {
        format!(
            "could not write Oven interop provenance {}: {error}",
            provenance_path.display()
        )
    })?;
    materialized_files.push(OvenArtifactMaterializedFile {
        source_path: provenance_path,
        relative_path: OVEN_INTEROP_EXECUTION_PROVENANCE_PATH.to_string(),
    });
    materialized_files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    if materialized_files
        .windows(2)
        .any(|pair| pair[0].relative_path == pair[1].relative_path)
    {
        return Err("final Oven interop plan repeats one materialized artifact path".to_string());
    }
    let payload = serde_json::to_vec(&final_plan)
        .map_err(|error| format!("could not encode final Oven interop direct-Rustc plan: {error}"))?;
    let domain = format!(
        "interop-{}",
        final_receipt
            .build_unit_identity
            .strip_prefix("sha256:")
            .unwrap_or(&final_receipt.build_unit_identity)
    );
    let published = request
        .store
        .publish(&OvenArtifactPublishRequest {
            receipt: final_receipt.clone(),
            domain,
            kind: OvenArtifactKind::DirectRustcPlan,
            payload,
            materialized_files,
        })
        .map_err(|error| format!("could not publish final Oven interop direct-Rustc plan: {error}"))?;
    Ok(OvenInteropNativeBake {
        receipt: final_receipt,
        plan_identity: published.identity,
        archive_names: archives.into_iter().map(|archive| archive.name).collect(),
        bundle_names: bundles.into_iter().map(|bundle| bundle.name).collect(),
        reused: false,
    })
}

/// Complete, receipt-bound interop-plan evidence selected for a warm reuse.
struct ValidatedInteropPlan {
    artifacts: Vec<crate::oven::rustc::OvenRustcMaterializedArtifact>,
    provenance: OvenInteropExecutionProvenance,
}

/// Validate a matching final receipt's complete, receipt-bound interop plan before reuse.
///
/// Selection already validates the direct-Rustc plan shape and retains an execution lease. This additional check
/// verifies every retained materialized file, requires the private provenance record, and rejects a plan whose
/// selected compiler/SDK identity does not exactly equal this bake request. A stale or incomplete hit is therefore
/// never mistaken for a cheap warm reuse.
fn validate_existing_interop_plan(
    selected: &crate::oven::store::OvenStoreExecutionPayload,
    receipt: &OvenReceipt,
    execution_receipt: &OvenInteropExecutionReceipt,
    target: &LockedInteropTarget,
) -> Result<ValidatedInteropPlan, String> {
    let plan = serde_json::from_slice::<OvenRustcArtifactManifest>(&selected.payload)
        .map_err(|error| format!("existing Oven interop payload is not a direct-Rustc plan: {error}"))?;
    let artifacts = plan
        .materialized_artifacts(&selected.artifact_root, &receipt.intent)
        .map_err(|error| format!("existing Oven interop plan is invalid: {error}"))?;
    let provenance = artifacts
        .iter()
        .find(|artifact| artifact.relative_path == OVEN_INTEROP_EXECUTION_PROVENANCE_PATH)
        .ok_or_else(|| "existing Oven interop plan has no selected-execution provenance".to_string())?;
    let bytes = fs::read(&provenance.source_path).map_err(|error| {
        format!(
            "could not read existing Oven interop provenance {}: {error}",
            provenance.source_path.display()
        )
    })?;
    let provenance = serde_json::from_slice::<OvenInteropExecutionProvenance>(&bytes)
        .map_err(|error| format!("existing Oven interop provenance is invalid: {error}"))?;
    if provenance.schema_version != OVEN_INTEROP_EXECUTION_PROVENANCE_SCHEMA_VERSION
        || provenance.receipt != *execution_receipt
    {
        return Err("existing Oven interop plan provenance does not match the selected execution receipt".to_string());
    }
    let expected_system_capabilities = locked_system_capabilities(target)?;
    if provenance.system_capabilities != expected_system_capabilities {
        return Err("existing Oven interop plan has stale selected system capabilities".to_string());
    }
    for bundle in &provenance.bundles {
        let materialized = artifacts
            .iter()
            .find(|artifact| artifact.relative_path == bundle.relative_path)
            .ok_or_else(|| format!("existing Oven interop plan has no bundled artifact `{}`", bundle.name))?;
        let digest = digest_regular_file(&materialized.source_path, "existing Oven interop bundled artifact")?;
        if digest != bundle.digest {
            return Err(format!(
                "existing Oven interop bundled artifact `{}` has inconsistent digest provenance",
                bundle.name
            ));
        }
    }
    Ok(ValidatedInteropPlan { artifacts, provenance })
}

/// Atomically stage one selected interop plan's bundled runtime files for a fixed Android or iOS consumer layout.
///
/// The operation deliberately consumes the same reconstructed final receipt as normal Oven execution. It neither
/// rereads package inputs nor discovers host tools, and its selector retains an active store lease until every staged
/// file is copied and the caller-owned directory has been atomically published.
pub(crate) fn stage_interop_adapter(
    request: OvenInteropAdapterStageRequest<'_>,
) -> Result<OvenInteropAdapterStage, String> {
    validate_interop_adapter_stage_request(&request)?;
    let receipt = final_interop_plan_receipt(request.base_receipt, request.execution_receipt)?;
    let selected = select_direct_rustc_plan_for_execution(request.store, &receipt)
        .map_err(|error| format!("could not select Oven interop plan for adapter staging: {error}"))?
        .ok_or_else(|| {
            "Oven interop adapter staging has no compatible final direct-Rustc plan; run `incan oven interop bake` for this locked target first"
                .to_string()
        })?;
    let plan_identity = selected.manifest.identity.clone();
    let validated = validate_existing_interop_plan(&selected, &receipt, request.execution_receipt, request.target)?;
    let output = request.output;
    match fs::symlink_metadata(output) {
        Ok(_) => {
            return Err(format!(
                "Oven interop adapter output already exists and will not be replaced: {}",
                output.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "could not inspect Oven interop adapter output {}: {error}",
                output.display()
            ));
        }
    }
    let parent = output.parent().ok_or_else(|| {
        format!(
            "Oven interop adapter output {} has no parent directory",
            output.display()
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "could not create Oven interop adapter output parent {}: {error}",
            parent.display()
        )
    })?;
    let staging = LoafTemporaryDirectory::create(parent, ".incan-interop-stage-")
        .map_err(|error| format!("could not create Oven interop adapter staging directory: {error}"))?;
    let mut staged_bundles = Vec::new();
    let mut staged_paths = BTreeSet::new();
    for bundle in &validated.provenance.bundles {
        let source = validated
            .artifacts
            .iter()
            .find(|artifact| artifact.relative_path == bundle.relative_path)
            .ok_or_else(|| {
                format!(
                    "selected Oven interop plan lost bundled artifact `{}` while staging",
                    bundle.name
                )
            })?;
        let staged_path = interop_adapter_bundle_path(request.adapter, request.target, bundle)?;
        if !staged_paths.insert(staged_path.clone()) {
            return Err(format!(
                "selected Oven interop plan maps multiple bundled artifacts to adapter path `{staged_path}`"
            ));
        }
        let destination = staging.path().join(&staged_path);
        let destination_parent = destination.parent().ok_or_else(|| {
            format!(
                "Oven interop adapter destination {} has no parent directory",
                destination.display()
            )
        })?;
        fs::create_dir_all(destination_parent).map_err(|error| {
            format!(
                "could not create Oven interop adapter destination {}: {error}",
                destination_parent.display()
            )
        })?;
        fs::copy(&source.source_path, &destination).map_err(|error| {
            format!(
                "could not stage Oven interop bundled artifact {} at {}: {error}",
                source.source_path.display(),
                destination.display()
            )
        })?;
        let actual = digest_regular_file(&destination, "staged Oven interop bundled artifact")?;
        if actual != bundle.digest {
            return Err(format!(
                "staged Oven interop bundled artifact `{}` does not match its selected digest",
                bundle.name
            ));
        }
        staged_bundles.push(OvenInteropStagedBundle {
            name: bundle.name.clone(),
            runtime_name: bundle.runtime_name.clone(),
            declared_placement: bundle.placement.clone(),
            minimum_platform: bundle.minimum_platform.clone(),
            digest: bundle.digest.clone(),
            staged_path,
        });
    }
    staged_bundles.sort_by(|left, right| left.name.cmp(&right.name));
    let manifest = OvenInteropAdapterManifest {
        schema_version: OVEN_INTEROP_ADAPTER_MANIFEST_SCHEMA_VERSION,
        adapter: request.adapter,
        target: request.target.target.clone(),
        locked_target_identity: locked_interop_target_identity(request.target)?,
        execution_receipt_identity: request.execution_receipt.identity.clone(),
        final_receipt_identity: receipt.identity.clone(),
        plan_identity: plan_identity.clone(),
        bundles: staged_bundles,
        system_capabilities: validated.provenance.system_capabilities,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| format!("could not serialize Oven interop adapter manifest: {error}"))?;
    fs::write(staging.path().join(OVEN_INTEROP_ADAPTER_MANIFEST_PATH), manifest_bytes).map_err(|error| {
        format!(
            "could not write Oven interop adapter manifest in {}: {error}",
            staging.path().display()
        )
    })?;
    publish_new_adapter_directory(staging.path(), output).map_err(|error| {
        format!(
            "could not atomically publish Oven interop adapter output {}: {error}",
            output.display()
        )
    })?;
    let _published = staging.persist();
    Ok(OvenInteropAdapterStage {
        receipt,
        plan_identity,
        manifest_path: output.join(OVEN_INTEROP_ADAPTER_MANIFEST_PATH),
        bundled_files: manifest.bundles.len(),
    })
}

/// Atomically publish a new caller-owned adapter directory without replacing an existing directory.
///
/// The staging directory and destination share a parent. macOS and Linux provide no-replace rename operations that
/// preserve this invariant even if another process creates the destination after the caller's earlier inspection.
/// Hosts without that primitive fail closed rather than silently replacing caller-owned output.
fn publish_new_adapter_directory(staging: &Path, output: &Path) -> std::io::Result<()> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let parent = output.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("adapter output {} has no parent directory", output.display()),
            )
        })?;
        let staging_name = staging.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("adapter staging path {} has no final component", staging.display()),
            )
        })?;
        let output_name = output.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("adapter output path {} has no final component", output.display()),
            )
        })?;
        let directory = fs::File::open(parent)?;
        rustix::fs::renameat_with(
            &directory,
            staging_name,
            &directory,
            output_name,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (staging, output);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Oven interop adapter staging requires an atomic no-replace rename primitive on this host",
        ))
    }
}

/// Require a lock-fresh mobile profile and a matching fixed adapter before any caller-owned files are created.
fn validate_interop_adapter_stage_request(request: &OvenInteropAdapterStageRequest<'_>) -> Result<(), String> {
    request
        .base_receipt
        .verify_identity()
        .map_err(|error| format!("base Oven interop receipt is invalid: {error}"))?;
    if request.base_receipt.intent.target != request.target.target {
        return Err(format!(
            "base Oven runtime target `{}` does not match locked interop target `{}`",
            request.base_receipt.intent.target, request.target.target
        ));
    }
    validate_interop_execution_receipt(request.target, request.execution_receipt)?;
    match request.adapter {
        OvenInteropAdapter::Android
            if request.target.target == "aarch64-linux-android"
                && matches!(
                    request.target.platform.as_ref(),
                    Some(InteropTargetPlatform::Android { .. })
                ) =>
        {
            Ok(())
        }
        OvenInteropAdapter::Ios
            if ios_target_kind(&request.target.target).is_some()
                && matches!(
                    request.target.platform.as_ref(),
                    Some(InteropTargetPlatform::Ios { .. })
                ) =>
        {
            Ok(())
        }
        OvenInteropAdapter::Android => Err(format!(
            "Android adapter staging requires the locked Android arm64 platform target, found `{target}`",
            target = request.target.target
        )),
        OvenInteropAdapter::Ios => Err(format!(
            "iOS adapter staging requires the locked iOS device or simulator platform target, found `{target}`",
            target = request.target.target
        )),
    }
}

/// Translate one verified bundled runtime into the adapter's fixed relative output layout.
fn interop_adapter_bundle_path(
    adapter: OvenInteropAdapter,
    target: &LockedInteropTarget,
    bundle: &OvenInteropBakedBundle,
) -> Result<String, String> {
    validate_bundled_path_component(&bundle.runtime_name, "runtime name")?;
    let prefix = match adapter {
        OvenInteropAdapter::Android => {
            if target.target != "aarch64-linux-android" {
                return Err(format!(
                    "Android adapter cannot stage non-Android target `{}`",
                    target.target
                ));
            }
            "jniLibs/arm64-v8a"
        }
        OvenInteropAdapter::Ios => {
            if ios_target_kind(&target.target).is_none() {
                return Err(format!("iOS adapter cannot stage non-iOS target `{}`", target.target));
            }
            "Frameworks"
        }
    };
    Ok(format!("{prefix}/{}", bundle.runtime_name))
}

/// Reject a native bake whose selected capability receipt, compiler inputs, or target profile is incomplete.
fn validate_interop_bake_request(request: &OvenInteropNativeBakeRequest<'_>) -> Result<(), String> {
    request
        .base_receipt
        .verify_identity()
        .map_err(|error| format!("base Oven interop receipt is invalid: {error}"))?;
    if request.base_receipt.intent.target != request.target.target {
        return Err(format!(
            "base Oven runtime target `{}` does not match locked interop target `{}`",
            request.base_receipt.intent.target, request.target.target
        ));
    }
    if request
        .base_receipt
        .sources
        .build_unit_inputs
        .keys()
        .any(|key| key == OVEN_INTEROP_EXECUTION_RECEIPT_INPUT || key == OVEN_INTEROP_PLAN_SCHEMA_INPUT)
    {
        return Err(
            "base Oven interop receipt must not already select native execution or a final-plan schema".to_string(),
        );
    }
    validate_interop_execution_receipt(request.target, request.execution_receipt)?;
    let has_c_shim = request
        .target
        .shims
        .iter()
        .any(|shim| shim.language == InteropShimLanguage::C);
    let has_cxx_shim = request
        .target
        .shims
        .iter()
        .any(|shim| shim.language == InteropShimLanguage::Cxx);
    if (has_c_shim || has_cxx_shim) && request.target.toolchain.is_none() {
        return Err(format!(
            "locked Oven interop target `{}` declares native shims but no toolchain capability",
            request.target.target
        ));
    }
    if let Some(selection) = &request.execution_receipt.toolchain {
        let selected_identity = selected_interop_toolchain_identity(
            request.target,
            request.c_compiler,
            request.cxx_compiler,
            request.archiver,
        )?;
        if selection.identity != selected_identity {
            return Err(format!(
                "selected Oven interop toolchain identity no longer matches the declared compiler/archiver executables: expected {}, got {selected_identity}",
                selection.identity
            ));
        }
    }
    validate_selected_sdk_identity(request)?;
    Ok(())
}

/// Recheck the selected SDK file immediately before publication.
///
/// The CLI makes this selection initially, but the core baker owns the admission decision. Recomputing the digest
/// here prevents a changed SDK descriptor, an identity-file symlink, or a mismatched local root from reusing a
/// receipt-bound native plan.
fn validate_selected_sdk_identity(request: &OvenInteropNativeBakeRequest<'_>) -> Result<(), String> {
    let Some(_requirement) = request.target.sdk.as_ref() else {
        if request.sdk_root.is_some() || request.sdk_identity_file.is_some() {
            return Err("locked Oven interop target does not declare an SDK capability".to_string());
        }
        return Ok(());
    };
    let root = request
        .sdk_root
        .ok_or_else(|| "locked Oven interop target requires an explicit selected SDK root".to_string())?;
    let root = fs::canonicalize(root).map_err(|error| {
        format!(
            "could not canonicalize selected Oven SDK root {}: {error}",
            root.display()
        )
    })?;
    if !root.is_dir() {
        return Err(format!("selected Oven SDK root is not a directory: {}", root.display()));
    }
    let identity_file = request
        .sdk_identity_file
        .ok_or_else(|| "locked Oven interop target requires an explicit selected SDK identity file".to_string())?;
    let metadata = fs::symlink_metadata(identity_file).map_err(|error| {
        format!(
            "could not inspect selected Oven SDK identity file {}: {error}",
            identity_file.display()
        )
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "selected Oven SDK identity file must be a regular non-symlink file: {}",
            identity_file.display()
        ));
    }
    let identity_file = fs::canonicalize(identity_file).map_err(|error| {
        format!(
            "could not canonicalize selected Oven SDK identity file {}: {error}",
            identity_file.display()
        )
    })?;
    if !identity_file.starts_with(&root) {
        return Err(format!(
            "selected Oven SDK identity file {} is outside selected SDK root {}",
            identity_file.display(),
            root.display()
        ));
    }
    let selected =
        request.execution_receipt.sdk.as_ref().ok_or_else(|| {
            "selected Oven interop execution receipt is missing its required SDK capability".to_string()
        })?;
    let actual = digest_regular_file(&identity_file, "selected Oven SDK identity file")?;
    if selected.identity != actual {
        return Err(format!(
            "selected Oven SDK identity no longer matches {}: expected {}, got {actual}",
            identity_file.display(),
            selected.identity
        ));
    }
    Ok(())
}

/// Return one portable identity for every selected native executable that can affect a target's shim archives.
///
/// A C++ compiler or archiver is not interchangeable with the C compiler that establishes a capability version.
/// Binding all executable bytes into the one selected-toolchain identity prevents a changed secondary tool from
/// silently reusing a plan whose receipt only named the primary compiler.
pub(crate) fn selected_interop_toolchain_identity(
    target: &LockedInteropTarget,
    c_compiler: Option<&Path>,
    cxx_compiler: Option<&Path>,
    archiver: Option<&Path>,
) -> Result<String, String> {
    let c_compiler = c_compiler.ok_or_else(|| {
        format!(
            "locked Oven interop target `{}` requires a selected C compiler for its toolchain capability",
            target.target
        )
    })?;
    validate_regular_executable(Some(c_compiler), "selected C compiler")?;
    let mut identities = BTreeMap::from([(
        "c-compiler".to_string(),
        digest_regular_executable(c_compiler, "selected C compiler")?,
    )]);
    let has_cxx_shim = target
        .shims
        .iter()
        .any(|shim| shim.language == InteropShimLanguage::Cxx);
    if has_cxx_shim {
        let cxx_compiler = cxx_compiler.ok_or_else(|| {
            format!(
                "locked Oven interop target `{}` requires a selected C++ compiler for its C++ shim",
                target.target
            )
        })?;
        validate_regular_executable(Some(cxx_compiler), "selected C++ compiler")?;
        identities.insert(
            "cxx-compiler".to_string(),
            digest_regular_executable(cxx_compiler, "selected C++ compiler")?,
        );
    }
    let has_shim = target
        .shims
        .iter()
        .any(|shim| matches!(shim.language, InteropShimLanguage::C | InteropShimLanguage::Cxx));
    if has_shim {
        let archiver = archiver.ok_or_else(|| {
            format!(
                "locked Oven interop target `{}` requires a selected native archiver for its shim",
                target.target
            )
        })?;
        validate_regular_executable(Some(archiver), "selected native archiver")?;
        identities.insert(
            "archiver".to_string(),
            digest_regular_executable(archiver, "selected native archiver")?,
        );
    }
    digest_serialized(&identities, "selected Oven interop toolchain executables")
}

/// Require an explicitly selected regular executable before it can start a compiler-owned native process.
fn validate_regular_executable(path: Option<&Path>, label: &str) -> Result<(), String> {
    let path = path.ok_or_else(|| format!("Oven interop bake requires {label}"))?;
    let _ = canonical_regular_executable(path, label)?;
    Ok(())
}

/// Resolve one selected executable and verify that the resolved target is a regular executable file.
///
/// System compiler entry points commonly use a stable symlink such as `/usr/bin/cc`. The resolved target—not the
/// mutable symlink path—is what Oven digests and authorizes for this one bake.
fn canonical_regular_executable(path: &Path, label: &str) -> Result<PathBuf, String> {
    let resolved = path
        .canonicalize()
        .map_err(|error| format!("could not resolve {label} {}: {error}", path.display()))?;
    let metadata = fs::metadata(&resolved)
        .map_err(|error| format!("could not inspect resolved {label} {}: {error}", resolved.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "{label} must resolve to a regular executable file: {}",
            path.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(format!("{label} is not executable: {}", path.display()));
        }
    }
    Ok(resolved)
}

/// Digest the resolved regular executable that an Oven interop bake is allowed to launch.
fn digest_regular_executable(path: &Path, label: &str) -> Result<String, String> {
    let resolved = canonical_regular_executable(path, label)?;
    digest_regular_file(&resolved, label)
}

/// Copy every package-declared static archive after rechecking its locked path and digest.
fn copy_locked_static_archives(
    project_root: &Path,
    target: &LockedInteropTarget,
    native_root: &Path,
) -> Result<Vec<OvenInteropBakedArchive>, String> {
    let mut archives = Vec::new();
    for artifact in &target.artifacts {
        if artifact.kind != InteropArtifactKind::Static {
            continue;
        }
        if !is_interop_native_library_name(&artifact.name) {
            return Err(format!(
                "locked static interop artifact `{}` has an unsafe native library name",
                artifact.name
            ));
        }
        let input = artifact.input.as_ref().ok_or_else(|| {
            format!(
                "locked static interop artifact `{}` has no package input",
                artifact.name
            )
        })?;
        let source = verified_locked_input(project_root, input, "static interop artifact")?;
        let relative_path = format!("{OVEN_INTEROP_NATIVE_DIRECTORY}/lib{}.a", artifact.name);
        let destination = native_root.join(format!("lib{}.a", artifact.name));
        fs::copy(&source, &destination).map_err(|error| {
            format!(
                "could not stage static interop artifact {} at {}: {error}",
                source.display(),
                destination.display()
            )
        })?;
        let digest = digest_regular_file(&destination, "staged static interop artifact")?;
        archives.push(OvenInteropBakedArchive {
            name: artifact.name.clone(),
            relative_path,
            digest,
            origin: artifact.origin.clone(),
        });
    }
    Ok(archives)
}

/// Copy every package-declared bundled runtime file after rechecking its locked path and digest.
///
/// Bundled inputs become digest-verified supporting artifacts and sealed direct-Rustc native search directories in
/// the same final plan. Direct linking therefore receives only the receipt-selected store copy, while a Gradle/Xcode
/// adapter can stage that identical immutable file without rereading mutable package inputs.
fn copy_locked_bundled_artifacts(
    project_root: &Path,
    target: &LockedInteropTarget,
    bundled_root: &Path,
) -> Result<Vec<OvenInteropBakedBundle>, String> {
    let mut bundles = Vec::new();
    for artifact in &target.artifacts {
        if artifact.kind != InteropArtifactKind::Bundled {
            continue;
        }
        let input = artifact.input.as_ref().ok_or_else(|| {
            format!(
                "locked bundled interop artifact `{}` has no package input",
                artifact.name
            )
        })?;
        let runtime_name = required_bundled_artifact_field(artifact.runtime_name.as_deref(), artifact, "runtime name")?;
        let placement = required_bundled_artifact_field(artifact.placement.as_deref(), artifact, "placement")?;
        let minimum_platform =
            required_bundled_artifact_field(artifact.minimum_platform.as_deref(), artifact, "minimum platform")?;
        let relative_path = bundled_artifact_relative_path(&artifact.name, &runtime_name)?;
        let source = verified_locked_input(project_root, input, "bundled interop artifact")?;
        let destination = bundled_root
            .parent()
            .ok_or_else(|| {
                format!(
                    "Oven interop bundled staging directory {} has no parent",
                    bundled_root.display()
                )
            })?
            .join(&relative_path);
        let parent = destination.parent().ok_or_else(|| {
            format!(
                "Oven interop bundled destination {} has no parent",
                destination.display()
            )
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "could not create Oven interop bundled artifact directory {}: {error}",
                parent.display()
            )
        })?;
        fs::copy(&source, &destination).map_err(|error| {
            format!(
                "could not stage bundled interop artifact {} at {}: {error}",
                source.display(),
                destination.display()
            )
        })?;
        bundles.push(OvenInteropBakedBundle {
            name: artifact.name.clone(),
            relative_path,
            digest: digest_regular_file(&destination, "staged bundled interop artifact")?,
            runtime_name,
            placement,
            minimum_platform,
            origin: artifact.origin.clone(),
        });
    }
    bundles.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(bundles)
}

/// Return the exact store-owned path for one bundled runtime file without trusting package path spelling.
fn bundled_artifact_relative_path(name: &str, runtime_name: &str) -> Result<String, String> {
    validate_bundled_path_component(name, "artifact name")?;
    validate_bundled_path_component(runtime_name, "runtime name")?;
    Ok(format!("{OVEN_INTEROP_BUNDLED_DIRECTORY}/{name}/{runtime_name}"))
}

/// Require an artifact-local packaging field from a canonical locked bundled input.
fn required_bundled_artifact_field(
    value: Option<&str>,
    artifact: &crate::oven_interop::LockedInteropArtifact,
    label: &str,
) -> Result<String, String> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("locked bundled interop artifact `{}` has no {label}", artifact.name))
}

/// Reject path traversal or directory spellings in artifact names and runtime loader names.
fn validate_bundled_path_component(value: &str, label: &str) -> Result<(), String> {
    let mut components = Path::new(value).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) if !value.trim().is_empty() => Ok(()),
        _ => Err(format!(
            "interop bundled {label} `{value}` must be one safe path component"
        )),
    }
}

/// Retain declared system capabilities in final-plan provenance without discovering ambient search paths.
fn locked_system_capabilities(target: &LockedInteropTarget) -> Result<Vec<String>, String> {
    let mut capabilities = BTreeSet::new();
    for artifact in &target.artifacts {
        if artifact.kind != InteropArtifactKind::System {
            continue;
        }
        let capability = artifact
            .capability
            .as_deref()
            .filter(|capability| !capability.trim().is_empty())
            .ok_or_else(|| format!("locked system interop artifact `{}` has no capability", artifact.name))?;
        capabilities.insert(capability.to_string());
    }
    Ok(capabilities.into_iter().collect())
}

/// Compile every declared C/C++ shim through the selected toolchain and assemble one archive per logical output.
fn compile_locked_interop_shims(
    request: &OvenInteropNativeBakeRequest<'_>,
    native_root: &Path,
) -> Result<Vec<OvenInteropBakedArchive>, String> {
    let include_roots = locked_include_roots(request.project_root, request.target)?;
    let mut archives = Vec::new();
    for shim in &request.target.shims {
        let compiler = match shim.language {
            InteropShimLanguage::C => request.c_compiler,
            InteropShimLanguage::Cxx => request.cxx_compiler,
        }
        .ok_or_else(|| format!("locked interop shim `{}` has no selected compiler", shim.name))?;
        let compiler_label = match shim.language {
            InteropShimLanguage::C => "selected C compiler",
            InteropShimLanguage::Cxx => "selected C++ compiler",
        };
        let compiler = canonical_regular_executable(compiler, compiler_label)?;
        if !is_interop_native_library_name(&shim.output) {
            return Err(format!("locked interop shim `{}` has an unsafe output name", shim.name));
        }
        let object_root = native_root.join("objects").join(&shim.output);
        fs::create_dir_all(&object_root).map_err(|error| {
            format!(
                "could not create Oven interop object directory {}: {error}",
                object_root.display()
            )
        })?;
        let mut objects = Vec::with_capacity(shim.sources.len());
        for (index, source) in shim.sources.iter().enumerate() {
            let source = verified_locked_input(request.project_root, source, "interop shim source")?;
            let object = object_root.join(format!("{index:04}.o"));
            let mut command = Command::new(&compiler);
            if shim.language == InteropShimLanguage::Cxx {
                command.args(["-x", "c++"]);
            }
            command.arg("-c").arg(&source).arg("-o").arg(&object);
            append_locked_compile_arguments(&mut command, request, &include_roots);
            run_interop_tool(command, &compiler, &format!("compile interop shim `{}`", shim.name))?;
            let _ = digest_regular_file(&object, "compiled interop shim object")?;
            objects.push(object);
        }
        let archive = native_root.join(format!("lib{}.a", shim.output));
        let archiver = request
            .archiver
            .ok_or_else(|| format!("locked interop shim `{}` has no selected archiver", shim.name))?;
        let archiver = canonical_regular_executable(archiver, "selected native archiver")?;
        let mut command = Command::new(&archiver);
        command.arg("crs").arg(&archive).args(&objects);
        run_interop_tool(command, &archiver, &format!("archive interop shim `{}`", shim.name))?;
        archives.push(OvenInteropBakedArchive {
            name: shim.output.clone(),
            relative_path: format!("{OVEN_INTEROP_NATIVE_DIRECTORY}/lib{}.a", shim.output),
            digest: digest_regular_file(&archive, "compiled interop shim archive")?,
            origin: None,
        });
    }
    Ok(archives)
}

/// Derive explicit package-owned include directories from every locked header input.
fn locked_include_roots(project_root: &Path, target: &LockedInteropTarget) -> Result<Vec<PathBuf>, String> {
    let mut roots = BTreeSet::new();
    for input in target
        .headers
        .iter()
        .chain(target.shims.iter().flat_map(|shim| shim.headers.iter()))
    {
        let path = verified_locked_input(project_root, input, "interop header")?;
        let parent = path
            .parent()
            .ok_or_else(|| format!("interop header {} has no parent", path.display()))?;
        roots.insert(parent.to_path_buf());
    }
    Ok(roots.into_iter().collect())
}

/// Add only declared target, sysroot, definition, and package include arguments to one selected compiler command.
fn append_locked_compile_arguments(
    command: &mut Command,
    request: &OvenInteropNativeBakeRequest<'_>,
    include_roots: &[PathBuf],
) {
    if interop_compiler_uses_clang_target_flag(request.target) {
        command
            .arg("-target")
            .arg(clang_target_for_rust_target(&request.target.target));
    }
    if let Some(sdk_root) = request.sdk_root {
        command.arg("-isysroot").arg(sdk_root);
    }
    for definition in &request.target.definitions {
        command.arg(format!("-D{definition}"));
    }
    for root in include_roots {
        command.arg("-I").arg(root);
    }
}

/// Return whether this locked compiler capability accepts Clang's explicit target flag.
///
/// A `host-c-compiler` is a native compiler selected by the maintainer for the host target. Its driver is allowed
/// to be GCC-compatible, where `-target` is not valid. Target-aware capabilities remain responsible for accepting
/// the explicit Clang spelling generated below; cross-target toolchains therefore do not inherit a host default.
fn interop_compiler_uses_clang_target_flag(target: &LockedInteropTarget) -> bool {
    target
        .toolchain
        .as_ref()
        .is_none_or(|toolchain| toolchain.capability != "host-c-compiler")
}

/// Translate the Rust target vocabulary into the corresponding Clang target spelling without ambient detection.
fn clang_target_for_rust_target(target: &str) -> &str {
    if let Some(kind) = ios_target_kind(target) {
        return kind.clang_target();
    }
    match target {
        "aarch64-apple-darwin" => "arm64-apple-macosx",
        "x86_64-apple-darwin" => "x86_64-apple-macosx",
        _ => target,
    }
}

/// Read one declared package input only after rechecking its portable path shape and locked content digest.
fn verified_locked_input(project_root: &Path, input: &LockedInteropInput, label: &str) -> Result<PathBuf, String> {
    let relative = Path::new(&input.path);
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("locked {label} path `{}` is not package-relative", input.path));
    }
    let path = project_root.join(relative);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("could not inspect locked {label} {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!("locked {label} must be a regular file: {}", path.display()));
    }
    let actual = digest_regular_file(&path, label)?;
    if actual != input.digest {
        return Err(format!(
            "locked {label} digest mismatch for {}: expected {}, got {actual}",
            path.display(),
            input.digest
        ));
    }
    Ok(path)
}

/// Hash one regular publisher input in bounded chunks.
fn digest_regular_file(path: &Path, label: &str) -> Result<String, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("could not inspect {label} {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!("{label} must be a regular file: {}", path.display()));
    }
    let mut file =
        fs::File::open(path).map_err(|error| format!("could not open {label} {}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("could not read {label} {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

/// Run one selected compiler or archiver inside an isolated process group with a bounded terminal transcript.
fn run_interop_tool(mut command: Command, executable: &Path, label: &str) -> Result<Output, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate_process_group(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start {label} with {}: {error}", executable.display()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{label} did not provide a stdout pipe"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("{label} did not provide a stderr pipe"))?;
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut stdout = stdout;
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut stderr = stderr;
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + OVEN_INTEROP_BAKE_COMMAND_TIMEOUT;
    let mut timed_out = false;
    let status = loop {
        match child
            .try_wait()
            .map_err(|error| format!("could not poll {label}: {error}"))?
        {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                timed_out = true;
                break terminate_process_group(&mut child)
                    .map_err(|error| format!("could not terminate timed-out {label}: {error}"))?;
            }
            None => thread::sleep(Duration::from_millis(5)),
        }
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| format!("{label} stdout reader panicked"))
        .and_then(|result| result.map_err(|error| format!("could not read {label} stdout: {error}")))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| format!("{label} stderr reader panicked"))
        .and_then(|result| result.map_err(|error| format!("could not read {label} stderr: {error}")))?;
    let output = Output { status, stdout, stderr };
    if timed_out {
        return Err(format!(
            "{label} exceeded the {} second Oven interop deadline and its process group was terminated",
            OVEN_INTEROP_BAKE_COMMAND_TIMEOUT.as_secs()
        ));
    }
    if !output.status.success() {
        return Err(format!(
            "{label} failed with {}:\n{}",
            output.status,
            bounded_process_output(&output)
        ));
    }
    Ok(output)
}

/// Render a bounded compiler transcript while retaining the original process status in the parent diagnostic.
fn bounded_process_output(output: &Output) -> String {
    const MAX_OUTPUT_BYTES: usize = 8 * 1024;
    let mut combined = output.stdout.clone();
    combined.extend_from_slice(&output.stderr);
    let truncated = combined.len() > MAX_OUTPUT_BYTES;
    let rendered = String::from_utf8_lossy(&combined[..combined.len().min(MAX_OUTPUT_BYTES)]);
    let mut result = rendered.into_owned();
    if truncated {
        result.push_str("\n… compiler output truncated");
    }
    result
}

/// Validate selected capability facts and receipt one locked interop target for a later native bake.
pub(crate) fn receipt_interop_execution(
    target: &LockedInteropTarget,
    toolchain: Option<OvenInteropCapabilitySelection>,
    sdk: Option<OvenInteropCapabilitySelection>,
) -> Result<OvenInteropExecutionReceipt, String> {
    let toolchain = validate_selected_capability("toolchain", target.toolchain.as_ref(), toolchain)?;
    let sdk = validate_selected_capability("SDK", target.sdk.as_ref(), sdk)?;
    let locked_target_identity = locked_interop_target_identity(target)?;
    let identity = digest_serialized(
        &OvenInteropExecutionReceiptIdentity {
            schema_version: OVEN_INTEROP_EXECUTION_RECEIPT_SCHEMA_VERSION,
            locked_target_identity: &locked_target_identity,
            target: &target.target,
            toolchain: toolchain.as_ref(),
            sdk: sdk.as_ref(),
        },
        "selected Oven interop execution receipt",
    )?;
    Ok(OvenInteropExecutionReceipt {
        schema_version: OVEN_INTEROP_EXECUTION_RECEIPT_SCHEMA_VERSION,
        locked_target_identity,
        target: target.target.clone(),
        toolchain,
        sdk,
        identity,
    })
}

/// Canonical identity fields excluding the self-referential receipt identity.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct OvenInteropExecutionReceiptIdentity<'a> {
    schema_version: u32,
    locked_target_identity: &'a str,
    target: &'a str,
    toolchain: Option<&'a OvenInteropCapabilitySelection>,
    sdk: Option<&'a OvenInteropCapabilitySelection>,
}

/// Check that one concrete selection satisfies the exact locked capability requirement.
fn validate_selected_capability(
    label: &str,
    requirement: Option<&ToolchainRequirement>,
    selection: Option<OvenInteropCapabilitySelection>,
) -> Result<Option<OvenInteropCapabilitySelection>, String> {
    let Some(requirement) = requirement else {
        return match selection {
            Some(_) => Err(format!(
                "locked Oven interop target does not declare a {label} capability"
            )),
            None => Ok(None),
        };
    };
    let Some(selection) = selection else {
        return Err(format!(
            "locked Oven interop target requires selected {label} capability `{}`",
            requirement.capability
        ));
    };
    if selection.capability != requirement.capability {
        return Err(format!(
            "selected {label} capability `{}` does not match locked requirement `{}`",
            selection.capability, requirement.capability
        ));
    }
    if selection.identity.trim().is_empty() || selection.identity.contains('\0') {
        return Err(format!(
            "selected {label} capability `{}` has no valid identity",
            selection.capability
        ));
    }
    let selected_version = Version::parse(&selection.version).map_err(|error| {
        format!(
            "selected {label} capability `{}` has invalid version `{}`: {error}",
            selection.capability, selection.version
        )
    })?;
    if let Some(required_version) = &requirement.version {
        let requirement = VersionReq::parse(required_version).map_err(|error| {
            format!(
                "locked {label} capability `{}` has invalid version requirement `{required_version}`: {error}",
                selection.capability
            )
        })?;
        if !requirement.matches(&selected_version) {
            return Err(format!(
                "selected {label} capability `{}` version `{selected_version}` does not satisfy `{required_version}`",
                selection.capability
            ));
        }
    }
    Ok(Some(selection))
}

/// Serialize one portable selected-contract shape before hashing it with Oven's stable digest encoding.
fn digest_serialized(value: &impl Serialize, label: &str) -> Result<String, String> {
    serde_json::to_vec(value)
        .map(|bytes| digest_bytes(&bytes))
        .map_err(|error| format!("failed to serialize {label}: {error}"))
}

#[cfg(test)]
mod tests {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    use super::selected_interop_toolchain_identity;
    use super::{
        OVEN_INTEROP_ADAPTER_MANIFEST_PATH, OVEN_INTEROP_EXECUTION_PROVENANCE_PATH, OVEN_INTEROP_NATIVE_DIRECTORY,
        OvenInteropAdapter, OvenInteropAdapterStageRequest, OvenInteropBakedArchive, OvenInteropBakedBundle,
        OvenInteropCapabilitySelection, OvenInteropNativeBakeRequest, bake_interop_native_plan,
        bind_interop_native_archives, default_interop_execution_receipt_path, interop_adapter_bundle_path,
        interop_compiler_uses_clang_target_flag, interop_execution_build_unit_inputs, load_interop_execution_receipt,
        receipt_interop_execution, stage_interop_adapter, validate_interop_execution_receipt,
        write_interop_execution_receipt,
    };
    #[cfg(target_os = "macos")]
    use crate::oven::rustc::select_direct_rustc_plan_for_execution;
    use crate::oven::rustc::{
        OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenRustcArtifactManifest, OvenRustcSupportingArtifact,
    };
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    use crate::oven::rustc::{
        OvenStoredDirectRustcRunRequest, bake_stored_direct_rustc_run, resolve_active_rustc, rustc_host_target,
        rustc_identity,
    };
    use crate::oven::store::{
        OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, OvenStoreLimits,
    };
    use crate::oven::{OvenBuildIntent, OvenGeneratedProjectRequest, digest_bytes, receipt_generated_project};
    use crate::oven_interop::{
        InteropArtifactKind, InteropArtifactOrigin, InteropTargetPlatform, LockedInteropArtifact, LockedInteropInput,
        LockedInteropShim, LockedInteropTarget, ToolchainRequirement,
    };
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    use std::process::Command;

    fn selected(capability: &str, version: &str, identity: &str) -> OvenInteropCapabilitySelection {
        OvenInteropCapabilitySelection {
            capability: capability.to_string(),
            version: version.to_string(),
            identity: identity.to_string(),
        }
    }

    fn locked_target() -> LockedInteropTarget {
        LockedInteropTarget {
            target: "aarch64-apple-ios-sim".to_string(),
            toolchain: Some(ToolchainRequirement {
                capability: "apple-clang".to_string(),
                version: Some(">=17, <18".to_string()),
            }),
            sdk: Some(ToolchainRequirement {
                capability: "iphonesimulator".to_string(),
                version: Some(">=18, <19".to_string()),
            }),
            platform: None,
            definitions: vec!["INCAN_INTEROP=1".to_string()],
            headers: vec![LockedInteropInput {
                path: "interop/include/bridge.h".to_string(),
                digest: "sha256:header".to_string(),
            }],
            artifacts: Vec::new(),
            bindings: Vec::new(),
            shims: vec![LockedInteropShim {
                name: "bridge".to_string(),
                language: crate::oven_interop::InteropShimLanguage::C,
                sources: vec![LockedInteropInput {
                    path: "interop/src/bridge.c".to_string(),
                    digest: "sha256:source".to_string(),
                }],
                headers: Vec::new(),
                output: "bridge".to_string(),
            }],
        }
    }

    fn base_plan(intent: OvenBuildIntent) -> OvenRustcArtifactManifest {
        OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent,
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            entrypoint_externs: std::collections::BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: std::collections::BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        }
    }

    #[test]
    fn host_c_compiler_uses_its_native_driver_target_while_clang_capabilities_are_explicit() {
        let mut host_target = locked_target();
        host_target.target = "x86_64-unknown-linux-gnu".to_string();
        host_target.sdk = None;
        host_target.platform = None;
        host_target.toolchain = Some(ToolchainRequirement {
            capability: "host-c-compiler".to_string(),
            version: Some(">=1, <2".to_string()),
        });

        assert!(!interop_compiler_uses_clang_target_flag(&host_target));
        assert!(interop_compiler_uses_clang_target_flag(&locked_target()));
    }

    fn static_bake_request<'a>(
        store: &'a OvenStore,
        project_root: &'a Path,
        target: &'a LockedInteropTarget,
        base_receipt: &'a crate::oven::OvenReceipt,
        execution_receipt: &'a super::OvenInteropExecutionReceipt,
    ) -> OvenInteropNativeBakeRequest<'a> {
        OvenInteropNativeBakeRequest {
            store,
            project_root,
            target,
            base_receipt,
            execution_receipt,
            c_compiler: None,
            cxx_compiler: None,
            archiver: None,
            sdk_root: None,
            sdk_identity_file: None,
        }
    }

    #[test]
    fn selected_interop_receipt_is_portable_and_bears_every_locked_input() -> Result<(), Box<dyn std::error::Error>> {
        let target = locked_target();
        let receipt = receipt_interop_execution(
            &target,
            Some(selected("apple-clang", "17.0.6", "sha256:clang")),
            Some(selected("iphonesimulator", "18.5.0", "sha256:sdk")),
        )?;
        assert_eq!(receipt.target, target.target);
        assert!(!receipt.locked_target_identity.is_empty());
        assert!(!receipt.identity.is_empty());
        let serialized = serde_json::to_string(&receipt)?;
        assert!(!serialized.contains("/Users/"));
        let inputs = interop_execution_build_unit_inputs(&receipt);
        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs.get("oven-interop-execution-receipt"), Some(&receipt.identity));
        assert_eq!(inputs.get("oven-interop-plan-schema"), Some(&"5".to_string()));

        let mut changed = target;
        changed.shims[0].sources[0].digest = "sha256:changed-source".to_string();
        let changed_receipt = receipt_interop_execution(
            &changed,
            Some(selected("apple-clang", "17.0.6", "sha256:clang")),
            Some(selected("iphonesimulator", "18.5.0", "sha256:sdk")),
        )?;
        assert_ne!(receipt.locked_target_identity, changed_receipt.locked_target_identity);
        assert_ne!(receipt.identity, changed_receipt.identity);
        Ok(())
    }

    #[test]
    fn selected_interop_receipt_rejects_missing_or_incompatible_capabilities() {
        let target = locked_target();
        let missing = receipt_interop_execution(&target, None, None);
        assert!(missing.is_err());
        let incompatible = receipt_interop_execution(
            &target,
            Some(selected("llvm", "17.0.6", "sha256:clang")),
            Some(selected("iphonesimulator", "18.5.0", "sha256:sdk")),
        );
        assert!(incompatible.is_err());
    }

    #[test]
    fn interop_baker_revalidates_the_selected_sdk_identity_before_admission() -> Result<(), Box<dyn std::error::Error>>
    {
        let project = tempfile::tempdir()?;
        let generated = project.path().join("generated.rs");
        fs::write(&generated, "fn main() {}\n")?;
        let base_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "interop-sdk-fixture",
                "0.1.0",
                "x86_64-unknown-linux-gnu",
                "fixture-rustc",
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated),
        )?;
        let target = LockedInteropTarget {
            target: base_receipt.intent.target.clone(),
            toolchain: None,
            sdk: Some(ToolchainRequirement {
                capability: "fixture-sdk".to_string(),
                version: Some(">=1, <2".to_string()),
            }),
            platform: None,
            definitions: Vec::new(),
            headers: Vec::new(),
            artifacts: Vec::new(),
            bindings: Vec::new(),
            shims: Vec::new(),
        };
        let sdk_root = project.path().join("sdk");
        fs::create_dir_all(&sdk_root)?;
        let identity_file = sdk_root.join("identity.txt");
        fs::write(&identity_file, "fixture-sdk-v1")?;
        let execution_receipt = receipt_interop_execution(
            &target,
            None,
            Some(selected("fixture-sdk", "1.0.0", &digest_bytes(b"fixture-sdk-v1"))),
        )?;
        let store = OvenStore::new(
            project.path().join("oven-store"),
            OvenStoreLimits::new(16 * 1024 * 1024, 16 * 1024 * 1024, 16 * 1024 * 1024),
        );
        let request = OvenInteropNativeBakeRequest {
            store: &store,
            project_root: project.path(),
            target: &target,
            base_receipt: &base_receipt,
            execution_receipt: &execution_receipt,
            c_compiler: None,
            cxx_compiler: None,
            archiver: None,
            sdk_root: Some(&sdk_root),
            sdk_identity_file: Some(&identity_file),
        };
        super::validate_interop_bake_request(&request)?;

        fs::write(&identity_file, "fixture-sdk-v2")?;
        let error = match super::validate_interop_bake_request(&request) {
            Ok(()) => return Err("changed SDK identity unexpectedly passed validation".into()),
            Err(error) => error,
        };
        assert!(error.contains("SDK identity"), "unexpected error: {error}");
        Ok(())
    }

    #[test]
    fn selected_interop_receipt_is_atomic_portable_and_revalidated_against_the_lock()
    -> Result<(), Box<dyn std::error::Error>> {
        let target = locked_target();
        let receipt = receipt_interop_execution(
            &target,
            Some(selected("apple-clang", "17.0.6", "sha256:clang")),
            Some(selected("iphonesimulator", "18.5.0", "sha256:sdk")),
        )?;
        let project = tempfile::tempdir()?;
        let path = default_interop_execution_receipt_path(project.path(), &target.target);
        assert!(path.starts_with(project.path()));
        assert_eq!(path.extension().and_then(|extension| extension.to_str()), Some("json"));
        write_interop_execution_receipt(&receipt, &path)?;
        let loaded = load_interop_execution_receipt(&path)?;
        assert_eq!(loaded, receipt);
        validate_interop_execution_receipt(&target, &loaded)?;

        let mut changed = target;
        changed.definitions.push("CHANGED=1".to_string());
        assert!(validate_interop_execution_receipt(&changed, &loaded).is_err());
        Ok(())
    }

    #[test]
    fn interop_native_archives_extend_one_complete_runtime_plan_with_portable_provenance()
    -> Result<(), Box<dyn std::error::Error>> {
        let target = locked_target();
        let receipt = receipt_interop_execution(
            &target,
            Some(selected("apple-clang", "17.0.6", "sha256:clang")),
            Some(selected("iphonesimulator", "18.5.0", "sha256:sdk")),
        )?;
        let intent = OvenBuildIntent {
            target: target.target.clone(),
            toolchain: "rustc fixture".to_string(),
            profile: "debug".to_string(),
            features: Vec::new(),
        };
        let (plan, provenance) = bind_interop_native_archives(
            &base_plan(intent),
            &OvenBuildIntent {
                target: target.target.clone(),
                toolchain: "rustc fixture".to_string(),
                profile: "debug".to_string(),
                features: Vec::new(),
            },
            &receipt,
            &[OvenInteropBakedArchive {
                name: "bridge".to_string(),
                relative_path: "interop-native/libbridge.a".to_string(),
                digest: format!("sha256:{}", "a".repeat(64)),
                origin: None,
            }],
            &[],
            &[],
        )?;
        assert_eq!(plan.native_search_paths, [OVEN_INTEROP_NATIVE_DIRECTORY]);
        assert!(
            plan.supporting_artifacts
                .iter()
                .any(|artifact| artifact.relative_path == "interop-native/libbridge.a")
        );
        assert!(
            plan.supporting_artifacts
                .iter()
                .any(|artifact| artifact.relative_path == OVEN_INTEROP_EXECUTION_PROVENANCE_PATH)
        );
        let rendered = String::from_utf8(provenance)?;
        assert!(rendered.contains("sha256:clang"));
        assert!(!rendered.contains("/Users/"));
        Ok(())
    }

    #[test]
    fn interop_baker_reuses_a_complete_verified_plan_without_starting_native_tools()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let generated = project.path().join("generated.rs");
        fs::write(&generated, "fn main() {}\n")?;
        let base_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "interop-fixture",
                "0.1.0",
                "x86_64-unknown-linux-gnu",
                "fixture-rustc",
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated),
        )?;
        let store = OvenStore::new(
            project.path().join("oven-store"),
            OvenStoreLimits::new(16 * 1024 * 1024, 16 * 1024 * 1024, 16 * 1024 * 1024),
        );
        let runtime = project.path().join("runtime.bin");
        fs::write(&runtime, b"sealed runtime closure")?;
        let mut runtime_plan = base_plan(base_receipt.intent.clone());
        runtime_plan.supporting_artifacts.push(OvenRustcSupportingArtifact {
            relative_path: "runtime/runtime.bin".to_string(),
            digest: digest_bytes(b"sealed runtime closure"),
        });
        let base_payload = serde_json::to_vec(&runtime_plan)?;
        let base_entry = store.publish(&OvenArtifactPublishRequest {
            receipt: base_receipt.clone(),
            domain: "runtime".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: base_payload,
            materialized_files: vec![OvenArtifactMaterializedFile {
                source_path: runtime,
                relative_path: "runtime/runtime.bin".to_string(),
            }],
        })?;

        let archive_path = project.path().join("libfixture.a");
        fs::write(&archive_path, b"fixture static archive")?;
        let target = LockedInteropTarget {
            target: base_receipt.intent.target.clone(),
            toolchain: None,
            sdk: None,
            platform: None,
            definitions: Vec::new(),
            headers: Vec::new(),
            artifacts: vec![LockedInteropArtifact {
                name: "fixture".to_string(),
                kind: InteropArtifactKind::Static,
                input: Some(LockedInteropInput {
                    path: "libfixture.a".to_string(),
                    digest: digest_bytes(b"fixture static archive"),
                }),
                origin: None,
                capability: None,
                runtime_name: None,
                placement: None,
                minimum_platform: None,
                dependencies: Vec::new(),
            }],
            bindings: Vec::new(),
            shims: Vec::new(),
        };
        let execution_receipt = receipt_interop_execution(&target, None, None)?;
        let capacity_limited = OvenStore::new(store.root(), OvenStoreLimits::new(16 * 1024 * 1024, 1, 1));
        let limited_bake = bake_interop_native_plan(static_bake_request(
            &capacity_limited,
            project.path(),
            &target,
            &base_receipt,
            &execution_receipt,
        ));
        let Err(error) = limited_bake else {
            return Err("capacity-limited interop bake should be rejected".into());
        };
        assert!(error.contains("per-domain allowance"));
        assert!(
            fs::read_dir(store.root())?
                .filter_map(Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().starts_with(".interop-bake-"))
        );

        let first = bake_interop_native_plan(static_bake_request(
            &store,
            project.path(),
            &target,
            &base_receipt,
            &execution_receipt,
        ))?;
        assert!(!first.reused);
        assert_eq!(first.archive_names, ["fixture"]);
        let inspection = store.inspect()?;
        let entries_after_first = inspection.entries.len();
        #[cfg(unix)]
        {
            let base = inspection
                .entries
                .iter()
                .find(|entry| entry.manifest.identity == base_entry.identity)
                .ok_or("base runtime entry should be retained")?;
            let final_entry = inspection
                .entries
                .iter()
                .find(|entry| entry.manifest.identity == first.plan_identity)
                .ok_or("final interop entry should be retained")?;
            let base_metadata = fs::metadata(base.path.join("artifacts/runtime/runtime.bin"))?;
            let final_metadata = fs::metadata(final_entry.path.join("artifacts/runtime/runtime.bin"))?;
            assert_eq!(base_metadata.ino(), final_metadata.ino());
            assert!(base_metadata.nlink() >= 2);
        }

        let second = bake_interop_native_plan(static_bake_request(
            &store,
            project.path(),
            &target,
            &base_receipt,
            &execution_receipt,
        ))?;
        assert!(second.reused);
        assert_eq!(second.plan_identity, first.plan_identity);
        assert_eq!(store.inspect()?.entries.len(), entries_after_first);
        Ok(())
    }

    #[test]
    fn interop_baker_materializes_bundled_artifacts_and_retains_system_capabilities_in_provenance()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let generated = project.path().join("generated.rs");
        fs::write(&generated, "fn main() {}\n")?;
        let base_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "interop-bundled-fixture",
                "0.1.0",
                "x86_64-unknown-linux-gnu",
                "fixture-rustc",
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated),
        )?;
        let store = OvenStore::new(
            project.path().join("oven-store"),
            OvenStoreLimits::new(16 * 1024 * 1024, 16 * 1024 * 1024, 16 * 1024 * 1024),
        );
        store.publish(&OvenArtifactPublishRequest {
            receipt: base_receipt.clone(),
            domain: "runtime".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&base_plan(base_receipt.intent.clone()))?,
            materialized_files: Vec::new(),
        })?;

        let bundled = project.path().join("libfixture_runtime.so");
        fs::write(&bundled, b"fixture bundled runtime")?;
        let target = LockedInteropTarget {
            target: base_receipt.intent.target.clone(),
            toolchain: None,
            sdk: None,
            platform: None,
            definitions: Vec::new(),
            headers: Vec::new(),
            artifacts: vec![
                LockedInteropArtifact {
                    name: "fixture_runtime".to_string(),
                    kind: InteropArtifactKind::Bundled,
                    input: Some(LockedInteropInput {
                        path: "libfixture_runtime.so".to_string(),
                        digest: digest_bytes(b"fixture bundled runtime"),
                    }),
                    origin: Some(InteropArtifactOrigin {
                        source: "https://example.invalid/fixture-runtime".to_string(),
                        revision: "v1.0.0".to_string(),
                        license: "LicenseRef-Fixture".to_string(),
                    }),
                    capability: None,
                    runtime_name: Some("libfixture_runtime.so".to_string()),
                    placement: Some("native-libs".to_string()),
                    minimum_platform: Some("1".to_string()),
                    dependencies: Vec::new(),
                },
                LockedInteropArtifact {
                    name: "libc".to_string(),
                    kind: InteropArtifactKind::System,
                    input: None,
                    origin: None,
                    capability: Some("system.libc".to_string()),
                    runtime_name: None,
                    placement: None,
                    minimum_platform: None,
                    dependencies: Vec::new(),
                },
            ],
            bindings: Vec::new(),
            shims: Vec::new(),
        };
        let execution_receipt = receipt_interop_execution(&target, None, None)?;

        let first = bake_interop_native_plan(static_bake_request(
            &store,
            project.path(),
            &target,
            &base_receipt,
            &execution_receipt,
        ))?;
        assert!(!first.reused);
        let final_entry = store
            .inspect()?
            .entries
            .into_iter()
            .find(|entry| entry.manifest.identity == first.plan_identity)
            .ok_or("expected final bundled interop entry")?;
        let bundled_path = final_entry
            .path
            .join("artifacts/interop-bundled/fixture_runtime/libfixture_runtime.so");
        assert_eq!(fs::read(&bundled_path)?, b"fixture bundled runtime");
        let provenance = fs::read_to_string(
            final_entry
                .path
                .join(format!("artifacts/{OVEN_INTEROP_EXECUTION_PROVENANCE_PATH}")),
        )?;
        assert!(provenance.contains("system.libc"));
        assert!(provenance.contains("https://example.invalid/fixture-runtime"));
        assert!(provenance.contains("LicenseRef-Fixture"));
        assert!(!provenance.contains(project.path().to_string_lossy().as_ref()));

        let second = bake_interop_native_plan(static_bake_request(
            &store,
            project.path(),
            &target,
            &base_receipt,
            &execution_receipt,
        ))?;
        assert!(second.reused);
        assert_eq!(second.plan_identity, first.plan_identity);
        Ok(())
    }

    #[test]
    fn interop_adapter_stages_only_selected_bundles_in_an_atomic_ios_simulator_layout()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let generated = project.path().join("generated.rs");
        fs::write(&generated, "fn main() {}\n")?;
        let base_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "interop-adapter-fixture",
                "0.1.0",
                "aarch64-apple-ios-sim",
                "fixture-rustc",
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated),
        )?;
        let store = OvenStore::new(
            project.path().join("oven-store"),
            OvenStoreLimits::new(16 * 1024 * 1024, 16 * 1024 * 1024, 16 * 1024 * 1024),
        );
        store.publish(&OvenArtifactPublishRequest {
            receipt: base_receipt.clone(),
            domain: "runtime".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&base_plan(base_receipt.intent.clone()))?,
            materialized_files: Vec::new(),
        })?;
        let runtime = project.path().join("libfixture_runtime.dylib");
        fs::write(&runtime, b"fixture iOS simulator runtime")?;
        let target = LockedInteropTarget {
            target: "aarch64-apple-ios-sim".to_string(),
            toolchain: None,
            sdk: None,
            platform: Some(InteropTargetPlatform::Ios {
                deployment_target: "13.0".to_string(),
            }),
            definitions: Vec::new(),
            headers: Vec::new(),
            artifacts: vec![
                LockedInteropArtifact {
                    name: "fixture_runtime".to_string(),
                    kind: InteropArtifactKind::Bundled,
                    input: Some(LockedInteropInput {
                        path: "libfixture_runtime.dylib".to_string(),
                        digest: digest_bytes(b"fixture iOS simulator runtime"),
                    }),
                    origin: None,
                    capability: None,
                    runtime_name: Some("libfixture_runtime.dylib".to_string()),
                    placement: Some("frameworks".to_string()),
                    minimum_platform: Some("13.0".to_string()),
                    dependencies: Vec::new(),
                },
                LockedInteropArtifact {
                    name: "accelerate".to_string(),
                    kind: InteropArtifactKind::System,
                    input: None,
                    origin: None,
                    capability: Some("apple.framework.Accelerate".to_string()),
                    runtime_name: None,
                    placement: None,
                    minimum_platform: None,
                    dependencies: Vec::new(),
                },
            ],
            bindings: Vec::new(),
            shims: Vec::new(),
        };
        let execution_receipt = receipt_interop_execution(&target, None, None)?;
        let baked = bake_interop_native_plan(static_bake_request(
            &store,
            project.path(),
            &target,
            &base_receipt,
            &execution_receipt,
        ))?;
        let output = project.path().join("simulator-stage");
        let staged = stage_interop_adapter(OvenInteropAdapterStageRequest {
            store: &store,
            target: &target,
            base_receipt: &base_receipt,
            execution_receipt: &execution_receipt,
            adapter: OvenInteropAdapter::Ios,
            output: &output,
        })?;
        assert_eq!(staged.plan_identity, baked.plan_identity);
        assert_eq!(staged.bundled_files, 1);
        assert_eq!(
            fs::read(output.join("Frameworks/libfixture_runtime.dylib"))?,
            b"fixture iOS simulator runtime"
        );
        let manifest = fs::read_to_string(&staged.manifest_path)?;
        assert!(staged.manifest_path.ends_with(OVEN_INTEROP_ADAPTER_MANIFEST_PATH));
        assert!(manifest.contains("aarch64-apple-ios-sim"));
        assert!(manifest.contains("apple.framework.Accelerate"));
        assert!(!manifest.contains(project.path().to_string_lossy().as_ref()));
        let second = stage_interop_adapter(OvenInteropAdapterStageRequest {
            store: &store,
            target: &target,
            base_receipt: &base_receipt,
            execution_receipt: &execution_receipt,
            adapter: OvenInteropAdapter::Ios,
            output: &output,
        });
        let Err(error) = second else {
            return Err("adapter staging must not replace an existing caller-owned output".into());
        };
        assert!(error.contains("will not be replaced"));
        Ok(())
    }

    #[test]
    fn interop_adapter_rejects_a_missing_final_plan_without_creating_output() -> Result<(), Box<dyn std::error::Error>>
    {
        let project = tempfile::tempdir()?;
        let generated = project.path().join("generated.rs");
        fs::write(&generated, "fn main() {}\n")?;
        let base_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "interop-no-plan-fixture",
                "0.1.0",
                "aarch64-linux-android",
                "fixture-rustc",
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated),
        )?;
        let target = LockedInteropTarget {
            target: "aarch64-linux-android".to_string(),
            toolchain: None,
            sdk: None,
            platform: Some(InteropTargetPlatform::Android { api_level: 34 }),
            definitions: Vec::new(),
            headers: Vec::new(),
            artifacts: Vec::new(),
            bindings: Vec::new(),
            shims: Vec::new(),
        };
        let execution_receipt = receipt_interop_execution(&target, None, None)?;
        let store = OvenStore::new(
            project.path().join("oven-store"),
            OvenStoreLimits::new(16 * 1024 * 1024, 16 * 1024 * 1024, 16 * 1024 * 1024),
        );
        let output = project.path().join("android-stage");
        let staged = stage_interop_adapter(OvenInteropAdapterStageRequest {
            store: &store,
            target: &target,
            base_receipt: &base_receipt,
            execution_receipt: &execution_receipt,
            adapter: OvenInteropAdapter::Android,
            output: &output,
        });
        let Err(error) = staged else {
            return Err("adapter staging without a final plan must fail".into());
        };
        assert!(error.contains("no compatible final direct-Rustc plan"));
        assert!(!output.exists());
        Ok(())
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn interop_adapter_publication_never_replaces_a_competing_empty_output() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let staging = project.path().join("staging");
        let output = project.path().join("output");
        fs::create_dir(&staging)?;
        fs::write(staging.join("manifest.json"), b"staged")?;
        fs::create_dir(&output)?;

        let publication = super::publish_new_adapter_directory(&staging, &output);
        let Err(error) = publication else {
            return Err("no-replace adapter publication accepted a competing output".into());
        };
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert!(staging.join("manifest.json").is_file());
        assert!(output.is_dir());
        assert!(fs::read_dir(&output)?.next().is_none());
        Ok(())
    }

    #[test]
    fn interop_adapter_uses_fixed_android_and_ios_relative_layouts() -> Result<(), Box<dyn std::error::Error>> {
        let bundle = OvenInteropBakedBundle {
            name: "fixture".to_string(),
            relative_path: "interop-bundled/fixture/libfixture.so".to_string(),
            digest: digest_bytes(b"fixture"),
            origin: None,
            runtime_name: "libfixture.so".to_string(),
            placement: "declared-by-package".to_string(),
            minimum_platform: "1".to_string(),
        };
        let android = LockedInteropTarget {
            target: "aarch64-linux-android".to_string(),
            toolchain: None,
            sdk: None,
            platform: Some(InteropTargetPlatform::Android { api_level: 34 }),
            definitions: Vec::new(),
            headers: Vec::new(),
            artifacts: Vec::new(),
            bindings: Vec::new(),
            shims: Vec::new(),
        };
        assert_eq!(
            interop_adapter_bundle_path(OvenInteropAdapter::Android, &android, &bundle)?,
            "jniLibs/arm64-v8a/libfixture.so"
        );
        let ios = LockedInteropTarget {
            target: "aarch64-apple-ios".to_string(),
            toolchain: None,
            sdk: None,
            platform: Some(InteropTargetPlatform::Ios {
                deployment_target: "13.0".to_string(),
            }),
            definitions: Vec::new(),
            headers: Vec::new(),
            artifacts: Vec::new(),
            bindings: Vec::new(),
            shims: Vec::new(),
        };
        assert_eq!(
            interop_adapter_bundle_path(OvenInteropAdapter::Ios, &ios, &bundle)?,
            "Frameworks/libfixture.so"
        );
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn selected_apple_clang_bakes_locked_c_and_cxx_shims_for_direct_rustc_without_cargo()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let generated = project.path().join("generated.rs");
        fs::write(
            &generated,
            r#"
#[link(name = "fixture", kind = "static")]
unsafe extern "C" {
    fn incan_interop_fixture() -> i32;
}
#[link(name = "fixture_cxx", kind = "static")]
unsafe extern "C" {
    fn incan_interop_cxx_fixture() -> i32;
}
fn main() {
    assert_eq!(unsafe { incan_interop_fixture() }, 7);
    assert_eq!(unsafe { incan_interop_cxx_fixture() }, 11);
}
"#,
        )?;
        let source = project.path().join("shim.c");
        fs::write(&source, "int incan_interop_fixture(void) { return 7; }\n")?;
        let cxx_source = project.path().join("shim.cpp");
        fs::write(
            &cxx_source,
            "extern \"C\" int incan_interop_cxx_fixture(void) { return 11; }\n",
        )?;
        let rustc = resolve_active_rustc()?;
        let base_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "interop-clang-fixture",
                "0.1.0",
                "aarch64-apple-darwin",
                rustc_identity(&rustc)?,
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated),
        )?;
        let store = OvenStore::new(
            project.path().join("oven-store"),
            OvenStoreLimits::new(16 * 1024 * 1024, 16 * 1024 * 1024, 16 * 1024 * 1024),
        );
        store.publish(&OvenArtifactPublishRequest {
            receipt: base_receipt.clone(),
            domain: "runtime".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&base_plan(base_receipt.intent.clone()))?,
            materialized_files: Vec::new(),
        })?;
        let target = LockedInteropTarget {
            target: base_receipt.intent.target.clone(),
            toolchain: Some(ToolchainRequirement {
                capability: "apple-clang".to_string(),
                version: Some(">=21, <22".to_string()),
            }),
            sdk: None,
            platform: None,
            definitions: Vec::new(),
            headers: Vec::new(),
            artifacts: Vec::new(),
            bindings: Vec::new(),
            shims: vec![
                LockedInteropShim {
                    name: "fixture".to_string(),
                    language: crate::oven_interop::InteropShimLanguage::C,
                    sources: vec![LockedInteropInput {
                        path: "shim.c".to_string(),
                        digest: digest_bytes(b"int incan_interop_fixture(void) { return 7; }\n"),
                    }],
                    headers: Vec::new(),
                    output: "fixture".to_string(),
                },
                LockedInteropShim {
                    name: "fixture-cxx".to_string(),
                    language: crate::oven_interop::InteropShimLanguage::Cxx,
                    sources: vec![LockedInteropInput {
                        path: "shim.cpp".to_string(),
                        digest: digest_bytes(b"extern \"C\" int incan_interop_cxx_fixture(void) { return 11; }\n"),
                    }],
                    headers: Vec::new(),
                    output: "fixture_cxx".to_string(),
                },
            ],
        };
        let selected_toolchain_identity = selected_interop_toolchain_identity(
            &target,
            Some(Path::new("/usr/bin/clang")),
            Some(Path::new("/usr/bin/clang++")),
            Some(Path::new("/usr/bin/ar")),
        )?;
        let execution_receipt = receipt_interop_execution(
            &target,
            Some(selected("apple-clang", "21.0.0", &selected_toolchain_identity)),
            None,
        )?;
        let baked = bake_interop_native_plan(OvenInteropNativeBakeRequest {
            store: &store,
            project_root: project.path(),
            target: &target,
            base_receipt: &base_receipt,
            execution_receipt: &execution_receipt,
            c_compiler: Some(Path::new("/usr/bin/clang")),
            cxx_compiler: Some(Path::new("/usr/bin/clang++")),
            archiver: Some(Path::new("/usr/bin/ar")),
            sdk_root: None,
            sdk_identity_file: None,
        })?;
        assert!(!baked.reused);
        assert_eq!(baked.archive_names, ["fixture", "fixture_cxx"]);
        let final_entry = store
            .inspect()?
            .entries
            .into_iter()
            .find(|entry| entry.manifest.identity == baked.plan_identity)
            .ok_or("baked interop entry should be retained")?;
        assert!(
            final_entry
                .manifest
                .materialized_files
                .iter()
                .any(|file| file.relative_path == "interop-native/libfixture.a")
        );
        assert!(
            final_entry
                .manifest
                .materialized_files
                .iter()
                .any(|file| file.relative_path == "interop-native/libfixture_cxx.a")
        );
        let direct = bake_stored_direct_rustc_run(&OvenStoredDirectRustcRunRequest {
            store: &store,
            plan_identity: baked.plan_identity,
            receipt: baked.receipt,
            rustc,
            source: generated,
            output: project.path().join("oven-linked-shim"),
            crate_name: "oven_linked_shim".to_string(),
            edition: "2024".to_string(),
            source_evidence_key: "generated-root".to_string(),
        })?;
        assert!(!direct.cargo_process_started);
        assert!(Command::new(&direct.output).status()?.success());
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn selected_apple_clang_bakes_and_runs_an_accelerate_framework_shim_without_cargo()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let generated = project.path().join("generated.rs");
        fs::write(
            &generated,
            r#"
#[link(name = "fixture_accelerate", kind = "static")]
unsafe extern "C" {
    fn incan_accelerate_sum(values: *const f32, value_count: usize) -> f32;
}
#[link(name = "Accelerate", kind = "framework")]
unsafe extern "C" {}
fn main() {
    let values = [1.0_f32, 2.0_f32, 3.0_f32];
    assert_eq!(unsafe { incan_accelerate_sum(values.as_ptr(), values.len()) }, 6.0_f32);
}
"#,
        )?;
        let source = project.path().join("accelerate_shim.c");
        fs::write(
            &source,
            "#include <Accelerate/Accelerate.h>\nfloat incan_accelerate_sum(const float *values, size_t value_count) { float output = 0.0f; vDSP_sve(values, 1, &output, value_count); return output; }\n",
        )?;
        let rustc = resolve_active_rustc()?;
        let base_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "interop-accelerate-fixture",
                "0.1.0",
                "aarch64-apple-darwin",
                rustc_identity(&rustc)?,
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated),
        )?;
        let store = OvenStore::new(
            project.path().join("oven-store"),
            OvenStoreLimits::new(16 * 1024 * 1024, 16 * 1024 * 1024, 16 * 1024 * 1024),
        );
        store.publish(&OvenArtifactPublishRequest {
            receipt: base_receipt.clone(),
            domain: "runtime".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&base_plan(base_receipt.intent.clone()))?,
            materialized_files: Vec::new(),
        })?;
        let shim_source = "#include <Accelerate/Accelerate.h>\nfloat incan_accelerate_sum(const float *values, size_t value_count) { float output = 0.0f; vDSP_sve(values, 1, &output, value_count); return output; }\n";
        let target = LockedInteropTarget {
            target: base_receipt.intent.target.clone(),
            toolchain: Some(ToolchainRequirement {
                capability: "apple-clang".to_string(),
                version: Some(">=21, <22".to_string()),
            }),
            sdk: None,
            platform: None,
            definitions: Vec::new(),
            headers: Vec::new(),
            artifacts: vec![LockedInteropArtifact {
                name: "accelerate".to_string(),
                kind: InteropArtifactKind::System,
                input: None,
                origin: None,
                capability: Some("apple.framework.Accelerate".to_string()),
                runtime_name: None,
                placement: None,
                minimum_platform: None,
                dependencies: Vec::new(),
            }],
            bindings: Vec::new(),
            shims: vec![LockedInteropShim {
                name: "accelerate-sum".to_string(),
                language: crate::oven_interop::InteropShimLanguage::C,
                sources: vec![LockedInteropInput {
                    path: "accelerate_shim.c".to_string(),
                    digest: digest_bytes(shim_source.as_bytes()),
                }],
                headers: Vec::new(),
                output: "fixture_accelerate".to_string(),
            }],
        };
        let selected_toolchain_identity = selected_interop_toolchain_identity(
            &target,
            Some(Path::new("/usr/bin/clang")),
            None,
            Some(Path::new("/usr/bin/ar")),
        )?;
        let execution_receipt = receipt_interop_execution(
            &target,
            Some(selected("apple-clang", "21.0.0", &selected_toolchain_identity)),
            None,
        )?;
        let baked = bake_interop_native_plan(OvenInteropNativeBakeRequest {
            store: &store,
            project_root: project.path(),
            target: &target,
            base_receipt: &base_receipt,
            execution_receipt: &execution_receipt,
            c_compiler: Some(Path::new("/usr/bin/clang")),
            cxx_compiler: None,
            archiver: Some(Path::new("/usr/bin/ar")),
            sdk_root: None,
            sdk_identity_file: None,
        })?;
        assert_eq!(baked.archive_names, ["fixture_accelerate"]);
        let direct = bake_stored_direct_rustc_run(&OvenStoredDirectRustcRunRequest {
            store: &store,
            plan_identity: baked.plan_identity,
            receipt: baked.receipt,
            rustc,
            source: generated,
            output: project.path().join("oven-linked-accelerate"),
            crate_name: "oven_linked_accelerate".to_string(),
            edition: "2024".to_string(),
            source_evidence_key: "generated-root".to_string(),
        })?;
        assert!(!direct.cargo_process_started);
        assert!(Command::new(&direct.output).status()?.success());
        Ok(())
    }

    /// Prove one real callback-free llama.cpp C lifecycle through declared, receipt-bound static archives.
    ///
    /// The external source and build directories are explicit maintainer evidence inputs rather than normal Oven
    /// discovery. The temporary package copies their exact headers and archives, locks their digests with the upstream
    /// revision/license declaration, and then exercises only Oven's selected compiler, static-archive baker, and
    /// direct-Rustc execution path.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires explicitly supplied official llama.cpp source and static build directories"]
    fn llama_cpp_callback_free_lifecycle_is_baked_and_run_from_declared_artifacts()
    -> Result<(), Box<dyn std::error::Error>> {
        const LLAMA_CPP_REVISION: &str = "08659901c43b51de735740f1cf61bb82fbe0c4e4";
        let source_root = required_real_library_directory("INCAN_TEST_LLAMA_CPP_SOURCE")?;
        let build_root = required_real_library_directory("INCAN_TEST_LLAMA_CPP_BUILD")?;
        let revision_output = Command::new("git")
            .arg("-C")
            .arg(&source_root)
            .args(["rev-parse", "HEAD"])
            .output()?;
        if !revision_output.status.success() {
            return Err(format!(
                "could not read the selected llama.cpp source revision from {}: {}",
                source_root.display(),
                String::from_utf8_lossy(&revision_output.stderr)
            )
            .into());
        }
        let actual_revision = String::from_utf8(revision_output.stdout)?;
        if actual_revision.trim() != LLAMA_CPP_REVISION {
            return Err(format!(
                "llama.cpp source revision must be {LLAMA_CPP_REVISION}, found {}",
                actual_revision.trim()
            )
            .into());
        }
        let project = tempfile::tempdir()?;
        let include = project.path().join("interop/include");
        let library_root = project.path().join("interop/lib");
        fs::create_dir_all(&include)?;
        fs::create_dir_all(&library_root)?;
        let declared_headers = [
            ("include/llama.h", "llama.h"),
            ("ggml/include/ggml.h", "ggml.h"),
            ("ggml/include/ggml-cpu.h", "ggml-cpu.h"),
            ("ggml/include/ggml-backend.h", "ggml-backend.h"),
            ("ggml/include/ggml-alloc.h", "ggml-alloc.h"),
            ("ggml/include/ggml-opt.h", "ggml-opt.h"),
            ("ggml/include/gguf.h", "gguf.h"),
        ];
        let mut locked_headers = Vec::with_capacity(declared_headers.len());
        for (source_relative, package_name) in declared_headers {
            let source = source_root.join(source_relative);
            if !source.is_file() {
                return Err(format!("llama.cpp source has no public header: {}", source.display()).into());
            }
            let destination = include.join(package_name);
            fs::copy(&source, &destination)?;
            locked_headers.push(LockedInteropInput {
                path: format!("interop/include/{package_name}"),
                digest: digest_bytes(&fs::read(&destination)?),
            });
        }

        let declared_archives = [
            ("llama", "src/libllama.a", Vec::<&str>::from(["ggml"])),
            (
                "ggml",
                "ggml/src/libggml.a",
                Vec::<&str>::from(["ggml_cpu", "ggml_base", "ggml_blas"]),
            ),
            ("ggml_cpu", "ggml/src/libggml-cpu.a", Vec::<&str>::from(["ggml_base"])),
            ("ggml_base", "ggml/src/libggml-base.a", Vec::<&str>::new()),
            (
                "ggml_blas",
                "ggml/src/ggml-blas/libggml-blas.a",
                Vec::<&str>::from(["ggml_base"]),
            ),
        ];
        let mut artifacts = Vec::new();
        for (name, build_relative, dependencies) in declared_archives {
            let source = build_root.join(build_relative);
            if !source.is_file() {
                return Err(format!("llama.cpp build has no declared archive: {}", source.display()).into());
            }
            let destination = library_root.join(format!("lib{name}.a"));
            fs::copy(&source, &destination)?;
            artifacts.push(LockedInteropArtifact {
                name: name.to_string(),
                kind: InteropArtifactKind::Static,
                input: Some(LockedInteropInput {
                    path: format!("interop/lib/lib{name}.a"),
                    digest: digest_bytes(&fs::read(&destination)?),
                }),
                origin: Some(InteropArtifactOrigin {
                    source: "https://github.com/ggml-org/llama.cpp".to_string(),
                    revision: LLAMA_CPP_REVISION.to_string(),
                    license: "MIT".to_string(),
                }),
                capability: None,
                runtime_name: None,
                placement: None,
                minimum_platform: None,
                dependencies: dependencies.into_iter().map(str::to_string).collect(),
            });
        }
        artifacts.push(LockedInteropArtifact {
            name: "accelerate".to_string(),
            kind: InteropArtifactKind::System,
            input: None,
            origin: None,
            capability: Some("apple.framework.Accelerate".to_string()),
            runtime_name: None,
            placement: None,
            minimum_platform: None,
            dependencies: Vec::new(),
        });
        artifacts.push(LockedInteropArtifact {
            name: "cxx-runtime".to_string(),
            kind: InteropArtifactKind::System,
            input: None,
            origin: None,
            capability: Some("apple.library.c++".to_string()),
            runtime_name: None,
            placement: None,
            minimum_platform: None,
            dependencies: Vec::new(),
        });

        let shim_source = "#include <llama.h>\nint incan_llama_backend_cycle(void) { llama_backend_init(); llama_backend_free(); return 1; }\n";
        fs::write(project.path().join("interop/llama_bridge.c"), shim_source)?;
        let generated = project.path().join("generated.rs");
        fs::write(
            &generated,
            r#"
#[link(name = "llama_bridge", kind = "static")]
unsafe extern "C" {
    fn incan_llama_backend_cycle() -> i32;
}
#[link(name = "llama", kind = "static")]
unsafe extern "C" {}
#[link(name = "ggml", kind = "static")]
unsafe extern "C" {}
#[link(name = "ggml_cpu", kind = "static")]
unsafe extern "C" {}
#[link(name = "ggml_base", kind = "static")]
unsafe extern "C" {}
#[link(name = "ggml_blas", kind = "static")]
unsafe extern "C" {}
#[link(name = "c++")]
unsafe extern "C" {}
#[link(name = "Accelerate", kind = "framework")]
unsafe extern "C" {}
fn main() {
    assert_eq!(unsafe { incan_llama_backend_cycle() }, 1);
}
"#,
        )?;

        let rustc = resolve_active_rustc()?;
        let base_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "interop-llama-callback-free",
                "0.1.0",
                "aarch64-apple-darwin",
                rustc_identity(&rustc)?,
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated),
        )?;
        let store = OvenStore::new(
            project.path().join("oven-store"),
            OvenStoreLimits::new(128 * 1024 * 1024, 128 * 1024 * 1024, 128 * 1024 * 1024),
        );
        store.publish(&OvenArtifactPublishRequest {
            receipt: base_receipt.clone(),
            domain: "runtime".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&base_plan(base_receipt.intent.clone()))?,
            materialized_files: Vec::new(),
        })?;
        let target = LockedInteropTarget {
            target: base_receipt.intent.target.clone(),
            toolchain: Some(ToolchainRequirement {
                capability: "apple-clang".to_string(),
                version: Some(">=21, <22".to_string()),
            }),
            sdk: None,
            platform: None,
            definitions: Vec::new(),
            headers: locked_headers.clone(),
            artifacts,
            bindings: Vec::new(),
            shims: vec![LockedInteropShim {
                name: "llama-callback-free".to_string(),
                language: crate::oven_interop::InteropShimLanguage::C,
                sources: vec![LockedInteropInput {
                    path: "interop/llama_bridge.c".to_string(),
                    digest: digest_bytes(shim_source.as_bytes()),
                }],
                headers: locked_headers,
                output: "llama_bridge".to_string(),
            }],
        };
        let selected_toolchain_identity = selected_interop_toolchain_identity(
            &target,
            Some(Path::new("/usr/bin/clang")),
            None,
            Some(Path::new("/usr/bin/ar")),
        )?;
        let execution_receipt = receipt_interop_execution(
            &target,
            Some(selected("apple-clang", "21.0.0", &selected_toolchain_identity)),
            None,
        )?;
        let baked = bake_interop_native_plan(OvenInteropNativeBakeRequest {
            store: &store,
            project_root: project.path(),
            target: &target,
            base_receipt: &base_receipt,
            execution_receipt: &execution_receipt,
            c_compiler: Some(Path::new("/usr/bin/clang")),
            cxx_compiler: None,
            archiver: Some(Path::new("/usr/bin/ar")),
            sdk_root: None,
            sdk_identity_file: None,
        })?;
        assert!(!baked.reused);
        assert!(baked.archive_names.contains(&"llama".to_string()));
        let selected_plan = select_direct_rustc_plan_for_execution(&store, &baked.receipt)?
            .ok_or("llama.cpp interop bake did not publish a selected plan")?;
        let provenance = fs::read_to_string(selected_plan.artifact_root.join(OVEN_INTEROP_EXECUTION_PROVENANCE_PATH))?;
        assert!(provenance.contains("apple.library.c++"));
        let direct = bake_stored_direct_rustc_run(&OvenStoredDirectRustcRunRequest {
            store: &store,
            plan_identity: baked.plan_identity,
            receipt: baked.receipt,
            rustc,
            source: generated,
            output: project.path().join("oven-linked-llama"),
            crate_name: "oven_linked_llama".to_string(),
            edition: "2024".to_string(),
            source_evidence_key: "generated-root".to_string(),
        })?;
        assert!(!direct.cargo_process_started);
        assert!(Command::new(&direct.output).status()?.success());
        Ok(())
    }

    /// Prove a real libcurl variadic and callback interaction can remain inside a declared C bridge.
    ///
    /// The source and static build roots are explicit maintainer-provided inputs, never discovery locations. The
    /// test checks the selected upstream revision, copies the complete public header surface and the one static
    /// archive into a temporary package, then runs the bridge through the same Oven bake and direct-Rustc execution
    /// path used for checked bindings. `curl_easy_setopt` is deliberately confined to C because it is variadic, and
    /// the callback's function-pointer ABI likewise remains behind the bounded integer-returning bridge surface.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires explicitly supplied official curl source and sealed static build directories"]
    fn curl_variadic_callback_bridge_is_baked_and_run_from_declared_artifacts() -> Result<(), Box<dyn std::error::Error>>
    {
        const CURL_REVISION: &str = "68720b4837284335b2d63cb358f8f6ce65f5bc55";
        let source_root = required_real_library_directory("INCAN_TEST_CURL_SOURCE")?;
        let build_root = required_real_library_directory("INCAN_TEST_CURL_BUILD")?;
        let revision_output = Command::new("git")
            .arg("-C")
            .arg(&source_root)
            .args(["rev-parse", "HEAD"])
            .output()?;
        if !revision_output.status.success() {
            return Err(format!(
                "could not read the selected curl source revision from {}: {}",
                source_root.display(),
                String::from_utf8_lossy(&revision_output.stderr)
            )
            .into());
        }
        let actual_revision = String::from_utf8(revision_output.stdout)?;
        if actual_revision.trim() != CURL_REVISION {
            return Err(format!(
                "curl source revision must be {CURL_REVISION}, found {}",
                actual_revision.trim()
            )
            .into());
        }
        let project = tempfile::tempdir()?;
        let include = project.path().join("interop/include");
        let curl_include = include.join("curl");
        let library_root = project.path().join("interop/lib");
        fs::create_dir_all(&curl_include)?;
        fs::create_dir_all(&library_root)?;
        let header_root = include.join("incan_curl_header_root.h");
        fs::write(&header_root, "#include <curl/curl.h>\n")?;
        let mut locked_headers = vec![LockedInteropInput {
            path: "interop/include/incan_curl_header_root.h".to_string(),
            digest: digest_bytes(&fs::read(&header_root)?),
        }];
        let declared_headers = [
            "curl.h",
            "curlver.h",
            "easy.h",
            "header.h",
            "mprintf.h",
            "multi.h",
            "options.h",
            "stdcheaders.h",
            "system.h",
            "typecheck-gcc.h",
            "urlapi.h",
            "websockets.h",
        ];
        for header_name in declared_headers {
            let source = source_root.join("include/curl").join(header_name);
            if !source.is_file() {
                return Err(format!("curl source has no public header: {}", source.display()).into());
            }
            let destination = curl_include.join(header_name);
            fs::copy(&source, &destination)?;
            locked_headers.push(LockedInteropInput {
                path: format!("interop/include/curl/{header_name}"),
                digest: digest_bytes(&fs::read(&destination)?),
            });
        }
        let curl_archive = build_root.join("lib/libcurl.a");
        if !curl_archive.is_file() {
            return Err(format!("curl build has no declared archive: {}", curl_archive.display()).into());
        }
        let packaged_archive = library_root.join("libcurl.a");
        fs::copy(&curl_archive, &packaged_archive)?;
        let shim_source = "#include <curl/curl.h>\nstatic size_t incan_curl_discard(char *data, size_t size, size_t count, void *context) { (void)data; (void)context; return size * count; }\nint incan_curl_variadic_callback_cycle(void) { if (curl_global_init(CURL_GLOBAL_DEFAULT) != CURLE_OK) { return 0; } CURL *easy = curl_easy_init(); if (easy == NULL) { curl_global_cleanup(); return 0; } CURLcode status = curl_easy_setopt(easy, CURLOPT_NOSIGNAL, 1L); if (status == CURLE_OK) { status = curl_easy_setopt(easy, CURLOPT_WRITEFUNCTION, incan_curl_discard); } curl_easy_cleanup(easy); curl_global_cleanup(); return status == CURLE_OK; }\n";
        fs::write(project.path().join("interop/curl_bridge.c"), shim_source)?;
        let generated = project.path().join("generated.rs");
        fs::write(
            &generated,
            r#"
#[link(name = "curl_bridge", kind = "static")]
unsafe extern "C" {
    fn incan_curl_variadic_callback_cycle() -> i32;
}
#[link(name = "curl", kind = "static")]
unsafe extern "C" {}
#[link(name = "SystemConfiguration", kind = "framework")]
unsafe extern "C" {}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {}
#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {}
fn main() {
    assert_eq!(unsafe { incan_curl_variadic_callback_cycle() }, 1);
}
"#,
        )?;

        let rustc = resolve_active_rustc()?;
        let base_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "interop-curl-variadic-callback",
                "0.1.0",
                "aarch64-apple-darwin",
                rustc_identity(&rustc)?,
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated),
        )?;
        let store = OvenStore::new(
            project.path().join("oven-store"),
            OvenStoreLimits::new(128 * 1024 * 1024, 128 * 1024 * 1024, 128 * 1024 * 1024),
        );
        store.publish(&OvenArtifactPublishRequest {
            receipt: base_receipt.clone(),
            domain: "runtime".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&base_plan(base_receipt.intent.clone()))?,
            materialized_files: Vec::new(),
        })?;
        let target = LockedInteropTarget {
            target: base_receipt.intent.target.clone(),
            toolchain: Some(ToolchainRequirement {
                capability: "apple-clang".to_string(),
                version: Some(">=21, <22".to_string()),
            }),
            sdk: None,
            platform: None,
            definitions: Vec::new(),
            headers: locked_headers.clone(),
            artifacts: vec![
                LockedInteropArtifact {
                    name: "curl".to_string(),
                    kind: InteropArtifactKind::Static,
                    input: Some(LockedInteropInput {
                        path: "interop/lib/libcurl.a".to_string(),
                        digest: digest_bytes(&fs::read(&packaged_archive)?),
                    }),
                    origin: Some(InteropArtifactOrigin {
                        source: "https://github.com/curl/curl".to_string(),
                        revision: CURL_REVISION.to_string(),
                        license: "curl".to_string(),
                    }),
                    capability: None,
                    runtime_name: None,
                    placement: None,
                    minimum_platform: None,
                    dependencies: Vec::new(),
                },
                LockedInteropArtifact {
                    name: "system-configuration".to_string(),
                    kind: InteropArtifactKind::System,
                    input: None,
                    origin: None,
                    capability: Some("apple.framework.SystemConfiguration".to_string()),
                    runtime_name: None,
                    placement: None,
                    minimum_platform: None,
                    dependencies: Vec::new(),
                },
                LockedInteropArtifact {
                    name: "core-foundation".to_string(),
                    kind: InteropArtifactKind::System,
                    input: None,
                    origin: None,
                    capability: Some("apple.framework.CoreFoundation".to_string()),
                    runtime_name: None,
                    placement: None,
                    minimum_platform: None,
                    dependencies: Vec::new(),
                },
                LockedInteropArtifact {
                    name: "core-services".to_string(),
                    kind: InteropArtifactKind::System,
                    input: None,
                    origin: None,
                    capability: Some("apple.framework.CoreServices".to_string()),
                    runtime_name: None,
                    placement: None,
                    minimum_platform: None,
                    dependencies: Vec::new(),
                },
            ],
            bindings: Vec::new(),
            shims: vec![LockedInteropShim {
                name: "curl-variadic-callback".to_string(),
                language: crate::oven_interop::InteropShimLanguage::C,
                sources: vec![LockedInteropInput {
                    path: "interop/curl_bridge.c".to_string(),
                    digest: digest_bytes(shim_source.as_bytes()),
                }],
                headers: locked_headers,
                output: "curl_bridge".to_string(),
            }],
        };
        let selected_toolchain_identity = selected_interop_toolchain_identity(
            &target,
            Some(Path::new("/usr/bin/clang")),
            None,
            Some(Path::new("/usr/bin/ar")),
        )?;
        let execution_receipt = receipt_interop_execution(
            &target,
            Some(selected("apple-clang", "21.0.0", &selected_toolchain_identity)),
            None,
        )?;
        let baked = bake_interop_native_plan(OvenInteropNativeBakeRequest {
            store: &store,
            project_root: project.path(),
            target: &target,
            base_receipt: &base_receipt,
            execution_receipt: &execution_receipt,
            c_compiler: Some(Path::new("/usr/bin/clang")),
            cxx_compiler: None,
            archiver: Some(Path::new("/usr/bin/ar")),
            sdk_root: None,
            sdk_identity_file: None,
        })?;
        assert!(!baked.reused);
        assert!(baked.archive_names.contains(&"curl".to_string()));
        let selected_plan = select_direct_rustc_plan_for_execution(&store, &baked.receipt)?
            .ok_or("curl interop bake did not publish a selected plan")?;
        let provenance = fs::read_to_string(selected_plan.artifact_root.join(OVEN_INTEROP_EXECUTION_PROVENANCE_PATH))?;
        assert!(provenance.contains("https://github.com/curl/curl"));
        assert!(provenance.contains("apple.framework.SystemConfiguration"));
        let direct = bake_stored_direct_rustc_run(&OvenStoredDirectRustcRunRequest {
            store: &store,
            plan_identity: baked.plan_identity,
            receipt: baked.receipt,
            rustc,
            source: generated,
            output: project.path().join("oven-linked-curl"),
            crate_name: "oven_linked_curl".to_string(),
            edition: "2024".to_string(),
            source_evidence_key: "generated-root".to_string(),
        })?;
        assert!(!direct.cargo_process_started);
        assert!(Command::new(&direct.output).status()?.success());
        Ok(())
    }

    /// Prove a declared bundled dylib remains linkable and runnable from its selected immutable plan.
    ///
    /// This compact CI-safe regression exercises the same path as a mobile runtime: the package owns a dynamic
    /// artifact and a narrow C bridge, Oven seals both under one receipt, direct Rustc links only the store copy,
    /// and the binary executes after inherited dynamic-loader paths are removed. The recorded rpath is necessary
    /// because a store identity contains `sha256:`, which cannot be transported through colon-separated Unix loader
    /// environment variables.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn selected_host_c_compiler_bakes_a_bundled_dynamic_library_for_direct_rustc_execution_without_cargo()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let include = project.path().join("interop/include");
        let library_root = project.path().join("interop/lib");
        fs::create_dir_all(&include)?;
        fs::create_dir_all(&library_root)?;
        let header = include.join("fixture_runtime.h");
        fs::write(&header, "int fixture_runtime_answer(void);\n")?;
        let runtime_source = project.path().join("interop/fixture_runtime.c");
        fs::write(&runtime_source, "int fixture_runtime_answer(void) { return 42; }\n")?;
        let runtime_name = if cfg!(target_os = "macos") {
            "libfixture_runtime.dylib"
        } else {
            "libfixture_runtime.so"
        };
        let runtime = library_root.join(runtime_name);
        let mut runtime_command = Command::new("/usr/bin/cc");
        if cfg!(target_os = "macos") {
            runtime_command.args(["-dynamiclib", "-Wl,-install_name,@rpath/libfixture_runtime.dylib"]);
        } else {
            runtime_command.args(["-shared", "-fPIC", "-Wl,-soname,libfixture_runtime.so"]);
        }
        let runtime_output = runtime_command.arg(&runtime_source).arg("-o").arg(&runtime).output()?;
        if !runtime_output.status.success() {
            return Err(format!(
                "could not compile bundled runtime fixture: {}",
                String::from_utf8_lossy(&runtime_output.stderr)
            )
            .into());
        }
        let shim_source = "#include <fixture_runtime.h>\nint incan_fixture_runtime_answer(void) { return fixture_runtime_answer(); }\n";
        fs::write(project.path().join("interop/fixture_bridge.c"), shim_source)?;
        let generated = project.path().join("generated.rs");
        fs::write(
            &generated,
            r#"
#[link(name = "fixture_bridge", kind = "static")]
unsafe extern "C" {
    fn incan_fixture_runtime_answer() -> i32;
}
#[link(name = "fixture_runtime", kind = "dylib")]
unsafe extern "C" {}

fn main() {
    if unsafe { incan_fixture_runtime_answer() } != 42 {
        std::process::exit(1);
    }
}
"#,
        )?;
        let rustc = resolve_active_rustc()?;
        let base_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "interop-bundled-dylib-runtime",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated),
        )?;
        let store = OvenStore::new(
            project.path().join("oven-store"),
            OvenStoreLimits::new(128 * 1024 * 1024, 128 * 1024 * 1024, 128 * 1024 * 1024),
        );
        store.publish(&OvenArtifactPublishRequest {
            receipt: base_receipt.clone(),
            domain: "runtime".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&base_plan(base_receipt.intent.clone()))?,
            materialized_files: Vec::new(),
        })?;
        let target = LockedInteropTarget {
            target: base_receipt.intent.target.clone(),
            toolchain: Some(ToolchainRequirement {
                capability: "host-c-compiler".to_string(),
                version: Some(">=1, <2".to_string()),
            }),
            sdk: None,
            platform: None,
            definitions: Vec::new(),
            headers: vec![LockedInteropInput {
                path: "interop/include/fixture_runtime.h".to_string(),
                digest: digest_bytes(&fs::read(&header)?),
            }],
            artifacts: vec![LockedInteropArtifact {
                name: "fixture_runtime".to_string(),
                kind: InteropArtifactKind::Bundled,
                input: Some(LockedInteropInput {
                    path: format!("interop/lib/{runtime_name}"),
                    digest: digest_bytes(&fs::read(&runtime)?),
                }),
                origin: Some(InteropArtifactOrigin {
                    source: "https://example.invalid/incan-fixture-runtime".to_string(),
                    revision: "fixture-v1".to_string(),
                    license: "LicenseRef-IncanFixture".to_string(),
                }),
                capability: None,
                runtime_name: Some(runtime_name.to_string()),
                placement: Some("runtime".to_string()),
                minimum_platform: Some("14.0".to_string()),
                dependencies: Vec::new(),
            }],
            bindings: Vec::new(),
            shims: vec![LockedInteropShim {
                name: "fixture-runtime-bridge".to_string(),
                language: crate::oven_interop::InteropShimLanguage::C,
                sources: vec![LockedInteropInput {
                    path: "interop/fixture_bridge.c".to_string(),
                    digest: digest_bytes(shim_source.as_bytes()),
                }],
                headers: vec![LockedInteropInput {
                    path: "interop/include/fixture_runtime.h".to_string(),
                    digest: digest_bytes(&fs::read(&header)?),
                }],
                output: "fixture_bridge".to_string(),
            }],
        };
        let selected_toolchain_identity = selected_interop_toolchain_identity(
            &target,
            Some(Path::new("/usr/bin/cc")),
            None,
            Some(Path::new("/usr/bin/ar")),
        )?;
        let execution_receipt = receipt_interop_execution(
            &target,
            Some(selected("host-c-compiler", "1.0.0", &selected_toolchain_identity)),
            None,
        )?;
        let baked = bake_interop_native_plan(OvenInteropNativeBakeRequest {
            store: &store,
            project_root: project.path(),
            target: &target,
            base_receipt: &base_receipt,
            execution_receipt: &execution_receipt,
            c_compiler: Some(Path::new("/usr/bin/cc")),
            cxx_compiler: None,
            archiver: Some(Path::new("/usr/bin/ar")),
            sdk_root: None,
            sdk_identity_file: None,
        })?;
        assert_eq!(baked.bundle_names, ["fixture_runtime"]);
        let (selected_entry, _lease) = store.select(&baked.plan_identity)?;
        let native_root = selected_entry.materialized_root();
        assert!(!native_root.to_string_lossy().contains(':'));
        let direct = bake_stored_direct_rustc_run(&OvenStoredDirectRustcRunRequest {
            store: &store,
            plan_identity: baked.plan_identity,
            receipt: baked.receipt,
            rustc,
            source: generated,
            output: project.path().join("oven-linked-fixture-runtime"),
            crate_name: "oven_linked_fixture_runtime".to_string(),
            edition: "2024".to_string(),
            source_evidence_key: "generated-root".to_string(),
        })?;
        assert!(!direct.cargo_process_started);
        let output = Command::new(&direct.output)
            .env_remove("DYLD_LIBRARY_PATH")
            .env_remove("DYLD_FALLBACK_LIBRARY_PATH")
            .env_remove("LD_LIBRARY_PATH")
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "sealed dynamic-runtime consumer exited with {}; stdout: {}; stderr: {}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            )
            .into());
        }
        Ok(())
    }

    /// Prove the official TensorFlow Lite C runtime is sealed, linked, and executed without a Cargo consumer.
    ///
    /// The supplied source and Bazel output roots are maintainer-selected evidence inputs. This test rejects a
    /// substituted revision or runtime/model byte stream, copies the complete public header closure consumed by
    /// `tensorflow/lite/c/c_api.h` into a temporary package, and records every package input in the final receipt.
    /// The runtime remains a `bundled` deployment artifact: its one receipt-selected store copy is both linkable by
    /// direct Rustc and available to a later Apple/Android adapter without accepting an ambient SDK location.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires explicitly supplied official TensorFlow source and TensorFlow Lite C build directories"]
    fn tensorflow_lite_c_api_lifecycle_is_baked_linked_and_run_from_declared_artifacts()
    -> Result<(), Box<dyn std::error::Error>> {
        const TENSORFLOW_REVISION: &str = "72fbba3d20f4616d7312b5e2b7f79daf6e82f2fa";
        const TFLITE_C_DYLIB_DIGEST: &str = "sha256:4a114deb76878061125c3bd3cf0751dfe495ba4a624e2571c398eee97e6029a8";
        const ADD_MODEL_DIGEST: &str = "sha256:038f8d317c07b094e3b7d0c7aba60a3d9e441eb4188c8515920108b963c3873d";
        let source_root = required_real_library_directory("INCAN_TEST_TFLITE_SOURCE")?;
        let build_root = required_real_library_directory("INCAN_TEST_TFLITE_BUILD")?;
        let revision_output = Command::new("git")
            .arg("-C")
            .arg(&source_root)
            .args(["rev-parse", "HEAD"])
            .output()?;
        if !revision_output.status.success() {
            return Err(format!(
                "could not read the selected TensorFlow source revision from {}: {}",
                source_root.display(),
                String::from_utf8_lossy(&revision_output.stderr)
            )
            .into());
        }
        let actual_revision = String::from_utf8(revision_output.stdout)?;
        if actual_revision.trim() != TENSORFLOW_REVISION {
            return Err(format!(
                "TensorFlow source revision must be {TENSORFLOW_REVISION}, found {}",
                actual_revision.trim()
            )
            .into());
        }
        let clean_output = Command::new("git")
            .arg("-C")
            .arg(&source_root)
            .args(["diff", "--quiet"])
            .status()?;
        if !clean_output.success() {
            return Err(format!("TensorFlow source {} has tracked modifications", source_root.display()).into());
        }

        let project = tempfile::tempdir()?;
        let include = project.path().join("interop/include");
        let library_root = project.path().join("interop/lib");
        let model_root = project.path().join("interop/models");
        fs::create_dir_all(&include)?;
        fs::create_dir_all(&library_root)?;
        fs::create_dir_all(&model_root)?;
        let declared_headers = [
            "tensorflow/lite/c/c_api.h",
            "tensorflow/lite/core/c/c_api.h",
            "tensorflow/lite/builtin_ops.h",
            "tensorflow/lite/core/async/c/types.h",
            "tensorflow/lite/core/c/c_api_types.h",
            "tensorflow/compiler/mlir/lite/core/c/tflite_types.h",
            "tensorflow/lite/core/c/operator.h",
        ];
        let mut locked_headers = Vec::with_capacity(declared_headers.len());
        for source_relative in declared_headers {
            let source = source_root.join(source_relative);
            if !source.is_file() {
                return Err(format!("TensorFlow source has no public C API header: {}", source.display()).into());
            }
            let destination = include.join(source_relative);
            let parent = destination
                .parent()
                .ok_or_else(|| format!("TensorFlow header destination {} has no parent", destination.display()))?;
            fs::create_dir_all(parent)?;
            fs::copy(&source, &destination)?;
            locked_headers.push(LockedInteropInput {
                path: format!("interop/include/{source_relative}"),
                digest: digest_bytes(&fs::read(&destination)?),
            });
        }
        let header_root = include.join("incan_tflite_header_root.h");
        fs::write(&header_root, "#include \"tensorflow/lite/c/c_api.h\"\n")?;
        locked_headers.push(LockedInteropInput {
            path: "interop/include/incan_tflite_header_root.h".to_string(),
            digest: digest_bytes(&fs::read(&header_root)?),
        });
        let runtime_source = build_root.join("tensorflow/lite/c/libtensorflowlite_c.dylib");
        if !runtime_source.is_file() {
            return Err(format!(
                "TensorFlow Lite C build has no declared runtime: {}",
                runtime_source.display()
            )
            .into());
        }
        if digest_bytes(&fs::read(&runtime_source)?) != TFLITE_C_DYLIB_DIGEST {
            return Err("TensorFlow Lite C runtime digest does not match the pinned official build evidence".into());
        }
        let runtime = library_root.join("libtensorflowlite_c.dylib");
        fs::copy(&runtime_source, &runtime)?;
        let model_source = source_root.join("tensorflow/lite/testdata/add.bin");
        if !model_source.is_file() {
            return Err(format!("TensorFlow source has no add-model fixture: {}", model_source.display()).into());
        }
        if digest_bytes(&fs::read(&model_source)?) != ADD_MODEL_DIGEST {
            return Err("TensorFlow Lite add-model digest does not match the pinned official source evidence".into());
        }
        let model = model_root.join("add.bin");
        fs::copy(&model_source, &model)?;

        let shim_source = r#"
#include "tensorflow/lite/c/c_api.h"

int incan_tflite_add_cycle(const char *model_path) {
    TfLiteModel *model = TfLiteModelCreateFromFile(model_path);
    TfLiteInterpreterOptions *options = NULL;
    TfLiteInterpreter *interpreter = NULL;
    int result = 0;
    int input_dims[1] = {2};
    float input[2] = {1.0f, 3.0f};
    float output[2] = {0.0f, 0.0f};
    if (model == NULL) goto done;
    options = TfLiteInterpreterOptionsCreate();
    if (options == NULL) goto done;
    TfLiteInterpreterOptionsSetNumThreads(options, 2);
    interpreter = TfLiteInterpreterCreate(model, options);
    if (interpreter == NULL) goto done;
    TfLiteInterpreterOptionsDelete(options);
    options = NULL;
    if (TfLiteInterpreterAllocateTensors(interpreter) != kTfLiteOk) goto done;
    if (TfLiteInterpreterResizeInputTensor(interpreter, 0, input_dims, 1) != kTfLiteOk) goto done;
    if (TfLiteInterpreterAllocateTensors(interpreter) != kTfLiteOk) goto done;
    TfLiteTensor *input_tensor = TfLiteInterpreterGetInputTensor(interpreter, 0);
    if (input_tensor == NULL) goto done;
    if (TfLiteTensorCopyFromBuffer(input_tensor, input, sizeof(input)) != kTfLiteOk) goto done;
    if (TfLiteInterpreterInvoke(interpreter) != kTfLiteOk) goto done;
    const TfLiteTensor *output_tensor = TfLiteInterpreterGetOutputTensor(interpreter, 0);
    if (output_tensor == NULL) goto done;
    if (TfLiteTensorCopyToBuffer(output_tensor, output, sizeof(output)) != kTfLiteOk) goto done;
    result = output[0] == 3.0f && output[1] == 9.0f;
done:
    if (options != NULL) TfLiteInterpreterOptionsDelete(options);
    if (interpreter != NULL) TfLiteInterpreterDelete(interpreter);
    if (model != NULL) TfLiteModelDelete(model);
    return result;
}
"#;
        fs::write(project.path().join("interop/tflite_bridge.c"), shim_source)?;
        let generated = project.path().join("generated.rs");
        fs::write(
            &generated,
            r#"
use std::ffi::CString;

#[link(name = "tflite_bridge", kind = "static")]
unsafe extern "C" {
    fn incan_tflite_add_cycle(model_path: *const std::ffi::c_char) -> i32;
}
#[link(name = "tensorflowlite_c", kind = "dylib")]
unsafe extern "C" {}

fn main() {
    let Some(model_path) = std::env::args().nth(1) else {
        std::process::exit(2);
    };
    let model_path = match CString::new(model_path) {
        Ok(model_path) => model_path,
        Err(_) => std::process::exit(3),
    };
    if unsafe { incan_tflite_add_cycle(model_path.as_ptr()) } != 1 {
        std::process::exit(1);
    }
}
"#,
        )?;

        let rustc = resolve_active_rustc()?;
        let base_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "interop-tensorflow-lite-c-api",
                "0.1.0",
                "aarch64-apple-darwin",
                rustc_identity(&rustc)?,
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated)
            .with_generated_source("tflite-add-model", &model),
        )?;
        let store = OvenStore::new(
            project.path().join("oven-store"),
            OvenStoreLimits::new(128 * 1024 * 1024, 128 * 1024 * 1024, 128 * 1024 * 1024),
        );
        store.publish(&OvenArtifactPublishRequest {
            receipt: base_receipt.clone(),
            domain: "runtime".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&base_plan(base_receipt.intent.clone()))?,
            materialized_files: Vec::new(),
        })?;
        let target = LockedInteropTarget {
            target: base_receipt.intent.target.clone(),
            toolchain: Some(ToolchainRequirement {
                capability: "apple-clang".to_string(),
                version: Some(">=21, <22".to_string()),
            }),
            sdk: None,
            platform: None,
            definitions: Vec::new(),
            headers: locked_headers.clone(),
            artifacts: vec![
                LockedInteropArtifact {
                    name: "tensorflowlite_c".to_string(),
                    kind: InteropArtifactKind::Bundled,
                    input: Some(LockedInteropInput {
                        path: "interop/lib/libtensorflowlite_c.dylib".to_string(),
                        digest: TFLITE_C_DYLIB_DIGEST.to_string(),
                    }),
                    origin: Some(InteropArtifactOrigin {
                        source: "https://github.com/tensorflow/tensorflow".to_string(),
                        revision: TENSORFLOW_REVISION.to_string(),
                        license: "Apache-2.0".to_string(),
                    }),
                    capability: None,
                    runtime_name: Some("libtensorflowlite_c.dylib".to_string()),
                    placement: Some("runtime".to_string()),
                    minimum_platform: Some("14.0".to_string()),
                    dependencies: Vec::new(),
                },
                LockedInteropArtifact {
                    name: "cxx-runtime".to_string(),
                    kind: InteropArtifactKind::System,
                    input: None,
                    origin: None,
                    capability: Some("apple.library.c++".to_string()),
                    runtime_name: None,
                    placement: None,
                    minimum_platform: None,
                    dependencies: Vec::new(),
                },
                LockedInteropArtifact {
                    name: "foundation".to_string(),
                    kind: InteropArtifactKind::System,
                    input: None,
                    origin: None,
                    capability: Some("apple.framework.Foundation".to_string()),
                    runtime_name: None,
                    placement: None,
                    minimum_platform: None,
                    dependencies: Vec::new(),
                },
                LockedInteropArtifact {
                    name: "objc-runtime".to_string(),
                    kind: InteropArtifactKind::System,
                    input: None,
                    origin: None,
                    capability: Some("apple.library.objc".to_string()),
                    runtime_name: None,
                    placement: None,
                    minimum_platform: None,
                    dependencies: Vec::new(),
                },
            ],
            bindings: Vec::new(),
            shims: vec![LockedInteropShim {
                name: "tflite-c-api".to_string(),
                language: crate::oven_interop::InteropShimLanguage::C,
                sources: vec![LockedInteropInput {
                    path: "interop/tflite_bridge.c".to_string(),
                    digest: digest_bytes(shim_source.as_bytes()),
                }],
                headers: locked_headers,
                output: "tflite_bridge".to_string(),
            }],
        };
        let selected_toolchain_identity = selected_interop_toolchain_identity(
            &target,
            Some(Path::new("/usr/bin/clang")),
            None,
            Some(Path::new("/usr/bin/ar")),
        )?;
        let execution_receipt = receipt_interop_execution(
            &target,
            Some(selected("apple-clang", "21.0.0", &selected_toolchain_identity)),
            None,
        )?;
        let baked = bake_interop_native_plan(OvenInteropNativeBakeRequest {
            store: &store,
            project_root: project.path(),
            target: &target,
            base_receipt: &base_receipt,
            execution_receipt: &execution_receipt,
            c_compiler: Some(Path::new("/usr/bin/clang")),
            cxx_compiler: None,
            archiver: Some(Path::new("/usr/bin/ar")),
            sdk_root: None,
            sdk_identity_file: None,
        })?;
        assert!(!baked.reused);
        assert_eq!(baked.bundle_names, ["tensorflowlite_c"]);
        assert!(baked.archive_names.contains(&"tflite_bridge".to_string()));
        let selected_plan = select_direct_rustc_plan_for_execution(&store, &baked.receipt)?
            .ok_or("TensorFlow Lite interop bake did not publish a selected plan")?;
        let stored_plan = serde_json::from_slice::<OvenRustcArtifactManifest>(&selected_plan.payload)?;
        assert!(
            stored_plan
                .native_search_paths
                .contains(&"interop-bundled/tensorflowlite_c".to_string())
        );
        let provenance = fs::read_to_string(selected_plan.artifact_root.join(OVEN_INTEROP_EXECUTION_PROVENANCE_PATH))?;
        assert!(provenance.contains("https://github.com/tensorflow/tensorflow"));
        assert!(provenance.contains(TENSORFLOW_REVISION));
        let direct = bake_stored_direct_rustc_run(&OvenStoredDirectRustcRunRequest {
            store: &store,
            plan_identity: baked.plan_identity,
            receipt: baked.receipt,
            rustc,
            source: generated,
            output: project.path().join("oven-linked-tensorflow-lite"),
            crate_name: "oven_linked_tensorflow_lite".to_string(),
            edition: "2024".to_string(),
            source_evidence_key: "generated-root".to_string(),
        })?;
        assert!(!direct.cargo_process_started);
        assert!(Command::new(&direct.output).arg(&model).status()?.success());
        Ok(())
    }

    /// Require a maintainer-selected external library directory for an ignored real-library evidence test.
    #[cfg(target_os = "macos")]
    fn required_real_library_directory(variable: &str) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
        let path = std::env::var(variable).map_err(|_| format!("real-library test requires {variable}"))?;
        let path = std::path::PathBuf::from(path);
        if !path.is_dir() {
            return Err(format!("{variable} is not a directory: {}", path.display()).into());
        }
        Ok(path)
    }

    /// Require an explicitly selected iOS Simulator rather than silently discovering or booting an arbitrary host
    /// device. Virtual-device execution is release evidence, not an Oven capability-selection input.
    #[cfg(target_os = "macos")]
    fn required_ios_simulator_udid() -> Result<String, Box<dyn std::error::Error>> {
        std::env::var("INCAN_TEST_IOS_SIMULATOR_UDID")
            .map_err(|_| "iOS virtual-device test requires INCAN_TEST_IOS_SIMULATOR_UDID".into())
    }

    /// Read one explicitly supplied Android development-tool location for a virtual-device evidence run.
    #[cfg(target_os = "macos")]
    fn required_android_virtual_device_path(variable: &str) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
        std::env::var_os(variable)
            .map(std::path::PathBuf::from)
            .ok_or_else(|| format!("Android virtual-device test requires {variable}").into())
    }

    /// Resolve the NDK's one compatible host toolchain without assuming native versus Rosetta installation layout.
    #[cfg(target_os = "macos")]
    fn android_ndk_arm64_clang(android_ndk: &Path) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
        let prebuilt_root = android_ndk.join("toolchains/llvm/prebuilt");
        let preferred_host = if cfg!(target_arch = "aarch64") {
            "darwin-arm64"
        } else {
            "darwin-x86_64"
        };
        let preferred = prebuilt_root
            .join(preferred_host)
            .join("bin/aarch64-linux-android21-clang");
        if preferred.is_file() {
            return Ok(preferred);
        }
        let mut candidates = fs::read_dir(&prebuilt_root)?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path().join("bin/aarch64-linux-android21-clang"))
            .filter(|candidate| candidate.is_file())
            .collect::<Vec<_>>();
        candidates.sort();
        match candidates.as_slice() {
            [candidate] => Ok(candidate.clone()),
            [] => Err(format!(
                "Android NDK has no arm64 Clang wrapper below {}",
                prebuilt_root.display()
            )
            .into()),
            _ => Err(format!(
                "Android NDK has multiple host Clang wrappers below {}; select a native host toolchain explicitly",
                prebuilt_root.display()
            )
            .into()),
        }
    }

    #[cfg(target_os = "macos")]
    fn run_virtual_device_command(
        command: &mut Command,
        label: &str,
    ) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        let output = command.output()?;
        if output.status.success() {
            return Ok(output);
        }
        Err(format!(
            "{label} failed with status {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into())
    }

    /// Exercise the platform-consumer boundary that follows a selected iOS-simulator interop plan.
    ///
    /// The test is explicit and ignored because the simulator is a host development dependency, not a CI or source
    /// dependency. It keeps all build facts in the selected Oven plan: Xcode receives only the staged framework
    /// directory, never package-authored ABI, include, or link-search configuration.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires a booted simulator selected by INCAN_TEST_IOS_SIMULATOR_UDID"]
    fn ios_simulator_launches_an_xcode_consumer_of_a_staged_oven_native_artifact()
    -> Result<(), Box<dyn std::error::Error>> {
        let simulator = required_ios_simulator_udid()?;
        let project = tempfile::tempdir()?;
        let source = project.path().join("incan_probe.c");
        let runtime = project.path().join("libincan_probe.dylib");
        let runtime_source = "int incan_native_probe(void) { return 42; }\n";
        fs::write(&source, runtime_source)?;
        let sdk_output = run_virtual_device_command(
            Command::new("xcrun").args(["--sdk", "iphonesimulator", "--show-sdk-path"]),
            "resolve iOS Simulator SDK",
        )?;
        let sdk = String::from_utf8(sdk_output.stdout)?.trim().to_string();
        if sdk.is_empty() {
            return Err("Xcode returned an empty iOS Simulator SDK path".into());
        }
        run_virtual_device_command(
            Command::new("/usr/bin/clang").args([
                "-target",
                "arm64-apple-ios13.0-simulator",
                "-isysroot",
                &sdk,
                "-dynamiclib",
                source.to_string_lossy().as_ref(),
                "-Wl,-install_name,@rpath/libincan_probe.dylib",
                "-o",
                runtime.to_string_lossy().as_ref(),
            ]),
            "compile iOS Simulator native probe",
        )?;

        let generated = project.path().join("generated.rs");
        fs::write(&generated, "fn main() {}\n")?;
        let base_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "ios-simulator-adapter-fixture",
                "0.1.0",
                "aarch64-apple-ios-sim",
                "fixture-rustc",
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated),
        )?;
        let store = OvenStore::new(
            project.path().join("oven-store"),
            OvenStoreLimits::new(32 * 1024 * 1024, 32 * 1024 * 1024, 32 * 1024 * 1024),
        );
        store.publish(&OvenArtifactPublishRequest {
            receipt: base_receipt.clone(),
            domain: "runtime".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&base_plan(base_receipt.intent.clone()))?,
            materialized_files: Vec::new(),
        })?;
        let target = LockedInteropTarget {
            target: "aarch64-apple-ios-sim".to_string(),
            toolchain: None,
            sdk: None,
            platform: Some(InteropTargetPlatform::Ios {
                deployment_target: "13.0".to_string(),
            }),
            definitions: Vec::new(),
            headers: Vec::new(),
            artifacts: vec![LockedInteropArtifact {
                name: "incan_probe".to_string(),
                kind: InteropArtifactKind::Bundled,
                input: Some(LockedInteropInput {
                    path: "libincan_probe.dylib".to_string(),
                    digest: digest_bytes(runtime_source.as_bytes()),
                }),
                origin: None,
                capability: None,
                runtime_name: Some("libincan_probe.dylib".to_string()),
                placement: Some("frameworks".to_string()),
                minimum_platform: Some("13.0".to_string()),
                dependencies: Vec::new(),
            }],
            bindings: Vec::new(),
            shims: Vec::new(),
        };
        // The staged artifact identity must cover the compiled target binary, not merely the C source fixture.
        let runtime_digest = digest_bytes(&fs::read(&runtime)?);
        let mut target = target;
        if let Some(input) = target.artifacts[0].input.as_mut() {
            input.digest = runtime_digest;
        }
        let execution_receipt = receipt_interop_execution(&target, None, None)?;
        let baked = bake_interop_native_plan(static_bake_request(
            &store,
            project.path(),
            &target,
            &base_receipt,
            &execution_receipt,
        ))?;
        let stage = project.path().join("adapter-stage");
        let staged = stage_interop_adapter(OvenInteropAdapterStageRequest {
            store: &store,
            target: &target,
            base_receipt: &base_receipt,
            execution_receipt: &execution_receipt,
            adapter: OvenInteropAdapter::Ios,
            output: &stage,
        })?;
        assert_eq!(staged.plan_identity, baked.plan_identity);

        let xcode = project.path().join("xcode");
        let xcode_project = xcode.join("IncanInteropProbe.xcodeproj");
        fs::create_dir_all(&xcode_project)?;
        fs::write(
            xcode.join("AppDelegate.swift"),
            r#"import UIKit
import Darwin

@_silgen_name("incan_native_probe") private func incan_native_probe() -> Int32

@main
final class AppDelegate: UIResponder, UIApplicationDelegate {
    var window: UIWindow?

    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
    ) -> Bool {
        let result = incan_native_probe()
        NSLog("INCAN_INTEROP_NATIVE_PROBE=%d", result)
        DispatchQueue.main.async {
            exit(result == 42 ? 0 : 1)
        }
        return true
    }
}
"#,
        )?;
        fs::write(
            xcode.join("Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleDevelopmentRegion</key><string>en</string>
<key>CFBundleExecutable</key><string>$(EXECUTABLE_NAME)</string>
<key>CFBundleIdentifier</key><string>$(PRODUCT_BUNDLE_IDENTIFIER)</string>
<key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
<key>CFBundleName</key><string>$(PRODUCT_NAME)</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>1.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSRequiresIPhoneOS</key><true/>
<key>UIApplicationSceneManifest</key><dict><key>UIApplicationSupportsMultipleScenes</key><false/></dict>
</dict></plist>
"#,
        )?;
        let staged_library = stage.join("Frameworks/libincan_probe.dylib");
        let staged_library_dir = staged_library.parent().ok_or_else(|| {
            format!(
                "staged iOS library path has no parent directory: {}",
                staged_library.display()
            )
        })?;
        let project_file = format!(
            r#"// !$*UTF8*$!
{{
    archiveVersion = 1;
    classes = {{}};
    objectVersion = 56;
    objects = {{

/* Begin PBXBuildFile section */
        A10000000000000000000001 /* AppDelegate.swift in Sources */ = {{isa = PBXBuildFile; fileRef = A10000000000000000000002 /* AppDelegate.swift */; }};
        A10000000000000000000003 /* libincan_probe.dylib in Frameworks */ = {{isa = PBXBuildFile; fileRef = A10000000000000000000004 /* libincan_probe.dylib */; }};
        A10000000000000000000005 /* libincan_probe.dylib in Embed Frameworks */ = {{isa = PBXBuildFile; fileRef = A10000000000000000000004 /* libincan_probe.dylib */; settings = {{ATTRIBUTES = (CodeSignOnCopy, ); }}; }};
/* End PBXBuildFile section */

/* Begin PBXCopyFilesBuildPhase section */
        A10000000000000000000006 /* Embed Frameworks */ = {{isa = PBXCopyFilesBuildPhase; buildActionMask = 2147483647; dstPath = ""; dstSubfolderSpec = 10; files = (A10000000000000000000005 /* libincan_probe.dylib in Embed Frameworks */, ); runOnlyForDeploymentPostprocessing = 0; }};
/* End PBXCopyFilesBuildPhase section */

/* Begin PBXFileReference section */
        A10000000000000000000002 /* AppDelegate.swift */ = {{isa = PBXFileReference; lastKnownFileType = sourcecode.swift; path = AppDelegate.swift; sourceTree = "<group>"; }};
        A10000000000000000000004 /* libincan_probe.dylib */ = {{isa = PBXFileReference; lastKnownFileType = compiled.mach-o.dylib; path = "{}"; sourceTree = "<absolute>"; }};
        A10000000000000000000007 /* IncanInteropProbe.app */ = {{isa = PBXFileReference; explicitFileType = wrapper.application; includeInIndex = 0; path = IncanInteropProbe.app; sourceTree = BUILT_PRODUCTS_DIR; }};
/* End PBXFileReference section */

/* Begin PBXFrameworksBuildPhase section */
        A10000000000000000000008 /* Frameworks */ = {{isa = PBXFrameworksBuildPhase; buildActionMask = 2147483647; files = (A10000000000000000000003 /* libincan_probe.dylib in Frameworks */, ); runOnlyForDeploymentPostprocessing = 0; }};
/* End PBXFrameworksBuildPhase section */

/* Begin PBXGroup section */
        A10000000000000000000009 = {{isa = PBXGroup; children = (A10000000000000000000002 /* AppDelegate.swift */, A10000000000000000000004 /* libincan_probe.dylib */, A1000000000000000000000A /* Products */, ); sourceTree = "<group>"; }};
        A1000000000000000000000A /* Products */ = {{isa = PBXGroup; children = (A10000000000000000000007 /* IncanInteropProbe.app */, ); name = Products; sourceTree = "<group>"; }};
/* End PBXGroup section */

/* Begin PBXNativeTarget section */
        A1000000000000000000000B /* IncanInteropProbe */ = {{isa = PBXNativeTarget; buildConfigurationList = A1000000000000000000000C /* Build configuration list for PBXNativeTarget */; buildPhases = (A1000000000000000000000D /* Sources */, A10000000000000000000008 /* Frameworks */, A10000000000000000000006 /* Embed Frameworks */, ); buildRules = (); dependencies = (); name = IncanInteropProbe; productName = IncanInteropProbe; productReference = A10000000000000000000007 /* IncanInteropProbe.app */; productType = "com.apple.product-type.application"; }};
/* End PBXNativeTarget section */

/* Begin PBXProject section */
        A1000000000000000000000E /* Project object */ = {{isa = PBXProject; attributes = {{LastSwiftUpdateCheck = 2600; LastUpgradeCheck = 2600; TargetAttributes = {{A1000000000000000000000B = {{CreatedOnToolsVersion = 26.0; }}; }}; }}; buildConfigurationList = A1000000000000000000000F /* Build configuration list for PBXProject */; compatibilityVersion = "Xcode 14.0"; developmentRegion = en; hasScannedForEncodings = 0; knownRegions = (en, Base, ); mainGroup = A10000000000000000000009; productRefGroup = A1000000000000000000000A /* Products */; projectDirPath = ""; projectRoot = ""; targets = (A1000000000000000000000B /* IncanInteropProbe */, ); }};
/* End PBXProject section */

/* Begin PBXSourcesBuildPhase section */
        A1000000000000000000000D /* Sources */ = {{isa = PBXSourcesBuildPhase; buildActionMask = 2147483647; files = (A10000000000000000000001 /* AppDelegate.swift in Sources */, ); runOnlyForDeploymentPostprocessing = 0; }};
/* End PBXSourcesBuildPhase section */

/* Begin XCBuildConfiguration section */
        A10000000000000000000010 /* Debug */ = {{isa = XCBuildConfiguration; buildSettings = {{ALWAYS_SEARCH_USER_PATHS = NO; CLANG_ENABLE_MODULES = YES; CODE_SIGNING_ALLOWED = NO; IPHONEOS_DEPLOYMENT_TARGET = 13.0; SDKROOT = iphonesimulator; }}; name = Debug; }};
        A10000000000000000000011 /* Release */ = {{isa = XCBuildConfiguration; buildSettings = {{ALWAYS_SEARCH_USER_PATHS = NO; CLANG_ENABLE_MODULES = YES; CODE_SIGNING_ALLOWED = NO; IPHONEOS_DEPLOYMENT_TARGET = 13.0; SDKROOT = iphonesimulator; }}; name = Release; }};
        A10000000000000000000012 /* Debug */ = {{isa = XCBuildConfiguration; buildSettings = {{ARCHS = arm64; CODE_SIGNING_ALLOWED = NO; INFOPLIST_FILE = Info.plist; IPHONEOS_DEPLOYMENT_TARGET = 13.0; LD_RUNPATH_SEARCH_PATHS = "@executable_path/Frameworks"; LIBRARY_SEARCH_PATHS = "{}"; PRODUCT_BUNDLE_IDENTIFIER = dev.incan.interop.probe; PRODUCT_NAME = IncanInteropProbe; SDKROOT = iphonesimulator; SUPPORTED_PLATFORMS = iphonesimulator; SWIFT_VERSION = 6.0; TARGETED_DEVICE_FAMILY = 1; }}; name = Debug; }};
        A10000000000000000000013 /* Release */ = {{isa = XCBuildConfiguration; buildSettings = {{ARCHS = arm64; CODE_SIGNING_ALLOWED = NO; INFOPLIST_FILE = Info.plist; IPHONEOS_DEPLOYMENT_TARGET = 13.0; LD_RUNPATH_SEARCH_PATHS = "@executable_path/Frameworks"; LIBRARY_SEARCH_PATHS = "{}"; PRODUCT_BUNDLE_IDENTIFIER = dev.incan.interop.probe; PRODUCT_NAME = IncanInteropProbe; SDKROOT = iphonesimulator; SUPPORTED_PLATFORMS = iphonesimulator; SWIFT_VERSION = 6.0; TARGETED_DEVICE_FAMILY = 1; }}; name = Release; }};
/* End XCBuildConfiguration section */

/* Begin XCConfigurationList section */
        A1000000000000000000000F /* Build configuration list for PBXProject */ = {{isa = XCConfigurationList; buildConfigurations = (A10000000000000000000010 /* Debug */, A10000000000000000000011 /* Release */, ); defaultConfigurationIsVisible = 0; defaultConfigurationName = Release; }};
        A1000000000000000000000C /* Build configuration list for PBXNativeTarget */ = {{isa = XCConfigurationList; buildConfigurations = (A10000000000000000000012 /* Debug */, A10000000000000000000013 /* Release */, ); defaultConfigurationIsVisible = 0; defaultConfigurationName = Release; }};
/* End XCConfigurationList section */
    }};
    rootObject = A1000000000000000000000E /* Project object */;
}}
"#,
            staged_library.display(),
            staged_library_dir.display(),
            staged_library_dir.display()
        );
        fs::write(xcode_project.join("project.pbxproj"), project_file)?;
        let derived_data = project.path().join("derived-data");
        run_virtual_device_command(
            Command::new("xcodebuild").args([
                "-project",
                xcode_project.to_string_lossy().as_ref(),
                "-scheme",
                "IncanInteropProbe",
                "-sdk",
                "iphonesimulator",
                "-destination",
                format!("id={simulator}").as_str(),
                "-derivedDataPath",
                derived_data.to_string_lossy().as_ref(),
                "build",
            ]),
            "build Xcode iOS Simulator consumer",
        )?;
        let application = derived_data.join("Build/Products/Debug-iphonesimulator/IncanInteropProbe.app");
        if !application.is_dir() {
            return Err(format!("Xcode did not produce iOS Simulator app {}", application.display()).into());
        }
        let bundle_id = "dev.incan.interop.probe";
        let _ = Command::new("xcrun")
            .args(["simctl", "uninstall", &simulator, bundle_id])
            .output();
        run_virtual_device_command(
            Command::new("xcrun").args(["simctl", "install", &simulator, application.to_string_lossy().as_ref()]),
            "install staged iOS Simulator probe",
        )?;
        let launch = run_virtual_device_command(
            Command::new("xcrun").args([
                "simctl",
                "launch",
                "--terminate-running-process",
                "--console",
                &simulator,
                bundle_id,
            ]),
            "launch staged iOS Simulator probe",
        )?;
        let _ = Command::new("xcrun")
            .args(["simctl", "uninstall", &simulator, bundle_id])
            .output();
        let launch_output = format!(
            "{}\n{}",
            String::from_utf8_lossy(&launch.stdout),
            String::from_utf8_lossy(&launch.stderr)
        );
        assert!(
            launch_output.contains("INCAN_INTEROP_NATIVE_PROBE=42"),
            "iOS Simulator did not report the staged native result:\n{launch_output}"
        );
        Ok(())
    }

    /// Exercise the Android platform-consumer boundary after Oven has staged a selected runtime bundle.
    ///
    /// The deliberately ignored evidence test makes the host tool locations explicit. Gradle consumes only Oven's
    /// fixed `jniLibs/arm64-v8a` staging layout; the package never supplies an ambient ABI filter, native compiler,
    /// or Cargo fallback to the Android project.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires explicit ADB, Android SDK, NDK, and Gradle paths for a booted Android emulator"]
    fn android_emulator_launches_a_gradle_consumer_of_a_staged_oven_native_artifact()
    -> Result<(), Box<dyn std::error::Error>> {
        let adb = required_android_virtual_device_path("INCAN_TEST_ANDROID_ADB")?;
        let android_sdk = required_android_virtual_device_path("INCAN_TEST_ANDROID_SDK_ROOT")?;
        let android_ndk = required_android_virtual_device_path("INCAN_TEST_ANDROID_NDK_ROOT")?;
        let gradle = required_android_virtual_device_path("INCAN_TEST_ANDROID_GRADLE")?;
        let android_clang = android_ndk_arm64_clang(&android_ndk)?;
        let android_toolchain_root = android_clang
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| format!("Android Clang has no NDK toolchain root: {}", android_clang.display()))?;
        let jni_headers = android_toolchain_root.join("sysroot/usr/include");
        for (label, path) in [
            ("Android ADB", &adb),
            ("Android SDK", &android_sdk),
            ("Android NDK", &android_ndk),
            ("Android Gradle", &gradle),
            ("Android arm64 Clang", &android_clang),
            ("Android JNI headers", &jni_headers),
        ] {
            if !path.exists() {
                return Err(format!("{label} path does not exist: {}", path.display()).into());
            }
        }

        let project = tempfile::tempdir()?;
        let source = project.path().join("incan_probe.c");
        let runtime = project.path().join("libincan_probe.so");
        let runtime_source = r#"#include <jni.h>

JNIEXPORT jint JNICALL
Java_dev_incan_interop_probe_MainActivity_nativeProbe(JNIEnv *environment, jclass activity) {
    (void)environment;
    (void)activity;
    return 42;
}
"#;
        fs::write(&source, runtime_source)?;
        let mut compile = Command::new(&android_clang);
        compile
            .args(["-shared", "-fPIC", "-I"])
            .arg(&jni_headers)
            .arg(&source)
            .args(["-o"])
            .arg(&runtime);
        run_virtual_device_command(&mut compile, "compile Android arm64 JNI native probe")?;

        let generated = project.path().join("generated.rs");
        fs::write(&generated, "fn main() {}\n")?;
        let base_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "android-emulator-adapter-fixture",
                "0.1.0",
                "aarch64-linux-android",
                "fixture-rustc",
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated),
        )?;
        let store = OvenStore::new(
            project.path().join("oven-store"),
            OvenStoreLimits::new(32 * 1024 * 1024, 32 * 1024 * 1024, 32 * 1024 * 1024),
        );
        store.publish(&OvenArtifactPublishRequest {
            receipt: base_receipt.clone(),
            domain: "runtime".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&base_plan(base_receipt.intent.clone()))?,
            materialized_files: Vec::new(),
        })?;
        let mut target = LockedInteropTarget {
            target: "aarch64-linux-android".to_string(),
            toolchain: None,
            sdk: None,
            platform: Some(InteropTargetPlatform::Android { api_level: 36 }),
            definitions: Vec::new(),
            headers: Vec::new(),
            artifacts: vec![LockedInteropArtifact {
                name: "incan_probe".to_string(),
                kind: InteropArtifactKind::Bundled,
                input: Some(LockedInteropInput {
                    path: "libincan_probe.so".to_string(),
                    digest: digest_bytes(runtime_source.as_bytes()),
                }),
                origin: None,
                capability: None,
                runtime_name: Some("libincan_probe.so".to_string()),
                placement: Some("jniLibs/arm64-v8a".to_string()),
                minimum_platform: Some("21".to_string()),
                dependencies: Vec::new(),
            }],
            bindings: Vec::new(),
            shims: Vec::new(),
        };
        if let Some(input) = target.artifacts[0].input.as_mut() {
            input.digest = digest_bytes(&fs::read(&runtime)?);
        }
        let execution_receipt = receipt_interop_execution(&target, None, None)?;
        let baked = bake_interop_native_plan(static_bake_request(
            &store,
            project.path(),
            &target,
            &base_receipt,
            &execution_receipt,
        ))?;
        let stage = project.path().join("adapter-stage");
        let staged = stage_interop_adapter(OvenInteropAdapterStageRequest {
            store: &store,
            target: &target,
            base_receipt: &base_receipt,
            execution_receipt: &execution_receipt,
            adapter: OvenInteropAdapter::Android,
            output: &stage,
        })?;
        assert_eq!(staged.plan_identity, baked.plan_identity);

        let android = project.path().join("android");
        let app = android.join("app");
        let java = app.join("src/main/java/dev/incan/interop/probe");
        fs::create_dir_all(&java)?;
        fs::write(
            android.join("settings.gradle"),
            r#"pluginManagement { repositories { google(); mavenCentral(); gradlePluginPortal() } }
dependencyResolutionManagement { repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS); repositories { google(); mavenCentral() } }
rootProject.name = "IncanInteropProbe"
include(":app")
"#,
        )?;
        fs::write(
            android.join("build.gradle"),
            r#"plugins {
    id "com.android.application" version "9.3.0" apply false
}
"#,
        )?;
        fs::write(
            app.join("build.gradle"),
            format!(
                r#"plugins {{
    id "com.android.application"
}}

android {{
    namespace "dev.incan.interop.probe"
    compileSdk 36

    defaultConfig {{
        applicationId "dev.incan.interop.probe"
        minSdk 21
        targetSdk 36
        versionCode 1
        versionName "1.0"
    }}

    sourceSets {{
        main {{
            jniLibs.srcDirs = ["{}/jniLibs"]
        }}
    }}
}}
"#,
                stage.display()
            ),
        )?;
        fs::write(
            app.join("src/main/AndroidManifest.xml"),
            r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android">
    <application android:label="Incan interop probe">
        <activity android:name=".MainActivity" android:exported="true">
            <intent-filter>
                <action android:name="android.intent.action.MAIN" />
                <category android:name="android.intent.category.LAUNCHER" />
            </intent-filter>
        </activity>
    </application>
</manifest>
"#,
        )?;
        fs::write(
            java.join("MainActivity.java"),
            r#"package dev.incan.interop.probe;

import android.app.Activity;
import android.os.Bundle;
import android.util.Log;

public final class MainActivity extends Activity {
    private static final String TAG = "IncanInteropProbe";

    static {
        System.loadLibrary("incan_probe");
    }

    private static native int nativeProbe();

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        int result = nativeProbe();
        Log.i(TAG, "INCAN_INTEROP_NATIVE_PROBE=" + result);
        if (result != 42) {
            throw new IllegalStateException("staged native probe returned " + result);
        }
        finish();
    }
}
"#,
        )?;
        let mut build = Command::new(&gradle);
        build
            .current_dir(&android)
            .env("ANDROID_HOME", &android_sdk)
            .env("ANDROID_SDK_ROOT", &android_sdk)
            .args(["--offline", "--no-daemon", ":app:assembleDebug"]);
        run_virtual_device_command(&mut build, "build Gradle Android emulator consumer")?;
        let apk = app.join("build/outputs/apk/debug/app-debug.apk");
        if !apk.is_file() {
            return Err(format!("Gradle did not produce Android probe APK {}", apk.display()).into());
        }

        let bundle_id = "dev.incan.interop.probe";
        let _ = Command::new(&adb).args(["logcat", "-c"]).output();
        let _ = Command::new(&adb).args(["uninstall", bundle_id]).output();
        let mut install = Command::new(&adb);
        install.args(["install", "-r"]).arg(&apk);
        run_virtual_device_command(&mut install, "install staged Android emulator probe")?;
        let mut launch = Command::new(&adb);
        launch.args([
            "shell",
            "am",
            "start",
            "-W",
            "-n",
            "dev.incan.interop.probe/.MainActivity",
        ]);
        run_virtual_device_command(&mut launch, "launch staged Android emulator probe")?;
        let logcat = run_virtual_device_command(
            Command::new(&adb).args(["logcat", "-d", "-s", "IncanInteropProbe:I", "*:S"]),
            "read Android emulator native-probe log",
        )?;
        let _ = Command::new(&adb).args(["uninstall", bundle_id]).output();
        let log_output = format!(
            "{}\n{}",
            String::from_utf8_lossy(&logcat.stdout),
            String::from_utf8_lossy(&logcat.stderr)
        );
        assert!(
            log_output.contains("INCAN_INTEROP_NATIVE_PROBE=42"),
            "Android emulator did not report the staged native result:\n{log_output}"
        );
        Ok(())
    }
}
