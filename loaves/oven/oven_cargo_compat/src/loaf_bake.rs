//! The explicit Loaf bake: a generated project's closure is prepared through the compatibility publisher, its
//! registry lock sealed, its inspection sources merged, and the compiler's vocab-support helpers baked and copied
//! into the envelope. This is the half of Oven's Loaf handling that runs Cargo, and it sits here, over
//! `oven_rustc::loaf`'s model, so that the Loaf model, selection and validation never do.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use oven_model::compiler_identity::CompilerIdentity;
use oven_rustc::loaf::{
    LoafTemporaryDirectory, OVEN_LOAF_SCHEMA_VERSION, OvenLoaf, OvenLoafAccounting, OvenLoafCompatibility,
    OvenLoafError, OvenLoafPreparation, OvenLoafProvenance, loaf_directory_byte_counts,
};
use oven_rustc::rustc::{
    OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH, OvenRustcArtifactExtern, OvenRustcArtifactManifest, OvenRustcRegistryLeaf,
    OvenRustcRegistrySource, OvenRustcRegistrySourcePackage, OvenRustcSupportingArtifact,
};
use oven_store::store::{OvenArtifactKind, OvenStore};
use oven_store::{OvenProviderHooks, OvenReceipt, digest_bytes};

use crate::{
    OvenLegacyCargoDirectDependencyClosure, OvenLegacyCargoError, OvenLegacyCargoInspectionPackage,
    OvenLegacyCargoInspectionSource, OvenLegacyCargoPrepareRequest, OvenLegacyCargoPublicationKind,
    OvenLegacyCargoSelectedUnitCapture, canonicalize_supporting_artifacts, copy_regular_directory_tree,
    direct_rustc_compile_environment, materialized_files_from_directory, prepare_direct_rustc_plan,
};

pub mod vocab_support;

pub use vocab_support::bake_source_compiler_vocab_support;
use vocab_support::{bake_compiler_vocab_support, is_incan_derive_artifact, is_named_rlib};

/// Explicit resources and bounded policy available to one hidden legacy-Cargo Loaf bake.
pub struct OvenLoafBakerContext<'a> {
    /// The compiler this Loaf is baked for; its version and provider revision are sealed into the provenance.
    pub compiler: &'a CompilerIdentity,
    /// The provider facts the bake's publisher asks the compiler for.
    pub provider_hooks: Arc<dyn OvenProviderHooks>,
    /// Compiler source root whose checked support crates and lock authority are being packaged.
    pub compiler_root: &'a Path,
    pub compiler_support_target: &'a Path,
    /// Every baker-owned persistent or transient root charged to the replacement high-water mark.
    pub capacity_roots: [&'a Path; 2],
    pub transient_limit: u64,
    pub cargo: &'a Path,
    pub rustc: &'a Path,
    /// Checked Rust dependency surface whose source is sealed into this one Loaf.
    pub inspection_packages: &'a [OvenLegacyCargoInspectionPackage],
    /// Locked source authority resolved once from the compiler root at the explicit baker boundary.
    ///
    /// Compiler-suite foundation Loafs retain this independently from their linkable generated-project leaves.
    pub inspection_sources: &'a [OvenLegacyCargoInspectionSource],
    /// Whether this broad foundation Loaf exposes every registry rlib actually emitted into its coherent closure.
    pub retain_complete_registry_leaves: bool,
    /// Whether the complete checked fixture dependency surface is direct-linkable by generated standard-library code.
    pub retain_checked_direct_dependencies: bool,
    pub limits: oven_store::store::OvenStoreLimits,
}

/// One exported Loaf and the physical Cargo unit selection captured by the same publisher transaction.
pub struct OvenPreparedLoafWithSelectedUnits {
    pub preparation: OvenLoafPreparation,
    pub selected_units: Option<super::OvenLegacyCargoSelectedUnitCapture>,
}

/// Export one compiler-owned Loaf from an already receipted generated Incan project.
///
/// Release packaging first drives the compiler's ordinary Oven analysis for a small in-package Incan program. That
/// produces the same provider, SDK, feature, dependency, target, and toolchain identity that an everyday command
/// would use. This explicit publisher then converts only that exact generated project into an immutable direct-rustc
/// loaf; its temporary store is dropped before the release archive is created.
pub fn prepare_loaf_from_generated_project(
    loaf_root: &Path,
    context: &OvenLoafBakerContext<'_>,
    receipt: OvenReceipt,
    generated_project: &Path,
) -> Result<OvenLoafPreparation, OvenLoafError> {
    prepare_loaf_from_generated_project_with_selected_units(loaf_root, context, receipt, generated_project)
        .map(|prepared| prepared.preparation)
}

/// Export one Loaf and retain its actual Cargo-selected physical unit capture for foundation publication.
pub fn prepare_loaf_from_generated_project_with_selected_units(
    loaf_root: &Path,
    context: &OvenLoafBakerContext<'_>,
    receipt: OvenReceipt,
    generated_project: &Path,
) -> Result<OvenPreparedLoafWithSelectedUnits, OvenLoafError> {
    prepare_loaf_from_generated_project_with_selected_unit_bindings(
        loaf_root,
        context,
        receipt,
        generated_project,
        None,
    )
}

