//! The authority a selected plan runs under: compiler-owned roots, rematerialized caller-owned closures, registry
//! leaf authorities, and the checks that a plan covers every declared Rust library.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::{env, fs, io};

use crate::backend::ProjectGenerator;
use crate::build::caller_owned::{
    caller_owned_library_dependencies_without_public_provider_edges, caller_owned_library_edition,
    caller_owned_library_is_proc_macro, caller_owned_library_receipt, caller_owned_library_rust_dependencies,
    deduplicate_caller_owned_libraries_prefer_extern, first_unselected_private_provider_edge,
    load_receipted_public_provider_dependency,
};
use crate::build::plan_selection::{registry_leaf_authority_for_plan_selection, select_published_project_plan};
use crate::build::provider_compilation::{
    caller_owned_library_dependencies_for_compilation,
    caller_owned_library_dependencies_missing_from_selected_plan_with_owned_roots, plan_with_provider_compilation_role,
};
use crate::build::source_authority::project_bake_receipt_path;
use crate::build::{OvenBakeProjectTarget, OvenProjectBakeAuthorityContext, OvenToolchainMaterialization};
use crate::error::{CliError, CliResult, oven_plan_error, oven_rustc_error};
use incan_frontend::library_manifest::{LibraryManifest, ProviderDependencyKind, digest_provider_artifact};
use incan_frontend::library_manifest_index::{LibraryArtifactKind, LibraryArtifactMetadata, dependency_project_root};
use incan_provider::ProviderPlan;
use incan_provider::dependency_resolver::ResolvedDependencies;
use oven_model::manifest::{DependencySource, DependencySpec};
use oven_rustc::plan::composition::provider_compilation_artifacts;
use oven_rustc::rustc::{
    OvenCallerOwnedRustcLibrary, OvenRegistryLeafAuthority, OvenRustcArtifactManifest, OvenRustcArtifactPlan,
    OvenSelectedPathRustcAuthority, OvenTrustedDirectRustcTargetRequest, attach_caller_owned_rustc_libraries,
    bake_trusted_direct_rustc_library, bake_trusted_direct_rustc_proc_macro,
    materialize_declared_rust_libraries_with_selected_path_authority, trusted_artifact_plan_for_source_evidence,
    validate_selected_sealed_registry_leaf,
};
use oven_store::store::OvenStore;

/// Return the immutable roots supplied by the compiler-suite scheduler.
fn compiler_suite_owned_roots() -> Vec<PathBuf> {
    if env::var_os("INCAN_INTERNAL_OVEN_LOAF_EXECUTION").is_none_or(|value| value != "1") {
        return Vec::new();
    }
    [
        "INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT",
        "INCAN_INTERNAL_OVEN_RUNTIME_ROOT",
        "INCAN_INTERNAL_SDK_PROVIDER_STORE",
    ]
    .into_iter()
    .filter_map(env::var_os)
    .filter(|value| !value.is_empty())
    .map(PathBuf::from)
    .filter(|path| path.is_dir())
    .filter_map(|path| fs::canonicalize(path).ok())
    .collect()
}

