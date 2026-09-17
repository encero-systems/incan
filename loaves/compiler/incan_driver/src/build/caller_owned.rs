//! Caller-owned Rust libraries: the receipts, editions, proc-macro facts and Rust dependencies of the libraries a
//! project owns rather than imports from a provider.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use crate::backend::ProjectGenerator;
use crate::build::package_loafs::{read_packaged_library_loaf_manifest, validated_packaged_library_loaf_profile};
use crate::build::source_authority::project_bake_receipt_path;
use crate::build::{OvenBakeProjectTarget, OvenProjectBakeAuthorityContext};
use crate::error::{CliError, CliResult};
use incan_frontend::library_manifest::published_layout::packaged_library_loaf_manifest_path;
use incan_frontend::library_manifest::{
    LibraryManifest, ProviderDependencyKind, ProviderDependencyMetadata, digest_provider_artifact,
};
use incan_frontend::library_manifest_index::{
    LibraryArtifactKind, LibraryArtifactMetadata, LibraryManifestIndexEntry, load_provider_dependency_artifact,
};
use incan_provider::ProviderPlan;
use oven_interop::{
    default_interop_execution_receipt_path, interop_execution_build_unit_inputs, load_interop_execution_receipt,
    validate_interop_execution_receipt,
};
use oven_model::manifest::{DependencySource, DependencySpec, ProjectManifest};
use oven_model::oven_interop::locked_oven_interop_targets;
use oven_rustc::rustc::{OvenCallerOwnedRustcLibrary, OvenRustcArtifactManifest, OvenRustcArtifactPlan};
use oven_store::{OvenGeneratedProjectRequest, digest_bytes, receipt_generated_project, write_receipt};

/// Add the exact selected interop execution receipt when this normal command targets a declared interop profile.
///
/// The portable lock remains the declaration authority; the small project-owned receipt proves which compatible
/// compiler and SDK Oven selected for that lock. A normal build only reads and revalidates this receipt. It neither
/// rediscovers a native toolchain nor tries Cargo when an interop-native plan is absent.
pub fn append_oven_interop_execution_build_inputs(
    build_inputs: &mut BTreeMap<String, String>,
    manifest: Option<&ProjectManifest>,
    target: &str,
) -> CliResult<()> {
    let Some(manifest) = manifest else {
        return Ok(());
    };
    let locked_targets = locked_oven_interop_targets(manifest)
        .map_err(|error| CliError::failure(format!("invalid locked Oven interop requirements: {error}")))?;
    if !locked_targets.iter().any(|candidate| candidate.target == target) {
        return Ok(());
    }
    // Reuse the command-side resolver here rather than treating a freshly recomputed declaration as sufficient.
    // It proves the package's current file receipts still equal the canonical standalone/workspace lock before a
    // normal consumer can select an immutable native plan.
    let locked = crate::interop_plan::locked_interop_plan_target(manifest.project_root(), target)?;
    let locked_target = &locked.target;
    let receipt_path = default_interop_execution_receipt_path(manifest.project_root(), target);
    let receipt = load_interop_execution_receipt(&receipt_path).map_err(|error| {
        CliError::failure(format!(
            "Oven interop target `{target}` has no current selected execution receipt at {}: {error}. Run the explicit `incan oven interop bake` command; normal build and run will not discover native tools or invoke Cargo.",
            receipt_path.display()
        ))
    })?;
    validate_interop_execution_receipt(locked_target, &receipt).map_err(|error| {
        CliError::failure(format!(
            "Oven interop target `{target}` has a stale selected execution receipt at {}: {error}. Re-run the explicit `incan oven interop bake` command; normal build and run will not fall back to Cargo.",
            receipt_path.display()
        ))
    })?;
    for (name, value) in interop_execution_build_unit_inputs(&receipt) {
        if build_inputs.insert(name.clone(), value).is_some() {
            return Err(CliError::failure(format!(
                "normal Oven build inputs already contain reserved interop key `{name}`"
            )));
        }
    }
    Ok(())
}

/// Resolve the direct-Rustc outputs of materialized caller-owned `pub::` dependencies.
///
/// These libraries are intentionally outside the immutable Loaf plan: they belong to the caller's project graph,
/// whereas that plan is restricted to compiler-owned SDK/runtime inputs. A prior Oven library materialization
/// establishes the caller-owned source and receipt boundary. Its old rlib is only an optional fast-path attachment: a
/// consumer can re-materialize that verified source under its selected direct-Rustc cohort when the old output is
/// absent or belongs to a different cohort. This is never a reason to invoke Cargo.
pub fn has_caller_owned_project_libraries(provider_plan: &ProviderPlan) -> bool {
    provider_plan.active_records().any(|provider| {
        matches!(
            provider.authority,
            incan_provider::NamespaceAuthority::ProjectDependency { .. }
        )
    })
}