/// Export one Loaf while binding exact physical capture identities to finalized selected units.
pub fn prepare_loaf_from_generated_project_with_selected_unit_bindings(
    loaf_root: &Path,
    context: &OvenLoafBakerContext<'_>,
    receipt: OvenReceipt,
    generated_project: &Path,
    selected_unit_bindings: Option<&BTreeMap<String, String>>,
) -> Result<OvenPreparedLoafWithSelectedUnits, OvenLoafError> {
    if loaf_root.exists() && !loaf_root.is_dir() {
        return Err(OvenLoafError::Preparation {
            message: format!("loaf root is not a directory: {}", loaf_root.display()),
        });
    }
    fs::create_dir_all(loaf_root).map_err(|source| OvenLoafError::Io {
        path: loaf_root.to_path_buf(),
        source,
    })?;
    let store_root =
        LoafTemporaryDirectory::create(loaf_root, ".incan-oven-loaf-store-").map_err(|source| OvenLoafError::Io {
            path: loaf_root.to_path_buf(),
            source,
        })?;
    let store = OvenStore::new(store_root.path(), context.limits);
    let generated_source = generated_project.join("src/main.rs");
    let compile_environment = direct_rustc_compile_environment(generated_project, &generated_source)?;
    let mut publication = prepare_direct_rustc_plan(&OvenLegacyCargoPrepareRequest {
        compiler: context.compiler.clone(),
        provider_hooks: context.provider_hooks.clone(),
        store: &store,
        receipt: receipt.clone(),
        generated_project: generated_project.to_path_buf(),
        cargo: context.cargo.to_path_buf(),
        rustc: context.rustc.to_path_buf(),
        sdk_inventory: None,
        compiler_loaf_root: None,
        domain: format!("toolchain-base-{}", receipt.intent.profile),
        publication_kind: OvenLegacyCargoPublicationKind::Executable,
        source_evidence_key: "generated-root".to_string(),
        compile_environment,
        inspection_packages: (!context.retain_complete_registry_leaves).then(|| context.inspection_packages.to_vec()),
        direct_dependency_closure: if context.retain_checked_direct_dependencies {
            OvenLegacyCargoDirectDependencyClosure::CheckedDeclared
        } else {
            OvenLegacyCargoDirectDependencyClosure::GeneratedSource
        },
        provider_compilations: &[],
        compact_debug_info: true,
        source_compiler_vocab_support: false,
        base_loaf: None,
    })?;
    let identity = receipt
        .build_unit_identity
        .strip_prefix("sha256:")
        .unwrap_or(receipt.build_unit_identity.as_str());
    let output_directory = loaf_root.join(format!(".building-{identity}.loaf"));
    if output_directory.exists() {
        return Err(OvenLoafError::Preparation {
            message: format!("loaf destination already exists: {}", output_directory.display()),
        });
    }
    let mut selected_units = publication.selected_units.take();
    if let Some(selected_units) = selected_units.as_mut() {
        super::bind_legacy_cargo_selected_registry_sources(selected_units, context.inspection_sources).map_err(
            |error| OvenLoafError::Preparation {
                message: error.to_string(),
            },
        )?;
    }
    if let Some(bindings) = selected_unit_bindings {
        let selected_units = selected_units.as_ref().ok_or_else(|| OvenLoafError::Preparation {
            message: "selected-unit bindings require an exact physical capture".to_string(),
        })?;
        bind_registry_leaf_selected_unit_identities(&mut publication.registry_leaves, selected_units, bindings)?;
    }
    let result = export_loaf(
        &store,
        &publication.plan_identity,
        &receipt,
        context,
        publication.transient_reservation_bytes,
        publication.registry_leaves,
        &output_directory,
    )?;
    let loaf_name = result
        .loaf_identity
        .strip_prefix("sha256:")
        .unwrap_or(&result.loaf_identity);
    let content_directory = loaf_root.join(format!("{loaf_name}.loaf"));
    if content_directory.exists() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "content-addressed Loaf destination already exists: {}",
                content_directory.display()
            ),
        });
    }
    fs::rename(&output_directory, &content_directory).map_err(|source| OvenLoafError::Io {
        path: content_directory,
        source,
    })?;
    Ok(OvenPreparedLoafWithSelectedUnits {
        preparation: result,
        selected_units,
    })
}

/// Bind every exported registry artifact to exactly one authenticated selected physical unit.
fn bind_registry_leaf_selected_unit_identities(
    leaves: &mut [OvenRustcRegistryLeaf],
    _selected_units: &OvenLegacyCargoSelectedUnitCapture,
    bindings: &BTreeMap<String, String>,
) -> Result<(), OvenLoafError> {
    let mut used = BTreeSet::new();
    for leaf in leaves {
        let capture_identity = leaf
            .selected_unit_identity
            .as_ref()
            .ok_or_else(|| OvenLoafError::Preparation {
                message: "registry artifact lacks its traced physical-unit identity".to_string(),
            })?;
        let selected_identity = bindings
            .get(capture_identity)
            .ok_or_else(|| OvenLoafError::Preparation {
                message: "registry artifact has no authenticated selected-unit binding".to_string(),
            })?;
        if !used.insert(selected_identity.clone()) {
            return Err(OvenLoafError::Preparation {
                message: "selected unit is bound to more than one registry artifact".to_string(),
            });
        }
        leaf.selected_unit_identity = Some(selected_identity.clone());
    }
    if used.len() != bindings.len() {
        return Err(OvenLoafError::Preparation {
            message: "selected-unit bindings contain an uncompiled physical unit".to_string(),
        });
    }
    Ok(())
}