/// Return compiler-owned path roots that may pair with an exact selected plan extern.
///
/// Besides a scheduler's sealed data roots, a normal command may reuse an active toolchain crate only when that
/// exact crate is exposed by its receipt-selected plan. Project paths and lookalike crates remain caller-owned.
fn compiler_owned_roots(artifact_plan: &OvenRustcArtifactPlan) -> Vec<PathBuf> {
    let mut roots = compiler_suite_owned_roots();
    for (crate_name, _) in &artifact_plan.externs {
        let candidate = oven_model::toolchain_layout::resolve_toolchain_crate_path(crate_name);
        if candidate.join("Cargo.toml").is_file()
            && let Ok(canonical) = fs::canonicalize(candidate)
        {
            roots.push(canonical);
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

/// Extend compiler-source authority with receipt-selected SDK/provider artifact roots.
///
/// A normal command may receive an SDK component's generated Cargo projection as a path dependency. That source
/// root is not necessarily one of the compiler crate directories (for example, `incan_stdlib_core` is a sealed SDK
/// component, not `crates/incan_stdlib_core`). A compiled provider can also retain a historical physical SDK path;
/// that path remains compiler-owned only when the checked provider plan has already rebound it to an equivalent active
/// SDK artifact *and* the selected direct-Rustc plan exposes its exact crate name. Project `pub::` artifacts never
/// meet either condition and remain caller-owned.
pub fn compiler_owned_roots_with_provider_plan(
    artifact_plan: &OvenRustcArtifactPlan,
    provider_plan: Option<&ProviderPlan>,
) -> Vec<PathBuf> {
    let mut roots = compiler_owned_roots(artifact_plan);
    let selected_externs = artifact_plan
        .externs
        .iter()
        .map(|(crate_name, _)| crate_name.replace('-', "_"))
        .collect::<BTreeSet<_>>();
    if let Some(provider_plan) = provider_plan {
        for provider in provider_plan.active_records().filter(|provider| {
            !matches!(
                provider.authority,
                incan_provider::NamespaceAuthority::ProjectDependency { .. }
            )
        }) {
            let Some(artifact) = provider.artifact.as_ref() else {
                continue;
            };
            let names = [
                artifact.dependency_key.replace('-', "_"),
                artifact.manifest_name.replace('-', "_"),
                provider.identity.name.replace('-', "_"),
            ];
            if names.iter().any(|name| selected_externs.contains(name))
                && let Ok(root) = fs::canonicalize(&artifact.crate_root)
            {
                roots.push(root);
            }
        }
        for rebinding in provider_plan.sdk_dependency_rebindings() {
            let names = [
                rebinding.provider_name.replace('-', "_"),
                rebinding.dependency_key.replace('-', "_"),
            ];
            if names.iter().any(|name| selected_externs.contains(name))
                && let Ok(root) = fs::canonicalize(&rebinding.source_crate_root)
            {
                // The provider plan has checked the frozen private edge against the active SDK's semantic identity.
                // This root is only a legacy coordinate: direct Rustc still consumes the selected sealed extern.
                roots.push(root);
            }
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

/// Build the narrow selected-path authority for compiler-owned dependencies.
pub fn compiler_selected_path_authority(
    artifact_plan: &OvenRustcArtifactPlan,
    provider_plan: Option<&ProviderPlan>,
) -> Option<OvenSelectedPathRustcAuthority> {
    let owned_roots = compiler_owned_roots_with_provider_plan(artifact_plan, provider_plan);
    (!owned_roots.is_empty()).then(|| OvenSelectedPathRustcAuthority::new(&owned_roots, artifact_plan))
}

/// Identify a generated compiler-runtime path only when the selected plan owns the same crate name.
///
/// The roots are compiler-owned and the plan must expose the same crate name. A lookalike path outside those roots is
/// still a caller package and must stay explicit, even if it uses the same crate name.
pub fn is_selected_compiler_runtime_path_dependency(
    dependency: &DependencySpec,
    selected_externs: &BTreeSet<&str>,
    owned_roots: &[PathBuf],
) -> bool {
    let DependencySource::Path { path } = &dependency.source else {
        return false;
    };
    let normalized_name = dependency.crate_name.replace('-', "_");
    selected_externs.contains(normalized_name.as_str())
        && fs::canonicalize(path)
            .ok()
            .is_some_and(|path| owned_roots.iter().any(|root| path.starts_with(root)))
}

/// Re-materialize one public provider graph by following only digest-verified public edges.
///
/// Every nested output is retained as a verified direct-Rustc search path. Only the current graph root is exposed to
/// its caller, which prevents an implementation dependency from becoming an accidental public package extern.
#[allow(clippy::too_many_arguments)]
fn rematerialize_caller_owned_provider_graph(
    artifact: &LibraryArtifactMetadata,
    manifest: &LibraryManifest,
    profile: &str,
    artifacts: &OvenRustcArtifactManifest,
    artifact_root: &Path,
    artifact_plan: &OvenRustcArtifactPlan,
    rustc: &Path,
    consumer_output_root: &Path,
    registry_authority: Option<&OvenRegistryLeafAuthority>,
    extra_dependency_search_paths: &[PathBuf],
    compiler_owned_roots: &[PathBuf],
    selected_path_authority: Option<&OvenSelectedPathRustcAuthority>,
    visiting: &mut BTreeSet<PathBuf>,
    authority_context: &mut Option<&mut OvenProjectBakeAuthorityContext>,
) -> CliResult<Vec<OvenCallerOwnedRustcLibrary>> {
    let canonical_root = fs::canonicalize(&artifact.crate_root).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot canonicalize generated artifact root for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.crate_root.display()
        ))
    })?;
    if !visiting.insert(canonical_root.clone()) {
        return Err(CliError::failure(format!(
            "Oven Alpha refuses a cyclic public provider graph while re-materializing pub::{} at {}",
            artifact.dependency_key,
            canonical_root.display()
        )));
    }
    let result = (|| {
        if let Some(dependency) = first_unselected_private_provider_edge(manifest, artifact_plan) {
            return Err(CliError::failure(format!(
                "Oven Alpha cannot re-materialize pub::{} because private provider edge `{}` is not a selected direct-Rustc foundation extern",
                artifact.dependency_key, dependency.dependency_key
            )));
        }

        let mut nested_libraries = Vec::new();
        for dependency in manifest
            .contract_metadata
            .provider
            .provider_dependencies
            .iter()
            .filter(|dependency| dependency.kind == ProviderDependencyKind::PublicPackage)
        {
            let (nested_manifest, nested_artifact) = load_receipted_public_provider_dependency(artifact, dependency)?;
            let mut materialized = rematerialize_caller_owned_provider_graph(
                &nested_artifact,
                &nested_manifest,
                profile,
                artifacts,
                artifact_root,
                artifact_plan,
                rustc,
                consumer_output_root,
                registry_authority,
                extra_dependency_search_paths,
                compiler_owned_roots,
                selected_path_authority,
                visiting,
                authority_context,
            )?;
            nested_libraries.append(&mut materialized);
        }
        deduplicate_caller_owned_libraries_prefer_extern(&mut nested_libraries);

        let receipt = caller_owned_library_receipt(artifact, profile, artifacts, authority_context.as_deref_mut())?;
        let provider_artifacts = provider_compilation_artifacts(artifacts).map_err(oven_plan_error)?;
        let provider_source_plan = plan_with_provider_compilation_role(artifact_plan, artifacts, &provider_artifacts);
        let mut provider_plan =
            trusted_artifact_plan_for_source_evidence(&provider_source_plan, &provider_artifacts, "generated-root")
                .map_err(oven_rustc_error)?;
        let edition = caller_owned_library_edition(artifact)?;
        let is_proc_macro = caller_owned_library_is_proc_macro(artifact)?;
        let provider_dependencies = caller_owned_library_rust_dependencies(artifact)?;
        let provider_dependencies =
            caller_owned_library_dependencies_for_compilation(provider_dependencies, &provider_plan);
        let provider_dependencies =
            caller_owned_library_dependencies_without_public_provider_edges(provider_dependencies, manifest);
        let provider_dependencies = caller_owned_library_dependencies_missing_from_selected_plan_with_owned_roots(
            &provider_dependencies,
            &provider_plan,
            compiler_owned_roots,
        );
        let mut provider_rust_libraries = materialize_declared_rust_libraries_with_selected_path_authority(
            &consumer_output_root
                .join("oven")
                .join("caller-owned-libraries")
                .join(profile)
                .join("provider-rust-dependencies"),
            rustc,
            &receipt.intent.target,
            profile,
            &provider_dependencies,
            registry_authority,
            selected_path_authority,
        )
        .map_err(oven_rustc_error)?;
        nested_libraries.append(&mut provider_rust_libraries);
        deduplicate_caller_owned_libraries_prefer_extern(&mut nested_libraries);

        let crate_name = ProjectGenerator::rust_target_name(&artifact.manifest_name);
        let artifact_digest = digest_provider_artifact(&artifact.crate_root).map_err(|error| {
            CliError::failure(format!(
                "Oven Alpha cannot fingerprint generated provider artifact for pub::{} at {}: {error}",
                artifact.dependency_key,
                artifact.crate_root.display()
            ))
        })?;
        let output = consumer_output_root
            .join("oven")
            .join("caller-owned-libraries")
            .join(profile)
            .join(artifact_digest.trim_start_matches("sha256:"))
            .join(if is_proc_macro {
                format!("lib{crate_name}{}", std::env::consts::DLL_SUFFIX)
            } else {
                format!("lib{crate_name}.rlib")
            });
        attach_caller_owned_rustc_libraries(&mut provider_plan, &nested_libraries).map_err(oven_rustc_error)?;
        if provider_dependencies
            .iter()
            .any(|dependency| matches!(dependency.source, DependencySource::Registry))
        {
            // The provider's own registry dependencies were each attached above as a direct `--extern`, but loading
            // any one of them can require Rustc to locate its *own* further dependencies purely through
            // `-L dependency=...` search -- including proc-macro/build-script outputs that never become a named
            // registry leaf at all. `extra_dependency_search_paths` is this provider's own already-materialized
            // closure (see `caller_owned_provider_registry_leaf_authority`), the same directories that made this
            // provider's own standalone bake link successfully.
            for directory in extra_dependency_search_paths {
                provider_plan.retain_caller_dependency_search_path(directory.clone());
            }
        }
        let bake_request = OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &provider_artifacts,
            artifact_root,
            artifact_plan: Some(&provider_plan),
            rustc,
            source: &artifact.crate_lib_path,
            output: &output,
            crate_name: &crate_name,
            edition: &edition,
            source_evidence_key: "generated-root",
            features: &receipt.intent.features,
            prefer_dynamic: false,
        };
        let bake = if is_proc_macro {
            bake_trusted_direct_rustc_proc_macro(&bake_request)
        } else {
            bake_trusted_direct_rustc_library(&bake_request)
        }
        .map_err(oven_rustc_error)?;

        for nested in &mut nested_libraries {
            nested.expose_extern = false;
        }
        nested_libraries.push(OvenCallerOwnedRustcLibrary {
            crate_name: artifact.dependency_key.clone(),
            output: bake.output,
            digest: bake.output_digest,
            expose_extern: true,
        });
        Ok(nested_libraries)
    })();
    visiting.remove(&canonical_root);
    result
}

/// Registry-leaf authorities and dependency-search closure collected from every caller-owned path-dependency
/// provider.
///
/// A `pub::` provider consumed by path (for example a query-engine library) was compiled against its own sealed
/// third-party registry closure. [`rematerialize_caller_owned_provider_graph`] re-materializes that provider's
/// compiled libraries into the consumer's own direct-Rustc plan, but resolving the provider's *own* declared
/// registry dependencies (its `[rust-dependencies]`) needs the provider's own registry-leaf authority and full
/// dependency search closure, not just the consumer's -- the consumer's own closure knows nothing about a package
/// the provider alone depends on, directly or transitively. Each provider's authority is kept separate here rather
/// than pre-merged so [`caller_owned_provider_registry_conflict`] can compare it against the consumer's own
/// authority before anything is joined; `dependency_search_paths` additionally exposes every directory the
/// providers' own standalone bakes needed to load their externs' further dependencies (including
/// proc-macro/build-script outputs that never become a named registry leaf at all) purely through Rustc's ordinary
/// `-L dependency=...` search.
#[derive(Default)]
pub struct CallerOwnedProviderRegistryClosure {
    pub provider_authorities: Vec<OvenRegistryLeafAuthority>,
    pub dependency_search_paths: Vec<PathBuf>,
}

impl CallerOwnedProviderRegistryClosure {
    /// Join the consumer's own authority with every collected provider authority into one lookup surface.
    ///
    /// Joining decides only what is *discoverable*; safety against a genuinely diverging shared package is decided
    /// beforehand by [`caller_owned_provider_registry_conflict`] and per-lookup by `select_sealed_registry_leaf`'s
    /// existing same-compilation check.
    pub fn merged_authority(&self, consumer: Option<OvenRegistryLeafAuthority>) -> Option<OvenRegistryLeafAuthority> {
        if self.provider_authorities.is_empty() {
            return consumer;
        }
        Some(OvenRegistryLeafAuthority::aggregate(
            consumer.into_iter().chain(self.provider_authorities.iter().cloned()),
        ))
    }
}

/// Collect the registry-leaf authorities and dependency search closure owned by every caller-owned path-dependency
/// provider.
///
/// Walks the exact same caller-owned provider graph [`rematerialize_caller_owned_provider_graph`] re-materializes.
/// The collected authorities feed both the pre-bake conflict decision
/// ([`caller_owned_provider_registry_conflict`]) and, via
/// [`CallerOwnedProviderRegistryClosure::merged_authority`], the re-materialization lookup surface.
pub fn collect_caller_owned_provider_registry_leaf_authority(
    store: &OvenStore,
    provider_plan: &ProviderPlan,
    profile: &str,
) -> CliResult<CallerOwnedProviderRegistryClosure> {
    let mut closure = CallerOwnedProviderRegistryClosure::default();
    let mut visiting = BTreeSet::new();
    for provider in provider_plan.active_records().filter(|provider| {
        matches!(
            provider.authority,
            incan_provider::NamespaceAuthority::ProjectDependency { .. }
        )
    }) {
        let artifact = provider.artifact.as_ref().ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha cannot resolve pub::{} because its generated library artifact is unavailable",
                provider.identity.name
            ))
        })?;
        let manifest = provider.manifest.as_ref().ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha cannot resolve pub::{} because its checked provider manifest is unavailable",
                artifact.dependency_key
            ))
        })?;
        collect_caller_owned_provider_registry_leaf_authority_graph(
            store,
            artifact,
            manifest,
            profile,
            &mut closure,
            &mut visiting,
        )?;
    }
    closure.dependency_search_paths.sort();
    closure.dependency_search_paths.dedup();
    Ok(closure)
}

