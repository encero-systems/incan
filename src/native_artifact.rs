//! Binding-kind-neutral native package inputs and their portable lock projection.
//!
//! A native package declaration names the physical inputs selected for one target. It does not perform ambient
//! discovery, compile a shim, or decide application semantics. Those actions consume this checked plan in later
//! RFC 116 slices.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::manifest::ProjectManifest;

/// Current compatibility format for the `[native]` manifest section.
pub const NATIVE_MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Current compatibility format for a target-resolved native deployment handoff.
pub(crate) const NATIVE_DEPLOYMENT_PLAN_SCHEMA_VERSION: u32 = 1;

/// Target-specific package inputs for checked native interop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct NativeSection {
    /// Version of the native package-input schema.
    pub schema: u32,
    /// Independently selected physical inputs for each target triple.
    #[serde(default)]
    pub targets: Vec<NativeTarget>,
}

impl NativeSection {
    /// Validate configuration that is independent from the package filesystem.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != NATIVE_MANIFEST_SCHEMA_VERSION {
            return Err(format!(
                "[native].schema must be {NATIVE_MANIFEST_SCHEMA_VERSION}, found {}",
                self.schema
            ));
        }
        if self.targets.is_empty() {
            return Err("[native] requires at least one [[native.targets]] entry".to_string());
        }

        let mut target_names = BTreeSet::new();
        for target in &self.targets {
            if target.target.trim().is_empty() || !target_names.insert(target.target.clone()) {
                return Err(format!(
                    "native target `{}` must be a unique non-empty target triple",
                    target.target
                ));
            }
            if target.toolchain.trim().is_empty() {
                return Err(format!(
                    "native target `{}` requires a managed toolchain identity",
                    target.target
                ));
            }
            validate_native_paths(&target.headers, &format!("native target `{}` header", target.target))?;
            if target.definitions.iter().any(|definition| definition.trim().is_empty()) {
                return Err(format!(
                    "native target `{}` contains an empty preprocessor definition",
                    target.target
                ));
            }
            if target
                .provenance
                .as_ref()
                .is_some_and(|provenance| provenance.trim().is_empty())
            {
                return Err(format!("native target `{}` provenance cannot be empty", target.target));
            }
            if let Some(platform) = &target.platform {
                validate_target_platform(platform, target)?;
            }

            let mut artifact_names = BTreeSet::new();
            for artifact in &target.artifacts {
                validate_artifact(artifact, &target.target, &mut artifact_names)?;
            }
            for artifact in &target.artifacts {
                validate_artifact_dependencies(artifact, &target.target, &artifact_names)?;
            }
            ordered_artifact_names(
                &target.target,
                target
                    .artifacts
                    .iter()
                    .map(|artifact| (artifact.name.clone(), artifact.dependencies.clone()))
                    .collect(),
            )?;
            let mut shim_names = BTreeSet::new();
            for shim in &target.shims {
                if shim.name.trim().is_empty() || !shim_names.insert(shim.name.clone()) {
                    return Err(format!(
                        "native target `{}` shim names must be unique and non-empty",
                        target.target
                    ));
                }
                if shim.sources.is_empty() {
                    return Err(format!(
                        "native shim `{}` requires at least one source input",
                        shim.name
                    ));
                }
                validate_native_paths(&shim.sources, &format!("native shim `{}` source", shim.name))?;
                validate_native_paths(&shim.headers, &format!("native shim `{}` header", shim.name))?;
                if shim.output.trim().is_empty() {
                    return Err(format!("native shim `{}` requires a logical output name", shim.name));
                }
            }
        }
        Ok(())
    }
}

/// Physical native inputs selected for one target triple.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct NativeTarget {
    /// The exact compilation and deployment target triple.
    pub target: String,
    /// Managed Clang-compatible toolchain identity selected for this target.
    pub toolchain: String,
    /// Optional selected SDK identity; Apple and Android SDKs remain toolchain capabilities rather than package files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk: Option<String>,
    /// Target-platform facts that affect ABI verification and deployment planning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<NativeTargetPlatform>,
    /// Package-relative public or shim headers used for verification.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<String>,
    /// Explicit preprocessor definitions supplied to verification and later shim baking.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub definitions: Vec<String>,
    /// Optional package provenance label retained in the lock projection without publication-policy interpretation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
    /// Selected prebuilt or system-native artifacts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<NativeArtifact>,
    /// Authored C or C++ shim source inputs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shims: Vec<NativeShim>,
}

/// Platform-specific constraints that complete a mobile native target identity.
///
/// The target triple remains the source of CPU and operating-system identity. This profile carries the platform
/// version selected by the target toolchain, which Android and Apple Clang need in addition to that triple.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", rename_all_fields = "kebab-case")]
pub enum NativeTargetPlatform {
    /// Android arm64 verification and deployment require an NDK API level.
    Android {
        /// Android API level selected for Clang, libc, and deployment compatibility.
        api_level: u32,
    },
    /// iOS verification and deployment require a minimum supported OS version.
    Ios {
        /// Minimum iOS version selected for Clang and deployment compatibility.
        deployment_target: String,
    },
}

