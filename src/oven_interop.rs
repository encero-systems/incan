//! Authored interop requirements and their portable lock projection.
//!
//! An interop declaration names package-owned inputs and compatibility requirements for one target. It does not claim
//! that a compiler or SDK has already been selected, perform ambient discovery, compile a shim, or decide application
//! semantics. Oven resolves those requirements and records its selections in a separate build receipt.
//!
//! RFC 117 puts these declarations at the Loaf level under `[interop]`, with each binding kind naming itself: C
//! declarations are `[interop.c]`. The envelope is deliberately binding-kind neutral, so a future kind adds a sibling
//! table rather than widening this one into a catch-all native namespace.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path};

use semver::VersionReq;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::manifest::ProjectManifest;

/// Current compatibility format for the `[interop.c]` manifest section.
pub const INTEROP_C_SCHEMA_VERSION: u32 = 1;

/// Current compatibility format for the locked Oven interop deployment-plan projection.
pub(crate) const OVEN_INTEROP_DEPLOYMENT_PLAN_SCHEMA_VERSION: u32 = 3;

/// The `[interop]` manifest root, holding one table per declared binding kind.
///
/// Only C is declared today. A binding kind gets its own field rather than sharing one, because each kind's RFC owns
/// its own source vocabulary and checking rules; merging them would make one kind's schema silently govern another.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct InteropSection {
    /// `[interop.c]` — target-specific checked C interop requirements.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c: Option<InteropCSection>,
}

/// Target-specific package inputs and compatibility requirements for checked C interop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct InteropCSection {
    /// Version of the C interop requirement schema.
    pub schema: u32,
    /// Independently declared requirements for each target triple.
    #[serde(default)]
    pub targets: Vec<InteropCTarget>,
}

impl InteropCSection {
    /// Validate configuration that is independent from the package filesystem.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != INTEROP_C_SCHEMA_VERSION {
            return Err(format!(
                "[interop.c].schema must be {INTEROP_C_SCHEMA_VERSION}, found {}",
                self.schema
            ));
        }
        if self.targets.is_empty() {
            return Err("[interop.c] requires at least one [[interop.c.targets]] entry".to_string());
        }

        let mut target_names = BTreeSet::new();
        for target in &self.targets {
            if target.target.trim().is_empty() || !target_names.insert(target.target.clone()) {
                return Err(format!(
                    "Oven interop target `{}` must be a unique non-empty target triple",
                    target.target
                ));
            }
            if let Some(toolchain) = &target.toolchain {
                toolchain.validate(&format!("Oven interop target `{}` toolchain", target.target))?;
            }
            if let Some(sdk) = &target.sdk {
                sdk.validate(&format!("Oven interop target `{}` SDK", target.target))?;
            }
            validate_interop_paths(
                &target.headers,
                &format!("Oven interop target `{}` header", target.target),
            )?;
            if target.definitions.iter().any(|definition| definition.trim().is_empty()) {
                return Err(format!(
                    "Oven interop target `{}` contains an empty preprocessor definition",
                    target.target
                ));
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
            let mut binding_names = BTreeSet::new();
            for binding in &target.bindings {
                validate_binding_artifacts(binding, &target.target, &artifact_names, &mut binding_names)?;
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
                        "Oven interop target `{}` shim names must be unique and non-empty",
                        target.target
                    ));
                }
                if shim.sources.is_empty() {
                    return Err(format!(
                        "interop shim `{}` requires at least one source input",
                        shim.name
                    ));
                }
                validate_interop_paths(&shim.sources, &format!("interop shim `{}` source", shim.name))?;
                validate_interop_paths(&shim.headers, &format!("interop shim `{}` header", shim.name))?;
                if shim.output.trim().is_empty() {
                    return Err(format!("interop shim `{}` requires a logical output name", shim.name));
                }
                if !is_interop_native_library_name(&shim.output) {
                    return Err(format!(
                        "interop shim `{}` output `{}` must be an ASCII native library name",
                        shim.name, shim.output
                    ));
                }
            }
            if !target.shims.is_empty() && target.toolchain.is_none() {
                return Err(format!(
                    "Oven interop target `{}` declares native shims but no toolchain capability",
                    target.target
                ));
            }
        }
        Ok(())
    }
}

/// Return whether one package-declared shim output safely maps to `lib<name>.a` below an Oven-owned directory.
pub(crate) fn is_interop_native_library_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// A compiler or SDK capability accepted by one package target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ToolchainRequirement {
    /// Stable capability identity resolved by Oven.
    pub capability: String,
    /// Optional compatible version requirement, not a claim about the locally selected version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl ToolchainRequirement {
    /// Reject an empty capability or a version expression outside the supported semantic-version requirement grammar.
    fn validate(&self, label: &str) -> Result<(), String> {
        if self.capability.trim().is_empty() {
            return Err(format!("{label} capability must be non-empty"));
        }
        if self.version.as_ref().is_some_and(|version| version.trim().is_empty()) {
            return Err(format!("{label} version requirement must be non-empty"));
        }
        if let Some(version) = &self.version {
            VersionReq::parse(version).map_err(|error| format!("{label} version requirement is invalid: {error}"))?;
        }
        Ok(())
    }

    /// Canonicalize a validated capability and version requirement before recording it in the semantic lock.
    fn normalized(&self) -> Result<Self, String> {
        let version = self
            .version
            .as_deref()
            .map(VersionReq::parse)
            .transpose()
            .map_err(|error| format!("invalid Oven capability version requirement: {error}"))?
            .map(|requirement| requirement.to_string());
        Ok(Self {
            capability: self.capability.trim().to_string(),
            version,
        })
    }
}

/// Package inputs and compatibility requirements declared for one target triple.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct InteropCTarget {
    /// Compilation and deployment target triple requested by the package.
    pub target: String,
    /// Optional compatible Clang-family capability; Oven records the selected executable separately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<ToolchainRequirement>,
    /// Optional compatible SDK capability; Oven records the selected SDK and sysroot separately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk: Option<ToolchainRequirement>,
    /// Platform facts that complete a mobile target's ABI and deployment requirements.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<InteropTargetPlatform>,
    /// Package-relative public or shim headers used for verification.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<String>,
    /// Explicit preprocessor definitions supplied to verification and later shim baking.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub definitions: Vec<String>,
    /// Package-owned artifacts or system capabilities required by this target.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<InteropArtifact>,
    /// Explicit checked-binding to target-artifact correspondences for this target.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<InteropBindingArtifact>,
    /// Authored C or C++ shim source inputs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shims: Vec<InteropShim>,
}