/// Recursive worker for [`collect_caller_owned_provider_registry_leaf_authority`].
///
/// Follows the same public-package provider edges [`rematerialize_caller_owned_provider_graph`] follows, so both
/// walks agree on which providers exist and which are another provider's own nested public-package dependency.
fn collect_caller_owned_provider_registry_leaf_authority_graph(
    store: &OvenStore,
    artifact: &LibraryArtifactMetadata,
    manifest: &LibraryManifest,
    profile: &str,
    closure: &mut CallerOwnedProviderRegistryClosure,
    visiting: &mut BTreeSet<PathBuf>,
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
            "Oven Alpha refuses a cyclic public provider graph while resolving pub::{} at {}",
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
            collect_caller_owned_provider_registry_leaf_authority_graph(
                store,
                &nested_artifact,
                &nested_manifest,
                profile,
                closure,
                visiting,
            )?;
        }
        if let Some((provider_authority, provider_search_paths)) =
            caller_owned_provider_registry_leaf_authority(store, artifact, profile)?
        {
            closure.provider_authorities.push(provider_authority);
            closure.dependency_search_paths.extend(provider_search_paths);
        }
        Ok(())
    })();
    visiting.remove(&canonical_root);
    result
}

/// Return one caller-owned provider's own receipt-bound registry-leaf authority and dependency search closure, if
/// it declared any registry dependencies of its own.
///
/// This never bakes or invokes Cargo -- it only selects an already-published receipt, the same select-only step
/// normal build/run try before falling back to the explicit baker. A provider without a verified receipt for this
/// profile, or without any registry dependencies of its own, contributes nothing here; the ordinary
/// [`materialize_declared_rust_libraries_with_selected_path_authority`] failure surfaces an actionable error once
/// something actually needs a registry leaf this authority does not have. The returned search paths are the
/// provider's own already-materialized `artifact_plan().dependency_search_paths` -- the same directories that made
/// this provider's own standalone bake link successfully, including proc-macro/build-script outputs that have no
/// registry-leaf entry of their own.
fn caller_owned_provider_registry_leaf_authority(
    store: &OvenStore,
    artifact: &LibraryArtifactMetadata,
    profile: &str,
) -> CliResult<Option<(OvenRegistryLeafAuthority, Vec<PathBuf>)>> {
    let Some(project_root) = dependency_project_root(&artifact.crate_root) else {
        return Ok(None);
    };
    let Some(receipt) = read_verified_caller_owned_provider_receipt(&project_root, profile) else {
        return Ok(None);
    };
    let Some(selection) = select_published_project_plan(store, &receipt, OvenToolchainMaterialization::Reused)? else {
        return Ok(None);
    };
    let Some(authority) = registry_leaf_authority_for_plan_selection(&selection.plan_selection)? else {
        return Ok(None);
    };
    let search_paths = selection.plan_selection.artifact_plan().dependency_search_paths.clone();
    Ok(Some((authority, search_paths)))
}