/// Validate that one mobile profile supplies the target triple, platform version, and SDK identity it requires.
fn validate_target_platform(platform: &NativeTargetPlatform, target: &NativeTarget) -> Result<(), String> {
    match platform {
        NativeTargetPlatform::Android { api_level } => {
            if target.target != "aarch64-linux-android" {
                return Err(format!(
                    "Android platform facts require the `aarch64-linux-android` target, found `{}`",
                    target.target
                ));
            }
            if *api_level < 21 {
                return Err(format!(
                    "Android arm64 target `{}` requires API level 21 or later, found {api_level}",
                    target.target
                ));
            }
            validate_sdk_identity(target.sdk.as_deref(), "android-", "Android", &target.target)
        }
        NativeTargetPlatform::Ios { deployment_target } => {
            if target.target != "aarch64-apple-ios" {
                return Err(format!(
                    "iOS platform facts require the `aarch64-apple-ios` target, found `{}`",
                    target.target
                ));
            }
            if !is_deployment_target_version(deployment_target) {
                return Err(format!(
                    "iOS deployment target `{deployment_target}` for `{}` must be a numeric `major.minor` version",
                    target.target
                ));
            }
            validate_sdk_identity(target.sdk.as_deref(), "iphoneos-", "iOS", &target.target)
        }
    }
}

/// Require one declared logical SDK identity to use the platform-specific namespace reserved for this target.
fn validate_sdk_identity(sdk: Option<&str>, expected_prefix: &str, platform: &str, target: &str) -> Result<(), String> {
    let Some(sdk) = sdk else {
        return Err(format!(
            "{platform} target `{target}` requires an SDK identity beginning with `{expected_prefix}`"
        ));
    };
    if sdk.starts_with(expected_prefix) {
        Ok(())
    } else {
        Err(format!(
            "{platform} target `{target}` requires an SDK identity beginning with `{expected_prefix}`, found `{sdk}`"
        ))
    }
}

/// Return whether a declared iOS deployment target uses an explicit numeric major and minor version.
fn is_deployment_target_version(value: &str) -> bool {
    let mut components = value.split('.');
    let Some(first) = components.next() else {
        return false;
    };
    let Some(second) = components.next() else {
        return false;
    };
    !first.is_empty()
        && first.bytes().all(|byte| byte.is_ascii_digit())
        && !second.is_empty()
        && second.bytes().all(|byte| byte.is_ascii_digit())
        && components.all(|component| !component.is_empty() && component.bytes().all(|byte| byte.is_ascii_digit()))
}

/// One declared physical native artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct NativeArtifact {
    /// Binding-local logical name for this artifact.
    pub name: String,
    /// How the selected target consumes this artifact.
    pub kind: NativeArtifactKind,
    /// Package-relative file for static and bundled artifacts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Toolchain or SDK capability for a system-provided artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    /// Runtime loader name required for a bundled artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_name: Option<String>,
    /// Platform-packaging destination for a bundled artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<String>,
    /// Minimum platform constraint for a bundled artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_platform: Option<String>,
    /// Logical names of transitive native artifact dependencies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
}

/// Deployment class for a selected native artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeArtifactKind {
    /// An archive linked directly into the generated product.
    Static,
    /// A dynamic library or framework staged for a platform packager.
    Bundled,
    /// A library or framework supplied by the selected SDK/toolchain capability.
    System,
}

/// One authored shim whose bounded exported surface will be verified and built by later native tooling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct NativeShim {
    /// Stable package-local shim name.
    pub name: String,
    /// Source language accepted by the future managed shim baker.
    pub language: NativeShimLanguage,
    /// Package-relative authored C or C++ source files.
    pub sources: Vec<String>,
    /// Package-relative headers that describe the shim's bounded exported contract.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<String>,
    /// Logical name for the generated native output.
    pub output: String,
}

/// Language of one authored native shim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeShimLanguage {
    /// C source compiled by the selected managed Clang-compatible toolchain.
    C,
    /// C++ source compiled only behind a bounded C-compatible shim surface.
    Cxx,
}

impl NativeShimLanguage {
    /// Stable manifest and inspection spelling for this shim language.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::C => "c",
            Self::Cxx => "cxx",
        }
    }
}

/// Portable native inputs frozen in one canonical semantic lock state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedNativeTarget {
    /// Exact target triple selected by the package plan.
    pub target: String,
    /// Selected managed toolchain identity.
    pub toolchain: String,
    /// Selected SDK identity, when the target requires one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk: Option<String>,
    /// Target-platform facts retained for target-specific verification and deployment planning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<NativeTargetPlatform>,
    /// Explicit definitions sorted as part of the target configuration identity.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub definitions: Vec<String>,
    /// Header inputs and their content identities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<LockedNativeInput>,
    /// Selected static, bundled, or system artifact identities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<LockedNativeArtifact>,
    /// Authored shim source and header identities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shims: Vec<LockedNativeShim>,
    /// Package-supplied provenance label retained without making publication-policy decisions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
}

/// One package-relative immutable native input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedNativeInput {
    /// Package-relative portable input path.
    pub path: String,
    /// Content digest computed from the exact declared file bytes.
    pub digest: String,
}

/// One selected artifact projected into portable lock data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedNativeArtifact {
    /// Binding-local artifact name.
    pub name: String,
    /// Static, bundled, or system deployment class.
    pub kind: NativeArtifactKind,
    /// File input identity when the package provides artifact bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<LockedNativeInput>,
    /// Toolchain capability when the selected artifact is system-provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    /// Runtime loader name for bundled outputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_name: Option<String>,
    /// Packaging destination for bundled outputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<String>,
    /// Minimum platform constraint for bundled outputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_platform: Option<String>,
    /// Transitive native dependencies in deterministic order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
}

/// One authored shim's immutable sources and intended logical output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedNativeShim {
    /// Stable package-local shim name.
    pub name: String,
    /// Selected C or C++ source language.
    pub language: NativeShimLanguage,
    /// Authored source file identities.
    pub sources: Vec<LockedNativeInput>,
    /// Shim-header identities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<LockedNativeInput>,
    /// Logical output name selected for later shim baking.
    pub output: String,
}