/// Platform-specific constraints that complete a mobile target identity.
///
/// The target triple supplies the CPU and operating-system identity. This profile carries the version fact required
/// by the Android or Apple toolchain and by a later platform handoff without selecting a machine-local SDK.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", rename_all_fields = "kebab-case")]
pub enum InteropTargetPlatform {
    /// Android arm64 verification and deployment require an Android API level.
    Android {
        /// Android API level selected for C ABI verification and deployment compatibility.
        api_level: u32,
    },
    /// iOS arm64 verification and deployment require a minimum supported OS version.
    Ios {
        /// Minimum iOS version selected for verification and deployment compatibility.
        deployment_target: String,
    },
}

/// Canonical device-versus-simulator interpretation for the supported iOS target vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IosTargetKind {
    /// Arm64 binary linked against the physical-device iPhoneOS SDK.
    Device,
    /// Arm64 binary linked against the iPhoneSimulator SDK.
    Simulator,
}

impl IosTargetKind {
    /// Return the one SDK capability that can satisfy this target kind.
    pub(crate) fn sdk_capability(self) -> &'static str {
        match self {
            Self::Device => "iphoneos",
            Self::Simulator => "iphonesimulator",
        }
    }

    /// Return the target portion used by a selected Clang-family compiler.
    pub(crate) fn clang_target(self) -> &'static str {
        match self {
            Self::Device => "arm64-apple-ios",
            Self::Simulator => "arm64-apple-ios-simulator",
        }
    }
}

/// Classify the supported Rust target spellings without consulting a host SDK or toolchain.
#[must_use]
pub(crate) fn ios_target_kind(target: &str) -> Option<IosTargetKind> {
    match target {
        "aarch64-apple-ios" => Some(IosTargetKind::Device),
        "aarch64-apple-ios-sim" => Some(IosTargetKind::Simulator),
        _ => None,
    }
}

/// Validate the target triple, platform version, and compatible SDK capability required by one mobile profile.
fn validate_target_platform(platform: &InteropTargetPlatform, target: &InteropCTarget) -> Result<(), String> {
    match platform {
        InteropTargetPlatform::Android { api_level } => {
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
            validate_sdk_capability(target.sdk.as_ref(), "android", "Android", &target.target)
        }
        InteropTargetPlatform::Ios { deployment_target } => {
            let Some(kind) = ios_target_kind(&target.target) else {
                return Err(format!(
                    "iOS platform facts require `aarch64-apple-ios` or `aarch64-apple-ios-sim`, found `{}`",
                    target.target
                ));
            };
            if !is_deployment_target_version(deployment_target) {
                return Err(format!(
                    "iOS deployment target `{deployment_target}` for `{}` must be a numeric `major.minor` version",
                    target.target
                ));
            }
            validate_sdk_capability(target.sdk.as_ref(), kind.sdk_capability(), "iOS", &target.target)
        }
    }
}

/// Require a mobile profile to name the SDK capability whose concrete installation Oven will later select.
fn validate_sdk_capability(
    sdk: Option<&ToolchainRequirement>,
    expected: &str,
    platform: &str,
    target: &str,
) -> Result<(), String> {
    let Some(sdk) = sdk else {
        return Err(format!(
            "{platform} target `{target}` requires the `{expected}` SDK capability"
        ));
    };
    if sdk.capability == expected {
        Ok(())
    } else {
        Err(format!(
            "{platform} target `{target}` requires the `{expected}` SDK capability, found `{}`",
            sdk.capability
        ))
    }
}

/// Return whether a declared iOS deployment target has an explicit numeric major and minor version.
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

/// One declared physical interop artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct InteropArtifact {
    /// Binding-local logical name for this artifact.
    pub name: String,
    /// How the requested target consumes this artifact.
    pub kind: InteropArtifactKind,
    /// Package-relative file for static and bundled artifacts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Optional immutable upstream identity for a third-party package-provided artifact.
    ///
    /// The locked package-file digest remains the integrity authority for the bytes consumed by Oven. This record
    /// supplies the separately declared human/audit provenance for those bytes; it never causes a download or an
    /// ambient lookup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<InteropArtifactOrigin>,
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
    /// Logical names of transitive interop artifact dependencies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
}

/// Declared upstream provenance for one package-provided native artifact.
///
/// A source URL, immutable revision, and license are recorded next to the package-relative artifact digest so an
/// Oven receipt can explain where a third-party binary came from without exposing a local path or fetching it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct InteropArtifactOrigin {
    /// Canonical HTTPS URL for the upstream release, source archive, or repository.
    pub source: String,
    /// Upstream immutable revision, release tag, or artifact version selected by the package.
    pub revision: String,
    /// SPDX expression or upstream license identifier supplied by the package author.
    pub license: String,
}

impl InteropArtifactOrigin {
    /// Validate a portable external origin without performing an ambient network request.
    fn validate(&self, label: &str) -> Result<(), String> {
        if !self.source.starts_with("https://") || self.source.chars().any(char::is_whitespace) {
            return Err(format!("{label} origin source must be one whitespace-free HTTPS URL"));
        }
        if self.revision.trim().is_empty() || self.revision.chars().any(char::is_control) {
            return Err(format!("{label} origin revision must be non-empty"));
        }
        if self.license.trim().is_empty() || self.license.chars().any(char::is_control) {
            return Err(format!("{label} origin license must be non-empty"));
        }
        Ok(())
    }

    /// Normalize presentation-only surrounding whitespace before the value participates in a lock identity.
    fn normalized(&self) -> Self {
        Self {
            source: self.source.trim().to_string(),
            revision: self.revision.trim().to_string(),
            license: self.license.trim().to_string(),
        }
    }
}

/// One package-authored correspondence between a checked C binding and declared target artifacts.
///
/// The declaration never derives a relation from header spelling, library name, generated Rust, or artifact path.
/// The selected compiler analysis verifies that this logical module/name pair exists before a binding-use receipt
/// reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct InteropBindingArtifact {
    /// Logical Incan module path that declares the checked binding.
    pub module: Vec<String>,
    /// Source-visible checked binding declaration name in that module.
    pub name: String,
    /// Target-artifact names explicitly required by this binding.
    pub artifacts: Vec<String>,
}

/// Deployment class for a declared interop artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteropArtifactKind {
    /// An archive linked directly into the generated product.
    Static,
    /// A dynamic library or framework sealed for receipt-bound execution and platform packaging.
    Bundled,
    /// A library or framework supplied by the resolved SDK or toolchain capability.
    System,
}

/// One authored shim whose bounded exported surface will be verified and built by Oven.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct InteropShim {
    /// Stable package-local shim name.
    pub name: String,
    /// Source language accepted by the future managed shim baker.
    pub language: InteropShimLanguage,
    /// Package-relative authored C or C++ source files.
    pub sources: Vec<String>,
    /// Package-relative headers that describe the shim's bounded exported contract.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<String>,
    /// Logical name for the generated interop output.
    pub output: String,
}

/// Language of one authored interop shim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteropShimLanguage {
    /// C source compiled by the selected managed Clang-compatible toolchain.
    C,
    /// C++ source compiled only behind a bounded C-compatible shim surface.
    Cxx,
}