/// Read and identity-verify one caller-owned provider's own Oven receipt for `profile`, if one exists.
///
/// An explicit project bake writes the library receipt under its target-qualified path.
///
/// The generic receipt is a legacy `incan build --lib` handoff; an explicit bake with a declared script overwrites it
/// with the last target prepared. Prefer the library-specific receipt so a consumer never borrows a script's unrelated
/// native closure. Retain the generic path only for a legacy standalone library build.
pub fn read_verified_caller_owned_provider_receipt(
    project_root: &Path,
    profile: &str,
) -> Option<oven_store::OvenReceipt> {
    let library_entrypoint = project_root.join(OvenBakeProjectTarget::Library.source_relative_path());
    let library_receipt = project_bake_receipt_path(
        project_root,
        OvenBakeProjectTarget::Library,
        &library_entrypoint,
        profile,
    )
    .ok();
    let path = match library_receipt {
        Some(path) => match fs::symlink_metadata(&path) {
            Ok(_) => path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => oven_store::default_receipt_path(project_root),
            Err(_) => return None,
        },
        None => oven_store::default_receipt_path(project_root),
    };
    let receipt = fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<oven_store::OvenReceipt>(&bytes).ok())?;
    receipt.verify_identity().ok()?;
    Some(receipt)
}

