//! Compose one direct-Rustc closure from compatible public package Loafs.
//!
//! Public providers are baked independently and may share compiler and runtime artifacts. The compositor admits only
//! extension entries that agree on the compiler base, build intent, artifact bytes, registry source identity, and
//! public crate identities, assigns one canonical copy of every byte-identical path, and retains every selected
//! lease for the whole command. Cargo is never asked to resolve those packages again.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::legacy_cargo::OVEN_PROVIDER_COMPILATION_KEY;
use crate::rustc::{
    OvenRegistryLeafAuthority, OvenRustcArtifactExtern, OvenRustcArtifactManifest, OvenRustcArtifactPlan,
    OvenRustcRegistryLeaf, OvenRustcRegistrySourcePackage, OvenRustcSupportingArtifact, OvenTrustedRustcArtifactRoot,
    OvenTrustedRustcSearchRoot,
};

use super::PackagedProviderCandidate;
use super::selection::SelectedPackagedProviderPlans;
use super::{
    OvenDirectPackagedProviderExecutionPlan, OvenDirectRustcPlanSelection, OvenExtensionPackagedProviderExecutionPlan,
    OvenPackagedDirectProviderFragment, OvenPackagedLibraryLoafEntry, OvenPackagedProviderExecutionPlan,
    OvenPackagedProviderFragment, OvenPlanError, OvenPlanResult, OvenProjectExtensionExecutionPlan,
    OvenStoredDirectRustcExecutionPlan,
};
use oven_store::OvenReceipt;

/// Restore package-fragment dependency paths under their owning roots, then normalize the resulting search list.
pub fn retain_packaged_provider_fragment_dependency_search_paths<'a>(
    plan: &mut OvenRustcArtifactPlan,
    fragments: impl IntoIterator<Item = (&'a Path, &'a [String])>,
) {
    for (artifact_root, dependency_search_paths) in fragments {
        for relative in dependency_search_paths {
            plan.retain_caller_dependency_search_path(artifact_root.join(relative));
        }
    }
    plan.dependency_search_paths.sort();
    plan.dependency_search_paths.dedup();
}

/// Use the publisher's provider-only roots for every body in the existing checked provider graph.
///
/// The caller's ordinary receipt still binds each actual source. This projection changes only extern visibility;
/// it never loads another artifact, invents a source route, or replaces an already selected macro.
pub fn provider_compilation_artifacts(
    artifacts: &OvenRustcArtifactManifest,
) -> OvenPlanResult<OvenRustcArtifactManifest> {
    let mut projected = artifacts.clone();
    let names = artifacts
        .entrypoint_externs
        .get(OVEN_PROVIDER_COMPILATION_KEY)
        .cloned()
        .unwrap_or_else(|| {
            artifacts
                .externs
                .iter()
                .map(|artifact| artifact.crate_name.clone())
                .collect()
        });
    if artifacts.schema_version == crate::rustc::OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION {
        let key = if artifacts.entrypoint_externs.contains_key(OVEN_PROVIDER_COMPILATION_KEY) {
            OVEN_PROVIDER_COMPILATION_KEY
        } else {
            "generated-root"
        };
        projected
            .entrypoint_dependency_search_paths
            .insert("generated-root".to_string(), artifacts.source_search_closure(key)?);
    }
    projected.entrypoint_externs.insert("generated-root".to_string(), names);
    projected.validate_shape(&artifacts.intent)?;
    Ok(projected)
}