impl InteropShimLanguage {
    /// Return the stable vocabulary spelling used by inspect output.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::C => "c",
            Self::Cxx => "cxx",
        }
    }
}

/// Portable Oven interop requirements frozen in one canonical semantic lock state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedInteropTarget {
    /// Target triple requested by the package plan.
    pub target: String,
    /// Compatible toolchain capability requested by the package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<ToolchainRequirement>,
    /// Compatible SDK capability requested by the package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk: Option<ToolchainRequirement>,
    /// Target-platform facts retained for target-specific verification and deployment planning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<InteropTargetPlatform>,
    /// Explicit definitions sorted as part of the target configuration identity.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub definitions: Vec<String>,
    /// Header inputs and their content identities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<LockedInteropInput>,
    /// Declared static, bundled, or system artifact identities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<LockedInteropArtifact>,
    /// Explicit checked-binding to target-artifact correspondences.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<LockedInteropBindingArtifact>,
    /// Authored shim source and header identities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shims: Vec<LockedInteropShim>,
}

/// One package-relative immutable interop input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedInteropInput {
    /// Package-relative portable input path.
    pub path: String,
    /// Content digest computed from the exact declared file bytes.
    pub digest: String,
}

/// One declared artifact projected into portable lock data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedInteropArtifact {
    /// Binding-local artifact name.
    pub name: String,
    /// Static, bundled, or system deployment class.
    pub kind: InteropArtifactKind,
    /// File input identity when the package provides artifact bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<LockedInteropInput>,
    /// Declared upstream origin retained beside an immutable package-file input digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<InteropArtifactOrigin>,
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
    /// Transitive interop dependencies in deterministic order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
}

/// One target-locked checked-binding correspondence with deterministic artifact ordering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedInteropBindingArtifact {
    /// Logical Incan module path that declares the checked binding.
    pub module: Vec<String>,
    /// Source-visible checked binding declaration name in that module.
    pub name: String,
    /// Target-artifact names explicitly selected for this binding.
    pub artifacts: Vec<String>,
}

/// One authored shim's immutable sources and intended logical output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedInteropShim {
    /// Stable package-local shim name.
    pub name: String,
    /// Selected C or C++ source language.
    pub language: InteropShimLanguage,
    /// Authored source file identities.
    pub sources: Vec<LockedInteropInput>,
    /// Shim-header identities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<LockedInteropInput>,
    /// Logical output name selected for later shim baking.
    pub output: String,
}

/// One portable Oven interop deployment projection emitted from canonical locked package requirements.
///
/// This is the directionally useful platform-packager boundary a future Loaf may carry. It records the package's
/// locked requirements without embedding a Gradle task, Xcode build phase, signing identity, credential, or a
/// machine-local toolchain path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct InteropDeploymentPlan {
    /// Compatibility version for this deployment-plan shape.
    pub(crate) schema_version: u32,
    /// Content-derived identity of the exact locked target requirements behind this handoff.
    pub(crate) locked_target_identity: String,
    /// Exact compilation and deployment target triple.
    pub(crate) target: String,
    /// Compatible toolchain requirement retained from the canonical lock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) toolchain: Option<ToolchainRequirement>,
    /// Compatible SDK requirement retained from the canonical lock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sdk: Option<ToolchainRequirement>,
    /// Platform version facts needed by a later Gradle or Xcode adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) platform: Option<InteropDeploymentPlatform>,
    /// Locked header files used by verification and future interop compilation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) headers: Vec<LockedInteropInput>,
    /// Portable package-relative include roots derived from locked headers and shim headers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) include_roots: Vec<String>,
    /// Explicit preprocessor definitions applied to verification and future shim baking.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) definitions: Vec<String>,
    /// Deterministic dependencies-first static, bundled, and system planning actions.
    ///
    /// Explicit dependency edges remain authoritative. A platform adapter must derive linker argument order rather
    /// than treating this planning sequence as a raw command line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) artifacts: Vec<InteropDeploymentArtifact>,
    /// Explicit checked-binding to target-artifact correspondences retained for tooling joins.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) bindings: Vec<InteropDeploymentBindingArtifact>,
    /// Authored shim build inputs and logical outputs required before final platform assembly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) shims: Vec<InteropShimBuildPlan>,
}

/// One dependency-ordered artifact action in an Oven interop deployment plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct InteropDeploymentArtifact {
    /// Stable package-local artifact name.
    pub(crate) name: String,
    /// Logical sibling artifacts that must be available before this artifact.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) dependencies: Vec<String>,
    /// Structured platform-neutral action for the declared deployment class.
    #[serde(flatten)]
    pub(crate) action: InteropDeploymentAction,
}

/// One portable checked-binding to target-artifact correspondence in the deployment handoff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct InteropDeploymentBindingArtifact {
    /// Logical Incan module path that declares the checked binding.
    pub(crate) module: Vec<String>,
    /// Source-visible checked binding declaration name in that module.
    pub(crate) name: String,
    /// Explicit target-artifact names required by this binding.
    pub(crate) artifacts: Vec<String>,
}

/// Mobile platform facts projected into the JSON handoff independently from manifest field spelling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum InteropDeploymentPlatform {
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

impl From<&InteropTargetPlatform> for InteropDeploymentPlatform {
    fn from(platform: &InteropTargetPlatform) -> Self {
        match platform {
            InteropTargetPlatform::Android { api_level } => Self::Android { api_level: *api_level },
            InteropTargetPlatform::Ios { deployment_target } => Self::Ios {
                deployment_target: deployment_target.clone(),
            },
        }
    }
}

/// Platform-neutral action consumed by a later Gradle, Xcode, or other packager adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "deployment", rename_all = "snake_case")]
pub(crate) enum InteropDeploymentAction {
    /// Link one locked package archive into the final product.
    StaticLink {
        /// Portable archive path and digest.
        input: LockedInteropInput,
    },
    /// Stage one locked dynamic library or framework for a platform packager.
    Bundle {
        /// Portable dynamic artifact path and digest.
        input: LockedInteropInput,
        /// Runtime loader name expected by the native dependency graph.
        runtime_name: String,
        /// Logical packager placement retained without embedding an absolute output path.
        placement: String,
        /// Minimum platform version required by this artifact.
        minimum_platform: String,
    },
    /// Request one explicit library or framework capability from the resolved toolchain or SDK.
    System {
        /// Stable capability identity such as `apple.framework.Accelerate`.
        capability: String,
    },
}

/// One authored shim action that Oven must bake before its deployment plan is ready for final assembly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct InteropShimBuildPlan {
    /// Stable package-local shim name.
    pub(crate) name: String,
    /// Selected C or C++ source language.
    pub(crate) language: InteropShimLanguage,
    /// Locked authored source inputs.
    pub(crate) sources: Vec<LockedInteropInput>,
    /// Locked headers describing the shim's bounded C contract.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) headers: Vec<LockedInteropInput>,
    /// Logical artifact name produced by the future managed shim baker.
    pub(crate) output: String,
}