/// Rebuild selected caller-owned Rust libraries in the consumer's direct-Rustc cohort.
#[allow(clippy::too_many_arguments)]
pub fn rematerialize_caller_owned_libraries_with_authority_context(
    provider_plan: &ProviderPlan,
    profile: &str,
    artifacts: &OvenRustcArtifactManifest,
    artifact_root: &Path,
    artifact_plan: &OvenRustcArtifactPlan,
    rustc: &Path,
    consumer_output_root: &Path,
    registry_authority: Option<&OvenRegistryLeafAuthority>,
    extra_dependency_search_paths: &[PathBuf],
    mut authority_context: Option<&mut OvenProjectBakeAuthorityContext>,
) -> CliResult<Vec<OvenCallerOwnedRustcLibrary>> {
    let mut libraries = Vec::new();
    let mut visiting = BTreeSet::new();
    let compiler_owned_roots = compiler_owned_roots_with_provider_plan(artifact_plan, Some(provider_plan));
    let selected_path_authority = (!compiler_owned_roots.is_empty())
        .then(|| OvenSelectedPathRustcAuthority::new(&compiler_owned_roots, artifact_plan));
    for provider in provider_plan.active_records().filter(|provider| {
        matches!(
            provider.authority,
            incan_provider::NamespaceAuthority::ProjectDependency { .. }
        )
    }) {
        let artifact = provider.artifact.as_ref().ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha cannot re-materialize pub::{} because its generated library artifact is unavailable",
                provider.identity.name
            ))
        })?;
        if artifact.kind != LibraryArtifactKind::Materialized {
            return Err(CliError::failure(format!(
                "Oven Alpha cannot re-materialize pub::{} from source-only metadata; run `incan build --lib` for that dependency first",
                artifact.dependency_key
            )));
        }
        let manifest = provider.manifest.as_ref().ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha cannot re-materialize pub::{} because its checked provider manifest is unavailable",
                artifact.dependency_key
            ))
        })?;
        libraries.extend(rematerialize_caller_owned_provider_graph(
            artifact,
            manifest,
            profile,
            artifacts,
            artifact_root,
            artifact_plan,
            rustc,
            consumer_output_root,
            registry_authority,
            extra_dependency_search_paths,
            &compiler_owned_roots,
            selected_path_authority.as_ref(),
            &mut visiting,
            &mut authority_context,
        )?);
    }
    libraries.sort_by(|left, right| {
        left.crate_name
            .cmp(&right.crate_name)
            .then_with(|| left.output.cmp(&right.output))
            .then_with(|| left.expose_extern.cmp(&right.expose_extern))
    });
    libraries.dedup_by(|left, right| {
        left.crate_name == right.crate_name && left.output == right.output && left.expose_extern == right.expose_extern
    });
    if libraries
        .windows(2)
        .any(|pair| pair[0].expose_extern && pair[1].expose_extern && pair[0].crate_name == pair[1].crate_name)
    {
        return Err(CliError::failure(
            "Oven Alpha resolved duplicate re-materialized caller-owned Rust library crate names",
        ));
    }
    Ok(libraries)
}

