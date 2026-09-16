//! What a selected provider needs compiled beside it: macro dependencies, packaged provider profiles and the
//! test-dependency packages an explicit bake seals.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::build::caller_owned::caller_owned_library_rust_dependencies;
#[cfg(test)]
use crate::build::caller_owned::{
    caller_owned_library_dependencies_without_public_provider_edges, load_receipted_public_provider_dependency,
};
use crate::build::oven_project::project_extension_base_loaf;
use crate::build::package_loafs::{
    import_checked_packaged_library_loaf, read_packaged_library_loaf_manifest, validated_packaged_library_loaf_profile,
};
use crate::build::plan_authority::is_selected_compiler_runtime_path_dependency;
use crate::build::{CheckedPackagedProviderProfile, OvenProjectBakeAuthorityContext, OvenProjectPlanMode};
use crate::error::{CliError, CliResult, oven_rustc_error};
use incan_frontend::library_manifest::published_layout::packaged_library_loaf_manifest_path;
#[cfg(test)]
use incan_frontend::library_manifest::{LibraryManifest, ProviderDependencyKind};
use incan_frontend::library_manifest_index::{LibraryArtifactKind, LibraryArtifactMetadata};
use incan_provider::ProviderPlan;
#[cfg(test)]
#[cfg(test)]
use incan_provider::requirements::dependency_specs_match;
use oven_model::manifest::{DependencySource, DependencySpec};
use oven_rustc::legacy_cargo::{OVEN_PROVIDER_COMPILATION_KEY, OvenCompilerMacroDependency};
use oven_rustc::plan::selection::SelectedPackagedProviderPlans;
use oven_rustc::rustc::{OvenRustcArtifactManifest, OvenRustcArtifactPlan, trusted_artifact_plan_for_source_evidence};
use oven_store::store::OvenStore;

/// Give a materialized plan the source role that the provider projection is about to select against.
///
/// `provider_compilation_artifacts` republishes one source role's closure under `generated-root` so the provider
/// compile has a role to select. When the manifest declared no source roles at all -- an unscoped publisher, whose
/// whole closure is its one implicit role -- that key is *new*, and the plan was materialized before it existed. Its
/// physical bindings therefore carry no role of that name, and projecting against it refuses a plan that is in fact
/// complete, with "source evidence `generated-root` has no materialized role".
///
/// Fill the gap rather than refuse: an unscoped publisher excludes nothing, so the role's selected directories are
/// exactly the directories the plan already declared. An existing role is never overwritten, so a scoped publisher
/// keeps the exclusion its own binding recorded.
pub fn plan_with_provider_compilation_role(
    plan: &OvenRustcArtifactPlan,
    artifacts: &OvenRustcArtifactManifest,
    provider_artifacts: &OvenRustcArtifactManifest,
) -> OvenRustcArtifactPlan {
    let mut plan = plan.clone();
    let Some(projection) = plan.source_path_projection.as_mut() else {
        return plan;
    };
    if projection.roles.contains_key("generated-root") {
        return plan;
    }
    let Some(closure) = provider_artifacts
        .entrypoint_dependency_search_paths
        .get("generated-root")
    else {
        return plan;
    };
    if artifacts
        .entrypoint_dependency_search_paths
        .contains_key("generated-root")
    {
        return plan;
    }
    let declared = projection.declared.clone();
    projection
        .roles
        .insert("generated-root".to_string(), (closure.clone(), declared));
    plan
}

/// Retain a compiler macro only when it is a named root of the selected provider compilation.
///
/// Generated manifests declare derive support unconditionally. A current publisher's explicit provider projection
/// distinguishes real use from an unused declaration, while an older broad plan preserves its selected macro. A missing
/// required named root is rejected by the existing source projection before this filter runs.
pub fn caller_owned_library_dependencies_for_compilation(
    dependencies: Vec<DependencySpec>,
    plan: &OvenRustcArtifactPlan,
) -> Vec<DependencySpec> {
    let has_derive = plan.externs.iter().any(|(name, _)| name == "incan_derive");
    dependencies
        .into_iter()
        .filter(|dependency| dependency.crate_name != "incan_derive" || has_derive)
        .collect()
}

