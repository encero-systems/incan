//! Publishing a project's inspection authority and output Loaf into the store.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::build::output_paths::{
    bake_generated_out_dir_units, project_inspection_root_dependencies, project_inspection_test_dependency_roots,
    project_relative_entrypoint, validated_project_output_relative_path,
};
use crate::build::output_selection::baked_project_owner_identity;
use crate::build::{
    BakeGeneratedOutDir, LibraryInspectionConstituent, OVEN_PROJECT_OUTPUT_ARTIFACT_PATH,
    OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION, OVEN_PROJECT_OUTPUT_PUBLICATION_RETRY,
    OVEN_PROJECT_OUTPUT_PUBLICATION_WAIT, OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION, OvenProjectOutputBakeFile,
    OvenProjectOutputBakeRequest, OvenProjectOutputFile, OvenProjectOutputPayload, OvenStoredProjectOutput,
    PreparedOvenTestDependencyEnvelope, PublishedProjectInspectionAuthority, oven_bake_project_target_identity,
};
use crate::error::{CliError, CliResult, oven_rustc_error};
use incan_lang::version::INCAN_VERSION;
use oven_model::manifest::DependencySpec;
use oven_rustc::plan::OvenDirectRustcPlanSelection;
use oven_rustc::rustc::{
    OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION, OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH,
    OvenProjectInspectionAuthorityPayload, OvenProjectInspectionAuthorityRef, OvenProjectInspectionConstituent,
    OvenProjectInspectionGeneratedOutDir, OvenProjectInspectionSource, OvenProjectInspectionSourceOwner,
    OvenProjectInspectionTestDependencyEnvelope, validate_project_inspection_authority_payload,
};
use oven_store::digest_bytes;
use oven_store::store::{
    OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, OvenStoreError,
    OvenStoreLease,
};