/// Rebuild selected caller-owned Rust libraries for ordinary consumers without an explicit-bake memo.
///
/// Unlike [`bake_oven_project`]/[`bake_oven_library`], this entry point does not (yet) collect each caller-owned
/// provider's own registry-leaf authority and dependency search closure via
/// [`collect_caller_owned_provider_registry_leaf_authority`] -- a caller-owned provider that declares registry
/// dependencies of its own is not yet supported through this path. Ordinary providers without their own registry
/// dependencies are unaffected.
#[allow(clippy::too_many_arguments)]
pub fn rematerialize_caller_owned_libraries(
    provider_plan: &ProviderPlan,
    profile: &str,
    artifacts: &OvenRustcArtifactManifest,
    artifact_root: &Path,
    artifact_plan: &OvenRustcArtifactPlan,
    rustc: &Path,
    consumer_output_root: &Path,
    registry_authority: Option<&OvenRegistryLeafAuthority>,
) -> CliResult<Vec<OvenCallerOwnedRustcLibrary>> {
    rematerialize_caller_owned_libraries_with_authority_context(
        provider_plan,
        profile,
        artifacts,
        artifact_root,
        artifact_plan,
        rustc,
        consumer_output_root,
        registry_authority,
        &[],
        None,
    )
}

/// Replace package-library attachments while preserving independently materialized inline Rust dependencies.
pub fn replace_caller_owned_package_libraries(
    libraries: &mut Vec<OvenCallerOwnedRustcLibrary>,
    re_materialized: Vec<OvenCallerOwnedRustcLibrary>,
) -> CliResult<()> {
    if re_materialized.is_empty() {
        return Ok(());
    }
    let rematerialized_names = re_materialized
        .iter()
        .filter(|library| library.expose_extern)
        .map(|library| library.crate_name.as_str())
        .collect::<BTreeSet<_>>();
    libraries.retain(|library| !rematerialized_names.contains(library.crate_name.as_str()));
    libraries.extend(re_materialized);
    libraries.sort_by(|left, right| {
        left.crate_name
            .cmp(&right.crate_name)
            .then_with(|| left.output.cmp(&right.output))
            .then_with(|| left.expose_extern.cmp(&right.expose_extern))
    });
    if libraries
        .windows(2)
        .any(|pair| pair[0].expose_extern && pair[1].expose_extern && pair[0].crate_name == pair[1].crate_name)
    {
        return Err(CliError::failure(
            "Oven Alpha resolved duplicate caller-owned Rust library crate names after package re-materialization",
        ));
    }
    Ok(())
}

/// Replace receipt-selected historical package outputs with their current-cohort re-materializations.
///
/// An explicit project plan can retain a public provider as a generated-source root, but that stored rlib belongs to
/// the producer's Rustc metadata cohort. Once the provider has been rebuilt from its receipt-checked source against
/// the consumer's selected plan, retaining both `--extern` values would either duplicate the crate name or allow a
/// mismatched provider closure to reach Rustc. Only direct package-root names are removed; registry leaves and every
/// other immutable compiler artifact remain owned by the selected plan.
pub fn replace_selected_package_library_externs(
    artifact_plan: &mut OvenRustcArtifactPlan,
    replacement_names: &BTreeSet<String>,
) {
    if replacement_names.is_empty() {
        return;
    }
    let removed_parents = artifact_plan
        .externs
        .iter()
        .filter(|(crate_name, _)| replacement_names.contains(crate_name))
        .filter_map(|(_, path)| path.parent().map(Path::to_path_buf))
        .collect::<BTreeSet<_>>();
    artifact_plan
        .externs
        .retain(|(crate_name, _)| !replacement_names.contains(crate_name));
    let retained_parents = artifact_plan
        .externs
        .iter()
        .filter_map(|(_, path)| path.parent().map(Path::to_path_buf))
        .collect::<BTreeSet<_>>();
    artifact_plan
        .dependency_search_paths
        .retain(|search_path| !removed_parents.contains(search_path) || retained_parents.contains(search_path));
}

/// Select exactly the resolved dependency specifications used by caller-authored inline `rust::` imports.
///
/// Provider-owned source imports remain inside the selected compiler-native closure. This prevents a caller from
/// asking the path materializer to rebuild the SDK/provider graph, while aliases such as `prost-types` /
/// `prost_types` retain Cargo's conventional spelling equivalence without invoking Cargo.
pub fn oven_source_inline_dependency_specs(
    resolved: &ResolvedDependencies,
    source_inline_crates: &BTreeSet<String>,
) -> CliResult<Vec<DependencySpec>> {
    let normalize = |name: &str| name.replace('-', "_");
    let mut dependencies = Vec::new();
    for source_crate in source_inline_crates {
        let normalized = normalize(source_crate);
        let dependency = resolved
            .dependencies
            .iter()
            .chain(&resolved.dev_dependencies)
            .find(|dependency| {
                normalize(&dependency.crate_name) == normalized
                    || dependency
                        .package
                        .as_deref()
                        .is_some_and(|package| normalize(package) == normalized)
            })
            .ok_or_else(|| {
                CliError::failure(format!(
                    "Oven Alpha could not resolve caller Rust import `{source_crate}` to a declared dependency"
                ))
            })?;
        dependencies.push(dependency.clone());
    }
    dependencies.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    dependencies.dedup_by(|left, right| left.crate_name == right.crate_name);
    Ok(dependencies)
}

/// Keep only declared Rust dependencies that the selected immutable plan does not already provide.
///
/// Registry leaves in a receipt-bound plan are compiler-owned direct-Rustc inputs. Recompiling one as a
/// caller-owned library would attach two `--extern` values with one crate name. A path dependency remains
/// caller-owned even with an overlapping selected extern, except for an explicit compiler-suite path under a
/// scheduler-leased immutable root; that narrow exception is the same ownership rule used while re-materializing a
/// source-backed provider graph.
pub fn declared_rust_libraries_missing_from_selected_plan(
    dependencies: &[DependencySpec],
    artifact_plan: &OvenRustcArtifactPlan,
) -> Vec<DependencySpec> {
    declared_rust_libraries_missing_from_selected_plan_with_current_project_paths(dependencies, artifact_plan, false)
}