/// Return the checked caller-owned `pub::` Rust libraries that an Oven consumer must attach directly.
pub fn oven_caller_owned_libraries(
    provider_plan: &ProviderPlan,
    profile: &str,
) -> CliResult<Vec<OvenCallerOwnedRustcLibrary>> {
    let mut libraries = Vec::new();
    for provider in provider_plan.active_records().filter(|provider| {
        matches!(
            provider.authority,
            incan_provider::NamespaceAuthority::ProjectDependency { .. }
        )
    }) {
        let artifact = provider.artifact.as_ref().ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha cannot link pub::{} because its generated library artifact is unavailable",
                provider.identity.name
            ))
        })?;
        if artifact.kind != LibraryArtifactKind::Materialized {
            return Err(CliError::failure(format!(
                "Oven Alpha cannot link pub::{} from source-only metadata; run `incan build --lib` for that dependency to produce its caller-owned Oven library",
                artifact.dependency_key
            )));
        }
        let output = artifact.crate_root.join("oven").join(profile).join(format!(
            "lib{}.rlib",
            ProjectGenerator::rust_target_name(&artifact.manifest_name)
        ));
        if !output.is_file() {
            // `rematerialize_caller_owned_libraries` follows immediately after native-plan selection and rebuilds
            // the receipt-authorized generated source with that exact cohort. Do not turn a missing convenience
            // output into a Cargo fallback or a false prerequisite for a source that is already materialized.
            continue;
        }
        let digest = digest_bytes(&fs::read(&output).map_err(|source| {
            CliError::failure(format!(
                "Oven Alpha cannot read caller-owned Rust library {}: {source}",
                output.display()
            ))
        })?);
        libraries.push(OvenCallerOwnedRustcLibrary {
            crate_name: artifact.dependency_key.clone(),
            output,
            digest,
            expose_extern: true,
        });
    }
    libraries.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    if libraries
        .windows(2)
        .any(|pair| pair[0].crate_name == pair[1].crate_name)
    {
        return Err(CliError::failure(
            "Oven Alpha resolved duplicate caller-owned Rust library crate names",
        ));
    }
    Ok(libraries)
}

/// Rebuild eligible caller-owned package libraries under a consumer's selected direct-Rustc cohort.
///
/// A prior `incan build --lib` output proves that the package was deliberately materialized, but its strict-version
/// hashes belong to the producer's old native plan. Attaching that rlib to a consumer selected with another complete
/// plan can therefore fail before the test harness runs. Rebuilding the producer's receipt-authorized generated
/// source with the already selected consumer plan keeps every direct extern in one Rustc cohort without making Cargo
/// a resolver or executor.
///
/// Compiler-owned private edges must already be part of the selected foundation plan. Public package edges follow
/// the separately receipt-authorized caller-owned recursion below; treating them as foundation inputs would widen a
/// selected Loaf with arbitrary package artifacts.
pub fn first_unselected_private_provider_edge<'a>(
    manifest: &'a LibraryManifest,
    artifact_plan: &OvenRustcArtifactPlan,
) -> Option<&'a ProviderDependencyMetadata> {
    manifest
        .contract_metadata
        .provider
        .provider_dependencies
        .iter()
        .find(|dependency| {
            dependency.kind == ProviderDependencyKind::PrivateImplementation
                && !artifact_plan
                    .externs
                    .iter()
                    .any(|(crate_name, _)| crate_name == &dependency.dependency_key)
        })
}

/// Exclude generated Cargo projection entries already supplied by a checked public provider edge.
///
/// A public `pub::` dependency is materialized from its digest-verified `.incnlib` graph above. Its generated Rust
/// projection also contains the same crate as a path dependency, but compiling that second projection would create a
/// distinct caller-owned rlib with the same Rust crate name. Keep the checked public graph authoritative while
/// letting every non-public Rust dependency continue through the direct-Rustc materializer.
pub fn caller_owned_library_dependencies_without_public_provider_edges(
    dependencies: Vec<DependencySpec>,
    manifest: &LibraryManifest,
) -> Vec<DependencySpec> {
    let public_keys = manifest
        .contract_metadata
        .provider
        .provider_dependencies
        .iter()
        .filter(|dependency| dependency.kind == ProviderDependencyKind::PublicPackage)
        .map(|dependency| dependency.dependency_key.replace('-', "_"))
        .collect::<BTreeSet<_>>();
    dependencies
        .into_iter()
        .filter(|dependency| !public_keys.contains(&dependency.crate_name.replace('-', "_")))
        .collect()
}

/// Collapse identical caller-owned artifacts while retaining the strongest direct-extern requirement.
///
/// A public provider can pass one artifact upward as transitive while its parent names that exact artifact directly.
/// Both search-path records identify the same bytes, but the direct parent still needs a `--extern` binding to compile.
/// Prefer that binding only for an identical crate/output pair; distinct outputs remain visible as an ambiguity.
pub fn deduplicate_caller_owned_libraries_prefer_extern(libraries: &mut Vec<OvenCallerOwnedRustcLibrary>) {
    libraries.sort_by(|left, right| {
        left.crate_name
            .cmp(&right.crate_name)
            .then_with(|| left.output.cmp(&right.output))
            .then_with(|| right.expose_extern.cmp(&left.expose_extern))
    });
    libraries.dedup_by(|left, right| left.crate_name == right.crate_name && left.output == right.output);
}