/// Copy a fully verified temporary store entry into the compiler-owned loaf layout and report its accounting.
fn export_loaf(
    store: &OvenStore,
    plan_identity: &str,
    receipt: &OvenReceipt,
    context: &OvenLoafBakerContext<'_>,
    publisher_transient_peak: u64,
    registry_leaves: Vec<OvenRustcRegistryLeaf>,
    output_directory: &Path,
) -> Result<OvenLoafPreparation, OvenLoafError> {
    let inspection = store.inspect()?;
    let entry = inspection
        .entries
        .iter()
        .find(|entry| entry.manifest.identity == plan_identity)
        .ok_or_else(|| OvenLoafError::Preparation {
            message: format!("temporary Loaf plan {plan_identity} is absent after publication"),
        })?;
    if entry.manifest.kind != OvenArtifactKind::DirectRustcPlan
        || entry.manifest.build_unit_identity != receipt.build_unit_identity
        || entry.manifest.intent != receipt.intent
    {
        return Err(OvenLoafError::Preparation {
            message: "temporary Loaf plan does not match its base runtime receipt".to_string(),
        });
    }
    let (_manifest, artifact_root, payload, _lease) = store.select_payload_for_execution(plan_identity)?;
    let mut plan =
        serde_json::from_slice::<OvenRustcArtifactManifest>(&payload).map_err(|error| OvenLoafError::Preparation {
            message: format!("temporary Loaf payload is not a direct-rustc plan: {error}"),
        })?;
    record_generated_root_externs(&mut plan)?;
    promote_compiler_runtime_externs(&mut plan)?;
    plan.registry_leaves = registry_leaves.clone();
    let materialized_files = plan.materialized_artifacts(&artifact_root, &receipt.intent)?;
    let parent = output_directory.parent().ok_or_else(|| OvenLoafError::Preparation {
        message: format!("loaf destination has no parent: {}", output_directory.display()),
    })?;
    let staging = LoafTemporaryDirectory::create(parent, ".incan-oven-loaf-").map_err(|source| OvenLoafError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    for file in materialized_files {
        let destination = staging.path().join(&file.relative_path);
        let destination_parent = destination.parent().ok_or_else(|| OvenLoafError::Preparation {
            message: format!("loaf artifact has no parent: {}", file.relative_path),
        })?;
        fs::create_dir_all(destination_parent).map_err(|source_error| OvenLoafError::Io {
            path: destination_parent.to_path_buf(),
            source: source_error,
        })?;
        fs::copy(&file.source_path, &destination).map_err(|source_error| OvenLoafError::Io {
            path: file.source_path,
            source: source_error,
        })?;
    }
    merge_loaf_inspection_sources(&mut plan, staging.path(), context.inspection_sources)?;
    seal_registry_lock_from_temporary_store(&mut plan, &artifact_root, staging.path())?;
    let vocab_transient_peak = bake_compiler_vocab_support(&mut plan, staging.path(), context)?;
    plan.materialized_artifacts(staging.path(), &receipt.intent)?;
    let (payload_logical_bytes, payload_physical_bytes) = loaf_directory_byte_counts(staging.path())?;
    // Export rewrites publisher-store paths and adds compiler-owned runtime/vocabulary inputs. Report the identity
    // of this final sealed plan, which is also what exact warm validation observes, rather than the discarded
    // temporary store entry identity.
    let plan_identity = digest_bytes(&serde_json::to_vec(&plan).map_err(|error| OvenLoafError::Preparation {
        message: format!("could not encode sealed Loaf plan identity: {error}"),
    })?);
    let loaf = OvenLoaf {
        schema_version: OVEN_LOAF_SCHEMA_VERSION,
        build_unit_identity: receipt.build_unit_identity.clone(),
        provenance: OvenLoafProvenance {
            compiler_version: context.compiler.version.clone(),
            rust_toolchain: receipt.intent.toolchain.clone(),
            sdk_provider_codegen_revision: context.compiler.sdk_provider_codegen_revision.to_string(),
            baker: "legacy_cargo".to_string(),
        },
        accounting: OvenLoafAccounting {
            payload_logical_bytes,
            payload_physical_bytes,
        },
        compatibility: OvenLoafCompatibility::from_receipt(receipt)?,
        registry_leaves,
        plan,
    };
    let loaf_bytes = serde_json::to_vec_pretty(&loaf).map_err(|error| OvenLoafError::Preparation {
        message: format!("could not encode Loaf: {error}"),
    })?;
    let loaf_identity = digest_bytes(&loaf_bytes);
    let loaf_path = staging.path().join("loaf.json");
    fs::write(&loaf_path, loaf_bytes).map_err(|source| OvenLoafError::Io {
        path: loaf_path,
        source,
    })?;
    fs::rename(staging.path(), output_directory).map_err(|source| OvenLoafError::Io {
        path: output_directory.to_path_buf(),
        source,
    })?;
    let _ = staging.persist();
    let (logical_bytes, physical_bytes) = loaf_directory_byte_counts(output_directory)?;
    Ok(OvenLoafPreparation {
        build_unit_identity: receipt.build_unit_identity.clone(),
        loaf_identity,
        plan_identity,
        logical_bytes,
        physical_bytes,
        transient_peak_physical_bytes: publisher_transient_peak.max(vocab_transient_peak),
    })
}

/// Carry the publisher's checked registry lock across the Loaf export boundary.
///
/// The temporary publisher store deliberately keeps the lock outside the executable artifact plan: it is inspection
/// authority, not a direct-rustc linker input. A shipped Loaf nevertheless needs that same immutable authority when
/// it exposes registry sources, so copy it into the final bundle and declare it as a verified supporting artifact.
fn seal_registry_lock_from_temporary_store(
    plan: &mut OvenRustcArtifactManifest,
    artifact_root: &Path,
    loaf_staging: &Path,
) -> Result<(), OvenLoafError> {
    if plan.registry_sources.is_empty() {
        return Ok(());
    }
    if plan
        .supporting_artifacts
        .iter()
        .any(|artifact| artifact.relative_path == OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH)
    {
        return Err(OvenLoafError::Preparation {
            message: "direct-rustc plan must not predeclare the sealed registry Cargo.lock".to_string(),
        });
    }
    let source_path = artifact_root.join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH);
    let source_metadata = fs::symlink_metadata(&source_path).map_err(|source| OvenLoafError::Io {
        path: source_path.clone(),
        source,
    })?;
    if source_metadata.file_type().is_symlink() || !source_metadata.is_file() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "publisher registry lock must be a regular file: {}",
                source_path.display()
            ),
        });
    }
    let destination = loaf_staging.join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH);
    let parent = destination.parent().ok_or_else(|| OvenLoafError::Preparation {
        message: "Loaf registry lock has no parent directory".to_string(),
    })?;
    fs::create_dir_all(parent).map_err(|source| OvenLoafError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    fs::copy(&source_path, &destination).map_err(|source| OvenLoafError::Io {
        path: source_path.clone(),
        source,
    })?;
    let digest = digest_bytes(&fs::read(&destination).map_err(|source| OvenLoafError::Io {
        path: destination,
        source,
    })?);
    plan.supporting_artifacts.push(OvenRustcSupportingArtifact {
        relative_path: OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH.to_string(),
        digest,
    });
    canonicalize_supporting_artifacts(&mut plan.supporting_artifacts)?;
    Ok(())
}