/// Seal the project's inspection authority: the constituents a normal command may inspect through, the registry
/// sources each one owns, the test-dependency envelope, and the build-script output the bake's Cargo targets wrote.
///
/// The library constituent, when the bake produced one, joins the constituents under the kind and base its selection
/// had, and its registry sources are added only where no earlier constituent already names the locked package.
pub fn publish_project_inspection_authority(
    store: &OvenStore,
    project_root: &Path,
    source_authority_digest: &str,
    registry_dependencies: &[DependencySpec],
    dev_registry_dependencies: &[DependencySpec],
    test_dependency_envelope: &PreparedOvenTestDependencyEnvelope,
    library: Option<&LibraryInspectionConstituent>,
) -> CliResult<PublishedProjectInspectionAuthority> {
    let receipt = &test_dependency_envelope.receipt;
    let selection = &test_dependency_envelope.plan_selection;
    let (
        constituents,
        mut registry_sources,
        lock_path,
        publisher_normal_roots,
        publisher_dev_roots,
        test_dependency_constituent_index,
    ) = match selection {
        OvenDirectRustcPlanSelection::Stored(selected) => {
            if selected.artifacts.intent != receipt.intent {
                return Err(CliError::failure(
                    "project inspection authority selected a stored direct plan with a different build intent",
                ));
            }
            let sources = selected
                .artifacts
                .registry_sources
                .iter()
                .cloned()
                .map(|package| OvenProjectInspectionSource {
                    package,
                    owner: OvenProjectInspectionSourceOwner::Constituent { index: 0 },
                })
                .collect::<Vec<_>>();
            (
                vec![OvenProjectInspectionConstituent::Stored {
                    identity: selected.identity.clone(),
                    artifact_kind: OvenArtifactKind::DirectRustcPlan,
                    receipt: receipt.clone(),
                    base_loaf_identity: None,
                }],
                sources,
                selected.artifact_root.join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH),
                None,
                None,
                Some(0),
            )
        }
        OvenDirectRustcPlanSelection::ToolchainLoaf(native) => {
            if native.artifacts.intent != receipt.intent {
                return Err(CliError::failure(
                    "project inspection authority selected a release Loaf with a different build intent",
                ));
            }
            let sources = native
                .artifacts
                .registry_sources
                .iter()
                .cloned()
                .map(|package| OvenProjectInspectionSource {
                    package,
                    owner: OvenProjectInspectionSourceOwner::Constituent { index: 0 },
                })
                .collect::<Vec<_>>();
            (
                vec![OvenProjectInspectionConstituent::ReleaseLoaf {
                    loaf_identity: native.loaf_identity.clone(),
                    build_unit_identity: native.loaf_build_unit_identity.clone(),
                    receipt: receipt.clone(),
                }],
                sources,
                native.artifact_root.join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH),
                None,
                None,
                Some(0),
            )
        }
        OvenDirectRustcPlanSelection::ProjectExtension(extension) => {
            if extension.source_payload.complete_plan.intent != receipt.intent
                || extension.extension.receipt.identity != receipt.identity
            {
                return Err(CliError::failure(
                    "project inspection authority selected a different receipt or build intent from its debug output",
                ));
            }
            let constituents = vec![
                OvenProjectInspectionConstituent::ReleaseLoaf {
                    loaf_identity: extension.base.loaf_identity.clone(),
                    build_unit_identity: extension.base.loaf_build_unit_identity.clone(),
                    receipt: receipt.clone(),
                },
                OvenProjectInspectionConstituent::Stored {
                    identity: extension.extension.identity.clone(),
                    artifact_kind: OvenArtifactKind::ProjectPayload,
                    receipt: extension.extension.receipt.clone(),
                    base_loaf_identity: Some(extension.base.loaf_identity.clone()),
                },
            ];
            let extension_paths = extension
                .source_payload
                .extension_paths
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            let mut sources = Vec::with_capacity(extension.source_payload.complete_plan.registry_sources.len());
            for package in &extension.source_payload.complete_plan.registry_sources {
                let prefix = format!("{}/", package.source.relative_root);
                let extension_owns = extension_paths
                    .iter()
                    .any(|path| *path == package.source.relative_root || path.starts_with(&prefix));
                let owner = if extension_owns {
                    OvenProjectInspectionSourceOwner::Constituent { index: 1 }
                } else {
                    // Source-inspection authority only needs the same locked source text, not the same compiled
                    // feature selection: `package`, `version`, and `source` (registry, checksum, staged content
                    // digest) together already pin one exact, receipt-checked source archive. Two independently
                    // resolved builds can unify a shared transitive dependency's Cargo features differently (for
                    // example the base release Loaf's own closure enabling only `std` where a project's closure
                    // also enables `default`) without the underlying vendored source ever differing. Compiled
                    // artifact reuse remains governed separately by `registry_leaves`, which does still require an
                    // exact feature match.
                    let matching_base = extension
                        .base
                        .artifacts
                        .registry_sources
                        .iter()
                        .filter(|candidate| {
                            candidate.package == package.package
                                && candidate.version == package.version
                                && candidate.source == package.source
                        })
                        .count();
                    if matching_base != 1 {
                        return Err(CliError::failure(format!(
                            "project inspection source `{}` {} is absent from both its project fragment and exact release Loaf",
                            package.package, package.version
                        )));
                    }
                    OvenProjectInspectionSourceOwner::Constituent { index: 0 }
                };
                sources.push(OvenProjectInspectionSource {
                    package: package.clone(),
                    owner,
                });
            }
            (
                constituents,
                sources,
                extension
                    .extension
                    .artifact_root
                    .join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH),
                Some(extension.source_payload.registry_source_dependencies.as_slice()),
                Some(extension.source_payload.dev_registry_source_dependencies.as_slice()),
                Some(1),
            )
        }
        OvenDirectRustcPlanSelection::PackagedProvider(_) => {
            return Err(CliError::failure(
                "explicit Oven project bake cannot use a package-provider composition as its project-owned test dependency envelope",
            ));
        }
    };
    let mut constituents = constituents;
    if let Some(library) = library
        && !constituents.iter().any(|constituent| {
            matches!(constituent, OvenProjectInspectionConstituent::Stored { identity, .. } if *identity == library.identity)
        })
    {
        if library.artifacts.intent != library.receipt.intent {
            return Err(CliError::failure(
                "project inspection authority library constituent has a different build intent from its receipt",
            ));
        }
        let index = constituents.len();
        constituents.push(OvenProjectInspectionConstituent::Stored {
            identity: library.identity.clone(),
            artifact_kind: library.artifact_kind,
            receipt: library.receipt.clone(),
            base_loaf_identity: library.base_loaf_identity.clone(),
        });
        // The library's registry sources overlap the test envelope's almost entirely; only the sources absent from
        // every earlier constituent are added, because the authority may name each locked package once.
        for package in &library.artifacts.registry_sources {
            let known = registry_sources.iter().any(|source| {
                source.package.package == package.package
                    && source.package.version == package.version
                    && source.package.source.registry == package.source.registry
                    && source.package.source.checksum == package.source.checksum
            });
            if !known {
                registry_sources.push(OvenProjectInspectionSource {
                    package: package.clone(),
                    owner: OvenProjectInspectionSourceOwner::Constituent { index },
                });
            }
        }
    }
    registry_sources.sort_by(|left, right| {
        (
            &left.package.package,
            &left.package.version,
            &left.package.source.registry,
            &left.package.source.checksum,
        )
            .cmp(&(
                &right.package.package,
                &right.package.version,
                &right.package.source.registry,
                &right.package.source.checksum,
            ))
    });
    let source_catalog = registry_sources
        .iter()
        .map(|source| source.package.clone())
        .collect::<Vec<_>>();
    let publisher_roots = publisher_normal_roots
        .into_iter()
        .flatten()
        .chain(publisher_dev_roots.into_iter().flatten())
        .cloned()
        .collect::<Vec<_>>();
    let publisher_roots = (!publisher_roots.is_empty()).then_some(publisher_roots.as_slice());
    let registry_source_dependencies =
        project_inspection_root_dependencies(registry_dependencies, &source_catalog, publisher_roots)?;
    let dev_registry_source_dependencies =
        project_inspection_root_dependencies(dev_registry_dependencies, &source_catalog, publisher_roots)?;
    let test_dependency_envelope = test_dependency_constituent_index
        .map(|constituent_index| {
            project_inspection_test_dependency_roots(
                &test_dependency_envelope.dependencies,
                &test_dependency_envelope.dependency_root_digests,
                &registry_source_dependencies,
                &dev_registry_source_dependencies,
            )
            .map(|dependency_roots| OvenProjectInspectionTestDependencyEnvelope {
                constituent_index,
                dependency_surface_digest: test_dependency_envelope.dependency_surface_digest.clone(),
                dependency_roots,
            })
        })
        .transpose()?;
    let (registry_lock_digest, mut materialized_files) = if registry_sources.is_empty() {
        (digest_bytes(&[]), Vec::new())
    } else {
        let lock = fs::read(&lock_path).map_err(|error| {
            CliError::failure(format!(
                "project inspection authority cannot read its canonical publisher lock {}: {error}",
                lock_path.display()
            ))
        })?;
        (
            digest_bytes(&lock),
            vec![OvenArtifactMaterializedFile {
                source_path: lock_path,
                relative_path: OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH.to_string(),
            }],
        )
    };
    // Seal the generated Rust the Cargo bootstrap wrote for the library's dependencies. Those files are the only
    // form in which prost's `include!`d modules exist; a normal command has no Cargo to regenerate them, so the
    // authority carries them and direct inspection workspaces read them from here.
    let mut generated_out_dirs = Vec::new();
    let generated_units = library
        .map(bake_generated_out_dir_units)
        .transpose()?
        .unwrap_or_default();
    for generated in generated_units {
        {
            let BakeGeneratedOutDir {
                crate_name,
                unit_relative_path,
                out_dir,
                version,
            } = generated;
            let relative_root = format!("generated-out-dirs/build/{unit_relative_path}/out");
            let mut sealed = false;
            for entry in fs::read_dir(&out_dir)
                .map_err(|error| CliError::failure(format!("cannot read {}: {error}", out_dir.display())))?
                .flatten()
            {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                    continue;
                }
                let file_name = entry.file_name().to_string_lossy().into_owned();
                materialized_files.push(OvenArtifactMaterializedFile {
                    source_path: path,
                    relative_path: format!("{relative_root}/{file_name}"),
                });
                sealed = true;
            }
            if sealed {
                generated_out_dirs.push(OvenProjectInspectionGeneratedOutDir {
                    crate_name,
                    relative_root,
                    version,
                });
            }
        }
    }
    let payload = OvenProjectInspectionAuthorityPayload {
        schema_version: OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION,
        project_identity: baked_project_owner_identity(project_root)?,
        source_authority_digest: source_authority_digest.to_string(),
        compiler_version: INCAN_VERSION.to_string(),
        registry_lock_digest,
        registry_source_dependencies,
        dev_registry_source_dependencies,
        test_dependency_envelope,
        constituents,
        registry_sources,
        generated_out_dirs,
    };
    validate_project_inspection_authority_payload(&payload).map_err(oven_rustc_error)?;
    let payload = serde_json::to_vec(&payload)
        .map_err(|error| CliError::failure(format!("failed to serialize project inspection authority: {error}")))?;
    let request = OvenArtifactPublishRequest {
        receipt: receipt.clone(),
        domain: format!("incan-release-{INCAN_VERSION}"),
        kind: OvenArtifactKind::ProjectInspectionAuthority,
        payload,
        materialized_files,
        materialized_directories: Vec::new(),
    };
    let deadline = Instant::now() + OVEN_PROJECT_OUTPUT_PUBLICATION_WAIT;
    let manifest = loop {
        match store.publish(&request) {
            Ok(manifest) => break manifest,
            Err(OvenStoreError::LegacyPublisherStagingActive { .. }) if Instant::now() < deadline => {
                std::thread::sleep(OVEN_PROJECT_OUTPUT_PUBLICATION_RETRY);
            }
            Err(error) => {
                return Err(CliError::failure(format!(
                    "failed to publish project inspection authority: {error}"
                )));
            }
        }
    };
    let (selected, _, _, lease) = store
        .select_payload_for_execution(&manifest.identity)
        .map_err(|error| CliError::failure(format!("failed to lease project inspection authority: {error}")))?;
    if selected.kind != OvenArtifactKind::ProjectInspectionAuthority
        || selected.receipt_identity != receipt.identity
        || selected.build_unit_identity != receipt.build_unit_identity
    {
        return Err(CliError::failure(
            "published project inspection authority changed identity, receipt, or kind during lease acquisition",
        ));
    }
    Ok(PublishedProjectInspectionAuthority {
        reference: OvenProjectInspectionAuthorityRef {
            identity: selected.identity,
            receipt_identity: selected.receipt_identity,
            build_unit_identity: selected.build_unit_identity,
        },
        _lease: lease,
    })
}