/// Capture the compiler macro set from admitted provider plans, retaining their existing physical selection.
///
/// The fixed provider projection also carries a facade's transitive macro requirement. No provider body or generated
/// source path is read here, and an unconditional Cargo declaration alone never requests macro preparation.
pub fn checked_provider_compilation_requirements(
    selections: &SelectedPackagedProviderPlans,
    checked_profiles: &[CheckedPackagedProviderProfile],
    profile: &str,
    runtime_inputs: &BTreeMap<String, String>,
) -> CliResult<Vec<OvenCompilerMacroDependency>> {
    let mut requirements = Vec::new();
    for checked in checked_profiles.iter().filter(|checked| checked.profile == profile) {
        let receipt = &checked.package.receipt;
        let mut needs_derive = false;
        for (alias, _, plan) in &selections.direct {
            if alias == &checked.dependency_key {
                needs_derive |= selected_provider_requires_derive(&plan.artifact_plan, &plan.artifacts)?;
            }
        }
        for (alias, _, plan) in &selections.extensions {
            if alias == &checked.dependency_key {
                needs_derive |= selected_provider_requires_derive(&plan.artifact_plan, &plan.artifacts)?;
            }
        }
        if checked.package.entries.is_empty() {
            let base = project_extension_base_loaf(receipt)?
                .ok_or_else(|| CliError::failure("provider has no packaged delta or compatible compiler base"))?;
            needs_derive = selected_provider_requires_derive(&base.artifact_plan, &base.artifacts)?;
        }
        if !needs_derive {
            continue;
        }
        let artifact = LibraryArtifactMetadata::from_crate_root(
            checked.dependency_key.clone(),
            receipt.project.name.clone(),
            &checked.artifact_root,
        );
        let dependencies = caller_owned_library_rust_dependencies(&artifact)?;
        let dependency = dependencies
            .iter()
            .find(|dependency| dependency.crate_name == "incan_derive")
            .ok_or_else(|| CliError::failure("provider native plan requires undeclared incan_derive"))?;
        let requirement = checked_provider_macro_dependency(dependency, receipt, runtime_inputs)?;
        if !requirements.contains(&requirement) {
            requirements.push(requirement);
        }
    }
    Ok(requirements)
}

/// Read direct or encapsulated provider macro use from the already selected named roots.
fn selected_provider_requires_derive(
    plan: &OvenRustcArtifactPlan,
    artifacts: &OvenRustcArtifactManifest,
) -> CliResult<bool> {
    let root =
        trusted_artifact_plan_for_source_evidence(plan, artifacts, "generated-root").map_err(oven_rustc_error)?;
    if root.externs.iter().any(|(name, _)| name == "incan_derive") {
        return Ok(true);
    }
    if artifacts.entrypoint_externs.contains_key(OVEN_PROVIDER_COMPILATION_KEY) {
        let providers = trusted_artifact_plan_for_source_evidence(plan, artifacts, OVEN_PROVIDER_COMPILATION_KEY)
            .map_err(oven_rustc_error)?;
        return Ok(providers.externs.iter().any(|(name, _)| name == "incan_derive"));
    }
    Ok(false)
}

/// Associate one declared compiler macro with matching checked runtime source and lock content.
fn checked_provider_macro_dependency(
    dependency: &DependencySpec,
    receipt: &oven_store::OvenReceipt,
    runtime_inputs: &BTreeMap<String, String>,
) -> CliResult<OvenCompilerMacroDependency> {
    let DependencySource::Path { path } = &dependency.source else {
        return Err(CliError::failure(
            "provider incan_derive must name the compiler-owned source path",
        ));
    };
    let root = fs::canonicalize(oven_model::toolchain_layout::resolve_toolchain_crate_path(
        "incan_derive",
    ))
    .map_err(|error| CliError::failure(format!("cannot resolve compiler macro source: {error}")))?;
    if dependency.crate_name != "incan_derive"
        || dependency.package.as_deref().is_some_and(|name| name != "incan_derive")
        || dependency.optional
        || !dependency.default_features
        || !dependency.features.is_empty()
        || fs::canonicalize(path).ok().as_ref() != Some(&root)
    {
        return Err(CliError::failure(
            "provider incan_derive declaration differs from the compiler-owned macro source",
        ));
    }
    let matching_input = |key: &str| -> CliResult<String> {
        let expected = runtime_inputs
            .get(key)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| CliError::failure(format!("consumer lacks checked {key}")))?;
        if receipt.sources.build_unit_inputs.get(key) != Some(expected) {
            return Err(CliError::failure(format!("provider macro has incompatible {key}")));
        }
        Ok(expected.clone())
    };
    Ok(OvenCompilerMacroDependency {
        alias: dependency.crate_name.clone(),
        package: "incan_derive".to_string(),
        source_root: root,
        source_digest: matching_input("runtime-source-incan-derive")?,
        core_source_digest: matching_input("runtime-source-incan-lang")?,
        runtime_lock_digest: matching_input("runtime-lock")?,
    })
}