/// Seal independently resolved registry sources into a Loaf without inventing linkable artifacts.
///
/// The compiler-suite source manifest may name packages that its small foundation program never imports. Those
/// packages still need immutable source authority for locked Rust-interoperability tests, but they must not appear as
/// externs or registry leaves. Existing source records are merged only when their complete identity agrees.
fn merge_loaf_inspection_sources(
    plan: &mut OvenRustcArtifactManifest,
    loaf_staging: &Path,
    sources: &[OvenLegacyCargoInspectionSource],
) -> Result<(), OvenLoafError> {
    for source in sources {
        let directory_name = source
            .source_root
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| OvenLoafError::Preparation {
                message: format!(
                    "sealed registry source for `{}` {} has no portable directory identity",
                    source.package, source.version
                ),
            })?;
        let relative_root = format!("registry-sources/{directory_name}");
        let destination = loaf_staging.join(&relative_root);
        if destination.exists() {
            let digest =
                oven_store::digest_source_tree(&destination).map_err(|message| OvenLoafError::Preparation {
                    message: format!(
                        "could not verify existing sealed source for `{}` {}: {message}",
                        source.package, source.version
                    ),
                })?;
            if digest != source.source_digest {
                return Err(OvenLoafError::Preparation {
                    message: format!(
                        "sealed registry source for `{}` {} conflicts with existing Loaf content",
                        source.package, source.version
                    ),
                });
            }
        } else {
            copy_regular_directory_tree(&source.source_root, &destination, "registry inspection source")?;
        }
        let actual_members = materialized_files_from_directory(&destination, "", "registry inspection source")?
            .into_iter()
            .map(|file| {
                let path = file
                    .relative_path
                    .strip_prefix('/')
                    .ok_or_else(|| OvenLoafError::Preparation {
                        message: "sealed registry source member lost its package-relative prefix".to_string(),
                    })?;
                Ok((
                    path.to_string(),
                    digest_bytes(&fs::read(&file.source_path).map_err(|source| OvenLoafError::Io {
                        path: file.source_path,
                        source,
                    })?),
                ))
            })
            .collect::<Result<Vec<_>, OvenLoafError>>()?;
        let expected_members = source
            .members
            .iter()
            .map(|member| (member.path.clone(), member.digest.clone()))
            .collect::<Vec<_>>();
        if actual_members != expected_members {
            return Err(OvenLoafError::Preparation {
                message: format!(
                    "sealed registry source for `{}` {} does not match its staged member inventory",
                    source.package, source.version
                ),
            });
        }
        for file in materialized_files_from_directory(&destination, &relative_root, "registry inspection source")? {
            let bytes = fs::read(&file.source_path).map_err(|source_error| OvenLoafError::Io {
                path: file.source_path.clone(),
                source: source_error,
            })?;
            plan.supporting_artifacts.push(OvenRustcSupportingArtifact {
                relative_path: file.relative_path,
                digest: digest_bytes(&bytes),
            });
        }

        let sealed = OvenRustcRegistrySource {
            registry: source.registry.clone(),
            checksum: source.checksum.clone(),
            relative_root,
            digest: source.source_digest.clone(),
        };
        if let Some(existing) = plan.registry_sources.iter_mut().find(|existing| {
            existing.package == source.package
                && existing.version == source.version
                && existing.source.registry == source.registry
        }) {
            if existing.source != sealed {
                return Err(OvenLoafError::Preparation {
                    message: format!(
                        "sealed registry source for `{}` {} disagrees with the generated-project authority",
                        source.package, source.version
                    ),
                });
            }
            existing.features.extend(source.features.iter().cloned());
            existing.features.sort();
            existing.features.dedup();
        } else {
            let mut features = source.features.clone();
            features.sort();
            features.dedup();
            plan.registry_sources.push(OvenRustcRegistrySourcePackage {
                package: source.package.clone(),
                version: source.version.clone(),
                features,
                source: sealed,
            });
        }
    }
    plan.registry_sources.sort_by(|left, right| {
        (&left.package, &left.version, &left.source.registry).cmp(&(
            &right.package,
            &right.version,
            &right.source.registry,
        ))
    });
    canonicalize_supporting_artifacts(&mut plan.supporting_artifacts)?;
    plan.validate_shape(&plan.intent)?;
    Ok(())
}

/// Preserve the publisher-selected generated-root dependency set before the loaf adds compiler-only helpers.
///
/// The ordinary Loaf is published from a minimal generated program, so its original direct externs are the roots that
/// generated caller code may receive. Later preparation adds compiler runtime and vocabulary capabilities to the same
/// immutable closure. Runtime roots are promoted into every declared entrypoint below, but the vocabulary helper roots
/// must remain private to vocabulary extraction: passing their independently built `serde` closure to a generated
/// library would make Rustc see two incompatible `serde` identities.
fn record_generated_root_externs(plan: &mut OvenRustcArtifactManifest) -> Result<(), OvenLoafError> {
    if plan.schema_version == oven_rustc::rustc::OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION
        && !plan.entrypoint_dependency_search_paths.contains_key("generated-root")
    {
        let closure = plan.capture_source_search_closure(&plan.dependency_search_paths)?;
        plan.entrypoint_dependency_search_paths
            .insert("generated-root".to_string(), closure);
    }
    plan.entrypoint_externs
        .entry("generated-root".to_string())
        .or_insert_with(|| {
            let mut crate_names = plan
                .externs
                .iter()
                .map(|artifact| artifact.crate_name.clone())
                .collect::<Vec<_>>();
            crate_names.sort();
            crate_names.dedup();
            crate_names
        });
    Ok(())
}