/// Compose the already leased package entries after the consumer's actual intent is known.
pub fn compose_selected_packaged_provider_plan(
    selected: SelectedPackagedProviderPlans,
    candidates: &[PackagedProviderCandidate<'_>],
    consumer_receipt: &OvenReceipt,
) -> OvenPlanResult<Option<OvenDirectRustcPlanSelection>> {
    let SelectedPackagedProviderPlans {
        extensions: extension_selected,
        direct: direct_selected,
    } = selected;
    if extension_selected.is_empty() && direct_selected.is_empty() {
        return Ok(None);
    }
    for candidate in candidates {
        let dependency_key = candidate.dependency_key;
        if candidate.receipt.intent != consumer_receipt.intent {
            return Err(OvenPlanError::selection(format!(
                "Oven Alpha cannot compose pub::{dependency_key} package Loaf with this consumer: the sealed provider intent differs from the consumer intent; rebake the provider for the selected target, toolchain, profile, and feature set"
            )));
        }
    }
    if !extension_selected.is_empty() && !direct_selected.is_empty() {
        return Err(OvenPlanError::selection(
            "Oven Alpha cannot compose package Loafs that mix compiler-base extensions with self-contained direct plans; rebake the participating providers with one installed Incan release so their Rust ABI closure authority is uniform",
        ));
    }
    let composed = if !extension_selected.is_empty() {
        compose_packaged_provider_plan(extension_selected, &consumer_receipt.intent)?
    } else {
        compose_direct_packaged_provider_plan(direct_selected, &consumer_receipt.intent)?
    };
    Ok(Some(OvenDirectRustcPlanSelection::PackagedProvider(Box::new(composed))))
}

/// Build one direct-Rustc execution plan from every ABI-compatible public package Loaf.
///
/// Each input has already passed receipt selection and retains its lease.  Composition is strictly byte based:
/// duplicate artifact paths are accepted only when their digest is identical; duplicate public extern names or
/// registry package identities must also agree on their sealed artifact and feature facts.  Those are genuine ABI
/// conflicts, unlike merely having more than one public package.
pub fn compose_packaged_provider_plan(
    selected: Vec<(String, OvenPackagedLibraryLoafEntry, OvenProjectExtensionExecutionPlan)>,
    expected_intent: &oven_store::OvenBuildIntent,
) -> OvenPlanResult<OvenPackagedProviderExecutionPlan> {
    let mut selected = selected.into_iter();
    let (first_dependency, first_entry, first) = selected
        .next()
        .ok_or_else(|| OvenPlanError::selection("package Loaf composition requires at least one selected provider"))?;
    let OvenProjectExtensionExecutionPlan {
        base,
        extension,
        artifacts,
        ..
    } = first;
    if artifacts.intent != *expected_intent {
        return Err(OvenPlanError::selection(
            "Oven Alpha refuses package Loaf composition because the first provider intent differs from the consumer",
        ));
    }
    let base_identity = base.loaf_identity.clone();
    let base_artifacts = base.artifacts.clone();
    let mut inputs = vec![(first_dependency, first_entry.receipt, extension, artifacts)];
    for (dependency_key, entry, plan) in selected {
        if plan.base.loaf_identity != base_identity {
            return Err(OvenPlanError::selection(format!(
                "Oven Alpha cannot compose pub::{dependency_key}: its package Loaf requires compiler base `{}`, while the selected package collection requires `{base_identity}`; rebake the packages with one Incan release",
                plan.base.loaf_identity
            )));
        }
        if plan.artifacts.intent != *expected_intent {
            return Err(OvenPlanError::selection(format!(
                "Oven Alpha cannot compose pub::{dependency_key}: its sealed intent differs from this consumer; rebake it for the selected target, toolchain, profile, and feature set"
            )));
        }
        inputs.push((dependency_key, entry.receipt, plan.extension, plan.artifacts));
    }
    let manifest_inputs = inputs
        .iter()
        .map(|(dependency_key, _, _, artifacts)| (dependency_key.as_str(), artifacts))
        .collect::<Vec<_>>();
    let artifacts = merge_packaged_provider_artifact_manifests_with_release_base(
        &manifest_inputs,
        &base_artifacts,
        expected_intent,
    )?;
    let base_paths = artifacts.partition_against_base(&base_artifacts)?.base_paths;
    let base_fragment = artifacts.artifact_fragment(&base_paths)?;
    let base_supporting_artifacts = base_fragment.composition_artifacts()?;
    let base_inventory = base_artifacts.composition_artifacts()?;
    let mut owned_paths = base_supporting_artifacts
        .iter()
        .map(|artifact| artifact.relative_path.clone())
        .collect::<BTreeSet<_>>();
    let mut fragments = Vec::new();
    for (dependency_key, receipt, extension, provider_artifacts) in inputs {
        let partition = provider_artifacts.partition_against_base(&base_artifacts)?;
        let extension_fragment = provider_artifacts.artifact_fragment(&partition.extension_paths)?;
        let root_inventory = extension_fragment.composition_artifacts()?;
        let mut supporting_artifacts = root_inventory
            .iter()
            .filter(|artifact| owned_paths.insert(artifact.relative_path.clone()))
            .cloned()
            .collect::<Vec<_>>();
        supporting_artifacts.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        let retains_artifact_below = |search_path: &str| {
            supporting_artifacts
                .iter()
                .any(|artifact| Path::new(&artifact.relative_path).starts_with(Path::new(search_path)))
        };
        let root_dependency_search_paths = extension_fragment.dependency_search_paths.clone();
        let mut dependency_search_paths = extension_fragment
            .dependency_search_paths
            .into_iter()
            .filter(|path| retains_artifact_below(path))
            .collect::<Vec<_>>();
        dependency_search_paths.sort();
        dependency_search_paths.dedup();
        let mut native_search_paths = extension_fragment
            .native_search_paths
            .into_iter()
            .filter(|path| retains_artifact_below(path))
            .collect::<Vec<_>>();
        native_search_paths.sort();
        native_search_paths.dedup();
        fragments.push(OvenPackagedProviderFragment {
            root_inventory,
            root_dependency_search_paths,
            dependency_key,
            receipt,
            identity: extension.identity.clone(),
            extension,
            dependency_search_paths,
            native_search_paths,
            supporting_artifacts,
        });
    }
    let output_guard_root = fragments
        .first()
        .map(|fragment| fragment.extension.artifact_root.clone())
        .ok_or_else(|| OvenPlanError::selection("package Loaf composition lost its provider artifact root"))?;
    let mut composed = OvenExtensionPackagedProviderExecutionPlan {
        base,
        fragments,
        artifacts,
        artifact_plan: OvenRustcArtifactPlan {
            source_path_projection: None,
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        },
        registry_leaf_authority: None,
        vocab_artifact_root: None,
        output_guard_root,
    };
    let mut roots = vec![OvenTrustedRustcArtifactRoot {
        artifact_root: &composed.base.artifact_root,
        dependency_search_paths: &base_fragment.dependency_search_paths,
        native_search_paths: &base_fragment.native_search_paths,
        supporting_artifacts: &base_supporting_artifacts,
        root_inventory: Some(&base_inventory),
    }];
    roots.extend(
        composed
            .fragments
            .iter()
            .filter(|fragment| !fragment.supporting_artifacts.is_empty())
            .map(|fragment| OvenTrustedRustcArtifactRoot {
                artifact_root: &fragment.extension.artifact_root,
                dependency_search_paths: &fragment.dependency_search_paths,
                native_search_paths: &fragment.native_search_paths,
                supporting_artifacts: &fragment.supporting_artifacts,
                root_inventory: Some(&fragment.root_inventory),
            }),
    );
    let mut search_roots = vec![OvenTrustedRustcSearchRoot {
        artifact_root: &composed.base.artifact_root,
        dependency_search_paths: &base_artifacts.dependency_search_paths,
        root_inventory: &base_inventory,
    }];
    search_roots.extend(composed.fragments.iter().map(|fragment| OvenTrustedRustcSearchRoot {
        artifact_root: &fragment.extension.artifact_root,
        dependency_search_paths: &fragment.root_dependency_search_paths,
        root_inventory: &fragment.root_inventory,
    }));
    composed.artifact_plan = composed
        .artifacts
        .materialize_trusted_store_composed_with_search_roots(&roots, &search_roots, expected_intent)?;
    for fragment in &composed.fragments {
        if composed
            .artifact_plan
            .caller_owned_library_digests
            .insert(
                format!("package-loaf:{}:{}", fragment.dependency_key, fragment.identity),
                fragment.identity.clone(),
            )
            .is_some()
        {
            return Err(OvenPlanError::selection(format!(
                "Oven Alpha package Loaf composition found duplicate entry `{}` for pub::{}",
                fragment.identity, fragment.dependency_key
            )));
        }
    }
    let mut artifact_roots = base_supporting_artifacts
        .iter()
        .map(|artifact| (artifact.relative_path.as_str(), composed.base.artifact_root.clone()))
        .collect::<BTreeMap<_, _>>();
    for fragment in &composed.fragments {
        for artifact in &fragment.supporting_artifacts {
            artifact_roots.insert(
                artifact.relative_path.as_str(),
                fragment.extension.artifact_root.clone(),
            );
        }
    }
    let registry_leaf_entries = composed
        .artifacts
        .registry_leaves
        .iter()
        .map(|leaf| {
            let root = artifact_roots
                .get(leaf.artifact.relative_path.as_str())
                .ok_or_else(|| {
                    OvenPlanError::selection(format!(
                        "Oven Alpha package Loaf composition omitted registry leaf `{}` `{}`",
                        leaf.package, leaf.version
                    ))
                })?;
            Ok((root.clone(), leaf.clone()))
        })
        .collect::<OvenPlanResult<Vec<_>>>()?;
    composed.registry_leaf_authority =
        OvenRegistryLeafAuthority::from_composed_plan(registry_leaf_entries, &composed.artifact_plan);
    let base_vocab_paths = base_artifacts
        .vocab_auxiliary_targets
        .iter()
        .flat_map(|target| target.externs.iter().map(|artifact| artifact.relative_path.as_str()))
        .collect::<BTreeSet<_>>();
    if base_vocab_paths.iter().all(|path| base_paths.contains(*path)) {
        composed.vocab_artifact_root = Some(composed.base.artifact_root.clone());
    } else {
        return Err(OvenPlanError::selection(
            "Oven Alpha package Loaf composition found vocabulary support outside its exact compiler base",
        ));
    }
    Ok(OvenPackagedProviderExecutionPlan::Extensions(Box::new(composed)))
}

/// Compose independently baked self-contained public-provider closures.
///
/// An explicit provider bake may legitimately have no installed compiler Loaf to partition against. Its direct plan
/// consequently contains the complete sealed Rust closure. Consumers still must not resolve that closure again: this
/// compositor verifies every package entry by its receipt and immutable identity, accepts only byte-identical overlap,
/// and materializes the union directly from the separately leased package roots.
pub fn compose_direct_packaged_provider_plan(
    selected: Vec<(String, OvenPackagedLibraryLoafEntry, OvenStoredDirectRustcExecutionPlan)>,
    expected_intent: &oven_store::OvenBuildIntent,
) -> OvenPlanResult<OvenPackagedProviderExecutionPlan> {
    if selected.is_empty() {
        return Err(OvenPlanError::selection(
            "self-contained package Loaf composition requires at least one selected provider",
        ));
    }
    let manifest_inputs = selected
        .iter()
        .map(|(dependency_key, _, plan)| (dependency_key.as_str(), &plan.artifacts))
        .collect::<Vec<_>>();
    let artifacts = merge_packaged_provider_artifact_manifests(&manifest_inputs, expected_intent)?;
    let mut owned_paths = BTreeSet::new();
    let mut fragments = Vec::new();
    for (dependency_key, entry, plan) in selected {
        if plan.artifacts.intent != *expected_intent {
            return Err(OvenPlanError::selection(format!(
                "Oven Alpha cannot compose pub::{dependency_key}: its sealed direct-plan intent differs from this consumer; rebake it for the selected target, toolchain, profile, and feature set"
            )));
        }
        let root_inventory = plan.artifacts.composition_artifacts()?;
        let mut supporting_artifacts = root_inventory
            .iter()
            .filter(|artifact| owned_paths.insert(artifact.relative_path.clone()))
            .cloned()
            .collect::<Vec<_>>();
        supporting_artifacts.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        let retains_artifact_below = |search_path: &str| {
            supporting_artifacts
                .iter()
                .any(|artifact| Path::new(&artifact.relative_path).starts_with(Path::new(search_path)))
        };
        let mut dependency_search_paths = plan
            .artifacts
            .dependency_search_paths
            .iter()
            .filter(|path| retains_artifact_below(path))
            .cloned()
            .collect::<Vec<_>>();
        dependency_search_paths.sort();
        dependency_search_paths.dedup();
        let mut native_search_paths = plan
            .artifacts
            .native_search_paths
            .iter()
            .filter(|path| retains_artifact_below(path))
            .cloned()
            .collect::<Vec<_>>();
        native_search_paths.sort();
        native_search_paths.dedup();
        fragments.push(OvenPackagedDirectProviderFragment {
            root_inventory,
            dependency_key,
            receipt: entry.receipt,
            identity: plan.identity.clone(),
            plan,
            dependency_search_paths,
            native_search_paths,
            supporting_artifacts,
        });
    }
    let output_guard_root = fragments
        .first()
        .map(|fragment| fragment.plan.artifact_root.clone())
        .ok_or_else(|| OvenPlanError::selection("package Loaf composition lost its provider artifact root"))?;
    let mut composed = OvenDirectPackagedProviderExecutionPlan {
        fragments,
        artifacts,
        artifact_plan: OvenRustcArtifactPlan {
            source_path_projection: None,
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        },
        registry_leaf_authority: None,
        vocab_artifact_root: None,
        output_guard_root,
    };
    let roots = composed
        .fragments
        .iter()
        .filter(|fragment| !fragment.supporting_artifacts.is_empty())
        .map(|fragment| OvenTrustedRustcArtifactRoot {
            artifact_root: &fragment.plan.artifact_root,
            dependency_search_paths: &fragment.dependency_search_paths,
            native_search_paths: &fragment.native_search_paths,
            supporting_artifacts: &fragment.supporting_artifacts,
            root_inventory: Some(&fragment.root_inventory),
        })
        .collect::<Vec<_>>();
    let search_roots = composed
        .fragments
        .iter()
        .map(|fragment| OvenTrustedRustcSearchRoot {
            artifact_root: &fragment.plan.artifact_root,
            dependency_search_paths: &fragment.plan.artifacts.dependency_search_paths,
            root_inventory: &fragment.root_inventory,
        })
        .collect::<Vec<_>>();
    composed.artifact_plan = composed
        .artifacts
        .materialize_trusted_store_composed_with_search_roots(&roots, &search_roots, expected_intent)?;
    for fragment in &composed.fragments {
        if composed
            .artifact_plan
            .caller_owned_library_digests
            .insert(
                format!("package-loaf:{}:{}", fragment.dependency_key, fragment.identity),
                fragment.identity.clone(),
            )
            .is_some()
        {
            return Err(OvenPlanError::selection(format!(
                "Oven Alpha package Loaf composition found duplicate entry `{}` for pub::{}",
                fragment.identity, fragment.dependency_key
            )));
        }
    }
    let mut artifact_roots = BTreeMap::new();
    for fragment in &composed.fragments {
        for artifact in &fragment.supporting_artifacts {
            artifact_roots.insert(artifact.relative_path.as_str(), fragment.plan.artifact_root.clone());
        }
    }
    let registry_leaf_entries = composed
        .artifacts
        .registry_leaves
        .iter()
        .map(|leaf| {
            let root = artifact_roots
                .get(leaf.artifact.relative_path.as_str())
                .ok_or_else(|| {
                    OvenPlanError::selection(format!(
                        "Oven Alpha package Loaf composition omitted registry leaf `{}` `{}`",
                        leaf.package, leaf.version
                    ))
                })?;
            Ok((root.clone(), leaf.clone()))
        })
        .collect::<OvenPlanResult<Vec<_>>>()?;
    composed.registry_leaf_authority =
        OvenRegistryLeafAuthority::from_composed_plan(registry_leaf_entries, &composed.artifact_plan);
    let vocab_paths = composed
        .artifacts
        .vocab_auxiliary_targets
        .iter()
        .flat_map(|target| target.externs.iter().map(|artifact| artifact.relative_path.as_str()))
        .collect::<BTreeSet<_>>();
    if vocab_paths.is_empty() {
        composed.vocab_artifact_root = composed
            .fragments
            .first()
            .map(|fragment| fragment.plan.artifact_root.clone());
    } else if let Some(fragment) = composed.fragments.iter().find(|fragment| {
        let paths = fragment
            .supporting_artifacts
            .iter()
            .map(|artifact| artifact.relative_path.as_str())
            .collect::<BTreeSet<_>>();
        vocab_paths.iter().all(|path| paths.contains(path))
    }) {
        composed.vocab_artifact_root = Some(fragment.plan.artifact_root.clone());
    } else {
        return Err(OvenPlanError::selection(
            "Oven Alpha package Loaf composition split one compiler-owned vocabulary closure across self-contained provider Loafs",
        ));
    }
    Ok(OvenPackagedProviderExecutionPlan::Direct(Box::new(composed)))
}

/// Project an exact release-base Loaf into a package collection without replacing provider-owned ABI roots.
///
/// A project extension records only the direct externs its producer source used. A different consumer may use any
/// public root shipped by the same release, so package composition must retain the exact base's generated-root map
/// and artifact closure. When a provider already owns a same-named direct extern, that provider artifact remains the
/// consumer's direct root and the base variant becomes supporting metadata for crates compiled against it. The
/// ordinary compositor still rejects path, digest, registry, vocabulary, and compile-environment conflicts.
fn release_base_consumer_overlay(
    base: &OvenRustcArtifactManifest,
    providers: &[&OvenRustcArtifactManifest],
    compiler_runtime_crate_names: &BTreeSet<String>,
) -> OvenPlanResult<OvenRustcArtifactManifest> {
    base.validate_shape(&base.intent)?;
    let mut provider_extern_names = BTreeSet::new();
    for provider in providers {
        provider.validate_shape(&base.intent)?;
        provider.validate_release_cohort_from_base(base)?;
        provider_extern_names.extend(provider.externs.iter().map(|artifact| artifact.crate_name.as_str()));
    }
    let mut overlay = base.clone();
    if base.schema_version == 9 {
        overlay.schema_version = crate::rustc::OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION;
        for key in base.entrypoint_externs.keys() {
            overlay
                .entrypoint_dependency_search_paths
                .insert(key.clone(), base.source_search_closure(key)?);
        }
        if !overlay.entrypoint_externs.contains_key("generated-root") {
            overlay.entrypoint_externs.insert(
                "generated-root".to_string(),
                base.externs
                    .iter()
                    .map(|artifact| artifact.crate_name.clone())
                    .collect(),
            );
            overlay.entrypoint_dependency_search_paths.insert(
                "generated-root".to_string(),
                base.source_search_closure("generated-root")?,
            );
        }
    }
    let base_extern_paths = base
        .externs
        .iter()
        .map(|artifact| artifact.relative_path.as_str())
        .collect::<BTreeSet<_>>();
    overlay.supporting_artifacts = base
        .release_execution_artifacts()?
        .into_iter()
        .filter(|artifact| !base_extern_paths.contains(artifact.relative_path.as_str()))
        .collect();
    let mut retained_externs = Vec::new();
    for artifact in std::mem::take(&mut overlay.externs) {
        if compiler_runtime_crate_names.contains(&artifact.crate_name)
            && !provider_extern_names.contains(artifact.crate_name.as_str())
        {
            retained_externs.push(artifact);
        } else {
            overlay.supporting_artifacts.push(OvenRustcSupportingArtifact {
                relative_path: artifact.relative_path,
                digest: artifact.digest,
            });
        }
    }
    overlay.externs = retained_externs;
    for crate_names in overlay.entrypoint_externs.values_mut() {
        crate_names.retain(|crate_name| {
            compiler_runtime_crate_names.contains(crate_name) && !provider_extern_names.contains(crate_name.as_str())
        });
    }
    overlay.registry_leaves.clear();
    overlay.registry_sources.clear();
    overlay.compile_environment.clear();
    overlay
        .supporting_artifacts
        .sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    overlay.validate_shape(&base.intent)?;
    Ok(overlay)
}

/// Merge provider extensions with the exact release base and restore the base's portable consumer-root map.
pub fn merge_packaged_provider_artifact_manifests_with_release_base(
    providers: &[(&str, &OvenRustcArtifactManifest)],
    base: &OvenRustcArtifactManifest,
    expected_intent: &oven_store::OvenBuildIntent,
) -> OvenPlanResult<OvenRustcArtifactManifest> {
    let compiler_runtime_crate_names = base.compiler_runtime_crate_names()?;
    let base_overlay = release_base_consumer_overlay(
        base,
        &providers.iter().map(|(_, artifacts)| *artifacts).collect::<Vec<_>>(),
        &compiler_runtime_crate_names,
    )?;
    let mut inputs = providers.to_vec();
    inputs.push(("Incan release base", &base_overlay));
    let mut composed = merge_packaged_provider_artifact_manifests(&inputs, expected_intent)?;
    for (source_key, base_names) in &base.entrypoint_externs {
        composed
            .entrypoint_dependency_search_paths
            .entry(source_key.clone())
            .or_default()
            .merge(&base.source_search_closure(source_key)?);
        let names = composed.entrypoint_externs.entry(source_key.clone()).or_default();
        names.extend(
            base_names
                .iter()
                .filter(|crate_name| compiler_runtime_crate_names.contains(*crate_name))
                .cloned(),
        );
        names.sort();
        names.dedup();
    }
    composed.validate_shape(expected_intent)?;
    Ok(composed)
}

/// Merge the compatible artifact declarations of independently baked public package Loafs.
///
/// This is intentionally not a Cargo-style resolver.  The package publishers already resolved their independent
/// graphs.  Oven only accepts their union when every overlap is byte-identical and every public crate or registry
/// identity denotes one sealed ABI; otherwise it returns an actionable incompatibility instead of selecting an
/// arbitrary first match.
pub fn merge_packaged_provider_artifact_manifests(
    inputs: &[(&str, &OvenRustcArtifactManifest)],
    expected_intent: &oven_store::OvenBuildIntent,
) -> OvenPlanResult<OvenRustcArtifactManifest> {
    let (first_name, first) = inputs
        .first()
        .ok_or_else(|| OvenPlanError::selection("package Loaf manifest composition requires at least one provider"))?;
    first.validate_shape(expected_intent)?;
    let mut dependency_search_paths = BTreeSet::new();
    let mut native_search_paths = BTreeSet::new();
    let mut externs = BTreeMap::<String, OvenRustcArtifactExtern>::new();
    let mut artifact_digests = BTreeMap::<String, String>::new();
    let mut supporting_artifacts = BTreeMap::<String, OvenRustcSupportingArtifact>::new();
    let mut entrypoint_externs = BTreeMap::<String, BTreeSet<String>>::new();
    let mut role_keys = inputs
        .iter()
        .flat_map(|(_, manifest)| manifest.entrypoint_externs.keys().cloned())
        .collect::<BTreeSet<_>>();
    let unscoped = role_keys.is_empty();
    if unscoped {
        role_keys.insert("generated-root".to_string());
    }
    let mut entrypoint_search_paths = BTreeMap::<String, crate::rustc::OvenRustcSourceSearchClosure>::new();
    let mut registry_leaves = BTreeMap::<(String, String), OvenRustcRegistryLeaf>::new();
    let mut registry_sources = BTreeMap::<(String, String, String), OvenRustcRegistrySourcePackage>::new();
    let mut compile_environment = BTreeMap::<String, String>::new();
    let vocabulary = first.vocab_auxiliary_targets.clone();
    for (name, manifest) in inputs {
        manifest.validate_shape(expected_intent)?;
        if manifest.vocab_auxiliary_targets != vocabulary {
            return Err(OvenPlanError::selection(format!(
                "Oven Alpha cannot compose pub::{name} with pub::{first_name}: their compiler-owned vocabulary closures differ"
            )));
        }
        dependency_search_paths.extend(manifest.dependency_search_paths.iter().cloned());
        native_search_paths.extend(manifest.native_search_paths.iter().cloned());
        for (key, value) in &manifest.compile_environment {
            if let Some(existing) = compile_environment.insert(key.clone(), value.clone())
                && existing != *value
            {
                return Err(OvenPlanError::selection(format!(
                    "Oven Alpha cannot compose pub::{name}: compile environment `{key}` conflicts with another sealed package Loaf"
                )));
            }
        }
        for artifact in &manifest.externs {
            if let Some(existing) = externs.get(&artifact.crate_name)
                && existing != artifact
            {
                return Err(OvenPlanError::selection(format!(
                    "Oven Alpha cannot compose pub::{name}: direct Rust crate `{}` has incompatible sealed artifacts",
                    artifact.crate_name
                )));
            }
            if let Some(existing_digest) =
                artifact_digests.insert(artifact.relative_path.clone(), artifact.digest.clone())
                && existing_digest != artifact.digest
            {
                return Err(OvenPlanError::selection(format!(
                    "Oven Alpha cannot compose pub::{name}: artifact path `{}` has conflicting sealed bytes",
                    artifact.relative_path
                )));
            }
            supporting_artifacts.remove(&artifact.relative_path);
            externs.insert(artifact.crate_name.clone(), artifact.clone());
        }
        for artifact in &manifest.supporting_artifacts {
            if let Some(existing_digest) =
                artifact_digests.insert(artifact.relative_path.clone(), artifact.digest.clone())
                && existing_digest != artifact.digest
            {
                return Err(OvenPlanError::selection(format!(
                    "Oven Alpha cannot compose pub::{name}: artifact path `{}` has conflicting sealed bytes",
                    artifact.relative_path
                )));
            }
            if !externs
                .values()
                .any(|direct| direct.relative_path == artifact.relative_path)
            {
                supporting_artifacts.insert(artifact.relative_path.clone(), artifact.clone());
            }
        }
        for source_key in &role_keys {
            let contributor_key = if manifest.entrypoint_externs.contains_key(source_key) {
                source_key.as_str()
            } else {
                "generated-root"
            };
            entrypoint_search_paths
                .entry(source_key.clone())
                .or_default()
                .merge(&manifest.source_search_closure(contributor_key)?);
        }
        for (source_key, names) in &manifest.entrypoint_externs {
            entrypoint_externs
                .entry(source_key.clone())
                .or_default()
                .extend(names.iter().cloned());
        }
        for leaf in &manifest.registry_leaves {
            let key = (leaf.package.clone(), leaf.version.clone());
            if let Some(existing) = registry_leaves.get(&key)
                && existing != leaf
            {
                return Err(OvenPlanError::selection(format!(
                    "Oven Alpha cannot compose pub::{name}: registry package `{}` `{}` has incompatible sealed features or artifacts",
                    leaf.package, leaf.version
                )));
            }
            registry_leaves.insert(key, leaf.clone());
        }
        for source in &manifest.registry_sources {
            let key = (
                source.package.clone(),
                source.version.clone(),
                source.source.registry.clone(),
            );
            if let Some(existing) = registry_sources.get(&key)
                && existing != source
            {
                return Err(OvenPlanError::selection(format!(
                    "Oven Alpha cannot compose pub::{name}: registry source `{}` `{}` has incompatible sealed source or feature facts",
                    source.package, source.version
                )));
            }
            registry_sources.insert(key, source.clone());
        }
    }
    let mut vocabulary_artifacts = BTreeSet::<String>::new();
    for target in &vocabulary {
        for artifact in &target.externs {
            if let Some(existing_digest) =
                artifact_digests.insert(artifact.relative_path.clone(), artifact.digest.clone())
                && existing_digest != artifact.digest
            {
                return Err(OvenPlanError::selection(format!(
                    "Oven Alpha cannot compose package Loafs: vocabulary artifact path `{}` has conflicting sealed bytes",
                    artifact.relative_path
                )));
            }
            vocabulary_artifacts.insert(artifact.relative_path.clone());
        }
    }
    for relative_path in vocabulary_artifacts {
        if !externs.values().any(|artifact| artifact.relative_path == relative_path) {
            supporting_artifacts.remove(&relative_path);
            // The vocabulary role carries the physical artifact; retaining it a second time would violate the
            // immutable manifest's one-path rule.
        }
    }
    if unscoped {
        entrypoint_externs.insert("generated-root".to_string(), externs.keys().cloned().collect());
    }
    let composed = OvenRustcArtifactManifest {
        schema_version: crate::rustc::OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: expected_intent.clone(),
        dependency_search_paths: dependency_search_paths.into_iter().collect(),
        native_search_paths: native_search_paths.into_iter().collect(),
        externs: externs.into_values().collect(),
        entrypoint_dependency_search_paths: entrypoint_search_paths,
        entrypoint_externs: entrypoint_externs
            .into_iter()
            .map(|(key, names)| (key, names.into_iter().collect()))
            .collect(),
        registry_leaves: registry_leaves.into_values().collect(),
        registry_sources: registry_sources.into_values().collect(),
        compile_environment,
        vocab_auxiliary_targets: vocabulary,
        supporting_artifacts: supporting_artifacts.into_values().collect(),
    };
    composed.validate_shape(expected_intent)?;
    Ok(composed)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::super::selection::select_packaged_direct_rustc_execution_plan;
    use super::super::test_support::package_loaf_manifest;
    use super::*;
    use crate::rustc::{direct_rustc_source_extern_names, trusted_artifact_plan_for_source_evidence};
    use oven_store::store::{
        OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, OvenStoreLimits,
    };
    use oven_store::{OvenGeneratedProjectRequest, digest_bytes, receipt_generated_project};

    #[test]
    fn packaged_provider_source_projection_keeps_private_transitive_path_without_exposing_extern()
    -> Result<(), Box<dyn std::error::Error>> {
        let intent = oven_store::OvenBuildIntent {
            target: "aarch64-apple-darwin".to_string(),
            toolchain: "rustc fixture".to_string(),
            profile: "debug".to_string(),
            features: Vec::new(),
        };
        let mut artifacts = package_loaf_manifest(intent, "receiver_factory", "sha256:receiver-factory");
        artifacts.dependency_search_paths = vec![
            "runtime/deps".to_string(),
            "target/aarch64-apple-darwin/debug/deps".to_string(),
        ];
        artifacts.externs[0].relative_path = "runtime/deps/libincan_std_core.rlib".to_string();
        artifacts.externs[1].relative_path =
            "target/aarch64-apple-darwin/debug/deps/libreceiver_factory.rlib".to_string();
        artifacts
            .entrypoint_externs
            .insert("generated-root".to_string(), vec!["incan_std_core".to_string()]);
        artifacts.schema_version = 9;
        artifacts.entrypoint_dependency_search_paths.clear();
        let package_root = PathBuf::from("sealed-provider.loaf");
        let private_dependency_path = package_root.join("target/aarch64-apple-darwin/debug/deps");
        let plan = OvenRustcArtifactPlan {
            source_path_projection: None,
            dependency_search_paths: vec![package_root.join("runtime/deps"), private_dependency_path.clone()],
            native_search_paths: Vec::new(),
            externs: vec![
                (
                    "incan_std_core".to_string(),
                    package_root.join("runtime/deps/libincan_std_core.rlib"),
                ),
                (
                    "receiver_factory".to_string(),
                    private_dependency_path.join("libreceiver_factory.rlib"),
                ),
            ],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };

        let mut projected = trusted_artifact_plan_for_source_evidence(&plan, &artifacts, "generated-root")?;
        assert!(!projected.dependency_search_paths.contains(&private_dependency_path));
        retain_packaged_provider_fragment_dependency_search_paths(
            &mut projected,
            [(package_root.as_path(), &artifacts.dependency_search_paths[1..])],
        );

        assert!(projected.dependency_search_paths.contains(&private_dependency_path));
        assert_eq!(
            projected
                .externs
                .iter()
                .map(|(crate_name, _)| crate_name.as_str())
                .collect::<Vec<_>>(),
            vec!["incan_std_core"]
        );
        Ok(())
    }

    struct DuplicateSearchFixture {
        _root: tempfile::TempDir,
        store: OvenStore,
        receipts: Vec<OvenReceipt>,
        identities: Vec<String>,
    }

    fn duplicate_search_fixture() -> Result<DuplicateSearchFixture, Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let store = OvenStore::new(
            root.path().join("store"),
            OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000),
        );
        let mut receipts = Vec::new();
        let mut identities = Vec::new();
        for (label, legacy) in [("a", true), ("b", false)] {
            let source = root.path().join(format!("{label}.rs"));
            fs::write(&source, format!("pub fn marker_{label}() {{}}\n"))?;
            let receipt = receipt_generated_project(
                &OvenGeneratedProjectRequest::new(
                    root.path(),
                    label,
                    "0.1.0",
                    "aarch64-apple-darwin",
                    "rustc fixture",
                    "debug",
                    Vec::new(),
                )
                .with_generated_source("generated-root", &source),
            )?;
            let mut manifest = package_loaf_manifest(receipt.intent.clone(), "common", "unused placeholder");
            manifest.externs.clear();
            manifest.entrypoint_externs.clear();
            manifest.entrypoint_dependency_search_paths.clear();
            manifest.dependency_search_paths = if legacy {
                vec!["host".to_string(), "target".to_string()]
            } else {
                vec!["host".to_string()]
            };
            let mut members = vec![("common", "host/libcommon.rlib", b"same common native bytes".as_slice())];
            if legacy {
                members.extend([
                    ("runtime", "target/libruntime.rlib", b"runtime".as_slice()),
                    (
                        "private_helper",
                        "host/libprivate_helper.rlib",
                        b"private helper".as_slice(),
                    ),
                ]);
            }
            let mut files = Vec::new();
            for (name, relative, bytes) in members {
                let input = root.path().join(label).join(relative);
                fs::create_dir_all(input.parent().ok_or("fixture member has no parent")?)?;
                fs::write(&input, bytes)?;
                manifest.externs.push(OvenRustcArtifactExtern {
                    crate_name: name.to_string(),
                    relative_path: relative.to_string(),
                    digest: digest_bytes(bytes),
                });
                files.push(OvenArtifactMaterializedFile {
                    source_path: input,
                    relative_path: relative.to_string(),
                });
            }
            manifest.entrypoint_externs.insert(
                "generated-root".to_string(),
                vec![if legacy { "runtime" } else { "common" }.to_string()],
            );
            manifest.entrypoint_externs.insert(
                "runtime-only".to_string(),
                if legacy {
                    vec!["runtime".to_string()]
                } else {
                    Vec::new()
                },
            );
            if legacy {
                manifest.schema_version = 9;
            } else {
                manifest.entrypoint_dependency_search_paths.insert(
                    "generated-root".to_string(),
                    manifest.capture_source_search_closure(&manifest.dependency_search_paths)?,
                );
                manifest
                    .entrypoint_dependency_search_paths
                    .insert("runtime-only".to_string(), manifest.capture_source_search_closure(&[])?);
            }
            manifest.validate_shape(&receipt.intent)?;
            let stored = store.publish(&OvenArtifactPublishRequest {
                receipt: receipt.clone(),
                domain: "duplicate-search-fixture".to_string(),
                kind: OvenArtifactKind::DirectRustcPlan,
                payload: serde_json::to_vec(&manifest)?,
                materialized_files: files,
            })?;
            // Each original contributor is admitted independently through the real selected-store reader.
            let selected = select_packaged_direct_rustc_execution_plan(&store, &receipt, &stored.identity)?
                .ok_or("fixture publication was not admitted")?;
            trusted_artifact_plan_for_source_evidence(&selected.artifact_plan, &selected.artifacts, "generated-root")?;
            receipts.push(receipt);
            identities.push(stored.identity);
        }
        Ok(DuplicateSearchFixture {
            _root: root,
            store,
            receipts,
            identities,
        })
    }

    type DuplicateSearchInput = (String, OvenPackagedLibraryLoafEntry, OvenStoredDirectRustcExecutionPlan);

    fn duplicate_search_input(
        fixture: &DuplicateSearchFixture,
        index: usize,
    ) -> Result<DuplicateSearchInput, Box<dyn std::error::Error>> {
        let receipt = fixture.receipts[index].clone();
        let identity = fixture.identities[index].clone();
        let selected = select_packaged_direct_rustc_execution_plan(&fixture.store, &receipt, &identity)?
            .ok_or("lost admitted fixture")?;
        Ok((
            format!("input_{index}"),
            OvenPackagedLibraryLoafEntry {
                receipt,
                identity,
                kind: OvenArtifactKind::DirectRustcPlan,
                base_loaf_identity: None,
            },
            selected,
        ))
    }

    #[test]
    fn direct_package_search_roles_reuse_a_clean_duplicate_in_either_contribution_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = duplicate_search_fixture()?;
        let a = duplicate_search_input(&fixture, 0)?;
        let b = duplicate_search_input(&fixture, 1)?;
        let a_root = a.2.artifact_root.clone();
        let b_root = b.2.artifact_root.clone();
        let logical = merge_packaged_provider_artifact_manifests(
            &[("a", &a.2.artifacts), ("b", &b.2.artifacts)],
            &fixture.receipts[0].intent,
        )?;
        let reversed_logical = merge_packaged_provider_artifact_manifests(
            &[("b", &b.2.artifacts), ("a", &a.2.artifacts)],
            &fixture.receipts[0].intent,
        )?;
        assert_eq!(logical, reversed_logical);
        let before = [
            fs::read(a_root.join("host/libcommon.rlib"))?,
            fs::read(a_root.join("host/libprivate_helper.rlib"))?,
            fs::read(a_root.join("target/libruntime.rlib"))?,
            fs::read(b_root.join("host/libcommon.rlib"))?,
        ];
        // Exercise the clean-first control before the duplicate-only fragment case.
        for order in [[1, 0], [0, 1]] {
            let inputs = order
                .into_iter()
                .map(|index| duplicate_search_input(&fixture, index))
                .collect::<Result<Vec<_>, _>>()?;
            let composed = compose_direct_packaged_provider_plan(inputs, &fixture.receipts[0].intent)
                .map_err(|error| format!("contribution order {order:?}: {error}"))?;
            assert_eq!(composed.artifacts(), &logical);
            // Isolate the compositor's validated source binding before the separate caller-fragment grants.
            let projected = trusted_artifact_plan_for_source_evidence(
                composed.artifact_plan(),
                composed.artifacts(),
                "generated-root",
            )?;
            assert!(projected.dependency_search_paths.contains(&b_root.join("host")));
            assert!(!projected.dependency_search_paths.contains(&a_root.join("host")));
            assert!(
                projected
                    .source_path_projection
                    .as_ref()
                    .ok_or("missing source binding")?
                    .declared
                    .contains(&b_root.join("host"))
            );
            assert_eq!(
                projected,
                trusted_artifact_plan_for_source_evidence(&projected, composed.artifacts(), "generated-root")?
            );
            let runtime_only =
                trusted_artifact_plan_for_source_evidence(&projected, composed.artifacts(), "runtime-only")?;
            assert!(runtime_only.dependency_search_paths.contains(&a_root.join("target")));
            assert!(!runtime_only.dependency_search_paths.contains(&b_root.join("host")));
            assert_eq!(
                projected
                    .externs
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<BTreeSet<_>>(),
                BTreeSet::from(["common", "runtime"])
            );
            assert_eq!(
                before,
                [
                    fs::read(a_root.join("host/libcommon.rlib"))?,
                    fs::read(a_root.join("host/libprivate_helper.rlib"))?,
                    fs::read(a_root.join("target/libruntime.rlib"))?,
                    fs::read(b_root.join("host/libcommon.rlib"))?,
                ]
            );
            eprintln!("admitted composition order {order:?}; exact source roles and native bytes preserved");
        }
        Ok(())
    }

    #[test]
    fn direct_package_search_roles_still_refuse_without_a_clean_admitted_directory()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = duplicate_search_fixture()?;
        let a = duplicate_search_input(&fixture, 0)?;
        let b = duplicate_search_input(&fixture, 1)?;
        let logical = merge_packaged_provider_artifact_manifests(
            &[("a", &a.2.artifacts), ("b", &b.2.artifacts)],
            &fixture.receipts[0].intent,
        )?;
        let a_inventory = a.2.artifacts.composition_artifacts()?;
        let roots = [OvenTrustedRustcArtifactRoot {
            artifact_root: &a.2.artifact_root,
            dependency_search_paths: &a.2.artifacts.dependency_search_paths,
            native_search_paths: &[],
            supporting_artifacts: &a_inventory,
            root_inventory: Some(&a_inventory),
        }];
        let error = logical
            .materialize_trusted_store_composed(&roots, &fixture.receipts[0].intent)
            .err()
            .ok_or("an excluded co-resident helper must still refuse without a clean admitted alternative")?;
        // The refusal must name the artifact that disqualified the directory, not merely report that one exists.
        let message = error.to_string();
        assert!(message.contains("never selected"), "{message}");
        assert!(message.contains("cannot isolate selected member"), "{message}");
        Ok(())
    }

    #[test]
    fn direct_package_search_roles_reject_incomplete_conflicting_or_escaped_candidates()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = duplicate_search_fixture()?;
        let a = duplicate_search_input(&fixture, 0)?;
        let b = duplicate_search_input(&fixture, 1)?;
        let logical = merge_packaged_provider_artifact_manifests(
            &[("a", &a.2.artifacts), ("b", &b.2.artifacts)],
            &fixture.receipts[0].intent,
        )?;
        let a_inventory = a.2.artifacts.composition_artifacts()?;
        let b_inventory = b.2.artifacts.composition_artifacts()?;
        let roots = [OvenTrustedRustcArtifactRoot {
            artifact_root: &a.2.artifact_root,
            dependency_search_paths: &a.2.artifacts.dependency_search_paths,
            native_search_paths: &[],
            supporting_artifacts: &a_inventory,
            root_inventory: Some(&a_inventory),
        }];
        let candidate = OvenTrustedRustcSearchRoot {
            artifact_root: &b.2.artifact_root,
            dependency_search_paths: &b.2.artifacts.dependency_search_paths,
            root_inventory: &b_inventory,
        };
        logical.materialize_trusted_store_composed_with_search_roots(
            &roots,
            std::slice::from_ref(&candidate),
            &fixture.receipts[0].intent,
        )?;
        for (inventory, message) in [
            (Vec::new(), "manifest-recorded"),
            (
                vec![OvenRustcSupportingArtifact {
                    relative_path: "host/libcommon.rlib".to_string(),
                    digest: digest_bytes(b"different bytes"),
                }],
                "conflicts",
            ),
        ] {
            let error = logical
                .materialize_trusted_store_composed_with_search_roots(
                    &roots,
                    &[OvenTrustedRustcSearchRoot {
                        root_inventory: &inventory,
                        ..candidate
                    }],
                    &fixture.receipts[0].intent,
                )
                .err()
                .ok_or("invalid candidate was accepted")?;
            assert!(error.to_string().contains(message), "{error}");
        }
        let escaped = vec!["../outside".to_string()];
        assert!(
            logical
                .materialize_trusted_store_composed_with_search_roots(
                    &roots,
                    &[OvenTrustedRustcSearchRoot {
                        dependency_search_paths: &escaped,
                        ..candidate
                    }],
                    &fixture.receipts[0].intent,
                )
                .is_err()
        );
        // Another admitted root cannot replace the canonical coverage or supply absent canonical inventory.
        assert!(
            logical
                .materialize_trusted_store_composed_with_search_roots(
                    &[],
                    std::slice::from_ref(&candidate),
                    &fixture.receipts[0].intent,
                )
                .is_err()
        );
        let mut missing_inventory = roots;
        missing_inventory[0].root_inventory = None;
        assert!(
            logical
                .materialize_trusted_store_composed_with_search_roots(
                    &missing_inventory,
                    std::slice::from_ref(&candidate),
                    &fixture.receipts[0].intent,
                )
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn package_loaf_composition_unifies_compatible_provider_closures() -> Result<(), Box<dyn std::error::Error>> {
        let intent = oven_store::OvenBuildIntent {
            target: "aarch64-apple-darwin".to_string(),
            toolchain: "rustc fixture".to_string(),
            profile: "debug".to_string(),
            features: Vec::new(),
        };
        let incql = package_loaf_manifest(intent.clone(), "incql", "sha256:incql");
        let analytics = package_loaf_manifest(intent.clone(), "analytics", "sha256:analytics");

        let composed =
            merge_packaged_provider_artifact_manifests(&[("incql", &incql), ("analytics", &analytics)], &intent)?;

        assert_eq!(
            composed
                .externs
                .iter()
                .map(|artifact| artifact.crate_name.as_str())
                .collect::<Vec<_>>(),
            vec!["analytics", "incan_std_core", "incql"]
        );
        assert_eq!(
            composed.entrypoint_externs.get("generated-root"),
            Some(&vec![
                "analytics".to_string(),
                "incan_std_core".to_string(),
                "incql".to_string()
            ])
        );
        Ok(())
    }

    #[test]
    fn mixed_package_search_roles_preserve_original_legacy_projection_without_republication()
    -> Result<(), Box<dyn std::error::Error>> {
        let intent = oven_store::OvenBuildIntent {
            target: "aarch64-apple-darwin".to_string(),
            toolchain: "rustc fixture".to_string(),
            profile: "debug".to_string(),
            features: Vec::new(),
        };
        let mut legacy = package_loaf_manifest(intent.clone(), "legacy_provider", "sha256:legacy");
        legacy.schema_version = 9;
        legacy.entrypoint_dependency_search_paths.clear();
        legacy.dependency_search_paths.push("host".to_string());
        legacy.externs.push(OvenRustcArtifactExtern {
            crate_name: "private_helper".to_string(),
            relative_path: "host/libprivate_helper.rlib".to_string(),
            digest: "sha256:private".to_string(),
        });
        let original = legacy.source_search_closure("generated-root")?;
        let mut current = package_loaf_manifest(intent.clone(), "current_provider", "sha256:current");
        current.dependency_search_paths = vec!["host".to_string()];
        current
            .externs
            .iter_mut()
            .find(|artifact| artifact.crate_name == "current_provider")
            .ok_or("current provider missing")?
            .relative_path = "host/libcurrent_provider.rlib".to_string();
        current.entrypoint_dependency_search_paths.insert(
            "generated-root".to_string(),
            current.capture_source_search_closure(&current.dependency_search_paths)?,
        );
        let composed =
            merge_packaged_provider_artifact_manifests(&[("legacy", &legacy), ("current", &current)], &intent)?;
        let closure = composed
            .entrypoint_dependency_search_paths
            .get("generated-root")
            .ok_or("composed role missing")?;
        assert_eq!(closure.legacy_projections, original.legacy_projections);
        assert_eq!(
            closure
                .publisher_paths
                .iter()
                .map(|directory| directory.relative_path.as_str())
                .collect::<Vec<_>>(),
            vec!["host"]
        );
        assert!(
            closure
                .directories()
                .filter(|directory| directory.relative_path == "host")
                .flat_map(|directory| &directory.artifacts)
                .all(|artifact| artifact.relative_path == "host/libcurrent_provider.rlib")
        );
        let aliased =
            merge_packaged_provider_artifact_manifests(&[("renamed_a", &legacy), ("renamed_b", &current)], &intent)?;
        assert_eq!(aliased, composed);
        let forwarded = provider_compilation_artifacts(&composed)?;
        assert_eq!(forwarded.entrypoint_dependency_search_paths["generated-root"], *closure);
        let third = package_loaf_manifest(intent.clone(), "third_provider", "sha256:third");
        let nested =
            merge_packaged_provider_artifact_manifests(&[("composed", &composed), ("third", &third)], &intent)?;
        assert_eq!(
            nested.entrypoint_dependency_search_paths["generated-root"].legacy_projections,
            original.legacy_projections
        );
        assert_eq!(legacy.schema_version, 9);
        assert!(legacy.entrypoint_dependency_search_paths.is_empty());
        Ok(())
    }

    #[test]
    fn package_loaf_collection_exposes_missing_roots_from_one_release_cohort() -> Result<(), Box<dyn std::error::Error>>
    {
        let intent = oven_store::OvenBuildIntent {
            target: "aarch64-apple-darwin".to_string(),
            toolchain: "rustc fixture".to_string(),
            profile: "debug".to_string(),
            features: Vec::new(),
        };
        let mut base = package_loaf_manifest(intent.clone(), "incan_stdlib_system", "sha256:base-system");
        base.vocab_auxiliary_targets = vec![
            crate::rustc::OvenRustcAuxiliaryTarget {
                target: intent.target.clone(),
                dependency_search_paths: vec!["compiler-support/host/deps".to_string()],
                externs: vec![OvenRustcArtifactExtern {
                    crate_name: "incan_vocab".to_string(),
                    relative_path: "compiler-support/host/deps/libincan_vocab-host.rlib".to_string(),
                    digest: "sha256:incan-vocab-host".to_string(),
                }],
            },
            crate::rustc::OvenRustcAuxiliaryTarget {
                target: "wasm32-wasip1".to_string(),
                dependency_search_paths: vec!["compiler-support/wasm/deps".to_string()],
                externs: vec![OvenRustcArtifactExtern {
                    crate_name: "incan_vocab".to_string(),
                    relative_path: "compiler-support/wasm/deps/libincan_vocab-wasm.rlib".to_string(),
                    digest: "sha256:incan-vocab-wasm".to_string(),
                }],
            },
        ];
        base.externs.push(OvenRustcArtifactExtern {
            crate_name: "windows_sys".to_string(),
            relative_path: "artifacts/deps/libwindows_sys-base.rlib".to_string(),
            digest: "sha256:base-windows-sys".to_string(),
        });
        base.externs.push(OvenRustcArtifactExtern {
            crate_name: "incan_partner".to_string(),
            relative_path: "artifacts/deps/libpartner_alias-base.rlib".to_string(),
            digest: "sha256:base-partner-alias".to_string(),
        });
        base.entrypoint_externs
            .get_mut("generated-root")
            .ok_or("base fixture omitted generated-root externs")?
            .push("windows_sys".to_string());
        base.entrypoint_externs
            .get_mut("generated-root")
            .ok_or("base fixture omitted generated-root externs")?
            .push("incan_partner".to_string());
        base.registry_sources = vec![OvenRustcRegistrySourcePackage {
            package: "windows-sys".to_string(),
            version: "0.61.2".to_string(),
            features: vec!["Win32".to_string()],
            source: crate::rustc::OvenRustcRegistrySource {
                registry: "registry+https://example.invalid/index".to_string(),
                checksum: "windows-sys-checksum".to_string(),
                relative_root: "registry-sources/windows-sys".to_string(),
                digest: "sha256:windows-sys-source".to_string(),
            },
        }];
        base.supporting_artifacts.push(OvenRustcSupportingArtifact {
            relative_path: "registry-sources/windows-sys/Cargo.toml".to_string(),
            digest: "sha256:windows-sys-cargo-toml".to_string(),
        });
        base.supporting_artifacts.push(OvenRustcSupportingArtifact {
            relative_path: "registry-sources/base-only/Cargo.toml".to_string(),
            digest: "sha256:base-only-source".to_string(),
        });
        let mut provider = package_loaf_manifest(intent.clone(), "analytics", "sha256:analytics");
        provider.vocab_auxiliary_targets = base.vocab_auxiliary_targets.clone();
        provider.registry_sources = vec![OvenRustcRegistrySourcePackage {
            package: "windows-sys".to_string(),
            version: "0.61.2".to_string(),
            features: vec!["Win32".to_string(), "Win32_Foundation".to_string()],
            source: crate::rustc::OvenRustcRegistrySource {
                registry: "registry+https://example.invalid/index".to_string(),
                checksum: "windows-sys-checksum".to_string(),
                relative_root: "registry-sources/windows-sys".to_string(),
                digest: "sha256:windows-sys-source".to_string(),
            },
        }];
        provider.supporting_artifacts.push(OvenRustcSupportingArtifact {
            relative_path: "registry-sources/windows-sys/Cargo.toml".to_string(),
            digest: "sha256:windows-sys-cargo-toml".to_string(),
        });
        let mut divergent_provider = provider.clone();
        divergent_provider.externs[0] = OvenRustcArtifactExtern {
            crate_name: "incan_std_core".to_string(),
            relative_path: "provider/deps/libincan_std_core-provider.rlib".to_string(),
            digest: "sha256:provider-stdlib".to_string(),
        };
        divergent_provider
            .dependency_search_paths
            .push("provider/deps".to_string());
        let divergent = merge_packaged_provider_artifact_manifests_with_release_base(
            &[("analytics", &divergent_provider)],
            &base,
            &intent,
        );
        assert!(
            divergent.is_err(),
            "a provider that did not inherit its exact release runtime must fail closed"
        );
        let provider = provider.with_release_cohort_from_base(&base, &BTreeSet::new())?;

        let composed =
            merge_packaged_provider_artifact_manifests_with_release_base(&[("analytics", &provider)], &base, &intent)?;

        assert!(composed.externs.iter().any(|artifact| {
            artifact.crate_name == "incan_std_core"
                && artifact.relative_path == "artifacts/deps/libincan_std_core-shared.rlib"
        }));
        assert!(
            composed
                .externs
                .iter()
                .any(|artifact| artifact.crate_name == "incan_stdlib_system")
        );
        assert!(
            composed
                .supporting_artifacts
                .iter()
                .all(|artifact| { artifact.relative_path != "artifacts/deps/libincan_std_core-shared.rlib" })
        );
        assert!(
            composed
                .supporting_artifacts
                .iter()
                .any(|artifact| artifact.relative_path == "artifacts/deps/libwindows_sys-base.rlib")
        );
        assert!(
            composed
                .supporting_artifacts
                .iter()
                .any(|artifact| artifact.relative_path == "artifacts/deps/libpartner_alias-base.rlib"),
            "an `incan_*` alias is not release-owned unless its sealed artifact belongs to the runtime family"
        );
        assert!(
            composed
                .supporting_artifacts
                .iter()
                .all(|artifact| { artifact.relative_path != "registry-sources/base-only/Cargo.toml" })
        );
        assert_eq!(composed.vocab_auxiliary_targets, base.vocab_auxiliary_targets);
        assert_eq!(composed.registry_sources, provider.registry_sources);
        assert_eq!(
            direct_rustc_source_extern_names(&composed, "generated-root")?,
            BTreeSet::from([
                "analytics".to_string(),
                "incan_std_core".to_string(),
                "incan_stdlib_system".to_string(),
            ])
        );
        assert!(!direct_rustc_source_extern_names(&composed, "generated-root")?.contains("incan_vocab"));
        assert!(!direct_rustc_source_extern_names(&composed, "generated-root")?.contains("windows_sys"));
        assert!(!direct_rustc_source_extern_names(&composed, "generated-root")?.contains("incan_partner"));
        Ok(())
    }

    #[test]
    fn package_loaf_composition_rejects_conflicting_public_crate_identity() -> Result<(), Box<dyn std::error::Error>> {
        let intent = oven_store::OvenBuildIntent {
            target: "aarch64-apple-darwin".to_string(),
            toolchain: "rustc fixture".to_string(),
            profile: "debug".to_string(),
            features: Vec::new(),
        };
        let first = package_loaf_manifest(intent.clone(), "shared_provider", "sha256:first");
        let second = package_loaf_manifest(intent.clone(), "shared_provider", "sha256:second");

        let result = merge_packaged_provider_artifact_manifests(&[("first", &first), ("second", &second)], &intent);
        let Err(error) = result else {
            return Err("distinct sealed public crate artifacts must not be composed".into());
        };

        assert!(error.to_string().contains("direct Rust crate `shared_provider`"));
        Ok(())
    }
}