/// Validate every required package profile once for one consumer preparation.
///
/// A public provider is an independently baked unit. Adding its registry dependencies to every consumer selection
/// would rebuild DataFusion (or any analogous provider closure) under that consumer's plan. Instead, an explicit
/// provider bake seals those dependencies once; a consumer selection retains only its own Rust surface and composes
/// the verified provider Loaf later. The checked records are command-local and flow through import and selection,
/// avoiding repeated recursive source scans without introducing a cross-command cache.
pub fn checked_packaged_provider_profiles(
    provider_plan: &ProviderPlan,
    profiles: &[&str],
    target: &str,
    toolchain: &str,
    mut authority_context: Option<&mut OvenProjectBakeAuthorityContext>,
) -> CliResult<Vec<CheckedPackagedProviderProfile>> {
    let mut checked = Vec::new();
    for provider in provider_plan.active_records().filter(|provider| {
        matches!(
            provider.authority,
            incan_provider::NamespaceAuthority::ProjectDependency { .. }
        )
    }) {
        let artifact = provider.artifact.as_ref().ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha cannot select pub::{} because its generated library artifact is unavailable",
                provider.identity.name
            ))
        })?;
        if artifact.kind != LibraryArtifactKind::Materialized {
            return Err(CliError::failure(format!(
                "Oven Alpha requires an explicit package Loaf for pub::{}; bake that provider with `incan oven bake --project {}` before baking this consumer",
                artifact.dependency_key,
                artifact
                    .crate_root
                    .parent()
                    .and_then(Path::parent)
                    .unwrap_or(&artifact.crate_root)
                    .display()
            )));
        }
        let packages = if let Some(context) = authority_context.as_deref_mut() {
            context.checked_packaged_library_loaf_profiles(artifact, profiles, target, toolchain)?
        } else {
            let Some(manifest) = read_packaged_library_loaf_manifest(artifact)? else {
                return Err(CliError::failure(format!(
                    "Oven Alpha requires an explicit package Loaf for pub::{}; bake that provider with `incan oven bake --project {}` before baking this consumer",
                    artifact.dependency_key,
                    artifact
                        .crate_root
                        .parent()
                        .and_then(Path::parent)
                        .unwrap_or(&artifact.crate_root)
                        .display()
                )));
            };
            let mut selected = Vec::with_capacity(profiles.len());
            for profile in profiles {
                let Some(package) =
                    validated_packaged_library_loaf_profile(artifact, &manifest, profile, target, toolchain)?
                else {
                    return Err(CliError::failure(format!(
                        "Oven Alpha has no compatible `{profile}` package Loaf for pub::{} (target `{target}`, toolchain `{toolchain}`); bake that provider with the active Incan release before baking this consumer",
                        artifact.dependency_key
                    )));
                };
                selected.push(package);
            }
            Some(selected)
        };
        let Some(packages) = packages else {
            let requested_profiles = profiles.join("`, `");
            return Err(CliError::failure(format!(
                "Oven Alpha has no compatible `{requested_profiles}` package Loaf for pub::{} (target `{target}`, toolchain `{toolchain}`); bake that provider with the active Incan release before baking this consumer",
                artifact.dependency_key
            )));
        };
        for (profile, package) in profiles.iter().zip(packages) {
            checked.push(CheckedPackagedProviderProfile {
                dependency_key: artifact.dependency_key.clone(),
                artifact_root: artifact.crate_root.clone(),
                profile: (*profile).to_string(),
                package,
            });
        }
    }
    Ok(checked)
}

/// Validate package-provider roots from the complete project dependency surface, including test-only imports.
///
/// Conventional main/library provider plans do not necessarily observe a package imported only by an owned test.
/// The canonical lock surface does: compiled Incan packages appear there as path dependencies rooted at their
/// generated artifact. An adjacent package-Loaf manifest is the explicit ownership marker; ordinary Rust path crates
/// remain in the publisher delta.
pub fn checked_test_dependency_package_profiles(
    dependencies: &[DependencySpec],
    receipt: &oven_store::OvenReceipt,
    mut authority_context: Option<&mut OvenProjectBakeAuthorityContext>,
) -> CliResult<Vec<CheckedPackagedProviderProfile>> {
    let mut checked = Vec::new();
    for dependency in dependencies {
        let DependencySource::Path { path } = &dependency.source else {
            continue;
        };
        let manifest_name = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
        let artifact =
            LibraryArtifactMetadata::from_crate_root(dependency.crate_name.clone(), manifest_name.to_string(), path);
        if !packaged_library_loaf_manifest_path(&artifact.crate_root).is_file() {
            continue;
        }
        let package = if let Some(context) = authority_context.as_deref_mut() {
            context
                .checked_packaged_library_loaf_profiles(
                    &artifact,
                    &["debug"],
                    &receipt.intent.target,
                    &receipt.intent.toolchain,
                )?
                .and_then(|mut profiles| profiles.pop())
        } else {
            let manifest = read_packaged_library_loaf_manifest(&artifact)?.ok_or_else(|| {
                CliError::failure(format!(
                    "Oven Alpha cannot validate the package Loaf declared by test dependency `{}`",
                    dependency.crate_name
                ))
            })?;
            validated_packaged_library_loaf_profile(
                &artifact,
                &manifest,
                "debug",
                &receipt.intent.target,
                &receipt.intent.toolchain,
            )?
        }
        .ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha has no compatible `debug` package Loaf for test dependency `{}`; rebake that provider with the active Incan release before baking this consumer",
                dependency.crate_name
            ))
        })?;
        checked.push(CheckedPackagedProviderProfile {
            dependency_key: artifact.dependency_key,
            artifact_root: artifact.crate_root,
            profile: "debug".to_string(),
            package,
        });
    }
    checked.sort_by(|left, right| left.dependency_key.cmp(&right.dependency_key));
    checked.dedup_by(|left, right| left.dependency_key == right.dependency_key);
    Ok(checked)
}