/// Promote compiler runtime artifacts required by generated provider libraries to direct externs.
///
/// The minimal loaf program need not use models or provider metadata, while generated caller-owned libraries do.
/// `incan_derive` and `incan_lang` are therefore promoted from the verified support closure. Leaving either only on
/// `-L dependency` relies on Cargo's implicit extern selection and makes a normal direct-Rustc consumer recompile
/// compiler source instead of linking the selected immutable plan.
fn promote_compiler_runtime_externs(plan: &mut OvenRustcArtifactManifest) -> Result<(), OvenLoafError> {
    promote_compiler_runtime_extern(plan, "incan_derive", is_incan_derive_artifact)?;
    promote_compiler_runtime_extern(plan, "incan_lang", |relative_path| {
        is_named_rlib(relative_path, "incan_lang")
    })
}

/// Promote one exact compiler-owned support artifact after confirming the loaf is unambiguous.
fn promote_compiler_runtime_extern(
    plan: &mut OvenRustcArtifactManifest,
    crate_name: &str,
    matches_artifact: impl Fn(&str) -> bool,
) -> Result<(), OvenLoafError> {
    if plan.externs.iter().any(|artifact| artifact.crate_name == crate_name) {
        return Ok(());
    }
    let candidates = plan
        .supporting_artifacts
        .iter()
        .enumerate()
        .filter(|(_, artifact)| matches_artifact(&artifact.relative_path))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let [index] = candidates.as_slice() else {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "native foundation must declare exactly one compiler `{crate_name}` direct-Rustc artifact; found {}",
                candidates.len()
            ),
        });
    };
    let artifact = plan.supporting_artifacts.remove(*index);
    plan.externs.push(OvenRustcArtifactExtern {
        crate_name: crate_name.to_string(),
        relative_path: artifact.relative_path,
        digest: artifact.digest,
    });
    plan.externs
        .sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    for crate_names in plan.entrypoint_externs.values_mut() {
        if !crate_names
            .iter()
            .any(|entrypoint_extern| entrypoint_extern == crate_name)
        {
            crate_names.push(crate_name.to_string());
            crate_names.sort();
        }
    }
    Ok(())
}