/// Keep direct Rust paths explicit unless the exact current-project plan already seals their projected externs.
///
/// `true` is valid only for a receipt-selected project plan. Imported packages may expose their own private path
/// dependencies with the same crate name, and compiler Loafs may expose compiler-owned paths, so neither is
/// authority to omit a caller declaration.
pub fn declared_rust_libraries_missing_from_selected_plan_with_current_project_paths(
    dependencies: &[DependencySpec],
    artifact_plan: &OvenRustcArtifactPlan,
    current_project_paths_are_sealed: bool,
) -> Vec<DependencySpec> {
    let selected_externs = artifact_plan
        .externs
        .iter()
        .map(|(crate_name, _)| crate_name.clone())
        .collect::<BTreeSet<_>>();
    let mut remaining = declared_rust_libraries_missing_from_selected_plan_with_owned_roots(
        dependencies,
        &selected_externs,
        &compiler_owned_roots(artifact_plan),
    );
    if current_project_paths_are_sealed {
        remaining.retain(|dependency| {
            !matches!(dependency.source, DependencySource::Path { .. })
                || !selected_externs.contains(&dependency.crate_name.replace('-', "_"))
        });
    }
    remaining
}

/// Verify the semantic registry contract for every dependency omitted because the selected plan exposes its crate.
///
/// Direct-Rustc extern names carry no Cargo package/version information. Without this check an impossible declared
/// version could silently borrow an unrelated compiler-owned artifact solely because both normalize to one crate
/// name. The sealed native catalog remains the only resolver; no Cargo cache, index, or network state is consulted.
pub fn validate_selected_plan_registry_dependencies(
    dependencies: &[DependencySpec],
    selected_plan: &OvenRustcArtifactPlan,
    registry_authority: Option<&OvenRegistryLeafAuthority>,
    profile: &str,
) -> CliResult<()> {
    for dependency in dependencies {
        if matches!(dependency.source, DependencySource::Registry) {
            let crate_name = dependency.crate_name.replace('-', "_");
            if let Some((_, selected_artifact)) = selected_plan
                .externs
                .iter()
                .find(|(selected_crate, _)| selected_crate == &crate_name)
            {
                validate_selected_sealed_registry_leaf(dependency, selected_artifact, registry_authority, profile)
                    .map_err(oven_rustc_error)?;
            }
        }
    }
    Ok(())
}

/// Variant with explicit scheduler-owned roots so the path-authority boundary is independently testable.
fn declared_rust_libraries_missing_from_selected_plan_with_owned_roots(
    dependencies: &[DependencySpec],
    selected_externs: &BTreeSet<String>,
    owned_roots: &[PathBuf],
) -> Vec<DependencySpec> {
    let selected_extern_names = selected_externs.iter().map(String::as_str).collect::<BTreeSet<_>>();
    dependencies
        .iter()
        .filter(|dependency| match dependency.source {
            DependencySource::Registry => !selected_externs.contains(&dependency.crate_name.replace('-', "_")),
            DependencySource::Path { .. } => {
                !is_selected_compiler_runtime_path_dependency(dependency, &selected_extern_names, owned_roots)
            }
            DependencySource::Git { .. } => true,
        })
        .cloned()
        .collect()
}