/// Load one public provider edge from the parent artifact's checked, relocation-safe projection.
///
/// The parent manifest supplies both the exact artifact-tree digest and the expected provider identity. Resolving the
/// relative path therefore does not restore normal dependency discovery: a child is admissible only when all three
/// values agree, and the normal artifact loader confirms its generated source and Cargo projection are complete.
pub fn load_receipted_public_provider_dependency(
    parent: &LibraryArtifactMetadata,
    dependency: &ProviderDependencyMetadata,
) -> CliResult<(LibraryManifest, LibraryArtifactMetadata)> {
    let candidate = parent.crate_root.join(&dependency.relative_artifact_path);
    let actual_digest = digest_provider_artifact(&candidate).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot re-materialize pub::{} because provider edge `{}` has an invalid artifact at {}: {error}",
            parent.dependency_key,
            dependency.dependency_key,
            candidate.display()
        ))
    })?;
    if actual_digest != dependency.artifact_digest {
        return Err(CliError::failure(format!(
            "Oven Alpha refuses provider edge `{}` below pub::{}: artifact digest {actual_digest} does not match its checked manifest identity {}",
            dependency.dependency_key, parent.dependency_key, dependency.artifact_digest
        )));
    }
    let entry = load_provider_dependency_artifact(&dependency.dependency_key, &candidate);
    let (manifest, metadata) = match entry {
        LibraryManifestIndexEntry::Loaded { manifest, metadata } => (*manifest, metadata),
        LibraryManifestIndexEntry::Failed(failure) => {
            return Err(CliError::failure(format!(
                "Oven Alpha cannot re-materialize provider edge `{}` below pub::{}: {failure}",
                dependency.dependency_key, parent.dependency_key
            )));
        }
    };
    if manifest.name != dependency.provider_name || manifest.version != dependency.provider_version {
        return Err(CliError::failure(format!(
            "Oven Alpha refuses provider edge `{}` below pub::{}: checked identity {}@{} differs from discovered {}@{}",
            dependency.dependency_key,
            parent.dependency_key,
            dependency.provider_name,
            dependency.provider_version,
            manifest.name,
            manifest.version
        )));
    }
    if metadata.kind != LibraryArtifactKind::Materialized {
        return Err(CliError::failure(format!(
            "Oven Alpha cannot re-materialize provider edge `{}` below pub::{} from parser-only metadata",
            dependency.dependency_key, parent.dependency_key
        )));
    }
    Ok((manifest, metadata))
}

/// Read and verify the producer receipt that authorizes one caller-owned generated library source.
pub fn caller_owned_library_receipt(
    artifact: &LibraryArtifactMetadata,
    profile: &str,
    artifacts: &OvenRustcArtifactManifest,
    authority_context: Option<&mut OvenProjectBakeAuthorityContext>,
) -> CliResult<oven_store::OvenReceipt> {
    let project_root = artifact.crate_root.parent().and_then(Path::parent).ok_or_else(|| {
        CliError::failure(format!(
            "Oven Alpha cannot locate the project root for pub::{} from generated artifact root {}",
            artifact.dependency_key,
            artifact.crate_root.display()
        ))
    })?;
    let selected_package = if let Some(context) = authority_context {
        context
            .checked_packaged_library_loaf_profiles(
                artifact,
                &[profile],
                &artifacts.intent.target,
                &artifacts.intent.toolchain,
            )?
            .and_then(|mut profiles| profiles.pop())
    } else if let Some(package) = read_packaged_library_loaf_manifest(artifact)? {
        validated_packaged_library_loaf_profile(
            artifact,
            &package,
            profile,
            &artifacts.intent.target,
            &artifacts.intent.toolchain,
        )?
    } else {
        None
    };
    if packaged_library_loaf_manifest_path(&artifact.crate_root).is_file() {
        let selected = selected_package.ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha has no compatible `{profile}` package receipt for pub::{} in the selected direct-Rustc cohort",
                artifact.dependency_key
            ))
        })?;
        return Ok(selected.receipt);
    }
    let receipt_path = project_bake_receipt_path(
        project_root,
        OvenBakeProjectTarget::Library,
        &project_root.join(OvenBakeProjectTarget::Library.source_relative_path()),
        profile,
    )?;
    let receipt = match fs::read(&receipt_path) {
        Ok(receipt_bytes) => {
            let receipt = serde_json::from_slice::<oven_store::OvenReceipt>(&receipt_bytes).map_err(|error| {
                CliError::failure(format!(
                    "Oven Alpha cannot parse the `{profile}` library receipt for pub::{} at {}: {error}",
                    artifact.dependency_key,
                    receipt_path.display()
                ))
            })?;
            receipt.verify_identity().map_err(|error| {
                CliError::failure(format!(
                    "Oven Alpha refuses pub::{} because its library receipt at {} is invalid: {error}",
                    artifact.dependency_key,
                    receipt_path.display()
                ))
            })?;
            receipt
        }
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                && !project_root
                    .join(oven_model::manifest::LOAF_MANIFEST_FILENAME)
                    .is_file() =>
        {
            mint_artifact_only_library_receipt(artifact, project_root, profile, &artifacts.intent, &receipt_path)?
        }
        Err(error) => {
            return Err(CliError::failure(format!(
                "Oven Alpha cannot read the `{profile}` library receipt for pub::{} at {}: {error}",
                artifact.dependency_key,
                receipt_path.display()
            )));
        }
    };
    if receipt.intent != artifacts.intent {
        return rebind_caller_owned_library_receipt(artifact, profile, &artifacts.intent, &receipt);
    }
    Ok(receipt)
}