/// One portable target-native handoff emitted from canonical locked package inputs.
///
/// This is the declared platform-packager boundary carried by a future Loaf. It records locked physical facts without
/// embedding a Gradle task, Xcode build phase, signing identity, credential, or machine-local toolchain path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NativeDeploymentPlan {
    /// Compatibility version for this deployment-plan shape.
    pub(crate) schema_version: u32,
    /// Exact compilation and deployment target triple.
    pub(crate) target: String,
    /// Logical managed toolchain identity retained from the lock.
    pub(crate) toolchain: String,
    /// Logical SDK identity retained from the lock, when the target requires one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sdk: Option<String>,
    /// Platform version facts needed by a Gradle or Xcode adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) platform: Option<NativeDeploymentPlatform>,
    /// Locked header files used by verification and downstream native compilation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) headers: Vec<LockedNativeInput>,
    /// Portable package-relative include roots derived from locked headers and shim headers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) include_roots: Vec<String>,
    /// Explicit preprocessor definitions applied to verification and shim compilation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) definitions: Vec<String>,
    /// Deterministic dependencies-first static, bundled, and system planning actions.
    ///
    /// The explicit dependency edges remain authoritative. A platform adapter must derive its linker's argument order
    /// rather than treating this planning sequence as a raw command line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) artifacts: Vec<NativeDeploymentArtifact>,
    /// Authored shim build inputs and logical outputs required before platform handoff.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) shims: Vec<NativeShimBuildPlan>,
    /// Package-supplied provenance retained as evidence rather than publication admission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) provenance: Option<String>,
}

/// One dependency-ordered native artifact action in a deployment plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NativeDeploymentArtifact {
    /// Stable package-local artifact name.
    pub(crate) name: String,
    /// Logical sibling artifacts that must be available before this artifact.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) dependencies: Vec<String>,
    /// Structured platform-neutral action for the selected deployment class.
    #[serde(flatten)]
    pub(crate) action: NativeDeploymentAction,
}

/// Mobile platform facts projected into the JSON handoff independently from manifest field spelling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum NativeDeploymentPlatform {
    /// Android arm64 handoff facts.
    Android {
        /// Android API level selected for verification and deployment.
        api_level: u32,
    },
    /// iOS arm64 handoff facts.
    Ios {
        /// Minimum supported iOS deployment target.
        deployment_target: String,
    },
}

impl From<&NativeTargetPlatform> for NativeDeploymentPlatform {
    fn from(platform: &NativeTargetPlatform) -> Self {
        match platform {
            NativeTargetPlatform::Android { api_level } => Self::Android { api_level: *api_level },
            NativeTargetPlatform::Ios { deployment_target } => Self::Ios {
                deployment_target: deployment_target.clone(),
            },
        }
    }
}

/// Platform-neutral action consumed by a later Gradle, Xcode, or other packager adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "deployment", rename_all = "snake_case")]
pub(crate) enum NativeDeploymentAction {
    /// Link one locked package archive into the final product.
    StaticLink {
        /// Portable archive path and digest.
        input: LockedNativeInput,
    },
    /// Stage one locked dynamic library or framework for the platform packager.
    Bundle {
        /// Portable dynamic artifact path and digest.
        input: LockedNativeInput,
        /// Runtime loader name expected by the native dependency graph.
        runtime_name: String,
        /// Logical packager placement retained without embedding an absolute output path.
        placement: String,
        /// Minimum platform version required by this artifact.
        minimum_platform: String,
    },
    /// Request one explicit library or framework capability from the selected toolchain or SDK.
    System {
        /// Stable capability identity such as `apple.framework.Accelerate`.
        capability: String,
    },
}

/// One governed authored shim action that must be baked before the deployment plan is ready for final assembly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NativeShimBuildPlan {
    /// Stable package-local shim name.
    pub(crate) name: String,
    /// Selected C or C++ source language.
    pub(crate) language: NativeShimLanguage,
    /// Locked authored source inputs.
    pub(crate) sources: Vec<LockedNativeInput>,
    /// Locked headers describing the shim's bounded C contract.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) headers: Vec<LockedNativeInput>,
    /// Logical artifact name produced by the future managed shim baker.
    pub(crate) output: String,
}

/// Resolve declared native package files into lockable content identities without ambient host discovery.
pub fn locked_native_targets(manifest: &ProjectManifest) -> Result<Vec<LockedNativeTarget>, String> {
    locked_native_targets_from_section(manifest.project_root(), manifest.native())
}

/// Resolve a parsed native declaration for the specified project root into portable lock entries.
///
/// This accepts the already-parsed manifest section so lock generation can include native inputs alongside the
/// provider and SDK semantic state without rediscovering or reparsing the manifest.
pub fn locked_native_targets_from_section(
    project_root: &Path,
    native: Option<&NativeSection>,
) -> Result<Vec<LockedNativeTarget>, String> {
    let Some(native) = native else {
        return Ok(Vec::new());
    };
    native.validate()?;
    let mut targets = native
        .targets
        .iter()
        .map(|target| lock_native_target(project_root, target))
        .collect::<Result<Vec<_>, _>>()?;
    targets.sort_by(|left, right| left.target.cmp(&right.target));
    Ok(targets)
}