/// Materialize every already baked provider Loaf into the consumer's bounded store at the explicit bake boundary.
pub fn import_packaged_provider_loafs_for_explicit_bake(
    mode: OvenProjectPlanMode,
    consumer_store: &OvenStore,
    checked_profiles: &[CheckedPackagedProviderProfile],
) -> CliResult<()> {
    if !mode.is_explicit_publisher() {
        return Ok(());
    }
    for checked in checked_profiles {
        import_checked_packaged_library_loaf(consumer_store, checked)?;
    }
    Ok(())
}

/// Follow the historical digest-verified public-provider graph for targeted migration coverage.
///
/// Production selection now requires one baked package Loaf for each public provider, so it no longer walks this
/// graph or rebuilds its registry closure from every consumer. The helper remains only to preserve direct coverage
/// of the checked graph traversal itself.
#[cfg(test)]
fn collect_caller_owned_project_rust_dependencies(
    artifact: &LibraryArtifactMetadata,
    manifest: &LibraryManifest,
    visiting: &mut BTreeSet<PathBuf>,
    dependencies: &mut Vec<DependencySpec>,
) -> CliResult<()> {
    let canonical_root = fs::canonicalize(&artifact.crate_root).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot canonicalize generated artifact root for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.crate_root.display()
        ))
    })?;
    if !visiting.insert(canonical_root.clone()) {
        return Err(CliError::failure(format!(
            "Oven Alpha refuses a cyclic public provider graph while selecting pub::{} at {}",
            artifact.dependency_key,
            canonical_root.display()
        )));
    }
    let result = (|| {
        for dependency in manifest
            .contract_metadata
            .provider
            .provider_dependencies
            .iter()
            .filter(|dependency| dependency.kind == ProviderDependencyKind::PublicPackage)
        {
            let (nested_manifest, nested_artifact) = load_receipted_public_provider_dependency(artifact, dependency)?;
            collect_caller_owned_project_rust_dependencies(&nested_artifact, &nested_manifest, visiting, dependencies)?;
        }

        let provider_dependencies = caller_owned_library_rust_dependencies(artifact)?;
        let provider_dependencies =
            caller_owned_library_dependencies_without_public_provider_edges(provider_dependencies, manifest);
        for dependency in provider_dependencies {
            merge_oven_dependency_surface(dependencies, dependency, &artifact.dependency_key)?;
        }
        Ok(())
    })();
    visiting.remove(&canonical_root);
    result
}

/// Merge Cargo-unifiable requirements while retaining a fail-closed identity boundary.
#[cfg(test)]
fn merge_oven_dependency_surface(
    dependencies: &mut Vec<DependencySpec>,
    candidate: DependencySpec,
    provider: &str,
) -> CliResult<()> {
    if let Some(existing) = dependencies
        .iter_mut()
        .find(|dependency| dependency.crate_name == candidate.crate_name)
    {
        let mut existing_identity = existing.clone();
        let mut candidate_identity = candidate.clone();
        for dependency in [&mut existing_identity, &mut candidate_identity] {
            dependency.features.clear();
            dependency.default_features = false;
            dependency.optional = false;
        }
        if !dependency_specs_match(&existing_identity, &candidate_identity) {
            return Err(CliError::failure(format!(
                "Oven Alpha cannot unify caller-owned dependency `{}` required by pub::{provider}; align its source, version, and package identity before baking an explicit project Loaf",
                candidate.crate_name
            )));
        }
        existing.features.extend(candidate.features);
        existing.features.sort();
        existing.features.dedup();
        existing.default_features |= candidate.default_features;
        existing.optional &= candidate.optional;
        return Ok(());
    }
    dependencies.push(candidate);
    Ok(())
}