/// Profiles an explicit `oven bake` materializes, and that its consumers then expect to find.
///
/// A bake normally produces every profile a later `build`, `test`, or `run` could select, so a project with a
/// large `[rust-dependencies]` closure pays a full optimized build even when only `debug` is ever loaded. On
/// IncQL that release half measured 2098 of 2717 rustc CPU seconds.
///
/// This is deliberately an environment policy rather than an `oven bake` flag. The profile set is not private to
/// the bake: `build` verifies project inspection authority across the same set, and a narrowed bake seen by a
/// consumer that still expects both profiles reports "no source-current project inspection authority is
/// available" rather than the profile it is actually missing. An environment override applies to every command in
/// a session — one `env:` block in CI covers `bake`, `build`, and `test` alike — whereas a per-invocation flag
/// would leave the consumers disagreeing with the bake that produced their inputs.
///
/// A future change could record the materialized profile set in the authority itself and let each consumer check
/// only the profile it needs; until then the set must be stated the same way to every command.
pub fn explicit_bake_profiles() -> Vec<&'static str> {
    match std::env::var("INCAN_OVEN_BAKE_PROFILES").ok().as_deref().map(str::trim) {
        Some("debug") => vec!["debug"],
        Some("release") => vec!["release"],
        // `all` is named rather than left to the catch-all because callers already write it to mean both, and a
        // spelling the matcher does not name is one rename away from silently selecting something else.
        Some("all") => vec!["debug", "release"],
        _ => vec!["debug", "release"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::PathBuf;

    use oven_model::manifest::{DependencySource, DependencySpec};

    use oven_rustc::rustc::{OvenRegistryLeafAuthority, OvenRustcArtifactPlan};

    #[test]
    fn selected_plan_registry_extern_is_not_materialized_as_caller_owned() {
        let registry = |crate_name: &str| DependencySpec {
            crate_name: crate_name.to_string(),
            version: Some("1".to_string()),
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        };
        let path = DependencySpec {
            crate_name: "serde_json".to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path {
                path: PathBuf::from("caller-owned-serde-json"),
            },
            optional: false,
            package: None,
        };
        let plan = OvenRustcArtifactPlan {
            source_path_projection: None,
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: vec![("serde_json".to_string(), PathBuf::from("sealed/serde_json.rlib"))],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };

        let remaining = declared_rust_libraries_missing_from_selected_plan(
            &[registry("serde-json"), registry("regex"), path.clone()],
            &plan,
        );

        assert_eq!(
            remaining,
            vec![registry("regex"), path],
            "only the compiler-owned registry leaf may be omitted; a caller path dependency remains explicit"
        );
    }

    #[test]
    fn current_project_plan_reuses_its_sealed_direct_path_extern() {
        let path = DependencySpec {
            crate_name: "receiver_factory".to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path {
                path: PathBuf::from("current-project-receiver-factory"),
            },
            optional: false,
            package: None,
        };
        let plan = OvenRustcArtifactPlan {
            source_path_projection: None,
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: vec![(
                "receiver_factory".to_string(),
                PathBuf::from("sealed/receiver_factory.rlib"),
            )],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };

        let remaining =
            declared_rust_libraries_missing_from_selected_plan_with_current_project_paths(&[path], &plan, true);

        assert!(
            remaining.is_empty(),
            "a receipt-selected project plan must reuse its own sealed direct path extern"
        );
    }

    #[test]
    fn selected_registry_extern_still_requires_a_compatible_sealed_version() -> Result<(), Box<dyn std::error::Error>> {
        use sha2::{Digest, Sha256};

        let root = tempfile::tempdir()?;
        let relative_path = "debug/deps/libserde_json.rlib";
        let artifact_path = root.path().join(relative_path);
        let artifact_parent = artifact_path.parent().ok_or("artifact parent")?;
        fs::create_dir_all(artifact_parent)?;
        let bytes = b"sealed serde_json 1.0.0";
        fs::write(&artifact_path, bytes)?;
        let authority = OvenRegistryLeafAuthority::new(
            root.path().to_path_buf(),
            vec![oven_rustc::rustc::OvenRustcRegistryLeaf {
                selected_unit_identity: None,
                package: "serde_json".to_string(),
                version: "1.0.0".to_string(),
                crate_name: "serde_json".to_string(),
                features: Vec::new(),
                source: oven_rustc::rustc::OvenRustcRegistrySource {
                    registry: "registry+https://example.invalid/index".to_string(),
                    checksum: "fixture-checksum".to_string(),
                    relative_root: "registry-sources/serde-json".to_string(),
                    digest: oven_store::digest_bytes(b"fixture registry source"),
                },
                artifact: oven_rustc::rustc::OvenRustcArtifactExtern {
                    crate_name: "serde_json".to_string(),
                    relative_path: relative_path.to_string(),
                    digest: format!("sha256:{}", hex::encode(Sha256::digest(bytes))),
                },
            }],
        );
        let dependency = |version: &str| DependencySpec {
            crate_name: "serde_json".to_string(),
            version: Some(version.to_string()),
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        };
        let selected_plan = OvenRustcArtifactPlan {
            source_path_projection: None,
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: vec![("serde_json".to_string(), artifact_path)],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };

        validate_selected_plan_registry_dependencies(&[dependency("1.0")], &selected_plan, Some(&authority), "debug")?;
        let error = match validate_selected_plan_registry_dependencies(
            &[dependency("999.0.0")],
            &selected_plan,
            Some(&authority),
            "debug",
        ) {
            Ok(()) => return Err("an incompatible declared version borrowed a selected extern by crate name".into()),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("999.0.0"),
            "unexpected version diagnostic: {error}"
        );
        Ok(())
    }

    #[test]
    fn selected_scheduler_owned_path_extern_is_not_materialized_as_inline_library()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let provider_root = workspace.path().join("sealed-providers");
        let scheduler_path = provider_root.join("components/stdlib-core");
        let caller_path = workspace.path().join("caller/incan_stdlib_core");
        fs::create_dir_all(&scheduler_path)?;
        fs::create_dir_all(&caller_path)?;
        let scheduler_dependency = DependencySpec {
            crate_name: "incan_stdlib_core".to_string(),
            version: None,
            features: Vec::new(),
            default_features: false,
            source: DependencySource::Path { path: scheduler_path },
            optional: false,
            package: None,
        };
        let caller_dependency = DependencySpec {
            source: DependencySource::Path { path: caller_path },
            ..scheduler_dependency.clone()
        };
        let selected_externs = BTreeSet::from(["incan_stdlib_core".to_string()]);

        let remaining = declared_rust_libraries_missing_from_selected_plan_with_owned_roots(
            &[scheduler_dependency, caller_dependency.clone()],
            &selected_externs,
            &[fs::canonicalize(provider_root)?],
        );

        assert_eq!(remaining, vec![caller_dependency]);
        Ok(())
    }
}