/// Project one canonical locked target into a deterministic native deployment handoff.
///
/// The projection neither reads the package filesystem nor discovers host libraries. Every physical file comes from
/// an existing locked input receipt, so relocating the package does not change the emitted plan.
pub(crate) fn native_deployment_plan(target: &LockedNativeTarget) -> Result<NativeDeploymentPlan, String> {
    // ---- Validate and order the artifact graph ----
    let artifact_names = ordered_artifact_names(
        &target.target,
        target
            .artifacts
            .iter()
            .map(|artifact| (artifact.name.clone(), artifact.dependencies.clone()))
            .collect(),
    )?;
    let artifacts_by_name = target
        .artifacts
        .iter()
        .map(|artifact| (artifact.name.as_str(), artifact))
        .collect::<BTreeMap<_, _>>();
    let artifacts = artifact_names
        .iter()
        .map(|name| {
            let artifact = artifacts_by_name.get(name.as_str()).ok_or_else(|| {
                format!(
                    "native deployment plan for `{}` lost artifact `{name}` while ordering dependencies",
                    target.target
                )
            })?;
            deployment_artifact(artifact, &target.target)
        })
        .collect::<Result<Vec<_>, _>>()?;

    // ---- Project governed shim inputs ----
    let shims = target
        .shims
        .iter()
        .map(|shim| NativeShimBuildPlan {
            name: shim.name.clone(),
            language: shim.language,
            sources: shim.sources.clone(),
            headers: shim.headers.clone(),
            output: shim.output.clone(),
        })
        .collect();

    Ok(NativeDeploymentPlan {
        schema_version: NATIVE_DEPLOYMENT_PLAN_SCHEMA_VERSION,
        target: target.target.clone(),
        toolchain: target.toolchain.clone(),
        sdk: target.sdk.clone(),
        platform: target.platform.as_ref().map(NativeDeploymentPlatform::from),
        headers: target.headers.clone(),
        include_roots: native_include_roots(target),
        definitions: target.definitions.clone(),
        artifacts,
        shims,
        provenance: target.provenance.clone(),
    })
}

/// Convert one locked artifact into the exact action required by its declared deployment class.
fn deployment_artifact(artifact: &LockedNativeArtifact, target: &str) -> Result<NativeDeploymentArtifact, String> {
    // ---- Select the structured deployment action ----
    let action = match artifact.kind {
        NativeArtifactKind::Static => NativeDeploymentAction::StaticLink {
            input: required_locked_artifact_input(artifact, target)?,
        },
        NativeArtifactKind::Bundled => NativeDeploymentAction::Bundle {
            input: required_locked_artifact_input(artifact, target)?,
            runtime_name: required_locked_artifact_field(
                artifact.runtime_name.as_deref(),
                artifact,
                target,
                "runtime name",
            )?,
            placement: required_locked_artifact_field(artifact.placement.as_deref(), artifact, target, "placement")?,
            minimum_platform: required_locked_artifact_field(
                artifact.minimum_platform.as_deref(),
                artifact,
                target,
                "minimum platform",
            )?,
        },
        NativeArtifactKind::System => NativeDeploymentAction::System {
            capability: required_locked_artifact_field(
                artifact.capability.as_deref(),
                artifact,
                target,
                "system capability",
            )?,
        },
    };

    // ---- Normalize explicit dependency edges ----
    let mut dependencies = artifact.dependencies.clone();
    dependencies.sort();
    Ok(NativeDeploymentArtifact {
        name: artifact.name.clone(),
        dependencies,
        action,
    })
}

/// Require one package-file receipt for a static or bundled artifact.
fn required_locked_artifact_input(artifact: &LockedNativeArtifact, target: &str) -> Result<LockedNativeInput, String> {
    artifact.input.clone().ok_or_else(|| {
        format!(
            "locked native artifact `{}` on target `{target}` is missing its package-file receipt",
            artifact.name
        )
    })
}

/// Require one non-empty deployment field from a canonical locked artifact.
fn required_locked_artifact_field(
    value: Option<&str>,
    artifact: &LockedNativeArtifact,
    target: &str,
    field: &str,
) -> Result<String, String> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            format!(
                "locked native artifact `{}` on target `{target}` is missing its {field}",
                artifact.name
            )
        })
}

/// Derive deterministic package-relative include roots without leaking the package's current absolute location.
fn native_include_roots(target: &LockedNativeTarget) -> Vec<String> {
    let mut roots = target
        .headers
        .iter()
        .chain(target.shims.iter().flat_map(|shim| shim.headers.iter()))
        .map(|input| {
            Path::new(&input.path)
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map_or_else(|| ".".to_string(), |parent| parent.to_string_lossy().to_string())
        })
        .collect::<Vec<_>>();
    roots.sort();
    roots.dedup();
    roots
}