/// Project one canonical locked target into a deterministic Oven interop deployment handoff.
///
/// The projection neither reads the package filesystem nor discovers host libraries. Every physical file comes from
/// an existing locked input receipt, so relocating the package does not change the emitted plan.
pub(crate) fn interop_deployment_plan(target: &LockedInteropTarget) -> Result<InteropDeploymentPlan, String> {
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
                    "Oven interop deployment plan for `{}` lost artifact `{name}` while ordering dependencies",
                    target.target
                )
            })?;
            deployment_artifact(artifact, &target.target)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let bindings = target
        .bindings
        .iter()
        .map(|binding| InteropDeploymentBindingArtifact {
            module: binding.module.clone(),
            name: binding.name.clone(),
            artifacts: binding.artifacts.clone(),
        })
        .collect();

    // ---- Project authored shim inputs ----
    let shims = target
        .shims
        .iter()
        .map(|shim| InteropShimBuildPlan {
            name: shim.name.clone(),
            language: shim.language,
            sources: shim.sources.clone(),
            headers: shim.headers.clone(),
            output: shim.output.clone(),
        })
        .collect();

    Ok(InteropDeploymentPlan {
        schema_version: OVEN_INTEROP_DEPLOYMENT_PLAN_SCHEMA_VERSION,
        locked_target_identity: locked_interop_target_identity(target)?,
        target: target.target.clone(),
        toolchain: target.toolchain.clone(),
        sdk: target.sdk.clone(),
        platform: target.platform.as_ref().map(InteropDeploymentPlatform::from),
        headers: target.headers.clone(),
        include_roots: interop_include_roots(target),
        definitions: target.definitions.clone(),
        artifacts,
        bindings,
        shims,
    })
}

/// Return the portable content identity shared by target handoff and checked binding-use receipts.
///
/// The identity is derived solely from canonical lock data. It neither inspects a local toolchain nor serializes a
/// package root, so a relocated package with the same locked interop inputs keeps the same join key.
pub(crate) fn locked_interop_target_identity(target: &LockedInteropTarget) -> Result<String, String> {
    serde_json::to_vec(target)
        .map(|bytes| format!("sha256:{}", hex::encode(Sha256::digest(bytes))))
        .map_err(|error| format!("failed to serialize locked Oven interop target identity: {error}"))
}

/// Convert one locked artifact into the exact action required by its declared deployment class.
fn deployment_artifact(artifact: &LockedInteropArtifact, target: &str) -> Result<InteropDeploymentArtifact, String> {
    // ---- Select the structured deployment action ----
    let action = match artifact.kind {
        InteropArtifactKind::Static => InteropDeploymentAction::StaticLink {
            input: required_locked_artifact_input(artifact, target)?,
        },
        InteropArtifactKind::Bundled => InteropDeploymentAction::Bundle {
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
        InteropArtifactKind::System => InteropDeploymentAction::System {
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
    Ok(InteropDeploymentArtifact {
        name: artifact.name.clone(),
        dependencies,
        action,
    })
}

/// Require one package-file receipt for a static or bundled artifact.
fn required_locked_artifact_input(
    artifact: &LockedInteropArtifact,
    target: &str,
) -> Result<LockedInteropInput, String> {
    artifact.input.clone().ok_or_else(|| {
        format!(
            "locked Oven interop artifact `{}` on target `{target}` is missing its package-file receipt",
            artifact.name
        )
    })
}

/// Require one non-empty deployment field from a canonical locked artifact.
fn required_locked_artifact_field(
    value: Option<&str>,
    artifact: &LockedInteropArtifact,
    target: &str,
    field: &str,
) -> Result<String, String> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            format!(
                "locked Oven interop artifact `{}` on target `{target}` is missing its {field}",
                artifact.name
            )
        })
}

/// Derive deterministic package-relative include roots without leaking the package's current absolute location.
fn interop_include_roots(target: &LockedInteropTarget) -> Vec<String> {
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

/// Resolve declared Oven interop files into lockable content identities without ambient host discovery.
pub fn locked_oven_interop_targets(manifest: &ProjectManifest) -> Result<Vec<LockedInteropTarget>, String> {
    locked_interop_targets_from_section(manifest.project_root(), manifest.interop_c())
}

/// Resolve a parsed Oven interop declaration for the specified project root into portable lock entries.
///
/// This accepts the already-parsed manifest section so lock generation can include interop inputs alongside the
/// provider and SDK semantic state without rediscovering or reparsing the manifest.
pub fn locked_interop_targets_from_section(
    project_root: &Path,
    interop: Option<&InteropCSection>,
) -> Result<Vec<LockedInteropTarget>, String> {
    let Some(interop) = interop else {
        return Ok(Vec::new());
    };
    interop.validate()?;
    let mut targets = interop
        .targets
        .iter()
        .map(|target| lock_interop_target(project_root, target))
        .collect::<Result<Vec<_>, _>>()?;
    targets.sort_by(|left, right| left.target.cmp(&right.target));
    Ok(targets)
}

/// Validate one target artifact's deployment kind and the fields it is allowed to declare.
///
/// The manifest uses mutually exclusive shapes for directly linked static archives, packager-staged bundled files,
/// and required toolchain/SDK capabilities. Enforcing that distinction before any file access keeps later target
/// resolution from interpreting an ambiguous declaration as ambient host discovery.
fn validate_artifact(
    artifact: &InteropArtifact,
    target: &str,
    artifact_names: &mut BTreeSet<String>,
) -> Result<(), String> {
    if artifact.name.trim().is_empty() || !artifact_names.insert(artifact.name.clone()) {
        return Err(format!(
            "Oven interop target `{target}` artifact names must be unique and non-empty"
        ));
    }
    if let Some(origin) = &artifact.origin {
        origin.validate(&format!("interop artifact `{}`", artifact.name))?;
    }
    match artifact.kind {
        InteropArtifactKind::Static => {
            if !is_interop_native_library_name(&artifact.name) {
                return Err(format!(
                    "static interop artifact `{}` on target `{target}` must use an ASCII native library name",
                    artifact.name
                ));
            }
            validate_required_path(
                artifact.path.as_deref(),
                &format!("static interop artifact `{}`", artifact.name),
            )?;
            if artifact.capability.is_some() {
                return Err(format!(
                    "static interop artifact `{}` cannot declare a system capability",
                    artifact.name
                ));
            }
            if artifact.runtime_name.is_some() || artifact.placement.is_some() || artifact.minimum_platform.is_some() {
                return Err(format!(
                    "static interop artifact `{}` cannot declare bundled deployment fields",
                    artifact.name
                ));
            }
        }
        InteropArtifactKind::Bundled => {
            validate_required_path(
                artifact.path.as_deref(),
                &format!("bundled interop artifact `{}`", artifact.name),
            )?;
            validate_non_empty(
                artifact.runtime_name.as_deref(),
                &format!("bundled interop artifact `{}` runtime-name", artifact.name),
            )?;
            validate_non_empty(
                artifact.placement.as_deref(),
                &format!("bundled interop artifact `{}` placement", artifact.name),
            )?;
            validate_non_empty(
                artifact.minimum_platform.as_deref(),
                &format!("bundled interop artifact `{}` minimum-platform", artifact.name),
            )?;
            if artifact.capability.is_some() {
                return Err(format!(
                    "bundled interop artifact `{}` cannot declare a system capability",
                    artifact.name
                ));
            }
        }
        InteropArtifactKind::System => {
            validate_non_empty(
                artifact.capability.as_deref(),
                &format!("system interop artifact `{}` capability", artifact.name),
            )?;
            if artifact.path.is_some() {
                return Err(format!(
                    "system interop artifact `{}` cannot declare a package path",
                    artifact.name
                ));
            }
            if artifact.origin.is_some() {
                return Err(format!(
                    "system interop artifact `{}` cannot declare a package-artifact origin",
                    artifact.name
                ));
            }
            if artifact.runtime_name.is_some() || artifact.placement.is_some() || artifact.minimum_platform.is_some() {
                return Err(format!(
                    "system interop artifact `{}` cannot declare bundled deployment fields",
                    artifact.name
                ));
            }
        }
    }
    Ok(())
}

/// Require each declared artifact dependency to name one distinct sibling in the same target declaration.
///
/// Interop artifact order is normalized in the lock projection, so dependencies must use stable logical names rather
/// than filesystem paths or an implicit declaration order.
fn validate_artifact_dependencies(
    artifact: &InteropArtifact,
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
                "interop artifact `{}` on target `{target}` must depend on distinct declared sibling artifacts",
                artifact.name
            ));
        }
    }
    Ok(())
}