/// The publisher's failure crosses back into the Loaf model as its rendered message: the model must not name
/// this crate, and the message is what every caller displays.
impl From<OvenLegacyCargoError> for OvenLoafError {
    fn from(error: OvenLegacyCargoError) -> Self {
        OvenLoafError::Publisher(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    use oven_rustc::rustc::{
        OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH, OvenRustcArtifactExtern,
        OvenRustcArtifactManifest, OvenRustcRegistryLeaf, OvenRustcRegistrySource, OvenRustcRegistrySourcePackage,
        OvenRustcSupportingArtifact,
    };
    use oven_store::{OvenGeneratedProjectRequest, digest_bytes, receipt_generated_project};

    use crate::{
        OvenLegacyCargoInspectionSource, OvenLegacyCargoInspectionSourceMember, OvenLegacyCargoSelectedUnit,
        OvenLegacyCargoSelectedUnitCapture, legacy_cargo_selected_unit_capture_identity,
        stage_registry_source_directory,
    };

    fn selected_registry_unit(cfg: &[&str]) -> OvenLegacyCargoSelectedUnit {
        OvenLegacyCargoSelectedUnit {
            package_id: "registry+https://example.invalid/index#blake2@0.10.6".to_string(),
            package: "blake2".to_string(),
            package_version: "0.10.6".to_string(),
            package_source: Some("registry+https://example.invalid/index".to_string()),
            target_name: "blake2".to_string(),
            target_kinds: vec!["lib".to_string()],
            crate_types: vec!["lib".to_string()],
            source_path: PathBuf::from("/sealed/blake2/src/lib.rs"),
            artifact_paths: Vec::new(),
            root_module: "src/lib.rs".to_string(),
            edition: "2021".to_string(),
            mode: "build".to_string(),
            platform: Some("aarch64-apple-darwin".to_string()),
            target_is_explicit: Some(true),
            cfg: cfg.iter().map(|value| (*value).to_string()).collect(),
            effective_features: vec!["std".to_string()],
            dependencies: Vec::new(),
            sysroot_externs: Vec::new(),
            build_script: None,
            registry_source: None,
        }
    }

    fn registry_leaf() -> OvenRustcRegistryLeaf {
        OvenRustcRegistryLeaf {
            selected_unit_identity: None,
            package: "blake2".to_string(),
            version: "0.10.6".to_string(),
            crate_name: "blake2".to_string(),
            features: vec!["std".to_string()],
            source: OvenRustcRegistrySource {
                registry: "registry+https://example.invalid/index".to_string(),
                checksum: "blake2-checksum".to_string(),
                relative_root: "registry-sources/blake2-0.10.6".to_string(),
                digest: "sha256:blake2-source".to_string(),
            },
            artifact: OvenRustcArtifactExtern {
                crate_name: "blake2".to_string(),
                relative_path: "deps/libblake2.rlib".to_string(),
                digest: "sha256:blake2-artifact".to_string(),
            },
        }
    }

    #[test]
    fn registry_leaf_binding_requires_exact_unambiguous_physical_capture() -> Result<(), Box<dyn std::error::Error>> {
        let unit = selected_registry_unit(&["target_has_atomic=\"64\""]);
        let capture_identity = legacy_cargo_selected_unit_capture_identity(&unit)?;
        let capture = OvenLegacyCargoSelectedUnitCapture {
            roots: vec![0],
            units: vec![unit.clone()],
            rustc_invocations_observed: true,
            build_script_tool_probes: Vec::new(),
            compiler: None,
        };
        let mut bindings = BTreeMap::from([(capture_identity, "sha256:selected-unit".to_string())]);
        let mut leaf = registry_leaf();
        leaf.selected_unit_identity = Some(capture_identity.clone());
        let mut leaves = vec![leaf.clone()];

        bind_registry_leaf_selected_unit_identities(&mut leaves, &capture, &bindings)?;
        assert_eq!(
            leaves[0].selected_unit_identity.as_deref(),
            Some("sha256:selected-unit")
        );

        let mut missing = vec![leaf.clone()];
        assert!(bind_registry_leaf_selected_unit_identities(&mut missing, &capture, &BTreeMap::new()).is_err());

        let mut variant = unit;
        variant.cfg.push("target_feature=\"neon\"".to_string());
        let variant_identity = legacy_cargo_selected_unit_capture_identity(&variant)?;
        let variants = OvenLegacyCargoSelectedUnitCapture {
            units: vec![capture.units[0].clone(), variant],
            ..capture
        };
        let mut variant_only = vec![leaf.clone()];
        let variant_bindings = BTreeMap::from([(variant_identity, "sha256:wrong-variant".to_string())]);
        assert!(bind_registry_leaf_selected_unit_identities(&mut variant_only, &variants, &variant_bindings).is_err());

        bindings.insert(
            "sha256:uncompiled-capture".to_string(),
            "sha256:uncompiled-unit".to_string(),
        );
        let mut extra = vec![leaf];
        assert!(bind_registry_leaf_selected_unit_identities(&mut extra, &variants, &bindings).is_err());
        Ok(())
    }
    fn runtime_receipt(
        source: &Path,
        providers: &str,
        rust_dependencies: &str,
        stdlib_facets: &str,
    ) -> Result<oven_store::OvenReceipt, Box<dyn std::error::Error>> {
        let provider_plan = digest_bytes(providers.as_bytes());
        let mut request = OvenGeneratedProjectRequest::new(
            source.parent().ok_or("source has no parent")?,
            "runtime_fixture",
            "0.1.0",
            "aarch64-apple-darwin",
            "rustc seeded-test",
            "debug",
            Vec::new(),
        )
        .with_generated_source("generated-root", source)
        .with_build_unit_input("runtime-lock", "runtime-lock")
        .with_build_unit_input("rust-dependencies", rust_dependencies)
        .with_build_unit_input("stdlib-facets", stdlib_facets)
        .with_build_unit_input("provider-plan", provider_plan);
        if !providers.is_empty() {
            request = request.with_build_unit_input("providers", providers);
        }
        Ok(receipt_generated_project(&request)?)
    }

    fn runtime_receipt_for_plan() -> Result<oven_store::OvenReceipt, Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("main.rs");
        fs::write(&source, "fn main() {}\n")?;
        // The receipt owns no filesystem path, so retaining only its value is valid after this helper drops the
        // temporary source tree.
        runtime_receipt(&source, "", "empty-rust-dependencies", "empty-stdlib-facets")
    }

    fn empty_manifest(receipt: &oven_store::OvenReceipt) -> OvenRustcArtifactManifest {
        OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            entrypoint_dependency_search_paths: Default::default(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        }
    }

    #[test]
    fn loaf_export_retains_the_publisher_registry_lock_for_registry_sources() -> Result<(), Box<dyn std::error::Error>>
    {
        let publisher_store = tempfile::tempdir()?;
        let publisher_lock = publisher_store.path().join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH);
        let publisher_parent = publisher_lock.parent().ok_or("publisher registry lock has no parent")?;
        fs::create_dir_all(publisher_parent)?;
        let lock_bytes = b"version = 4\n";
        fs::write(&publisher_lock, lock_bytes)?;

        let receipt = runtime_receipt_for_plan()?;
        let mut plan = empty_manifest(&receipt);
        plan.registry_sources.push(OvenRustcRegistrySourcePackage {
            package: "blake2".to_string(),
            version: "0.10.6".to_string(),
            features: vec!["std".to_string()],
            source: OvenRustcRegistrySource {
                registry: "registry+https://example.invalid/index".to_string(),
                checksum: "blake2-checksum".to_string(),
                relative_root: "registry-sources/blake2-0.10.6".to_string(),
                digest: "sha256:blake2-source".to_string(),
            },
        });
        let loaf_staging = tempfile::tempdir()?;

        seal_registry_lock_from_temporary_store(&mut plan, publisher_store.path(), loaf_staging.path())?;

        assert_eq!(
            fs::read(loaf_staging.path().join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH))?,
            lock_bytes
        );
        assert!(plan.supporting_artifacts.iter().any(|artifact| {
            artifact.relative_path == OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH
                && artifact.digest == digest_bytes(lock_bytes)
        }));
        Ok(())
    }

    #[test]
    fn envelope_source_authority_is_sealed_without_fabricating_a_linkable_leaf()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = tempfile::tempdir()?;
        fs::create_dir_all(source.path().join("src"))?;
        fs::write(
            source.path().join("Cargo.toml"),
            "[package]\nname = \"blake2\"\nversion = \"0.10.6\"\n",
        )?;
        fs::write(source.path().join("src/lib.rs"), "pub fn sealed() {}\n")?;
        fs::write(source.path().join("src.rs"), "// sorts before src/lib.rs\n")?;
        let staged_sources = tempfile::tempdir()?;
        let (source_root, source_digest, members) = stage_registry_source_directory(
            staged_sources.path(),
            "blake2",
            "0.10.6",
            "registry+https://example.invalid/index",
            "blake2-checksum",
            source.path(),
        )?;
        let authority = OvenLegacyCargoInspectionSource {
            package: "blake2".to_string(),
            version: "0.10.6".to_string(),
            registry: "registry+https://example.invalid/index".to_string(),
            checksum: "blake2-checksum".to_string(),
            features: vec!["derive".to_string(), "std".to_string()],
            source_root,
            source_digest,
            members,
        };
        let staging = tempfile::tempdir()?;
        let receipt = runtime_receipt_for_plan()?;
        let mut plan = empty_manifest(&receipt);

        merge_loaf_inspection_sources(&mut plan, staging.path(), std::slice::from_ref(&authority))?;

        assert!(plan.registry_leaves.is_empty());
        assert_eq!(plan.registry_sources.len(), 1);
        assert!(plan.supporting_artifacts.iter().any(|artifact| {
            artifact.relative_path.starts_with("registry-sources/") && artifact.relative_path.ends_with("/Cargo.toml")
        }));

        fs::create_dir_all(staging.path().join("deps"))?;
        let artifact = b"sealed rlib";
        fs::write(staging.path().join("deps/libblake2.rlib"), artifact)?;
        plan.supporting_artifacts.push(OvenRustcSupportingArtifact {
            relative_path: "deps/libblake2.rlib".to_string(),
            digest: digest_bytes(artifact),
        });
        plan.registry_leaves.push(OvenRustcRegistryLeaf {
            selected_unit_identity: None,
            package: authority.package.clone(),
            version: authority.version.clone(),
            crate_name: "blake2".to_string(),
            features: vec!["std".to_string()],
            source: plan.registry_sources[0].source.clone(),
            artifact: OvenRustcArtifactExtern {
                crate_name: "blake2".to_string(),
                relative_path: "deps/libblake2.rlib".to_string(),
                digest: digest_bytes(artifact),
            },
        });
        plan.validate_shape(&receipt.intent)?;

        let conflicting = OvenLegacyCargoInspectionSource {
            checksum: "different-checksum".to_string(),
            ..authority
        };
        let error = merge_loaf_inspection_sources(&mut plan, staging.path(), &[conflicting])
            .err()
            .ok_or("conflicting source identity must fail closed")?;
        assert!(
            error
                .to_string()
                .contains("disagrees with the generated-project authority")
        );
        Ok(())
    }

    #[test]
    fn envelope_source_authority_refuses_incomplete_or_changed_member_inventory()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = tempfile::tempdir()?;
        fs::create_dir_all(source.path().join("src"))?;
        fs::write(
            source.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"1.0.0\"\n",
        )?;
        fs::write(source.path().join("src/lib.rs"), "pub fn sealed() {}\n")?;
        fs::write(source.path().join("src.rs"), "// sorts before src/lib.rs\n")?;
        let staged_sources = tempfile::tempdir()?;
        let (source_root, source_digest, members) = stage_registry_source_directory(
            staged_sources.path(),
            "fixture",
            "1.0.0",
            "registry+https://example.invalid/index",
            "fixture-checksum",
            source.path(),
        )?;
        let authority = OvenLegacyCargoInspectionSource {
            package: "fixture".to_string(),
            version: "1.0.0".to_string(),
            registry: "registry+https://example.invalid/index".to_string(),
            checksum: "fixture-checksum".to_string(),
            features: Vec::new(),
            source_root,
            source_digest,
            members,
        };

        let assert_refused = |candidate: OvenLegacyCargoInspectionSource| -> Result<(), Box<dyn std::error::Error>> {
            let staging = tempfile::tempdir()?;
            let receipt = runtime_receipt_for_plan()?;
            let mut plan = empty_manifest(&receipt);
            let error = merge_loaf_inspection_sources(&mut plan, staging.path(), &[candidate])
                .expect_err("changed source inventory must be refused");
            assert!(error.to_string().contains("does not match its staged member inventory"));
            Ok(())
        };

        let mut missing = authority.clone();
        missing.members.pop();
        assert_refused(missing)?;

        let mut extra = authority.clone();
        extra.members.push(OvenLegacyCargoInspectionSourceMember {
            path: "src/not-staged.rs".to_string(),
            digest: digest_bytes(b"not staged"),
        });
        assert_refused(extra)?;

        let mut tampered = authority.clone();
        tampered.members[0].digest = digest_bytes(b"different bytes");
        assert_refused(tampered)?;

        let staging = tempfile::tempdir()?;
        let receipt = runtime_receipt_for_plan()?;
        let mut plan = empty_manifest(&receipt);
        merge_loaf_inspection_sources(&mut plan, staging.path(), &[authority])?;
        assert_eq!(plan.registry_sources.len(), 1);
        Ok(())
    }

    #[test]
    fn a_native_loaf_retains_rmeta_sidecars_for_split_metadata_rlibs() -> Result<(), Box<dyn std::error::Error>> {
        // Since Rust 1.98, an `.rlib` may carry only a metadata stub with the real crate metadata in the sibling
        // `.rmeta`. The loaf pipeline must never strip that sidecar: rustc discovers it next to the rlib, and
        // without it a direct-rustc consumer fails with E0463 "can't find crate".
        let receipt = runtime_receipt_for_plan()?;
        let mut plan = empty_manifest(&receipt);
        plan.supporting_artifacts = vec![
            oven_rustc::rustc::OvenRustcSupportingArtifact {
                relative_path: "deps/libruntime.rlib".to_string(),
                digest: digest_bytes(b"runtime"),
            },
            oven_rustc::rustc::OvenRustcSupportingArtifact {
                relative_path: "deps/libruntime.rmeta".to_string(),
                digest: digest_bytes(b"metadata"),
            },
            oven_rustc::rustc::OvenRustcSupportingArtifact {
                relative_path: "provenance/legacy-cargo.json".to_string(),
                digest: digest_bytes(b"provenance"),
            },
        ];

        super::record_generated_root_externs(&mut plan)?;

        assert_eq!(
            plan.supporting_artifacts
                .iter()
                .map(|artifact| artifact.relative_path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "deps/libruntime.rlib",
                "deps/libruntime.rmeta",
                "provenance/legacy-cargo.json"
            ]
        );
        Ok(())
    }

    #[test]
    fn native_loaf_captures_search_paths_before_helper_promotion() -> Result<(), Box<dyn std::error::Error>> {
        let receipt = runtime_receipt_for_plan()?;
        let mut plan = empty_manifest(&receipt);
        plan.dependency_search_paths = vec!["target/deps".to_string(), "host/deps".to_string()];
        plan.externs.push(oven_rustc::rustc::OvenRustcArtifactExtern {
            crate_name: "runtime".to_string(),
            relative_path: "target/deps/libruntime.rlib".to_string(),
            digest: digest_bytes(b"runtime"),
        });
        plan.supporting_artifacts
            .push(oven_rustc::rustc::OvenRustcSupportingArtifact {
                relative_path: "host/deps/libderive.dylib".to_string(),
                digest: digest_bytes(b"macro metadata dependency"),
            });
        super::record_generated_root_externs(&mut plan)?;
        let captured = plan.entrypoint_dependency_search_paths["generated-root"].clone();
        assert_eq!(
            captured.paths().map(String::as_str).collect::<Vec<_>>(),
            vec!["target/deps", "host/deps"]
        );
        plan.dependency_search_paths.push("helper/deps".to_string());
        plan.externs.push(oven_rustc::rustc::OvenRustcArtifactExtern {
            crate_name: "private_helper".to_string(),
            relative_path: "helper/deps/libprivate_helper.rlib".to_string(),
            digest: digest_bytes(b"unrelated helper"),
        });
        super::record_generated_root_externs(&mut plan)?;
        assert_eq!(plan.entrypoint_dependency_search_paths["generated-root"], captured);
        assert_eq!(plan.entrypoint_externs["generated-root"], vec!["runtime"]);
        plan.validate_shape(&receipt.intent)?;
        Ok(())
    }

    #[test]
    fn native_loaf_promotes_compiler_runtime_externs_for_compatible_callers() -> Result<(), Box<dyn std::error::Error>>
    {
        let receipt = runtime_receipt_for_plan()?;
        let mut plan = empty_manifest(&receipt);
        plan.entrypoint_externs.insert("generated-root".to_string(), Vec::new());
        plan.entrypoint_dependency_search_paths.insert(
            "generated-root".to_string(),
            oven_rustc::rustc::OvenRustcSourceSearchClosure::default(),
        );
        plan.supporting_artifacts = vec![
            oven_rustc::rustc::OvenRustcSupportingArtifact {
                relative_path: "host/deps/libincan_derive-verified.dylib".to_string(),
                digest: digest_bytes(b"derive macro"),
            },
            oven_rustc::rustc::OvenRustcSupportingArtifact {
                relative_path: "target/deps/libincan_lang-verified.rlib".to_string(),
                digest: digest_bytes(b"compiler runtime"),
            },
            oven_rustc::rustc::OvenRustcSupportingArtifact {
                relative_path: "target/deps/libruntime.rlib".to_string(),
                digest: digest_bytes(b"runtime"),
            },
        ];

        super::promote_compiler_runtime_externs(&mut plan)?;

        assert_eq!(
            plan.externs
                .iter()
                .map(|artifact| artifact.crate_name.as_str())
                .collect::<Vec<_>>(),
            vec!["incan_derive", "incan_lang"]
        );
        assert!(plan.externs.iter().any(|artifact| {
            artifact.crate_name == "incan_derive"
                && artifact.relative_path == "host/deps/libincan_derive-verified.dylib"
        }));
        assert!(plan.externs.iter().any(|artifact| {
            artifact.crate_name == "incan_lang" && artifact.relative_path == "target/deps/libincan_lang-verified.rlib"
        }));
        assert_eq!(
            plan.supporting_artifacts
                .iter()
                .map(|artifact| artifact.relative_path.as_str())
                .collect::<Vec<_>>(),
            vec!["target/deps/libruntime.rlib"]
        );
        assert_eq!(
            plan.entrypoint_externs.get("generated-root"),
            Some(&vec!["incan_derive".to_string(), "incan_lang".to_string()])
        );
        Ok(())
    }

    #[test]
    fn native_loaf_keeps_compiler_vocab_helpers_off_generated_root() -> Result<(), Box<dyn std::error::Error>> {
        let receipt = runtime_receipt_for_plan()?;
        let mut plan = empty_manifest(&receipt);
        plan.externs.push(oven_rustc::rustc::OvenRustcArtifactExtern {
            crate_name: "incan_std_core".to_string(),
            relative_path: "target/deps/libincan_std_core-verified.rlib".to_string(),
            digest: digest_bytes(b"stdlib runtime"),
        });
        plan.supporting_artifacts = vec![
            oven_rustc::rustc::OvenRustcSupportingArtifact {
                relative_path: "host/deps/libincan_derive-verified.dylib".to_string(),
                digest: digest_bytes(b"derive macro"),
            },
            oven_rustc::rustc::OvenRustcSupportingArtifact {
                relative_path: "target/deps/libincan_lang-verified.rlib".to_string(),
                digest: digest_bytes(b"compiler runtime"),
            },
        ];

        super::record_generated_root_externs(&mut plan)?;
        super::promote_compiler_runtime_externs(&mut plan)?;
        plan.externs.extend([
            oven_rustc::rustc::OvenRustcArtifactExtern {
                crate_name: "incan_vocab".to_string(),
                relative_path: "compiler-support/deps/libincan_vocab-verified.rlib".to_string(),
                digest: digest_bytes(b"compiler vocabulary"),
            },
            oven_rustc::rustc::OvenRustcArtifactExtern {
                crate_name: "serde_json".to_string(),
                relative_path: "compiler-support/deps/libserde_json-verified.rlib".to_string(),
                digest: digest_bytes(b"compiler json"),
            },
        ]);
        plan.externs
            .sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
        plan.validate_shape(&receipt.intent)?;

        assert_eq!(
            plan.entrypoint_externs.get("generated-root"),
            Some(&vec![
                "incan_derive".to_string(),
                "incan_lang".to_string(),
                "incan_std_core".to_string(),
            ])
        );
        Ok(())
    }
}
