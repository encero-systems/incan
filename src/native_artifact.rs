//! Binding-kind-neutral native package inputs and their portable lock projection.
//!
//! A native package declaration names the physical inputs selected for one target. It does not perform ambient
//! discovery, compile a shim, or decide application semantics. Those actions consume this checked plan in later
//! RFC 116 slices.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::manifest::ProjectManifest;

/// Current compatibility format for the `[native]` manifest section.
pub const NATIVE_MANIFEST_SCHEMA_VERSION: u32 = 1;

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