/// Validate one target artifact's deployment kind and the fields it is allowed to declare.
///
/// The manifest uses mutually exclusive shapes for directly linked static archives, packager-staged bundled files,
/// and selected toolchain/SDK capabilities. Enforcing that distinction before any file access keeps later target
/// resolution from interpreting an ambiguous declaration as ambient host discovery.
fn validate_artifact(
    artifact: &NativeArtifact,
    target: &str,
    artifact_names: &mut BTreeSet<String>,
) -> Result<(), String> {
    if artifact.name.trim().is_empty() || !artifact_names.insert(artifact.name.clone()) {
        return Err(format!(
            "native target `{target}` artifact names must be unique and non-empty"
        ));
    }
    match artifact.kind {
        NativeArtifactKind::Static => {
            validate_required_path(
                artifact.path.as_deref(),
                &format!("native static artifact `{}`", artifact.name),
            )?;
            if artifact.capability.is_some() {
                return Err(format!(
                    "native static artifact `{}` cannot declare a system capability",
                    artifact.name
                ));
            }
            if artifact.runtime_name.is_some() || artifact.placement.is_some() || artifact.minimum_platform.is_some() {
                return Err(format!(
                    "native static artifact `{}` cannot declare bundled deployment fields",
                    artifact.name
                ));
            }
        }
        NativeArtifactKind::Bundled => {
            validate_required_path(
                artifact.path.as_deref(),
                &format!("native bundled artifact `{}`", artifact.name),
            )?;
            validate_non_empty(
                artifact.runtime_name.as_deref(),
                &format!("native bundled artifact `{}` runtime-name", artifact.name),
            )?;
            validate_non_empty(
                artifact.placement.as_deref(),
                &format!("native bundled artifact `{}` placement", artifact.name),
            )?;
            validate_non_empty(
                artifact.minimum_platform.as_deref(),
                &format!("native bundled artifact `{}` minimum-platform", artifact.name),
            )?;
            if artifact.capability.is_some() {
                return Err(format!(
                    "native bundled artifact `{}` cannot declare a system capability",
                    artifact.name
                ));
            }
        }
        NativeArtifactKind::System => {
            validate_non_empty(
                artifact.capability.as_deref(),
                &format!("native system artifact `{}` capability", artifact.name),
            )?;
            if artifact.path.is_some() {
                return Err(format!(
                    "native system artifact `{}` cannot declare a package path",
                    artifact.name
                ));
            }
            if artifact.runtime_name.is_some() || artifact.placement.is_some() || artifact.minimum_platform.is_some() {
                return Err(format!(
                    "native system artifact `{}` cannot declare bundled deployment fields",
                    artifact.name
                ));
            }
        }
    }
    Ok(())
}

/// Require each declared artifact dependency to name one distinct sibling in the same target declaration.
///
/// Native artifact order is normalized in the lock projection, so dependencies must use stable logical names rather
/// than filesystem paths or an implicit declaration order.
fn validate_artifact_dependencies(
    artifact: &NativeArtifact,
    target: &str,
    artifact_names: &BTreeSet<String>,
) -> Result<(), String> {
    let mut dependencies = BTreeSet::new();
    for dependency in &artifact.dependencies {
        if dependency.trim().is_empty()
            || dependency == &artifact.name
            || !artifact_names.contains(dependency)
            || !dependencies.insert(dependency.clone())
        {
            return Err(format!(
                "native artifact `{}` on target `{target}` must depend on distinct declared sibling artifacts",
                artifact.name
            ));
        }
    }
    Ok(())
}

/// Return a stable dependencies-first artifact order and reject malformed or cyclic locked graphs.
fn ordered_artifact_names(target: &str, artifacts: Vec<(String, Vec<String>)>) -> Result<Vec<String>, String> {
    // ---- Validate the declared graph shape ----
    let names = artifacts.iter().map(|(name, _)| name.clone()).collect::<BTreeSet<_>>();
    if names.len() != artifacts.len() {
        return Err(format!(
            "native target `{target}` artifact names must be unique and non-empty"
        ));
    }

    let mut dependency_counts = BTreeMap::new();
    let mut dependents = BTreeMap::<String, BTreeSet<String>>::new();
    for (name, dependencies) in artifacts {
        if name.trim().is_empty() {
            return Err(format!(
                "native target `{target}` artifact names must be unique and non-empty"
            ));
        }
        let unique_dependencies = dependencies.iter().cloned().collect::<BTreeSet<_>>();
        if unique_dependencies.len() != dependencies.len()
            || unique_dependencies
                .iter()
                .any(|dependency| dependency == &name || !names.contains(dependency))
        {
            return Err(format!(
                "native artifact `{name}` on target `{target}` must depend on distinct declared sibling artifacts"
            ));
        }
        dependency_counts.insert(name.clone(), unique_dependencies.len());
        for dependency in unique_dependencies {
            dependents.entry(dependency).or_default().insert(name.clone());
        }
    }

    // ---- Resolve one stable dependencies-first order ----
    let mut ready = dependency_counts
        .iter()
        .filter_map(|(name, count)| (*count == 0).then_some(name.clone()))
        .collect::<BTreeSet<_>>();
    let mut ordered = Vec::with_capacity(dependency_counts.len());
    while let Some(name) = ready.pop_first() {
        ordered.push(name.clone());
        if let Some(dependent_names) = dependents.get(&name) {
            for dependent in dependent_names {
                let count = dependency_counts.get_mut(dependent).ok_or_else(|| {
                    format!("native deployment plan for target `{target}` lost dependency state for `{dependent}`")
                })?;
                *count = count.checked_sub(1).ok_or_else(|| {
                    format!("native deployment plan for target `{target}` counted dependency `{name}` more than once")
                })?;
                if *count == 0 {
                    ready.insert(dependent.clone());
                }
            }
        }
    }

    // ---- Report unresolved cycles ----
    if ordered.len() != dependency_counts.len() {
        let cycle = dependency_counts
            .iter()
            .filter_map(|(name, count)| (*count > 0).then_some(name.as_str()))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "native target `{target}` artifact dependency graph contains a cycle involving: {cycle}"
        ));
    }
    Ok(ordered)
}

/// Require one optional native path field and validate it with the common package-relative path policy.
fn validate_required_path(path: Option<&str>, label: &str) -> Result<(), String> {
    let Some(path) = path else {
        return Err(format!("{label} requires a package-relative path"));
    };
    validate_native_path(path, label)
}

/// Require one optional manifest string field to contain a non-whitespace value.
fn validate_non_empty(value: Option<&str>, label: &str) -> Result<(), String> {
    if value.is_none_or(|value| value.trim().is_empty()) {
        return Err(format!("{label} must be non-empty"));
    }
    Ok(())
}