/// Bind verified caller-owned provider source to the consumer's selected direct-Rustc cohort.
///
/// A producer receipt names the compiler cohort that originally materialized a library. A downstream consumer may
/// legitimately select a newer compatible toolchain or a different feature-unified project Loaf, so retaining that
/// historical intent would prevent the promised source re-materialization. The replacement receipt is in-memory and
/// records both the complete current artifact digest and the verified producer receipt identity; it does not mutate
/// the provider's stored receipt or invoke Cargo.
fn rebind_caller_owned_library_receipt(
    artifact: &LibraryArtifactMetadata,
    profile: &str,
    intent: &oven_store::OvenBuildIntent,
    producer_receipt: &oven_store::OvenReceipt,
) -> CliResult<oven_store::OvenReceipt> {
    if intent.profile != profile {
        return Err(CliError::failure(format!(
            "Oven Alpha cannot bind the `{profile}` provider receipt for pub::{} to selected `{}` artifacts",
            artifact.dependency_key, intent.profile
        )));
    }
    let manifest = LibraryManifest::read_from_path(&artifact.manifest_path).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot read the checked artifact manifest for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.manifest_path.display()
        ))
    })?;
    let artifact_digest = digest_provider_artifact(&artifact.crate_root).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot fingerprint re-materialized pub::{} artifact at {}: {error}",
            artifact.dependency_key,
            artifact.crate_root.display()
        ))
    })?;
    let receipt_request = OvenGeneratedProjectRequest::new(
        &artifact.crate_root,
        manifest.name,
        manifest.version,
        intent.target.clone(),
        intent.toolchain.clone(),
        profile,
        intent.features.clone(),
    )
    .with_generated_source("generated-root", &artifact.crate_lib_path)
    .with_generated_source_tree("generated-source-tree", artifact.crate_root.join("src"))
    .with_generated_source("provider-contract", &artifact.manifest_path)
    .with_build_unit_input("caller-owned-provider-digest", artifact_digest)
    .with_build_unit_input("producer-library-receipt", producer_receipt.identity.clone());
    receipt_generated_project(&receipt_request).map_err(|error| CliError::failure(error.to_string()))
}

/// Mint a local receipt for a checked source-free provider artifact without invoking Cargo.
///
/// A portable `.incnlib` package can intentionally ship only its generated Rust projection, so it has no project
/// manifest from which a nested `incan build --lib` could create a receipt. Its checked provider manifest, generated
/// source tree, and complete artifact digest are sufficient authority for a receipt bound to the already-selected
/// direct-Rustc intent. Source-backed dependencies still use the normal nested Oven library preparation path.
fn mint_artifact_only_library_receipt(
    artifact: &LibraryArtifactMetadata,
    project_root: &Path,
    profile: &str,
    intent: &oven_store::OvenBuildIntent,
    receipt_path: &Path,
) -> CliResult<oven_store::OvenReceipt> {
    if intent.profile != profile {
        return Err(CliError::failure(format!(
            "Oven Alpha cannot mint the `{profile}` receipt for pub::{} because the selected direct-Rustc plan uses `{}`",
            artifact.dependency_key, intent.profile
        )));
    }
    let manifest = LibraryManifest::read_from_path(&artifact.manifest_path).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot read the checked artifact manifest for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.manifest_path.display()
        ))
    })?;
    let source_tree = artifact.crate_root.join("src");
    let artifact_digest = digest_provider_artifact(&artifact.crate_root).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot fingerprint source-free pub::{} artifact at {}: {error}",
            artifact.dependency_key,
            artifact.crate_root.display()
        ))
    })?;
    let receipt_request = OvenGeneratedProjectRequest::new(
        project_root,
        manifest.name,
        manifest.version,
        intent.target.clone(),
        intent.toolchain.clone(),
        profile,
        intent.features.clone(),
    )
    .with_generated_source("generated-root", &artifact.crate_lib_path)
    .with_generated_source_tree("generated-source-tree", source_tree)
    .with_generated_source("provider-contract", &artifact.manifest_path)
    .with_build_unit_input("artifact-only-provider-digest", artifact_digest);
    let receipt = receipt_generated_project(&receipt_request).map_err(|error| CliError::failure(error.to_string()))?;
    write_receipt(&receipt, receipt_path).map_err(|error| CliError::failure(error.to_string()))?;
    Ok(receipt)
}