/// Variant with explicit scheduler-owned roots so the authority rule is independently testable.
pub fn caller_owned_library_dependencies_missing_from_selected_plan_with_owned_roots(
    dependencies: &[DependencySpec],
    artifact_plan: &OvenRustcArtifactPlan,
    owned_roots: &[PathBuf],
) -> Vec<DependencySpec> {
    let selected_externs = artifact_plan
        .externs
        .iter()
        .map(|(crate_name, _)| crate_name.as_str())
        .collect::<BTreeSet<_>>();
    dependencies
        .iter()
        .filter(|dependency| match dependency.source {
            DependencySource::Registry => !selected_externs.contains(dependency.crate_name.replace('-', "_").as_str()),
            DependencySource::Path { .. } => {
                !is_selected_compiler_runtime_path_dependency(dependency, &selected_externs, owned_roots)
            }
            DependencySource::Git { .. } => true,
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;

    use crate::build::caller_owned::caller_owned_library_rust_dependencies;
    use crate::build::plan_authority::{
        compiler_owned_roots_with_provider_plan, is_selected_compiler_runtime_path_dependency,
    };
    use crate::error::oven_plan_error;
    use incan_frontend::library_manifest::{
        LibraryManifest, ProviderDependencyKind, ProviderDependencyMetadata, digest_provider_artifact,
    };
    use incan_frontend::library_manifest_index::{LibraryArtifactKind, LibraryArtifactMetadata, LibraryManifestIndex};
    use incan_provider::ProviderPlan;
    use oven_model::manifest::{DependencySource, DependencySpec};
    use oven_rustc::legacy_cargo::OVEN_PROVIDER_COMPILATION_KEY;
    use oven_rustc::plan::composition::provider_compilation_artifacts;
    use oven_rustc::rustc::{
        OvenRustcArtifactManifest, OvenRustcArtifactPlan, trusted_artifact_plan_for_source_evidence,
    };
    use oven_store::{OvenGeneratedProjectRequest, digest_bytes, receipt_generated_project};

    /// Unused macro declarations do not request a build; transitive facade requirements preserve the selected macro.
    #[test]
    fn selected_provider_macro_requirements_follow_named_roots() -> Result<(), Box<dyn std::error::Error>> {
        // The plan declares no physical search paths, but it still has to bind the one source role the manifests
        // below declare a closure for -- projecting a plan for a role the plan never materialized is refused.
        let plan = OvenRustcArtifactPlan {
            source_path_projection: Some(oven_rustc::rustc::OvenRustcSourcePathProjection {
                declared: BTreeSet::new(),
                roles: ["generated-root", OVEN_PROVIDER_COMPILATION_KEY]
                    .into_iter()
                    .map(|role| {
                        (
                            role.to_string(),
                            (
                                oven_rustc::rustc::OvenRustcSourceSearchClosure::default(),
                                BTreeSet::new(),
                            ),
                        )
                    })
                    .collect(),
            }),
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: vec![("incan_derive".to_string(), PathBuf::from("/selected/current-macro"))],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };
        let mut artifacts = OvenRustcArtifactManifest {
            schema_version: oven_rustc::rustc::OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: oven_store::OvenBuildIntent {
                target: "target".to_string(),
                toolchain: "rustc".to_string(),
                profile: "debug".to_string(),
                features: Vec::new(),
            },
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: vec![oven_rustc::rustc::OvenRustcArtifactExtern {
                crate_name: "incan_derive".to_string(),
                relative_path: "macro".to_string(),
                digest: digest_bytes(b"macro"),
            }],
            entrypoint_dependency_search_paths: BTreeMap::from([(
                "generated-root".to_string(),
                oven_rustc::rustc::OvenRustcSourceSearchClosure::default(),
            )]),
            entrypoint_externs: BTreeMap::from([("generated-root".to_string(), Vec::new())]),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        };
        assert!(!selected_provider_requires_derive(&plan, &artifacts)?);
        let empty = trusted_artifact_plan_for_source_evidence(&plan, &artifacts, "generated-root")?;
        let declaration = DependencySpec {
            crate_name: "incan_derive".to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path {
                path: PathBuf::from("/unused-declaration"),
            },
            optional: false,
            package: None,
        };
        assert!(caller_owned_library_dependencies_for_compilation(vec![declaration.clone()], &empty).is_empty());
        artifacts.entrypoint_externs.insert(
            OVEN_PROVIDER_COMPILATION_KEY.to_string(),
            vec!["incan_derive".to_string()],
        );
        artifacts.entrypoint_dependency_search_paths.insert(
            OVEN_PROVIDER_COMPILATION_KEY.to_string(),
            oven_rustc::rustc::OvenRustcSourceSearchClosure::default(),
        );
        assert!(selected_provider_requires_derive(&plan, &artifacts)?);
        let providers = provider_compilation_artifacts(&artifacts).map_err(oven_plan_error)?;
        assert_eq!(
            providers.entrypoint_dependency_search_paths["generated-root"],
            artifacts.entrypoint_dependency_search_paths[OVEN_PROVIDER_COMPILATION_KEY],
        );
        let selected = trusted_artifact_plan_for_source_evidence(&plan, &providers, "generated-root")?;
        assert_eq!(
            trusted_artifact_plan_for_source_evidence(&selected, &providers, "generated-root")?,
            selected,
        );
        assert_eq!(selected.externs, plan.externs);
        assert_eq!(
            caller_owned_library_dependencies_for_compilation(vec![declaration.clone()], &selected),
            vec![declaration]
        );
        assert!(
            trusted_artifact_plan_for_source_evidence(&plan, &artifacts, "generated-root")?
                .externs
                .is_empty()
        );
        artifacts.validate_shape(&artifacts.intent)?;
        artifacts.externs.clear();
        let Err(error) = artifacts.validate_shape(&artifacts.intent) else {
            return Err("manifest admission accepted a missing provider macro".into());
        };
        assert!(matches!(
            error,
            oven_rustc::rustc::OvenRustcError::InvalidInput {
                field: "artifact manifest entrypoint externs",
                message,
            } if message == "source evidence `provider-compilation` names undeclared extern `incan_derive`"
        ));
        Ok(())
    }

    /// A macro requirement needs the compiler-owned declaration and all retained source/lock authorities.
    #[test]
    fn provider_macro_dependency_refuses_changed_sources_and_declarations() -> Result<(), Box<dyn std::error::Error>> {
        let source = tempfile::tempdir()?;
        let root = source.path().join("lib.rs");
        fs::write(&root, "pub fn answer() -> i64 { 42 }\n")?;
        let owned = fs::canonicalize(oven_model::toolchain_layout::resolve_toolchain_crate_path(
            "incan_derive",
        ))?;
        let dependency = DependencySpec {
            crate_name: "incan_derive".to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path { path: owned },
            optional: false,
            package: None,
        };
        let inputs = BTreeMap::from([
            ("runtime-source-incan-derive".to_string(), "sha256:macro".to_string()),
            ("runtime-source-incan-lang".to_string(), "sha256:core".to_string()),
            ("runtime-lock".to_string(), "sha256:lock".to_string()),
        ]);
        let mut request = OvenGeneratedProjectRequest::new(
            source.path(),
            "provider",
            "0.1.0",
            "target",
            "rustc",
            "debug",
            Vec::new(),
        )
        .with_generated_source("generated-root", &root);
        for (name, value) in &inputs {
            request = request.with_build_unit_input(name, value);
        }
        let receipt = receipt_generated_project(&request)?;
        checked_provider_macro_dependency(&dependency, &receipt, &inputs)?;
        for key in inputs.keys() {
            let mut changed = inputs.clone();
            changed.insert(key.clone(), "sha256:changed".to_string());
            assert!(checked_provider_macro_dependency(&dependency, &receipt, &changed).is_err());
            changed.remove(key);
            assert!(checked_provider_macro_dependency(&dependency, &receipt, &changed).is_err());
        }
        for changed in [
            DependencySpec {
                package: Some("unrelated".to_string()),
                ..dependency.clone()
            },
            DependencySpec {
                features: vec!["extra".to_string()],
                ..dependency.clone()
            },
            DependencySpec {
                optional: true,
                ..dependency.clone()
            },
            DependencySpec {
                default_features: false,
                ..dependency.clone()
            },
            DependencySpec {
                source: DependencySource::Path {
                    path: source.path().to_path_buf(),
                },
                ..dependency
            },
        ] {
            assert!(checked_provider_macro_dependency(&changed, &receipt, &inputs).is_err());
        }
        Ok(())
    }

    #[test]
    fn caller_owned_provider_manifest_dependencies_are_explicit_direct_rustc_inputs()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let artifact_root = workspace.path().join("target/lib");
        let toolchain_root = workspace.path().join("toolchain");
        fs::create_dir_all(artifact_root.join("src"))?;
        fs::create_dir_all(toolchain_root.join("incan_std_core"))?;
        fs::write(
            artifact_root.join("Cargo.toml"),
            "[package]\nname = \"provider\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nincan_std_core = { path = \"../../toolchain/incan_std_core\" }\nserde = { version = \"1.0\", features = [\"derive\"] }\nrust_shadow = { path = \"../../rust_shadow\" }\n",
        )?;
        fs::write(artifact_root.join("src/lib.rs"), "pub fn marker() {}\n")?;
        let artifact = LibraryArtifactMetadata {
            dependency_key: "provider".to_string(),
            manifest_name: "provider".to_string(),
            manifest_path: workspace.path().join("provider.incnlib"),
            crate_root: artifact_root.clone(),
            cargo_toml_path: artifact_root.join("Cargo.toml"),
            crate_lib_path: artifact_root.join("src/lib.rs"),
            kind: LibraryArtifactKind::Materialized,
        };

        let dependencies = caller_owned_library_rust_dependencies(&artifact)?;
        assert_eq!(dependencies.len(), 3);
        assert!(dependencies.iter().any(|dependency| {
            dependency.crate_name == "serde"
                && dependency.version.as_deref() == Some("1.0")
                && dependency.features == ["derive"]
                && dependency.source == DependencySource::Registry
        }));
        assert!(dependencies.iter().any(|dependency| {
            dependency.crate_name == "rust_shadow"
                && matches!(
                    &dependency.source,
                    DependencySource::Path { path } if path == &artifact_root.join("../../rust_shadow")
                )
        }));

        let plan = OvenRustcArtifactPlan {
            source_path_projection: None,
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: vec![(
                "incan_std_core".to_string(),
                workspace.path().join("sealed/incan_std_core.rlib"),
            )],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };
        let remaining = caller_owned_library_dependencies_missing_from_selected_plan_with_owned_roots(
            &dependencies,
            &plan,
            &[fs::canonicalize(&toolchain_root)?],
        );
        assert_eq!(remaining.len(), 2);
        assert!(remaining.iter().any(|dependency| dependency.crate_name == "serde"));
        assert!(
            remaining
                .iter()
                .any(|dependency| dependency.crate_name == "rust_shadow")
        );
        Ok(())
    }

    #[test]
    fn selected_compiler_runtime_path_is_not_rematerialized_as_a_caller_dependency()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let toolchain_data_root = workspace.path().join("toolchain-data");
        let runtime_root = workspace.path().join("sdk-runtime");
        let provider_root = workspace.path().join("sdk-providers");
        let runtime_path = runtime_root.join("crates/incan_std_core");
        let component_path = provider_root.join("components/stdlib-core");
        let caller_path = workspace.path().join("caller/incan_std_core");
        fs::create_dir_all(&runtime_path)?;
        fs::create_dir_all(&component_path)?;
        fs::create_dir_all(&caller_path)?;
        let runtime = DependencySpec {
            crate_name: "incan_std_core".to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path {
                path: runtime_path.clone(),
            },
            optional: false,
            package: None,
        };
        let caller = DependencySpec {
            source: DependencySource::Path {
                path: caller_path.clone(),
            },
            ..runtime.clone()
        };
        let component = DependencySpec {
            crate_name: "incan_stdlib_core".to_string(),
            source: DependencySource::Path {
                path: component_path.clone(),
            },
            ..runtime.clone()
        };
        let selected_names = BTreeSet::from(["incan_std_core", "incan_stdlib_core"]);
        fs::create_dir_all(toolchain_data_root.join("share/incan/oven/loafs"))?;
        let owned_roots = vec![
            fs::canonicalize(&toolchain_data_root)?,
            fs::canonicalize(&runtime_root)?,
            fs::canonicalize(&provider_root)?,
        ];

        assert!(is_selected_compiler_runtime_path_dependency(
            &runtime,
            &selected_names,
            &owned_roots,
        ));
        assert!(is_selected_compiler_runtime_path_dependency(
            &component,
            &selected_names,
            &owned_roots,
        ));
        assert!(!is_selected_compiler_runtime_path_dependency(
            &caller,
            &selected_names,
            &owned_roots,
        ));
        let plan = OvenRustcArtifactPlan {
            source_path_projection: None,
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: vec![
                (
                    "incan_std_core".to_string(),
                    workspace.path().join("sealed/incan_std_core.rlib"),
                ),
                (
                    "incan_stdlib_core".to_string(),
                    workspace.path().join("sealed/incan_stdlib_core.rlib"),
                ),
            ],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };
        let remaining = caller_owned_library_dependencies_missing_from_selected_plan_with_owned_roots(
            &[runtime, component, caller.clone()],
            &plan,
            &owned_roots,
        );
        assert_eq!(remaining, vec![caller]);
        Ok(())
    }

    #[test]
    fn checked_historical_sdk_rebinding_reuses_the_selected_sealed_extern() -> Result<(), Box<dyn std::error::Error>> {
        use incan_provider::{NamespaceAuthority, ProviderIdentity, ProviderProvenance, ProviderRecord};

        let workspace = tempfile::tempdir()?;
        let library_root = workspace.path().join("library/target/lib");
        let historical_sdk_root = library_root.join("private/stdlib-core");
        let active_sdk_root = workspace.path().join("active-sdk/stdlib-core");
        let caller_lookalike = workspace.path().join("caller/incan_stdlib_core");
        for root in [&library_root, &historical_sdk_root, &active_sdk_root, &caller_lookalike] {
            fs::create_dir_all(root.join("src"))?;
            fs::write(
                root.join("Cargo.toml"),
                "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
            )?;
            fs::write(root.join("src/lib.rs"), "pub fn marker() {}\n")?;
        }

        let active_digest = digest_provider_artifact(&active_sdk_root)?;
        let mut library_manifest = LibraryManifest::new("library", "0.1.0");
        library_manifest
            .contract_metadata
            .provider
            .provider_dependencies
            .push(ProviderDependencyMetadata {
                kind: ProviderDependencyKind::PrivateImplementation,
                dependency_key: "incan_stdlib_core".to_string(),
                provider_name: "incan_stdlib_core".to_string(),
                provider_version: "0.5.0".to_string(),
                artifact_digest: active_digest.clone(),
                relative_artifact_path: "private/stdlib-core".to_string(),
                requested_features: BTreeSet::new(),
                default_features: false,
                optional: false,
            });
        let library_manifest_path = library_root.join("library.incnlib");
        library_manifest.write_to_path(&library_manifest_path)?;
        let library_artifact = LibraryArtifactMetadata::from_crate_root("library", "library", &library_root);
        let active_sdk_artifact =
            LibraryArtifactMetadata::from_crate_root("incan_stdlib_core", "incan_stdlib_core", &active_sdk_root);
        let provider_plan = ProviderPlan::new(
            LibraryManifestIndex::default(),
            vec![
                ProviderRecord {
                    identity: ProviderIdentity {
                        name: "library".to_string(),
                        version: "0.1.0".to_string(),
                        digest: digest_provider_artifact(&library_root)?,
                        feature_projection: BTreeSet::new(),
                    },
                    provenance: ProviderProvenance::ProjectDependency {
                        dependency_key: "library".to_string(),
                        manifest_path: library_manifest_path,
                    },
                    authority: NamespaceAuthority::ProjectDependency {
                        dependency_key: "library".to_string(),
                    },
                    namespace_claims: BTreeSet::new(),
                    available: true,
                    enabled: true,
                    manifest: Some(Arc::new(library_manifest)),
                    artifact: Some(library_artifact),
                    implementation_facets: Vec::new(),
                },
                ProviderRecord {
                    identity: ProviderIdentity {
                        name: "incan_stdlib_core".to_string(),
                        version: "0.5.0".to_string(),
                        digest: active_digest,
                        feature_projection: BTreeSet::new(),
                    },
                    provenance: ProviderProvenance::Sdk {
                        sdk_identity: "incan@0.5.1-rc2".to_string(),
                        component_id: "stdlib-core".to_string(),
                        inventory_path: None,
                    },
                    authority: NamespaceAuthority::SdkReserved,
                    namespace_claims: BTreeSet::new(),
                    available: true,
                    enabled: true,
                    manifest: Some(Arc::new(LibraryManifest::new("incan_stdlib_core", "0.5.0"))),
                    artifact: Some(active_sdk_artifact),
                    implementation_facets: Vec::new(),
                },
            ],
            [],
        )?;
        let artifact_plan = OvenRustcArtifactPlan {
            source_path_projection: None,
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: vec![(
                "incan_stdlib_core".to_string(),
                workspace.path().join("sealed/incan_stdlib_core.rlib"),
            )],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };

        let owned_roots = compiler_owned_roots_with_provider_plan(&artifact_plan, Some(&provider_plan));
        assert!(
            owned_roots.contains(&fs::canonicalize(&historical_sdk_root)?),
            "the provider plan verified this stale coordinate against the active SDK artifact"
        );
        let historical_dependency = DependencySpec {
            crate_name: "incan_stdlib_core".to_string(),
            version: None,
            features: Vec::new(),
            default_features: false,
            source: DependencySource::Path {
                path: historical_sdk_root,
            },
            optional: false,
            package: None,
        };
        let lookalike_dependency = DependencySpec {
            source: DependencySource::Path { path: caller_lookalike },
            ..historical_dependency.clone()
        };
        let remaining = caller_owned_library_dependencies_missing_from_selected_plan_with_owned_roots(
            &[historical_dependency, lookalike_dependency.clone()],
            &artifact_plan,
            &owned_roots,
        );
        assert_eq!(remaining, vec![lookalike_dependency]);
        Ok(())
    }

    #[test]
    fn public_provider_registry_closure_constrains_loaf_selection() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let artifact_root = workspace.path().join("target/lib");
        fs::create_dir_all(artifact_root.join("src"))?;
        fs::write(
            artifact_root.join("Cargo.toml"),
            "[package]\nname = \"query_provider\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\ndatafusion = \"53\"\nsubstrait = \"0.58\"\n",
        )?;
        fs::write(artifact_root.join("src/lib.rs"), "pub fn query() {}\n")?;
        let artifact = LibraryArtifactMetadata {
            dependency_key: "query_provider".to_string(),
            manifest_name: "query_provider".to_string(),
            manifest_path: workspace.path().join("query_provider.incnlib"),
            crate_root: artifact_root.clone(),
            cargo_toml_path: artifact_root.join("Cargo.toml"),
            crate_lib_path: artifact_root.join("src/lib.rs"),
            kind: LibraryArtifactKind::Materialized,
        };
        let manifest = LibraryManifest::new("query_provider", "0.1.0");
        let mut dependencies = Vec::new();
        let mut visiting = BTreeSet::new();

        collect_caller_owned_project_rust_dependencies(&artifact, &manifest, &mut visiting, &mut dependencies)?;

        assert_eq!(dependencies.len(), 2);
        assert!(dependencies.iter().any(|dependency| {
            dependency.crate_name == "datafusion"
                && dependency.version.as_deref() == Some("53")
                && dependency.source == DependencySource::Registry
        }));
        assert!(dependencies.iter().any(|dependency| {
            dependency.crate_name == "substrait"
                && dependency.version.as_deref() == Some("0.58")
                && dependency.source == DependencySource::Registry
        }));
        Ok(())
    }
}