/// Validate every package path in one manifest list using the shared normalized-relative-path contract.
fn validate_native_paths(paths: &[String], label: &str) -> Result<(), String> {
    for path in paths {
        validate_native_path(path, label)?;
    }
    Ok(())
}

/// Reject a path that could escape the package or acquire platform-dependent meaning outside the declaration.
///
/// Native inputs are locked by their package-relative identity and content bytes. Absolute, parent-relative, current
/// directory, backslash, duplicate-separator, and directory spellings would make that identity ambiguous or permit
/// ambient filesystem lookup.
fn validate_native_path(path: &str, label: &str) -> Result<(), String> {
    let candidate = Path::new(path);
    if path.trim().is_empty()
        || path.contains('\\')
        || path.contains("//")
        || path.ends_with('/')
        || candidate.is_absolute()
        || candidate.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::CurDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "{label} path `{path}` must be a non-empty normalized package-relative path"
        ));
    }
    Ok(())
}

/// Convert one validated target declaration into a canonical, content-addressed lock projection.
///
/// This stage records only already-declared package files and logical deployment facts. It does not build shims,
/// download artifacts, or probe the host for a library.
fn lock_native_target(root: &Path, target: &NativeTarget) -> Result<LockedNativeTarget, String> {
    let mut definitions = target.definitions.clone();
    definitions.sort();
    definitions.dedup();
    let headers = lock_inputs(root, &target.headers)?;
    let mut artifacts = target
        .artifacts
        .iter()
        .map(|artifact| lock_artifact(root, artifact))
        .collect::<Result<Vec<_>, _>>()?;
    artifacts.sort_by(|left, right| left.name.cmp(&right.name));
    let mut shims = target
        .shims
        .iter()
        .map(|shim| {
            Ok(LockedNativeShim {
                name: shim.name.clone(),
                language: shim.language,
                sources: lock_inputs(root, &shim.sources)?,
                headers: lock_inputs(root, &shim.headers)?,
                output: shim.output.clone(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    shims.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(LockedNativeTarget {
        target: target.target.clone(),
        toolchain: target.toolchain.clone(),
        sdk: target.sdk.clone(),
        platform: target.platform.clone(),
        definitions,
        headers,
        artifacts,
        shims,
        provenance: target.provenance.clone(),
    })
}

/// Lock one physical artifact while retaining capability-only system artifacts without a package file receipt.
fn lock_artifact(root: &Path, artifact: &NativeArtifact) -> Result<LockedNativeArtifact, String> {
    let input = artifact
        .path
        .as_deref()
        .map(|path| lock_input(root, path))
        .transpose()?;
    let mut dependencies = artifact.dependencies.clone();
    dependencies.sort();
    dependencies.dedup();
    Ok(LockedNativeArtifact {
        name: artifact.name.clone(),
        kind: artifact.kind,
        input,
        capability: artifact.capability.clone(),
        runtime_name: artifact.runtime_name.clone(),
        placement: artifact.placement.clone(),
        minimum_platform: artifact.minimum_platform.clone(),
        dependencies,
    })
}

/// Resolve a list of declared package files into unique, path-sorted content receipts.
fn lock_inputs(root: &Path, paths: &[String]) -> Result<Vec<LockedNativeInput>, String> {
    let mut inputs = paths
        .iter()
        .map(|path| lock_input(root, path))
        .collect::<Result<Vec<_>, _>>()?;
    inputs.sort_by(|left, right| left.path.cmp(&right.path));
    inputs.dedup_by(|left, right| left.path == right.path);
    Ok(inputs)
}

/// Hash one declared regular package file without following a symlink or consulting an ambient search path.
fn lock_input(root: &Path, relative: &str) -> Result<LockedNativeInput, String> {
    validate_native_path(relative, "native input")?;
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("failed to inspect declared native input {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "declared native input {} must be a regular file",
            path.display()
        ));
    }
    let bytes =
        fs::read(&path).map_err(|error| format!("failed to read declared native input {}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(LockedNativeInput {
        path: relative.to_string(),
        digest: format!("sha256:{}", hex::encode(hasher.finalize())),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn project_with_native_inputs() -> Result<(TempDir, ProjectManifest), Box<dyn std::error::Error>> {
        let workspace = TempDir::new()?;
        fs::create_dir_all(workspace.path().join("native/include"))?;
        fs::create_dir_all(workspace.path().join("native/src"))?;
        fs::create_dir_all(workspace.path().join("native/lib"))?;
        fs::write(workspace.path().join("native/include/bridge.h"), "int bridge(void);\n")?;
        fs::write(
            workspace.path().join("native/src/bridge.c"),
            "int bridge(void) { return 7; }\n",
        )?;
        fs::write(workspace.path().join("native/lib/libfixture.a"), b"fixture archive")?;
        let manifest_path = workspace.path().join("incan.toml");
        let manifest = ProjectManifest::from_str(
            r#"
[native]
schema = 1

[[native.targets]]
target = "aarch64-apple-ios"
toolchain = "apple-clang-17"
sdk = "iphoneos-18.0"
headers = ["native/include/bridge.h"]
definitions = ["FIXTURE=1"]
provenance = "fixture-source"

[native.targets.platform]
kind = "ios"
deployment-target = "13.0"

[[native.targets.artifacts]]
name = "fixture"
kind = "static"
path = "native/lib/libfixture.a"
dependencies = ["foundation"]

[[native.targets.artifacts]]
name = "foundation"
kind = "system"
capability = "apple.framework.Foundation"

[[native.targets.shims]]
name = "fixture_bridge"
language = "c"
sources = ["native/src/bridge.c"]
headers = ["native/include/bridge.h"]
output = "fixture_bridge"
"#,
            &manifest_path,
        )?;
        Ok((workspace, manifest))
    }

    fn mobile_target(target: &str, sdk: Option<&str>, platform: NativeTargetPlatform) -> NativeTarget {
        NativeTarget {
            target: target.to_string(),
            toolchain: "managed-clang".to_string(),
            sdk: sdk.map(str::to_string),
            platform: Some(platform),
            headers: Vec::new(),
            definitions: Vec::new(),
            provenance: None,
            artifacts: Vec::new(),
            shims: Vec::new(),
        }
    }

    #[test]
    fn native_inputs_lock_portably_and_change_with_declared_bytes() -> Result<(), Box<dyn std::error::Error>> {
        let (workspace, manifest) = project_with_native_inputs()?;
        let first = locked_native_targets(&manifest)?;
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].target, "aarch64-apple-ios");
        assert_eq!(
            first[0].platform,
            Some(NativeTargetPlatform::Ios {
                deployment_target: "13.0".to_string(),
            })
        );
        assert_eq!(first[0].headers[0].path, "native/include/bridge.h");
        assert_eq!(
            first[0].artifacts[0].input.as_ref().map(|input| input.path.as_str()),
            Some("native/lib/libfixture.a")
        );
        assert_eq!(
            first[0].artifacts[1].capability.as_deref(),
            Some("apple.framework.Foundation")
        );
        assert_eq!(first[0].shims[0].sources[0].path, "native/src/bridge.c");

        fs::write(
            workspace.path().join("native/src/bridge.c"),
            "int bridge(void) { return 8; }\n",
        )?;
        let second = locked_native_targets(&manifest)?;
        assert_ne!(
            first[0].shims[0].sources[0].digest,
            second[0].shims[0].sources[0].digest
        );
        Ok(())
    }

    #[test]
    fn native_deployment_plan_is_portable_dependency_ordered_and_complete() -> Result<(), Box<dyn std::error::Error>> {
        let (workspace, manifest) = project_with_native_inputs()?;
        let locked = locked_native_targets(&manifest)?;
        let plan = native_deployment_plan(&locked[0])?;

        assert_eq!(plan.schema_version, NATIVE_DEPLOYMENT_PLAN_SCHEMA_VERSION);
        assert_eq!(plan.target, "aarch64-apple-ios");
        assert_eq!(plan.toolchain, "apple-clang-17");
        assert_eq!(plan.sdk.as_deref(), Some("iphoneos-18.0"));
        assert_eq!(plan.include_roots, ["native/include"]);
        assert_eq!(
            plan.artifacts
                .iter()
                .map(|artifact| artifact.name.as_str())
                .collect::<Vec<_>>(),
            ["foundation", "fixture"]
        );
        assert!(matches!(
            &plan.artifacts[0].action,
            NativeDeploymentAction::System { capability }
                if capability == "apple.framework.Foundation"
        ));
        assert!(matches!(
            &plan.artifacts[1].action,
            NativeDeploymentAction::StaticLink { input }
                if input.path == "native/lib/libfixture.a"
                    && input.digest.starts_with("sha256:")
        ));
        assert_eq!(plan.artifacts[1].dependencies, ["foundation"]);
        assert_eq!(plan.shims[0].output, "fixture_bridge");
        assert_eq!(plan.shims[0].sources[0].path, "native/src/bridge.c");
        assert_eq!(plan.provenance.as_deref(), Some("fixture-source"));

        let serialized = serde_json::to_string(&plan)?;
        assert!(!serialized.contains(&workspace.path().to_string_lossy().to_string()));
        Ok(())
    }

    #[test]
    fn native_deployment_plan_carries_bundled_runtime_placement() -> Result<(), Box<dyn std::error::Error>> {
        let plan = native_deployment_plan(&LockedNativeTarget {
            target: "aarch64-linux-android".to_string(),
            toolchain: "android-ndk-r29".to_string(),
            sdk: Some("android-36".to_string()),
            platform: Some(NativeTargetPlatform::Android { api_level: 34 }),
            definitions: Vec::new(),
            headers: Vec::new(),
            artifacts: vec![LockedNativeArtifact {
                name: "tflite".to_string(),
                kind: NativeArtifactKind::Bundled,
                input: Some(LockedNativeInput {
                    path: "native/android/arm64-v8a/libtensorflowlite_c.so".to_string(),
                    digest: "sha256:fixture".to_string(),
                }),
                capability: None,
                runtime_name: Some("libtensorflowlite_c.so".to_string()),
                placement: Some("jniLibs/arm64-v8a".to_string()),
                minimum_platform: Some("21".to_string()),
                dependencies: Vec::new(),
            }],
            shims: Vec::new(),
            provenance: Some("tflite-fixture".to_string()),
        })?;

        assert!(matches!(
            &plan.artifacts[0].action,
            NativeDeploymentAction::Bundle {
                input,
                runtime_name,
                placement,
                minimum_platform,
            } if input.path == "native/android/arm64-v8a/libtensorflowlite_c.so"
                && runtime_name == "libtensorflowlite_c.so"
                && placement == "jniLibs/arm64-v8a"
                && minimum_platform == "21"
        ));
        Ok(())
    }

    #[test]
    fn native_artifact_dependency_cycles_are_rejected() {
        let mut target = mobile_target(
            "aarch64-linux-android",
            Some("android-36"),
            NativeTargetPlatform::Android { api_level: 34 },
        );
        target.artifacts = vec![
            NativeArtifact {
                name: "model".to_string(),
                kind: NativeArtifactKind::Static,
                path: Some("native/lib/libmodel.a".to_string()),
                capability: None,
                runtime_name: None,
                placement: None,
                minimum_platform: None,
                dependencies: vec!["runtime".to_string()],
            },
            NativeArtifact {
                name: "runtime".to_string(),
                kind: NativeArtifactKind::Static,
                path: Some("native/lib/libruntime.a".to_string()),
                capability: None,
                runtime_name: None,
                placement: None,
                minimum_platform: None,
                dependencies: vec!["model".to_string()],
            },
        ];
        let native = NativeSection {
            schema: NATIVE_MANIFEST_SCHEMA_VERSION,
            targets: vec![target],
        };

        assert!(
            native
                .validate()
                .is_err_and(|error| error.contains("dependency graph contains a cycle"))
        );
    }

    #[test]
    fn mobile_platform_profiles_require_matching_target_sdk_and_version_facts() {
        let android = NativeSection {
            schema: NATIVE_MANIFEST_SCHEMA_VERSION,
            targets: vec![mobile_target(
                "aarch64-linux-android",
                Some("android-36"),
                NativeTargetPlatform::Android { api_level: 34 },
            )],
        };
        assert!(android.validate().is_ok());

        let apple = NativeSection {
            schema: NATIVE_MANIFEST_SCHEMA_VERSION,
            targets: vec![mobile_target(
                "aarch64-apple-ios",
                Some("iphoneos-26.5"),
                NativeTargetPlatform::Ios {
                    deployment_target: "13.0".to_string(),
                },
            )],
        };
        assert!(apple.validate().is_ok());

        let mut unsupported_android_api = android.clone();
        unsupported_android_api.targets[0].platform = Some(NativeTargetPlatform::Android { api_level: 20 });
        assert!(
            unsupported_android_api
                .validate()
                .is_err_and(|error| error.contains("API level 21 or later"))
        );

        let mut wrong_android_sdk = android.clone();
        wrong_android_sdk.targets[0].sdk = Some("iphoneos-26.5".to_string());
        assert!(
            wrong_android_sdk
                .validate()
                .is_err_and(|error| error.contains("SDK identity beginning with `android-`"))
        );

        let mut incompatible_apple_target = apple.clone();
        incompatible_apple_target.targets[0].target = "aarch64-apple-darwin".to_string();
        assert!(
            incompatible_apple_target
                .validate()
                .is_err_and(|error| error.contains("aarch64-apple-ios"))
        );

        let mut malformed_apple_version = apple;
        malformed_apple_version.targets[0].platform = Some(NativeTargetPlatform::Ios {
            deployment_target: "iOS 13".to_string(),
        });
        assert!(
            malformed_apple_version
                .validate()
                .is_err_and(|error| error.contains("numeric `major.minor` version"))
        );
    }

    #[test]
    fn android_platform_profile_parses_and_locks_its_ndk_api_level() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = TempDir::new()?;
        let manifest = ProjectManifest::from_str(
            r#"
[native]
schema = 1

[[native.targets]]
target = "aarch64-linux-android"
toolchain = "android-ndk-r29"
sdk = "android-36"

[native.targets.platform]
kind = "android"
api-level = 34
"#,
            &workspace.path().join("incan.toml"),
        )?;
        let locked = locked_native_targets(&manifest)?;
        assert_eq!(
            locked[0].platform,
            Some(NativeTargetPlatform::Android { api_level: 34 })
        );
        Ok(())
    }

    #[test]
    fn native_inputs_reject_ambient_paths_and_incomplete_bundles() {
        let absolute = NativeSection {
            schema: NATIVE_MANIFEST_SCHEMA_VERSION,
            targets: vec![NativeTarget {
                target: "x86_64-unknown-linux-gnu".to_string(),
                toolchain: "clang-18".to_string(),
                sdk: None,
                platform: None,
                headers: vec!["/usr/include/fixture.h".to_string()],
                definitions: Vec::new(),
                provenance: None,
                artifacts: Vec::new(),
                shims: Vec::new(),
            }],
        };
        assert!(absolute.validate().is_err());

        let bundled = NativeSection {
            schema: NATIVE_MANIFEST_SCHEMA_VERSION,
            targets: vec![NativeTarget {
                target: "x86_64-apple-darwin".to_string(),
                toolchain: "apple-clang-17".to_string(),
                sdk: Some("macosx-15.0".to_string()),
                platform: None,
                headers: Vec::new(),
                definitions: Vec::new(),
                provenance: None,
                artifacts: vec![NativeArtifact {
                    name: "fixture".to_string(),
                    kind: NativeArtifactKind::Bundled,
                    path: Some("native/lib/libfixture.dylib".to_string()),
                    capability: None,
                    runtime_name: None,
                    placement: None,
                    minimum_platform: None,
                    dependencies: Vec::new(),
                }],
                shims: Vec::new(),
            }],
        };
        assert!(bundled.validate().is_err());

        let invalid_dependency = NativeSection {
            schema: NATIVE_MANIFEST_SCHEMA_VERSION,
            targets: vec![NativeTarget {
                target: "x86_64-unknown-linux-gnu".to_string(),
                toolchain: "clang-18".to_string(),
                sdk: None,
                platform: None,
                headers: Vec::new(),
                definitions: Vec::new(),
                provenance: None,
                artifacts: vec![NativeArtifact {
                    name: "fixture".to_string(),
                    kind: NativeArtifactKind::Static,
                    path: Some("native/lib/libfixture.a".to_string()),
                    capability: None,
                    runtime_name: None,
                    placement: None,
                    minimum_platform: None,
                    dependencies: vec!["missing".to_string()],
                }],
                shims: Vec::new(),
            }],
        };
        assert!(invalid_dependency.validate().is_err());
    }
}