/// Parse the edition directly from a generated provider's checked Cargo projection.
pub fn caller_owned_library_edition(artifact: &LibraryArtifactMetadata) -> CliResult<String> {
    if !artifact.crate_lib_path.is_file() {
        return Err(CliError::failure(format!(
            "Oven Alpha cannot re-materialize pub::{} because its generated library source is absent at {}",
            artifact.dependency_key,
            artifact.crate_lib_path.display()
        )));
    }
    let cargo_manifest = fs::read_to_string(&artifact.cargo_toml_path).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot read the generated library manifest for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.cargo_toml_path.display()
        ))
    })?;
    let cargo_manifest = toml::from_str::<toml::Value>(&cargo_manifest).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot parse the generated library manifest for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.cargo_toml_path.display()
        ))
    })?;
    cargo_manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .and_then(|package| package.get("edition"))
        .and_then(toml::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha cannot re-materialize pub::{} because its generated library manifest has no package edition",
                artifact.dependency_key
            ))
        })
}

/// Determine whether the checked generated provider must be compiled as a procedural macro.
///
/// This is manifest interpretation only: it selects a direct-`rustc` crate type and never invokes Cargo or accepts
/// target-conditional metadata. A malformed value fails closed rather than being treated as an ordinary library.
pub fn caller_owned_library_is_proc_macro(artifact: &LibraryArtifactMetadata) -> CliResult<bool> {
    let manifest_text = fs::read_to_string(&artifact.cargo_toml_path).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot read the generated library manifest for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.cargo_toml_path.display()
        ))
    })?;
    let manifest = toml::from_str::<toml::Value>(&manifest_text).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot parse the generated library manifest for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.cargo_toml_path.display()
        ))
    })?;
    let Some(lib) = manifest.get("lib") else {
        return Ok(false);
    };
    let lib = lib.as_table().ok_or_else(|| {
        CliError::failure(format!(
            "Oven Alpha cannot re-materialize pub::{} because {} has a non-table [lib] declaration",
            artifact.dependency_key,
            artifact.cargo_toml_path.display()
        ))
    })?;
    lib.get("proc-macro")
        .map(|value| {
            value.as_bool().ok_or_else(|| {
                CliError::failure(format!(
                    "Oven Alpha cannot re-materialize pub::{} because {} has a non-boolean lib.proc-macro declaration",
                    artifact.dependency_key,
                    artifact.cargo_toml_path.display()
                ))
            })
        })
        .transpose()
        .map(|value| value.unwrap_or(false))
}