/// Validate that one checked-binding correspondence names a unique logical binding and declared target artifacts.
fn validate_binding_artifacts(
    binding: &InteropBindingArtifact,
    target: &str,
    artifact_names: &BTreeSet<String>,
    binding_names: &mut BTreeSet<(Vec<String>, String)>,
) -> Result<(), String> {
    if binding.module.is_empty()
        || binding.module.iter().any(|segment| segment.trim().is_empty())
        || binding.name.trim().is_empty()
        || !binding_names.insert((binding.module.clone(), binding.name.clone()))
    {
        return Err(format!(
            "interop binding correspondences on target `{target}` require one unique non-empty module path and binding name"
        ));
    }
    if binding.artifacts.is_empty() {
        return Err(format!(
            "interop binding `{}::{}` on target `{target}` requires at least one declared artifact",
            binding.module.join("::"),
            binding.name
        ));
    }
    let mut names = BTreeSet::new();
    for artifact in &binding.artifacts {
        if artifact.trim().is_empty() || !artifact_names.contains(artifact) || !names.insert(artifact.clone()) {
            return Err(format!(
                "interop binding `{}::{}` on target `{target}` must reference distinct declared artifacts",
                binding.module.join("::"),
                binding.name
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
            "Oven interop target `{target}` artifact names must be unique and non-empty"
        ));
    }

    let mut dependency_counts = BTreeMap::new();
    let mut dependents = BTreeMap::<String, BTreeSet<String>>::new();
    for (name, dependencies) in artifacts {
        if name.trim().is_empty() {
            return Err(format!(
                "Oven interop target `{target}` artifact names must be unique and non-empty"
            ));
        }
        let unique_dependencies = dependencies.iter().cloned().collect::<BTreeSet<_>>();
        if unique_dependencies.len() != dependencies.len()
            || unique_dependencies
                .iter()
                .any(|dependency| dependency == &name || !names.contains(dependency))
        {
            return Err(format!(
                "interop artifact `{name}` on target `{target}` must depend on distinct declared sibling artifacts"
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
                    format!(
                        "Oven interop deployment plan for target `{target}` lost dependency state for `{dependent}`"
                    )
                })?;
                *count = count.checked_sub(1).ok_or_else(|| {
                    format!(
                        "Oven interop deployment plan for target `{target}` counted dependency `{name}` more than once"
                    )
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
            "Oven interop target `{target}` artifact dependency graph contains a cycle involving: {cycle}"
        ));
    }
    Ok(ordered)
}

/// Require one optional interop path field and validate it with the common package-relative path policy.
fn validate_required_path(path: Option<&str>, label: &str) -> Result<(), String> {
    let Some(path) = path else {
        return Err(format!("{label} requires a package-relative path"));
    };
    validate_interop_path(path, label)
}

/// Require one optional manifest string field to contain a non-whitespace value.
fn validate_non_empty(value: Option<&str>, label: &str) -> Result<(), String> {
    if value.is_none_or(|value| value.trim().is_empty()) {
        return Err(format!("{label} must be non-empty"));
    }
    Ok(())
}

/// Validate every package path in one manifest list using the shared normalized-relative-path contract.
fn validate_interop_paths(paths: &[String], label: &str) -> Result<(), String> {
    for path in paths {
        validate_interop_path(path, label)?;
    }
    Ok(())
}

/// Reject a path that could escape the package or acquire platform-dependent meaning outside the declaration.
///
/// Interop inputs are locked by their package-relative identity and content bytes. Absolute, parent-relative, current
/// directory, backslash, duplicate-separator, and directory spellings would make that identity ambiguous or permit
/// ambient filesystem lookup.
fn validate_interop_path(path: &str, label: &str) -> Result<(), String> {
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
fn lock_interop_target(root: &Path, target: &InteropCTarget) -> Result<LockedInteropTarget, String> {
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
    let mut bindings = target
        .bindings
        .iter()
        .map(|binding| {
            let mut artifacts = binding.artifacts.clone();
            artifacts.sort();
            artifacts.dedup();
            LockedInteropBindingArtifact {
                module: binding.module.clone(),
                name: binding.name.clone(),
                artifacts,
            }
        })
        .collect::<Vec<_>>();
    bindings.sort_by(|left, right| left.module.cmp(&right.module).then_with(|| left.name.cmp(&right.name)));
    let mut shims = target
        .shims
        .iter()
        .map(|shim| {
            Ok(LockedInteropShim {
                name: shim.name.clone(),
                language: shim.language,
                sources: lock_inputs(root, &shim.sources)?,
                headers: lock_inputs(root, &shim.headers)?,
                output: shim.output.clone(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    shims.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(LockedInteropTarget {
        target: target.target.clone(),
        toolchain: target
            .toolchain
            .as_ref()
            .map(ToolchainRequirement::normalized)
            .transpose()?,
        sdk: target.sdk.as_ref().map(ToolchainRequirement::normalized).transpose()?,
        platform: target.platform.clone(),
        definitions,
        headers,
        artifacts,
        bindings,
        shims,
    })
}

/// Lock one physical artifact while retaining capability-only system artifacts without a package file receipt.
fn lock_artifact(root: &Path, artifact: &InteropArtifact) -> Result<LockedInteropArtifact, String> {
    let input = artifact
        .path
        .as_deref()
        .map(|path| lock_input(root, path))
        .transpose()?;
    let mut dependencies = artifact.dependencies.clone();
    dependencies.sort();
    dependencies.dedup();
    let origin = if let Some(origin) = &artifact.origin {
        origin.validate(&format!("interop artifact `{}`", artifact.name))?;
        Some(origin.normalized())
    } else {
        None
    };
    Ok(LockedInteropArtifact {
        name: artifact.name.clone(),
        kind: artifact.kind,
        input,
        origin,
        capability: artifact.capability.clone(),
        runtime_name: artifact.runtime_name.clone(),
        placement: artifact.placement.clone(),
        minimum_platform: artifact.minimum_platform.clone(),
        dependencies,
    })
}

/// Resolve a list of declared package files into unique, path-sorted content receipts.
fn lock_inputs(root: &Path, paths: &[String]) -> Result<Vec<LockedInteropInput>, String> {
    let mut inputs = paths
        .iter()
        .map(|path| lock_input(root, path))
        .collect::<Result<Vec<_>, _>>()?;
    inputs.sort_by(|left, right| left.path.cmp(&right.path));
    inputs.dedup_by(|left, right| left.path == right.path);
    Ok(inputs)
}

/// Hash one declared regular package file without following a symlink or consulting an ambient search path.
fn lock_input(root: &Path, relative: &str) -> Result<LockedInteropInput, String> {
    validate_interop_path(relative, "interop input")?;
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("failed to inspect declared interop input {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "declared interop input {} must be a regular file",
            path.display()
        ));
    }
    let bytes = fs::read(&path)
        .map_err(|error| format!("failed to read declared interop input {}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(LockedInteropInput {
        path: relative.to_string(),
        digest: format!("sha256:{}", hex::encode(hasher.finalize())),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn project_with_oven_interop_inputs() -> Result<(TempDir, ProjectManifest), Box<dyn std::error::Error>> {
        let workspace = TempDir::new()?;
        fs::create_dir_all(workspace.path().join("interop/include"))?;
        fs::create_dir_all(workspace.path().join("interop/src"))?;
        fs::create_dir_all(workspace.path().join("interop/lib"))?;
        fs::write(workspace.path().join("interop/include/bridge.h"), "int bridge(void);\n")?;
        fs::write(
            workspace.path().join("interop/src/bridge.c"),
            "int bridge(void) { return 7; }\n",
        )?;
        fs::write(workspace.path().join("interop/lib/libfixture.a"), b"fixture archive")?;
        let manifest_path = workspace.path().join("incan.toml");
        let manifest = ProjectManifest::from_str(
            r#"
[interop.c]
schema = 1

[[interop.c.targets]]
target = "aarch64-apple-ios"
toolchain = { capability = "apple-clang", version = ">=17, <18" }
sdk = { capability = "iphoneos", version = ">=18, <19" }
headers = ["interop/include/bridge.h"]
definitions = ["FIXTURE=1"]

[interop.c.targets.platform]
kind = "ios"
deployment-target = "13.0"

[[interop.c.targets.artifacts]]
name = "fixture"
kind = "static"
path = "interop/lib/libfixture.a"
origin = { source = "https://example.invalid/fixture", revision = "v1.0.0", license = "LicenseRef-Fixture" }
dependencies = ["foundation"]

[[interop.c.targets.artifacts]]
name = "foundation"
kind = "system"
capability = "apple.framework.Foundation"

[[interop.c.targets.bindings]]
module = ["fixture"]
name = "Fixture"
artifacts = ["fixture"]

[[interop.c.targets.shims]]
name = "fixture_bridge"
language = "c"
sources = ["interop/src/bridge.c"]
headers = ["interop/include/bridge.h"]
output = "fixture_bridge"
"#,
            &manifest_path,
        )?;
        Ok((workspace, manifest))
    }

    #[test]
    fn oven_interop_inputs_lock_portably_and_change_with_declared_bytes() -> Result<(), Box<dyn std::error::Error>> {
        let (workspace, manifest) = project_with_oven_interop_inputs()?;
        let first = locked_oven_interop_targets(&manifest)?;
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].target, "aarch64-apple-ios");
        assert_eq!(
            first[0]
                .toolchain
                .as_ref()
                .map(|requirement| requirement.capability.as_str()),
            Some("apple-clang")
        );
        assert_eq!(first[0].headers[0].path, "interop/include/bridge.h");
        assert_eq!(
            first[0].artifacts[0].input.as_ref().map(|input| input.path.as_str()),
            Some("interop/lib/libfixture.a")
        );
        let origin = first[0].artifacts[0]
            .origin
            .as_ref()
            .ok_or("locked static interop artifact lost its declared origin")?;
        assert_eq!(origin.source, "https://example.invalid/fixture");
        assert_eq!(origin.revision, "v1.0.0");
        assert_eq!(origin.license, "LicenseRef-Fixture");
        assert_eq!(
            first[0].artifacts[1].capability.as_deref(),
            Some("apple.framework.Foundation")
        );
        assert_eq!(first[0].bindings[0].module, ["fixture"]);
        assert_eq!(first[0].bindings[0].name, "Fixture");
        assert_eq!(first[0].bindings[0].artifacts, ["fixture"]);
        assert_eq!(first[0].shims[0].sources[0].path, "interop/src/bridge.c");

        fs::write(
            workspace.path().join("interop/src/bridge.c"),
            "int bridge(void) { return 8; }\n",
        )?;
        let second = locked_oven_interop_targets(&manifest)?;
        assert_ne!(
            first[0].shims[0].sources[0].digest,
            second[0].shims[0].sources[0].digest
        );
        Ok(())
    }

    #[test]
    fn interop_deployment_plan_is_portable_dependency_ordered_and_complete() -> Result<(), Box<dyn std::error::Error>> {
        let (workspace, manifest) = project_with_oven_interop_inputs()?;
        let locked = locked_oven_interop_targets(&manifest)?;
        let plan = interop_deployment_plan(&locked[0])?;

        assert_eq!(plan.schema_version, OVEN_INTEROP_DEPLOYMENT_PLAN_SCHEMA_VERSION);
        assert_eq!(plan.target, "aarch64-apple-ios");
        assert_eq!(
            plan.toolchain
                .as_ref()
                .map(|requirement| requirement.capability.as_str()),
            Some("apple-clang")
        );
        assert_eq!(
            plan.sdk.as_ref().map(|requirement| requirement.capability.as_str()),
            Some("iphoneos")
        );
        assert_eq!(plan.include_roots, ["interop/include"]);
        assert_eq!(
            plan.artifacts
                .iter()
                .map(|artifact| artifact.name.as_str())
                .collect::<Vec<_>>(),
            ["foundation", "fixture"]
        );
        assert!(matches!(
            &plan.artifacts[0].action,
            InteropDeploymentAction::System { capability }
                if capability == "apple.framework.Foundation"
        ));
        assert!(matches!(
            &plan.artifacts[1].action,
            InteropDeploymentAction::StaticLink { input }
                if input.path == "interop/lib/libfixture.a"
                    && input.digest.starts_with("sha256:")
        ));
        assert_eq!(plan.artifacts[1].dependencies, ["foundation"]);
        assert_eq!(plan.bindings[0].module, ["fixture"]);
        assert_eq!(plan.bindings[0].name, "Fixture");
        assert_eq!(plan.bindings[0].artifacts, ["fixture"]);
        assert_eq!(plan.shims[0].output, "fixture_bridge");
        let serialized = serde_json::to_string(&plan)?;
        assert!(!serialized.contains(&workspace.path().to_string_lossy().to_string()));
        Ok(())
    }

    #[test]
    fn interop_deployment_plan_carries_bundled_runtime_placement() -> Result<(), Box<dyn std::error::Error>> {
        let plan = interop_deployment_plan(&LockedInteropTarget {
            target: "aarch64-linux-android".to_string(),
            toolchain: Some(ToolchainRequirement {
                capability: "android-ndk".to_string(),
                version: Some(">=29, <30".to_string()),
            }),
            sdk: Some(ToolchainRequirement {
                capability: "android".to_string(),
                version: Some(">=36, <37".to_string()),
            }),
            platform: Some(InteropTargetPlatform::Android { api_level: 34 }),
            definitions: Vec::new(),
            headers: Vec::new(),
            artifacts: vec![LockedInteropArtifact {
                name: "tflite".to_string(),
                kind: InteropArtifactKind::Bundled,
                input: Some(LockedInteropInput {
                    path: "interop/android/arm64-v8a/libtensorflowlite_c.so".to_string(),
                    digest: "sha256:fixture".to_string(),
                }),
                origin: None,
                capability: None,
                runtime_name: Some("libtensorflowlite_c.so".to_string()),
                placement: Some("jniLibs/arm64-v8a".to_string()),
                minimum_platform: Some("21".to_string()),
                dependencies: Vec::new(),
            }],
            bindings: Vec::new(),
            shims: Vec::new(),
        })?;

        assert!(matches!(
            &plan.artifacts[0].action,
            InteropDeploymentAction::Bundle {
                input,
                runtime_name,
                placement,
                minimum_platform,
            } if input.path == "interop/android/arm64-v8a/libtensorflowlite_c.so"
                && runtime_name == "libtensorflowlite_c.so"
                && placement == "jniLibs/arm64-v8a"
                && minimum_platform == "21"
        ));
        Ok(())
    }

    #[test]
    fn interop_artifact_dependency_cycles_are_rejected() {
        let target = InteropCTarget {
            target: "aarch64-linux-android".to_string(),
            toolchain: Some(ToolchainRequirement {
                capability: "android-ndk".to_string(),
                version: Some(">=29, <30".to_string()),
            }),
            sdk: Some(ToolchainRequirement {
                capability: "android".to_string(),
                version: Some(">=36, <37".to_string()),
            }),
            platform: Some(InteropTargetPlatform::Android { api_level: 34 }),
            headers: Vec::new(),
            definitions: Vec::new(),
            artifacts: vec![
                InteropArtifact {
                    name: "model".to_string(),
                    kind: InteropArtifactKind::Static,
                    path: Some("interop/lib/model.a".to_string()),
                    origin: None,
                    capability: None,
                    runtime_name: None,
                    placement: None,
                    minimum_platform: None,
                    dependencies: vec!["runtime".to_string()],
                },
                InteropArtifact {
                    name: "runtime".to_string(),
                    kind: InteropArtifactKind::Static,
                    path: Some("interop/lib/runtime.a".to_string()),
                    origin: None,
                    capability: None,
                    runtime_name: None,
                    placement: None,
                    minimum_platform: None,
                    dependencies: vec!["model".to_string()],
                },
            ],
            bindings: Vec::new(),
            shims: Vec::new(),
        };
        let interop = InteropCSection {
            schema: INTEROP_C_SCHEMA_VERSION,
            targets: vec![target],
        };
        assert!(
            interop
                .validate()
                .is_err_and(|error| error.contains("dependency graph contains a cycle"))
        );

        let mut dangling_binding = interop.clone();
        dangling_binding.targets[0].bindings = vec![InteropBindingArtifact {
            module: vec!["fixture".to_string()],
            name: "Fixture".to_string(),
            artifacts: vec!["missing".to_string()],
        }];
        assert!(
            dangling_binding
                .validate()
                .is_err_and(|error| error.contains("must reference distinct declared artifacts"))
        );
    }

    #[test]
    fn oven_interop_inputs_reject_ambient_paths_and_incomplete_bundles() {
        let invalid_requirement = InteropCSection {
            schema: INTEROP_C_SCHEMA_VERSION,
            targets: vec![InteropCTarget {
                target: "x86_64-unknown-linux-gnu".to_string(),
                toolchain: Some(ToolchainRequirement {
                    capability: "clang".to_string(),
                    version: Some("eighteen-ish".to_string()),
                }),
                sdk: None,
                platform: None,
                headers: Vec::new(),
                definitions: Vec::new(),
                artifacts: Vec::new(),
                bindings: Vec::new(),
                shims: Vec::new(),
            }],
        };
        assert!(invalid_requirement.validate().is_err());

        let absolute = InteropCSection {
            schema: INTEROP_C_SCHEMA_VERSION,
            targets: vec![InteropCTarget {
                target: "x86_64-unknown-linux-gnu".to_string(),
                toolchain: Some(ToolchainRequirement {
                    capability: "clang".to_string(),
                    version: Some(">=18, <19".to_string()),
                }),
                sdk: None,
                platform: None,
                headers: vec!["/usr/include/fixture.h".to_string()],
                definitions: Vec::new(),
                artifacts: Vec::new(),
                bindings: Vec::new(),
                shims: Vec::new(),
            }],
        };
        assert!(absolute.validate().is_err());

        let bundled = InteropCSection {
            schema: INTEROP_C_SCHEMA_VERSION,
            targets: vec![InteropCTarget {
                target: "x86_64-apple-darwin".to_string(),
                toolchain: None,
                sdk: Some(ToolchainRequirement {
                    capability: "macosx".to_string(),
                    version: Some(">=15, <16".to_string()),
                }),
                platform: None,
                headers: Vec::new(),
                definitions: Vec::new(),
                artifacts: vec![InteropArtifact {
                    name: "fixture".to_string(),
                    kind: InteropArtifactKind::Bundled,
                    path: Some("interop/lib/libfixture.dylib".to_string()),
                    origin: None,
                    capability: None,
                    runtime_name: None,
                    placement: None,
                    minimum_platform: None,
                    dependencies: Vec::new(),
                }],
                bindings: Vec::new(),
                shims: Vec::new(),
            }],
        };
        assert!(bundled.validate().is_err());

        let invalid_dependency = InteropCSection {
            schema: INTEROP_C_SCHEMA_VERSION,
            targets: vec![InteropCTarget {
                target: "x86_64-unknown-linux-gnu".to_string(),
                toolchain: None,
                sdk: None,
                platform: None,
                headers: Vec::new(),
                definitions: Vec::new(),
                artifacts: vec![InteropArtifact {
                    name: "fixture".to_string(),
                    kind: InteropArtifactKind::Static,
                    path: Some("interop/lib/libfixture.a".to_string()),
                    origin: None,
                    capability: None,
                    runtime_name: None,
                    placement: None,
                    minimum_platform: None,
                    dependencies: vec!["missing".to_string()],
                }],
                bindings: Vec::new(),
                shims: Vec::new(),
            }],
        };
        assert!(invalid_dependency.validate().is_err());

        let invalid_origin = InteropCSection {
            schema: INTEROP_C_SCHEMA_VERSION,
            targets: vec![InteropCTarget {
                target: "x86_64-unknown-linux-gnu".to_string(),
                toolchain: None,
                sdk: None,
                platform: None,
                headers: Vec::new(),
                definitions: Vec::new(),
                artifacts: vec![InteropArtifact {
                    name: "fixture".to_string(),
                    kind: InteropArtifactKind::Static,
                    path: Some("interop/lib/libfixture.a".to_string()),
                    origin: Some(InteropArtifactOrigin {
                        source: "file:///private/fixture".to_string(),
                        revision: "v1".to_string(),
                        license: "MIT".to_string(),
                    }),
                    capability: None,
                    runtime_name: None,
                    placement: None,
                    minimum_platform: None,
                    dependencies: Vec::new(),
                }],
                bindings: Vec::new(),
                shims: Vec::new(),
            }],
        };
        assert!(
            invalid_origin
                .validate()
                .is_err_and(|error| error.contains("origin source"))
        );

        let unsafe_shim_output = InteropCSection {
            schema: INTEROP_C_SCHEMA_VERSION,
            targets: vec![InteropCTarget {
                target: "x86_64-unknown-linux-gnu".to_string(),
                toolchain: None,
                sdk: None,
                platform: None,
                headers: Vec::new(),
                definitions: Vec::new(),
                artifacts: Vec::new(),
                bindings: Vec::new(),
                shims: vec![InteropShim {
                    name: "fixture".to_string(),
                    language: InteropShimLanguage::C,
                    sources: vec!["interop/src/fixture.c".to_string()],
                    headers: Vec::new(),
                    output: "../escape".to_string(),
                }],
            }],
        };
        assert!(
            unsafe_shim_output
                .validate()
                .is_err_and(|error| error.contains("native library name"))
        );
    }

    #[test]
    fn mobile_platform_profiles_require_matching_target_sdk_and_version_facts() {
        let android = InteropCSection {
            schema: INTEROP_C_SCHEMA_VERSION,
            targets: vec![InteropCTarget {
                target: "aarch64-linux-android".to_string(),
                toolchain: Some(ToolchainRequirement {
                    capability: "android-ndk".to_string(),
                    version: Some(">=29, <30".to_string()),
                }),
                sdk: Some(ToolchainRequirement {
                    capability: "android".to_string(),
                    version: Some(">=36, <37".to_string()),
                }),
                platform: Some(InteropTargetPlatform::Android { api_level: 34 }),
                headers: Vec::new(),
                definitions: Vec::new(),
                artifacts: Vec::new(),
                bindings: Vec::new(),
                shims: Vec::new(),
            }],
        };
        assert!(android.validate().is_ok());

        let apple = InteropCSection {
            schema: INTEROP_C_SCHEMA_VERSION,
            targets: vec![InteropCTarget {
                target: "aarch64-apple-ios".to_string(),
                toolchain: Some(ToolchainRequirement {
                    capability: "apple-clang".to_string(),
                    version: Some(">=17, <18".to_string()),
                }),
                sdk: Some(ToolchainRequirement {
                    capability: "iphoneos".to_string(),
                    version: Some(">=18, <19".to_string()),
                }),
                platform: Some(InteropTargetPlatform::Ios {
                    deployment_target: "13.0".to_string(),
                }),
                headers: Vec::new(),
                definitions: Vec::new(),
                artifacts: Vec::new(),
                bindings: Vec::new(),
                shims: Vec::new(),
            }],
        };
        assert!(apple.validate().is_ok());

        let mut apple_simulator = apple.clone();
        apple_simulator.targets[0].target = "aarch64-apple-ios-sim".to_string();
        apple_simulator.targets[0].sdk = Some(ToolchainRequirement {
            capability: "iphonesimulator".to_string(),
            version: Some(">=18, <19".to_string()),
        });
        assert!(apple_simulator.validate().is_ok());

        let mut unsupported_android_api = android.clone();
        unsupported_android_api.targets[0].platform = Some(InteropTargetPlatform::Android { api_level: 20 });
        assert!(
            unsupported_android_api
                .validate()
                .is_err_and(|error| error.contains("API level 21 or later"))
        );

        let mut wrong_android_sdk = android;
        wrong_android_sdk.targets[0].sdk = Some(ToolchainRequirement {
            capability: "iphoneos".to_string(),
            version: Some(">=18, <19".to_string()),
        });
        assert!(
            wrong_android_sdk
                .validate()
                .is_err_and(|error| error.contains("`android` SDK capability"))
        );

        let mut incompatible_apple_target = apple.clone();
        incompatible_apple_target.targets[0].target = "aarch64-apple-darwin".to_string();
        assert!(
            incompatible_apple_target
                .validate()
                .is_err_and(|error| error.contains("aarch64-apple-ios"))
        );

        let mut wrong_simulator_sdk = apple_simulator;
        wrong_simulator_sdk.targets[0].sdk = Some(ToolchainRequirement {
            capability: "iphoneos".to_string(),
            version: Some(">=18, <19".to_string()),
        });
        assert!(
            wrong_simulator_sdk
                .validate()
                .is_err_and(|error| error.contains("`iphonesimulator` SDK capability"))
        );

        let mut malformed_apple_version = apple;
        malformed_apple_version.targets[0].platform = Some(InteropTargetPlatform::Ios {
            deployment_target: "iOS 13".to_string(),
        });
        assert!(
            malformed_apple_version
                .validate()
                .is_err_and(|error| error.contains("numeric `major.minor` version"))
        );
    }
}