/// Publish one completed project-native output at the explicit project bake boundary.
///
/// The project-output Loaf shares the release compatibility domain with its selected direct-rustc inputs, so ordinary
/// bounded retention can reclaim an inactive completed output without creating a separate unbounded cache class.
pub fn publish_project_output_loaf(
    store: &OvenStore,
    receipt: &oven_store::OvenReceipt,
    payload: &OvenProjectOutputPayload,
    files: &[OvenProjectOutputBakeFile],
) -> CliResult<OvenStoredProjectOutput> {
    receipt
        .verify_identity()
        .map_err(|error| CliError::failure(format!("cannot publish invalid Oven project-output receipt: {error}")))?;
    if payload.schema_version != OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION
        || payload.compiler_version != INCAN_VERSION
        || payload.target_identity.trim().is_empty()
        || payload.receipt_identity != receipt.identity
        || payload.build_unit_identity != receipt.build_unit_identity
        || payload.plan_identity.trim().is_empty()
        || payload.files.is_empty()
        || payload
            .build_report
            .as_ref()
            .is_some_and(|snapshot| snapshot.schema_version != OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION)
    {
        return Err(CliError::failure(
            "cannot publish project-output Loaf with inconsistent receipt or output authority",
        ));
    }
    let sources = files
        .iter()
        .map(|file| (file.output_relative_path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    if sources.len() != files.len() || sources.len() != payload.files.len() {
        return Err(CliError::failure(
            "cannot publish Oven project-output Loaf with duplicate or missing output files",
        ));
    }
    for file in &payload.files {
        let Some(source) = sources.get(file.output_relative_path.as_str()) else {
            return Err(CliError::failure(
                "cannot publish Oven project-output Loaf whose payload omits a source file",
            ));
        };
        if source.caller_relative_path != file.caller_relative_path
            || digest_bytes(&fs::read(&source.source_path).map_err(|error| {
                CliError::failure(format!(
                    "cannot publish missing Oven project output {}: {error}",
                    source.source_path.display()
                ))
            })?) != file.digest
        {
            return Err(CliError::failure(
                "cannot publish Oven project output whose source differs from its payload",
            ));
        }
    }
    let payload_bytes = serde_json::to_vec(payload)
        .map_err(|error| CliError::failure(format!("failed to serialize Oven project-output Loaf: {error}")))?;
    let request = OvenArtifactPublishRequest {
        receipt: receipt.clone(),
        domain: format!("incan-release-{INCAN_VERSION}"),
        kind: OvenArtifactKind::ProjectOutput,
        payload: payload_bytes,
        materialized_files: files
            .iter()
            .map(|file| OvenArtifactMaterializedFile {
                source_path: file.source_path.clone(),
                relative_path: file.output_relative_path.clone(),
            })
            .collect(),
        materialized_directories: Vec::new(),
    };
    let deadline = Instant::now() + OVEN_PROJECT_OUTPUT_PUBLICATION_WAIT;
    let manifest = loop {
        match store.publish(&request) {
            Ok(manifest) => break manifest,
            Err(OvenStoreError::LegacyPublisherStagingActive { .. }) if Instant::now() < deadline => {
                // The active named publisher owns the remaining physical staging capacity. Waiting preserves that hard
                // bound while allowing independent explicit project bakes to converge safely.
                std::thread::sleep(OVEN_PROJECT_OUTPUT_PUBLICATION_RETRY);
            }
            Err(error) => {
                return Err(CliError::failure(format!(
                    "failed to publish Oven project-output Loaf: {error}"
                )));
            }
        }
    };
    let selected = store
        .select_payload_for_execution(&manifest.identity)
        .map_err(|error| {
            CliError::failure(format!(
                "failed to acquire the newly published Oven project-output lease: {error}"
            ))
        })?;
    if selected.0 != manifest {
        return Err(CliError::failure(
            "published Oven project output changed its immutable manifest during exact lease acquisition",
        ));
    }
    let selected_payload = serde_json::from_slice::<OvenProjectOutputPayload>(&selected.2).map_err(|error| {
        CliError::failure(format!(
            "newly published Oven project output has an invalid payload: {error}"
        ))
    })?;
    if &selected_payload != payload {
        return Err(CliError::failure(
            "newly published Oven project output differs from the completed target payload",
        ));
    }
    stored_project_output_from_parts(selected.0, selected.1, selected_payload, selected.3)
}

/// Validate one already leased project-output payload without rehashing its immutable artifact bytes.
pub fn stored_project_output_from_parts(
    manifest: oven_store::store::OvenArtifactManifest,
    artifact_root: PathBuf,
    payload: OvenProjectOutputPayload,
    lease: OvenStoreLease,
) -> CliResult<OvenStoredProjectOutput> {
    if manifest.kind != OvenArtifactKind::ProjectOutput
        || payload.schema_version != OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION
        || payload.compiler_version != INCAN_VERSION
        || payload.target_identity.trim().is_empty()
        || payload.receipt_identity != manifest.receipt_identity
        || payload.build_unit_identity != manifest.build_unit_identity
        || payload.plan_identity.trim().is_empty()
        || payload.files.is_empty()
        || payload
            .build_report
            .as_ref()
            .is_some_and(|snapshot| snapshot.schema_version != OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION)
    {
        return Err(CliError::failure(format!(
            "selected Oven project-output Loaf `{}` has inconsistent schema, receipt, compiler, or output authority",
            manifest.identity
        )));
    }
    let inspection_authority = payload.inspection_authority.as_ref().ok_or_else(|| {
        CliError::failure(format!(
            "selected Oven project-output Loaf `{}` has no project inspection authority",
            manifest.identity
        ))
    })?;
    if inspection_authority.identity.trim().is_empty()
        || inspection_authority.receipt_identity.trim().is_empty()
        || inspection_authority.build_unit_identity.trim().is_empty()
    {
        return Err(CliError::failure(format!(
            "selected Oven project-output Loaf `{}` has an incomplete project inspection authority",
            manifest.identity
        )));
    }
    let native_outputs = payload
        .files
        .iter()
        .filter(|file| file.output_relative_path == OVEN_PROJECT_OUTPUT_ARTIFACT_PATH)
        .collect::<Vec<_>>();
    if native_outputs.len() != 1 {
        return Err(CliError::failure(format!(
            "selected Oven project-output Loaf `{}` must contain exactly one native output",
            manifest.identity
        )));
    }
    let mut stored_paths = BTreeSet::new();
    let mut caller_paths = BTreeSet::new();
    for file in &payload.files {
        let _ = validated_project_output_relative_path(&file.output_relative_path, "stored output")?;
        let _ = validated_project_output_relative_path(&file.caller_relative_path, "caller output")?;
        if !stored_paths.insert(file.output_relative_path.as_str())
            || !caller_paths.insert(file.caller_relative_path.as_str())
        {
            return Err(CliError::failure(
                "selected Oven project-output Loaf contains duplicate output paths",
            ));
        }
    }
    if let Some(package_loaf_store_relative_path) = payload.package_loaf_store_relative_path.as_deref() {
        let _ = validated_project_output_relative_path(package_loaf_store_relative_path, "package Loaf store")?;
    } else if !payload.required_project_loafs.is_empty() {
        return Err(CliError::failure(
            "selected executable Oven project-output Loaf unexpectedly carries library package Loafs",
        ));
    }
    let native = native_outputs[0];
    let native_output = artifact_root.join(validated_project_output_relative_path(
        &native.output_relative_path,
        "stored output",
    )?);
    let metadata = fs::metadata(&native_output).map_err(|error| {
        CliError::failure(format!(
            "selected Oven project-output Loaf is missing its sealed native output {}: {error}",
            native_output.display()
        ))
    })?;
    if !metadata.is_file() || metadata.len() != native.logical_bytes {
        return Err(CliError::failure(format!(
            "selected Oven project-output Loaf native output length differs at {}",
            native_output.display()
        )));
    }
    let profile = manifest.intent.profile.clone();
    let intent = manifest.intent.clone();
    Ok(OvenStoredProjectOutput {
        identity: manifest.identity,
        profile,
        intent,
        payload,
        artifact_root,
        native_output,
        _lease: lease,
    })
}

/// Derive the completed-output authority from the project state that the explicit baker has already validated and
/// compiled.
pub fn project_output_payload_for_bake(
    request: OvenProjectOutputBakeRequest<'_>,
) -> CliResult<OvenProjectOutputPayload> {
    request.backend_receipt.verify_identity().map_err(|error| {
        CliError::failure(format!(
            "completed Oven project output has an invalid backend receipt: {error}"
        ))
    })?;
    let entrypoint_relative_path =
        project_relative_entrypoint(request.project_root, request.entrypoint).ok_or_else(|| {
            CliError::failure(format!(
                "explicit Oven project bake entrypoint {} is outside its project root {}",
                request.entrypoint.display(),
                request.project_root.display()
            ))
        })?;
    let target_identity = oven_bake_project_target_identity(request.project_root, request.target, request.entrypoint)?;
    if request.receipt.intent.profile != request.profile {
        return Err(CliError::failure(
            "explicit Oven project bake profile differs from its prepared receipt",
        ));
    }
    let mut files = request
        .files
        .into_iter()
        .map(|file| {
            let bytes = fs::read(&file.source_path).map_err(|error| {
                CliError::failure(format!(
                    "failed to digest completed Oven project output {}: {error}",
                    file.source_path.display()
                ))
            })?;
            let logical_bytes = u64::try_from(bytes.len()).map_err(|_| {
                CliError::failure(format!(
                    "completed Oven project output {} is too large to account",
                    file.source_path.display()
                ))
            })?;
            Ok(OvenProjectOutputFile {
                caller_relative_path: file.caller_relative_path,
                output_relative_path: file.output_relative_path,
                digest: digest_bytes(&bytes),
                logical_bytes,
            })
        })
        .collect::<CliResult<Vec<_>>>()?;
    files.sort_by(|left, right| left.output_relative_path.cmp(&right.output_relative_path));
    let native_outputs = files
        .iter()
        .filter(|file| file.output_relative_path == OVEN_PROJECT_OUTPUT_ARTIFACT_PATH)
        .count();
    if native_outputs != 1 {
        return Err(CliError::failure(
            "completed Oven project-output Loaf must contain exactly one native output",
        ));
    }
    if request.inspection_authority.identity.trim().is_empty()
        || request.inspection_authority.receipt_identity.trim().is_empty()
        || request.inspection_authority.build_unit_identity.trim().is_empty()
    {
        return Err(CliError::failure(
            "completed Oven project output must name one exact project inspection authority",
        ));
    }
    let mut required_project_loafs = request.required_project_loafs;
    required_project_loafs.sort_by(|left, right| {
        (&left.identity, &left.receipt.identity).cmp(&(&right.identity, &right.receipt.identity))
    });
    required_project_loafs.dedup_by(|left, right| {
        left.identity == right.identity && left.receipt.identity == right.receipt.identity && left.kind == right.kind
    });
    if request.package_loaf_store_relative_path.is_none() && !required_project_loafs.is_empty() {
        return Err(CliError::failure(
            "completed executable Oven project output cannot carry library package Loafs",
        ));
    }
    if let Some(path) = request.package_loaf_store_relative_path.as_deref() {
        let _ = validated_project_output_relative_path(path, "package Loaf store")?;
    }
    if request
        .lock_dependencies_fingerprint
        .as_deref()
        .is_some_and(|fingerprint| fingerprint.trim().is_empty())
    {
        return Err(CliError::failure(
            "completed Oven project output cannot carry an empty lock dependency fingerprint",
        ));
    }
    if request.build_report.as_ref().is_some_and(|snapshot| {
        snapshot.schema_version != OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION || !snapshot.report.is_object()
    }) {
        return Err(CliError::failure(
            "completed Oven project output cannot carry an invalid build-report snapshot",
        ));
    }
    Ok(OvenProjectOutputPayload {
        schema_version: OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION,
        project_target: request.target.as_str().to_string(),
        target_identity,
        project_identity: baked_project_owner_identity(request.project_root)?,
        source_authority_digest: request.source_authority_digest.to_string(),
        lock_dependencies_fingerprint: request.lock_dependencies_fingerprint,
        compiler_version: INCAN_VERSION.to_string(),
        entrypoint_relative_path,
        build_unit_identity: request.receipt.build_unit_identity.clone(),
        receipt_identity: request.receipt.identity.clone(),
        plan_identity: request.plan_identity,
        backend_receipt: request.backend_receipt,
        inspection_authority: Some(request.inspection_authority),
        files,
        required_project_loafs,
        package_loaf_store_relative_path: request.package_loaf_store_relative_path,
        build_report: request.build_report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::test_support::fixture_project_output_publication;
    use std::collections::BTreeSet;
    use std::fs;

    use oven_store::store::OvenStore;

    #[test]
    fn project_output_publication_retains_the_complete_set_under_tight_policy() -> Result<(), Box<dyn std::error::Error>>
    {
        let project = tempfile::tempdir()?;
        fs::create_dir(project.path().join("src"))?;
        fs::write(project.path().join("loaf.toml"), "[project]\nname = \"fixture\"\n")?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let debug = fixture_project_output_publication(project.path(), "debug", "debug")?;
        let release = fixture_project_output_publication(project.path(), "release", "release")?;

        let prototype_root = tempfile::tempdir()?;
        let prototype = OvenStore::new(
            prototype_root.path(),
            oven_store::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        let prototype_debug = publish_project_output_loaf(&prototype, &debug.0, &debug.1, &debug.2)?;
        let prototype_release = publish_project_output_loaf(&prototype, &release.0, &release.1, &release.2)?;
        let measured = prototype.inspect()?;
        drop((prototype_debug, prototype_release));

        let tight_root = tempfile::tempdir()?;
        let tight = OvenStore::new(
            tight_root.path(),
            oven_store::store::OvenStoreLimits::new(
                measured.physical_bytes,
                measured.physical_bytes,
                measured.logical_bytes,
            ),
        );
        let retained_debug = publish_project_output_loaf(&tight, &debug.0, &debug.1, &debug.2)?;
        let retained_release = publish_project_output_loaf(&tight, &release.0, &release.1, &release.2)?;
        let complete = tight.inspect()?;
        assert_eq!(complete.entries.len(), 2);
        assert_eq!(complete.active_lease_physical_bytes, complete.physical_bytes);
        assert_eq!(complete.reclaimable_physical_bytes, 0);

        let overflow = fixture_project_output_publication(project.path(), "debug", "overflow")?;
        let result = publish_project_output_loaf(&tight, &overflow.0, &overflow.1, &overflow.2);
        let Err(error) = result else {
            return Err("tight policy admitted a third output by pruning a leased project sibling".into());
        };
        assert!(error.message.contains("policy cannot admit"));
        let after = tight.inspect()?;
        assert_eq!(after.entries.len(), 2);
        let identities = after
            .entries
            .iter()
            .map(|entry| entry.manifest.identity.as_str())
            .collect::<BTreeSet<_>>();
        assert!(identities.contains(retained_debug.identity.as_str()));
        assert!(identities.contains(retained_release.identity.as_str()));
        Ok(())
    }
}