/// Recover the narrow Rust dependency closure required to compile a generated provider library.
///
/// The generated `Cargo.toml` remains a checked projection of the provider artifact, not an instruction to run
/// Cargo. Oven reads only its unconditional library dependencies and converts them to the existing direct-Rustc
/// dependency representation. Target-conditional, workspace-inherited, Git, and malformed declarations fail closed
/// because selecting those semantics would reintroduce an unreceipted resolver policy.
pub fn caller_owned_library_rust_dependencies(artifact: &LibraryArtifactMetadata) -> CliResult<Vec<DependencySpec>> {
    let manifest_text = fs::read_to_string(&artifact.cargo_toml_path).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot read the generated library manifest for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.cargo_toml_path.display()
        ))
    })?;
    let manifest = toml::from_str::<toml::Value>(&manifest_text).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot parse the generated library manifest for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.cargo_toml_path.display()
        ))
    })?;
    if manifest.get("target").is_some() {
        return Err(CliError::failure(format!(
            "Oven Alpha cannot re-materialize pub::{} because {} declares target-conditional Rust dependencies; prepare an explicit Oven-native closure",
            artifact.dependency_key,
            artifact.cargo_toml_path.display()
        )));
    }
    let manifest_directory = artifact.cargo_toml_path.parent().ok_or_else(|| {
        CliError::failure(format!(
            "Oven Alpha cannot determine the generated library directory for pub::{} at {}",
            artifact.dependency_key,
            artifact.cargo_toml_path.display()
        ))
    })?;
    let mut dependencies = BTreeMap::new();
    let Some(dependency_table) = manifest.get("dependencies").and_then(toml::Value::as_table) else {
        return Ok(Vec::new());
    };
    for (crate_name, value) in dependency_table {
        let dependency = match value {
            toml::Value::String(version) => DependencySpec {
                crate_name: crate_name.clone(),
                version: Some(version.clone()),
                features: Vec::new(),
                default_features: true,
                source: DependencySource::Registry,
                optional: false,
                package: None,
            },
            toml::Value::Table(table) => {
                if table.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
                    return Err(CliError::failure(format!(
                        "Oven Alpha cannot re-materialize pub::{} because dependency `{crate_name}` inherits a Cargo workspace declaration; prepare an explicit Oven-native closure",
                        artifact.dependency_key
                    )));
                }
                if table.get("git").is_some() {
                    return Err(CliError::failure(format!(
                        "Oven Alpha cannot re-materialize pub::{} because dependency `{crate_name}` is Git-sourced; prepare an explicit Oven-native closure",
                        artifact.dependency_key
                    )));
                }
                let features = table
                    .get("features")
                    .map(|features| {
                        features
                            .as_array()
                            .ok_or_else(|| {
                                CliError::failure(format!(
                                    "Oven Alpha cannot re-materialize pub::{} because dependency `{crate_name}` has a non-array feature declaration",
                                    artifact.dependency_key
                                ))
                            })?
                            .iter()
                            .map(|feature| {
                                feature.as_str().map(str::to_string).ok_or_else(|| {
                                    CliError::failure(format!(
                                        "Oven Alpha cannot re-materialize pub::{} because dependency `{crate_name}` has a non-string feature",
                                        artifact.dependency_key
                                    ))
                                })
                            })
                            .collect::<CliResult<Vec<_>>>()
                    })
                    .transpose()?
                    .unwrap_or_default();
                let package = table
                    .get("package")
                    .map(|package| {
                        package.as_str().map(str::to_string).ok_or_else(|| {
                            CliError::failure(format!(
                                "Oven Alpha cannot re-materialize pub::{} because dependency `{crate_name}` has a non-string package alias",
                                artifact.dependency_key
                            ))
                        })
                    })
                    .transpose()?;
                let default_features = table
                    .get("default-features")
                    .map(|value| {
                        value.as_bool().ok_or_else(|| {
                            CliError::failure(format!(
                                "Oven Alpha cannot re-materialize pub::{} because dependency `{crate_name}` has a non-boolean default-features value",
                                artifact.dependency_key
                            ))
                        })
                    })
                    .transpose()?
                    .unwrap_or(true);
                let optional = table
                    .get("optional")
                    .map(|value| {
                        value.as_bool().ok_or_else(|| {
                            CliError::failure(format!(
                                "Oven Alpha cannot re-materialize pub::{} because dependency `{crate_name}` has a non-boolean optional value",
                                artifact.dependency_key
                            ))
                        })
                    })
                    .transpose()?
                    .unwrap_or(false);
                let version = table
                    .get("version")
                    .map(|value| {
                        value.as_str().map(str::to_string).ok_or_else(|| {
                            CliError::failure(format!(
                                "Oven Alpha cannot re-materialize pub::{} because dependency `{crate_name}` has a non-string version",
                                artifact.dependency_key
                            ))
                        })
                    })
                    .transpose()?;
                let source = match table.get("path") {
                    Some(path) => {
                        let path = path.as_str().ok_or_else(|| {
                            CliError::failure(format!(
                                "Oven Alpha cannot re-materialize pub::{} because dependency `{crate_name}` has a non-string path",
                                artifact.dependency_key
                            ))
                        })?;
                        DependencySource::Path {
                            path: manifest_directory.join(path),
                        }
                    }
                    None => {
                        if version.is_none() {
                            return Err(CliError::failure(format!(
                                "Oven Alpha cannot re-materialize pub::{} because registry dependency `{crate_name}` has no version requirement",
                                artifact.dependency_key
                            )));
                        }
                        DependencySource::Registry
                    }
                };
                DependencySpec {
                    crate_name: crate_name.clone(),
                    version,
                    features,
                    default_features,
                    source,
                    optional,
                    package,
                }
            }
            _ => {
                return Err(CliError::failure(format!(
                    "Oven Alpha cannot re-materialize pub::{} because dependency `{crate_name}` has an unsupported Cargo manifest shape",
                    artifact.dependency_key
                )));
            }
        }
        .normalized();
        if let Some(existing) = dependencies.insert(crate_name.clone(), dependency.clone())
            && existing != dependency
        {
            return Err(CliError::failure(format!(
                "Oven Alpha cannot re-materialize pub::{} because generated manifest dependency `{crate_name}` is ambiguous",
                artifact.dependency_key
            )));
        }
    }
    Ok(dependencies.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::PathBuf;

    use incan_frontend::library_manifest::{LibraryManifest, ProviderDependencyKind, ProviderDependencyMetadata};
    use incan_frontend::library_manifest_index::{LibraryArtifactKind, LibraryArtifactMetadata};
    use oven_interop::{
        OvenInteropCapabilitySelection, default_interop_execution_receipt_path, interop_execution_build_unit_inputs,
        receipt_interop_execution, write_interop_execution_receipt,
    };
    use oven_model::lock::{CargoFeatureSelection, IncanLock, LockedOvenState, SemanticLockState};
    use oven_model::manifest::{DependencySource, DependencySpec, ProjectManifest};
    use oven_model::oven_interop::locked_oven_interop_targets;
    use oven_rustc::rustc::{OvenCallerOwnedRustcLibrary, OvenRustcArtifactPlan};
    use oven_store::{OvenGeneratedProjectRequest, receipt_generated_project};

    #[test]
    fn normal_oven_build_inputs_require_a_current_selected_interop_receipt() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("interop/include"))?;
        let header = project.path().join("interop/include/bridge.h");
        fs::write(&header, "int incan_bridge(void);\n")?;
        let manifest_source = r#"
[interop.c]
schema = 1

[[interop.c.targets]]
target = "aarch64-apple-darwin"
toolchain = { capability = "apple-clang", version = ">=17, <18" }
sdk = { capability = "macosx", version = ">=18, <19" }
headers = ["interop/include/bridge.h"]
"#;
        let manifest_path = project.path().join("loaf.toml");
        fs::write(&manifest_path, manifest_source)?;
        let manifest = ProjectManifest::from_str(manifest_source, &manifest_path)?;
        let locked = locked_oven_interop_targets(&manifest)?;
        IncanLock::new_with_semantic(
            incan_lang::version::INCAN_VERSION,
            "fixture".to_string(),
            CargoFeatureSelection::default(),
            SemanticLockState {
                oven: Some(LockedOvenState {
                    interop: locked.clone(),
                }),
                ..SemanticLockState::default()
            },
            String::new(),
        )
        .write(&project.path().join("oven.lock"))?;
        let receipt = receipt_interop_execution(
            &locked[0],
            Some(OvenInteropCapabilitySelection {
                capability: "apple-clang".to_string(),
                version: "17.0.6".to_string(),
                identity: "sha256:clang".to_string(),
            }),
            Some(OvenInteropCapabilitySelection {
                capability: "macosx".to_string(),
                version: "18.5.0".to_string(),
                identity: "sha256:sdk".to_string(),
            }),
        )?;
        write_interop_execution_receipt(
            &receipt,
            default_interop_execution_receipt_path(project.path(), "aarch64-apple-darwin"),
        )?;
        let mut inputs = BTreeMap::new();
        append_oven_interop_execution_build_inputs(&mut inputs, Some(&manifest), "aarch64-apple-darwin")?;
        assert_eq!(inputs, interop_execution_build_unit_inputs(&receipt));

        fs::write(header, "int incan_bridge_changed(void);\n")?;
        assert!(
            append_oven_interop_execution_build_inputs(&mut BTreeMap::new(), Some(&manifest), "aarch64-apple-darwin")
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn source_free_provider_artifact_mints_a_verified_oven_receipt() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let project_root = workspace.path();
        let artifact_root = project_root.join("target/lib");
        let source_root = artifact_root.join("src");
        fs::create_dir_all(&source_root)?;
        let crate_lib_path = source_root.join("lib.rs");
        fs::write(&crate_lib_path, "pub fn provider() {}\n")?;
        fs::write(
            artifact_root.join("Cargo.toml"),
            "[package]\nname = \"provider\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        let manifest_path = artifact_root.join("provider.incnlib");
        LibraryManifest::new("provider", "0.1.0").write_to_path(&manifest_path)?;
        let artifact = LibraryArtifactMetadata {
            dependency_key: "provider".to_string(),
            manifest_name: "provider".to_string(),
            manifest_path,
            crate_root: artifact_root.clone(),
            cargo_toml_path: artifact_root.join("Cargo.toml"),
            crate_lib_path,
            kind: LibraryArtifactKind::Materialized,
        };
        let intent = oven_store::OvenBuildIntent {
            target: "aarch64-apple-darwin".to_string(),
            toolchain: "rustc test".to_string(),
            profile: "debug".to_string(),
            features: Vec::new(),
        };
        let receipt_path = oven_store::default_receipt_path(project_root).with_file_name("library-debug-receipt.json");

        let receipt = mint_artifact_only_library_receipt(&artifact, project_root, "debug", &intent, &receipt_path)?;

        assert_eq!(receipt.intent, intent);
        assert!(receipt_path.is_file());
        receipt.verify_identity()?;
        Ok(())
    }

    #[test]
    fn caller_owned_provider_receipt_rebinds_to_the_selected_consumer_cohort() -> Result<(), Box<dyn std::error::Error>>
    {
        let workspace = tempfile::tempdir()?;
        let artifact_root = workspace.path().join("target/lib");
        fs::create_dir_all(artifact_root.join("src"))?;
        fs::write(
            artifact_root.join("Cargo.toml"),
            "[package]\nname = \"provider\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        let crate_lib_path = artifact_root.join("src/lib.rs");
        fs::write(&crate_lib_path, "pub fn provider() {}\n")?;
        let manifest_path = artifact_root.join("provider.incnlib");
        LibraryManifest::new("provider", "0.1.0").write_to_path(&manifest_path)?;
        let artifact = LibraryArtifactMetadata {
            dependency_key: "provider".to_string(),
            manifest_name: "provider".to_string(),
            manifest_path: manifest_path.clone(),
            crate_root: artifact_root.clone(),
            cargo_toml_path: artifact_root.join("Cargo.toml"),
            crate_lib_path: crate_lib_path.clone(),
            kind: LibraryArtifactKind::Materialized,
        };
        let producer_request = OvenGeneratedProjectRequest::new(
            workspace.path(),
            "provider",
            "0.1.0",
            "aarch64-apple-darwin",
            "rustc 1.95.0",
            "debug",
            Vec::new(),
        )
        .with_generated_source("generated-root", &crate_lib_path)
        .with_generated_source_tree("generated-source-tree", artifact_root.join("src"))
        .with_generated_source("provider-contract", &manifest_path);
        let producer_receipt = receipt_generated_project(&producer_request)?;
        let consumer_intent = oven_store::OvenBuildIntent {
            target: "aarch64-apple-darwin".to_string(),
            toolchain: "rustc 1.99.0-nightly".to_string(),
            profile: "debug".to_string(),
            features: Vec::new(),
        };

        let rebound = rebind_caller_owned_library_receipt(&artifact, "debug", &consumer_intent, &producer_receipt)?;

        assert_eq!(rebound.intent, consumer_intent);
        assert_eq!(
            rebound.sources.build_unit_inputs.get("producer-library-receipt"),
            Some(&producer_receipt.identity)
        );
        rebound.verify_identity()?;
        Ok(())
    }

    #[test]
    fn caller_owned_provider_proc_macro_is_classified_from_its_checked_manifest()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let artifact_root = workspace.path().join("target/lib");
        fs::create_dir_all(artifact_root.join("src"))?;
        fs::write(
            artifact_root.join("Cargo.toml"),
            "[package]\nname = \"provider_macros\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\nproc-macro = true\n",
        )?;
        fs::write(artifact_root.join("src/lib.rs"), "pub fn marker() {}\n")?;
        let artifact = LibraryArtifactMetadata {
            dependency_key: "provider_macros".to_string(),
            manifest_name: "provider_macros".to_string(),
            manifest_path: workspace.path().join("provider_macros.incnlib"),
            crate_root: artifact_root.clone(),
            cargo_toml_path: artifact_root.join("Cargo.toml"),
            crate_lib_path: artifact_root.join("src/lib.rs"),
            kind: LibraryArtifactKind::Materialized,
        };

        assert!(caller_owned_library_is_proc_macro(&artifact)?);
        Ok(())
    }

    #[test]
    fn caller_owned_re_materialization_requires_only_private_provider_edges_in_the_foundation_plan() {
        let mut manifest = LibraryManifest::new("set_library", "0.1.0");
        manifest
            .contract_metadata
            .provider
            .provider_dependencies
            .push(ProviderDependencyMetadata {
                kind: ProviderDependencyKind::PrivateImplementation,
                dependency_key: "incan_stdlib_core".to_string(),
                provider_name: "incan_stdlib_core".to_string(),
                provider_version: "0.5.0".to_string(),
                artifact_digest: "sha256:core".to_string(),
                relative_artifact_path: "providers/stdlib-core".to_string(),
                requested_features: BTreeSet::new(),
                default_features: false,
                optional: false,
            });
        let plan = OvenRustcArtifactPlan {
            source_path_projection: None,
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: vec![("incan_stdlib_core".to_string(), PathBuf::from("core.rlib"))],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };
        assert!(first_unselected_private_provider_edge(&manifest, &plan).is_none());

        manifest.contract_metadata.provider.provider_dependencies[0].dependency_key = "missing_core".to_string();
        assert_eq!(
            first_unselected_private_provider_edge(&manifest, &plan)
                .map(|dependency| dependency.dependency_key.as_str()),
            Some("missing_core")
        );

        manifest.contract_metadata.provider.provider_dependencies[0].kind = ProviderDependencyKind::PublicPackage;
        manifest.contract_metadata.provider.provider_dependencies[0].dependency_key = "incan_stdlib_core".to_string();
        assert_eq!(
            first_unselected_private_provider_edge(&manifest, &plan)
                .map(|dependency| dependency.dependency_key.as_str()),
            None
        );
    }

    #[test]
    fn caller_owned_provider_graph_prefers_checked_public_edges_over_duplicate_cargo_projection_paths() {
        let mut manifest = LibraryManifest::new("parent", "0.1.0");
        manifest
            .contract_metadata
            .provider
            .provider_dependencies
            .push(ProviderDependencyMetadata {
                kind: ProviderDependencyKind::PublicPackage,
                dependency_key: "compiled-leaf".to_string(),
                provider_name: "compiled_leaf".to_string(),
                provider_version: "0.1.0".to_string(),
                artifact_digest: "sha256:leaf".to_string(),
                relative_artifact_path: "providers/compiled-leaf".to_string(),
                requested_features: BTreeSet::new(),
                default_features: false,
                optional: false,
            });
        let dependencies = vec![
            DependencySpec {
                crate_name: "compiled_leaf".to_string(),
                version: None,
                features: Vec::new(),
                default_features: true,
                source: DependencySource::Path {
                    path: PathBuf::from("generated/compiled-leaf"),
                },
                optional: false,
                package: None,
            },
            DependencySpec {
                crate_name: "rust_shadow".to_string(),
                version: None,
                features: Vec::new(),
                default_features: true,
                source: DependencySource::Path {
                    path: PathBuf::from("generated/rust-shadow"),
                },
                optional: false,
                package: None,
            },
        ];

        let remaining = caller_owned_library_dependencies_without_public_provider_edges(dependencies, &manifest);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].crate_name, "rust_shadow");
    }

    #[test]
    fn caller_owned_library_deduplication_keeps_an_identical_direct_extern() {
        let output = PathBuf::from("direct-rustc/rust-shadow.rlib");
        let mut libraries = vec![
            OvenCallerOwnedRustcLibrary {
                crate_name: "rust_shadow".to_string(),
                output: output.clone(),
                digest: "sha256:rust-shadow".to_string(),
                expose_extern: false,
            },
            OvenCallerOwnedRustcLibrary {
                crate_name: "rust_shadow".to_string(),
                output,
                digest: "sha256:rust-shadow".to_string(),
                expose_extern: true,
            },
        ];

        deduplicate_caller_owned_libraries_prefer_extern(&mut libraries);
        assert_eq!(libraries.len(), 1);
        assert!(libraries[0].expose_extern);
    }
}
