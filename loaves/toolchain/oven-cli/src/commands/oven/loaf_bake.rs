//! `incan oven legacy-cargo bake-loafs`: bake or reuse the compiler-owned Loaf envelope generations.
//!
//! The one place Cargo is allowed to build Incan's own runtime and compiler-suite closures. Everything it publishes
//! is keyed on the evidence in `loaf_bake_evidence` and committed atomically as a generation.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use std::time::Instant;

use incan_driver::build::publication::stored_project_output_from_parts;
use incan_driver::build::{OvenProjectOutputPayload, OvenStoredProjectOutput};
use oven_cargo_compat::loaf_bake::{
    prepare_loaf_from_generated_project_with_selected_units, republish_loaf_under_final_receipt,
};
use oven_cargo_compat::{
    HarvestEvidenceInputs, HarvestPublisherIdentity, LoafRegistryAuthority,
    OVEN_LEGACY_CARGO_BUILD_SCRIPT_CLOSURE_INPUT, OvenLegacyCargoCompilerSuiteResult,
    OvenLegacyCargoFoundationSelection, OvenLegacyCargoSelectedUnitCapture, ambient_harvest_hazards,
    encode_selected_graph_policy_request, finalize_compiler_support_selected_graph, harvest_notes_for_checkout,
    harvest_registry_units_to_dir, legacy_cargo_build_script_closure_digest, legacy_cargo_foundation_projection,
    legacy_cargo_generated_archive_bindings, legacy_cargo_generated_output_bindings, proposal_directory_names,
    runtime_foundation_for_publisher_rebuild, runtime_foundation_inventories_from_policy_response,
};
use oven_model::digest::digest_bytes;
use oven_model::loaf_registry::{LoafRegistry, checkout_head_commit};
use oven_model::manifest::{ProjectManifest, RustFactArgument, RustFactLink, RustFactTool};
use oven_rustc::loaf::{
    OVEN_RELEASE_RUNTIME_CLOSURE_MEMBER_SCHEMA_VERSION, OVEN_RELEASE_RUNTIME_FOUNDATION_MEMBER_SCHEMA_VERSION,
    OVEN_RELEASE_STORE_MEMBER_SCHEMA_VERSION, OvenLoaf, OvenLoafPreparation, OvenReleaseRuntimeClosureMember,
    OvenReleaseRuntimeFoundationMember, OvenReleaseStoreMember, OvenReleaseToolchainMember,
    committed_release_runtime_members, direct_rustc_compiler_closure_identity,
    stage_release_runtime_foundation_toolchain,
};
use oven_rustc::rustc::direct_compiler::{OvenPublisherLinkBakeRequest, bake_publisher_link, publisher_archive_format};
use oven_rustc::rustc::{
    OvenPublisherLinkProduct, OvenRuntimeCompilerClosure, OvenRuntimeFoundationAsset, OvenRustcArtifactManifest,
    OvenSelectedRustFacetLinkedLibrary, OvenSelectedRustFacetOwnerRoot, ValidatedOvenSelectedRustFacetGraph,
    execute_runtime_foundation_rebuild, finalize_publisher_link_product, finalize_publisher_tool_product,
    publish_runtime_closure, publish_runtime_foundation_asset_with_generated_owners, rustc_host_target,
};
use oven_store::process::{BoundedProcessLimits, BoundedProcessTermination, run_bounded_process};
use oven_store::publisher_execution::{
    OvenPublisherExecutionMode, OvenPublisherToolOwner, OvenPublisherToolRequest, execute_publisher_tool,
    write_publisher_execution_receipt,
};
use oven_store::publisher_owner::publisher_owner_identity;
use oven_store::store::{
    OvenArtifactKind, OvenArtifactMaterializedDirectory, OvenArtifactMaterializedFile, OvenArtifactPublishRequest,
    OvenStore, PublishedOvenStore,
};
use oven_store::{OvenReceipt, receipt_with_build_unit_input};
use serde::{Deserialize, Serialize};

const POLICY_EXCHANGE_MAX_BYTES: u64 = 16 * 1024 * 1024;

use super::equivalence::{
    CaptureFileInput, CaptureManifestInput, CaptureSetup, DependencyIdentity, ExecutableIdentity, IdentityInputs,
    NameDigest, UnitManifest, write_capture_manifest,
};
use super::{
    CliError, CliResult, CompleteLoafEnvelopeReuseInput, DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_LOGICAL_BYTES,
    DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_PHYSICAL_BYTES, DEFAULT_OVEN_COMPILER_SUITE_MAX_PHYSICAL_BYTES,
    DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES, DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES, DEFAULT_OVEN_MAX_PHYSICAL_BYTES,
    ExitCode, LoafTemporaryDirectory, OVEN_LEGACY_CARGO_INSPECTION_AUTHORITY_ENV, OVEN_LOAF_ENV,
    OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION, OvenCompilerSuiteBakeReport, OvenInspectionRegistrySource,
    OvenLegacyCargoDirectDependencyClosure, OvenLegacyCargoInspectionSource, OvenLegacyCargoPrepareRequest,
    OvenLegacyCargoPublicationKind, OvenLoafBakeCommandOptions, OvenLoafBakeEntryReport, OvenLoafBakePhaseTiming,
    OvenLoafBakeReport, OvenLoafBakerContext, OvenLoafEnvelope, OvenLoafEnvelopeArgument, OvenLoafEnvelopeManifest,
    OvenLoafEnvelopeMember, OvenLoafFixtureAction, OvenLoafHarvestReport, OvenOutputFormat, OvenStoreCommandOptions,
    OvenStoreLimits, acquire_exclusive_loaf_generation_lock, announce_oven_progress, commit_loaf_generation,
    compiler_libtests_receipt, elapsed_detail, env, human_bytes, import_loaf_envelope_from_configured_mirrors,
    isolate_loaf_fixture_toolchain_data, legacy_cargo_inspection_sources, legacy_cargo_resolved_registry_sources,
    loaf_compiler_lock_path, loaf_compiler_manifest_path, loaf_directory_byte_counts,
    loaf_envelope_compatibility_map_with_release_member, loaf_envelope_evidence, loaf_envelope_inspection_packages,
    loaf_envelope_name, loaf_envelope_specifications, loaf_fixture_action_name, loaf_fixture_probe_is_expected_miss,
    loaf_generation_identity_with_release_member, loaf_raw_disk_bytes, open_store, oven_error, pin_loaf_fixture_rustc,
    prepare_compiler_test_suite, print_json, read_receipt, release_store_member_byte_counts,
    retire_unreferenced_loaf_generations, reuse_complete_loaf_envelope, stage_locked_loaf_fixture, user_home,
    write_receipt, write_sealed_oven_inspection_source_authority,
};

/// The release stdlib fixture's exact physical capture, and what the foundation steps after the fixture loop need
/// beside it.
struct ReleaseFoundationCapture {
    /// The generated-project receipt the publisher ran under.
    receipt: OvenReceipt,
    /// Every Cargo-selected physical unit, with registry sources bound.
    capture: OvenLegacyCargoSelectedUnitCapture,
    /// The staged registry source catalogs the capture was bound against.
    sources: Vec<OvenLegacyCargoInspectionSource>,
    /// The checked fixture manifest text.
    manifest_source: String,
    /// Identity of the provisional Loaf the capture was exported into (`<staged root>/<identity>.loaf`).
    loaf_identity: String,
}

/// The bounded compiler/sysroot closure one release generation retains as its selected graph's Toolchain owner.
struct StagedReleaseToolchain {
    /// Safe generation-relative directory holding the retained members.
    relative_path: PathBuf,
    /// Staged location of that directory.
    root: PathBuf,
    /// Closure digest: the Toolchain owner identity the selected graph declares.
    compiler_closure_identity: String,
    /// Exact retained members below `root`.
    members: Vec<OvenReleaseToolchainMember>,
}

/// One publisher-generated owner root that must remain immutable through foundation publication.
struct PublisherGeneratedOwnerRoot {
    /// Receipt identity used by the selected graph.
    identity: String,
    /// Physical root containing exactly the receipt-bound products.
    root: PathBuf,
    /// Registry binding whose publisher work produced this owner.
    binding: String,
    /// Final selected unit identity after the product is attached.
    unit_identity: String,
    /// Receipt that attests the product closure.
    attestation_reference: String,
    /// Original capture index used to resolve the final identity after all products rekey the graph.
    capture_index: usize,
}

/// Zero-byte registry `asset` event claim emitted beside a locally baked publisher asset.
#[derive(Serialize)]
struct PublisherAssetClaim<'a> {
    schema: u64,
    event: &'static str,
    binding: &'a str,
    archive_digest: &'a str,
    unit_identity: &'a str,
    builder_kind: &'static str,
    attestation_reference: &'a str,
}

/// Stable coordinates used to find one selected unit after another publisher product rekeys the graph.
#[derive(Clone)]
struct PublisherUnitCoordinates {
    package: String,
    version: String,
    crate_name: String,
    domain: oven_rustc::rustc::OvenSelectedRustFacetDomain,
    source_identity: String,
    source_root: String,
}

/// User-owned Oven configuration relevant to publisher execution.
#[derive(Default, Deserialize)]
struct PublisherConfigFile {
    /// Explicit publisher capabilities; projects cannot supply or weaken these roots.
    #[serde(default)]
    publisher: PublisherConfig,
}

/// Publisher executable-owner roots admitted by the user or organization configuration.
#[derive(Default, Deserialize)]
struct PublisherConfig {
    /// Immutable compiler/tool roots, using the same repeatable semantics as `--link-owner`.
    #[serde(rename = "link-owner", default)]
    link_owners: Vec<PathBuf>,
}

/// Finalize one publisher-only native product as an asset-side receipt and selected-unit link input.
///
/// This is deliberately part of the explicit Loaf publisher rather than any normal build path. The returned graph
/// carries the receipt identity as the archive owner; callers publish `product_root` as that owner's immutable asset
/// and provide the same root when the graph is physically materialized.
pub(crate) fn finalize_publisher_native_link(
    selected: ValidatedOvenSelectedRustFacetGraph,
    consuming_unit_identity: &str,
    product: &OvenPublisherLinkProduct,
) -> CliResult<ValidatedOvenSelectedRustFacetGraph> {
    write_publisher_execution_receipt(&product.receipt, &product.product_root)
        .map_err(|error| CliError::failure(format!("could not finalize native publisher receipt: {error}")))?;
    finalize_publisher_link_product(selected, consuming_unit_identity, Some(product))
        .map_err(|error| CliError::failure(format!("could not bind native publisher product: {error}")))
}

/// Execute every adopted publisher `link` and `tool` record and bind its products into the selected graph.
///
/// Records are taken only from the already-resolved registry authority. Executables are selected solely from
/// explicit owner roots whose identity reproduces the record; a near match is a refusal. Products remain under
/// caller-owned immutable roots until the runtime-foundation publisher seals them into the release asset.
fn execute_adopted_publisher_work(
    finalized: &mut oven_cargo_compat::OvenFinalizedCompilerSupportSelectedGraph,
    authority: &LoafRegistryAuthority,
    supplied_owner_roots: &[PathBuf],
    product_parent: &Path,
) -> CliResult<Vec<PublisherGeneratedOwnerRoot>> {
    let coordinates = finalized
        .unit_identities
        .iter()
        .map(|(capture_index, identity)| {
            publisher_unit_coordinates(finalized.graph.graph(), identity)
                .map(|coordinates| (*capture_index, coordinates))
        })
        .collect::<CliResult<BTreeMap<_, _>>>()?;
    let mut graph = finalized.graph.clone();
    let mut generated = Vec::new();
    for (capture_index, adoption) in authority.adoptions() {
        if adoption.record.link.is_empty() && adoption.record.tool.is_empty() {
            continue;
        }
        let unit = coordinates.get(&capture_index).ok_or_else(|| {
            CliError::failure(format!(
                "publisher record for `{}` {} has no selected unit",
                adoption.package, adoption.version
            ))
        })?;
        for link in &adoption.record.link {
            let (next_graph, owner) = execute_adopted_publisher_link(
                graph,
                adoption,
                link,
                unit,
                supplied_owner_roots,
                product_parent,
                capture_index,
            )?;
            graph = next_graph;
            generated.push(owner);
        }
        for tool in &adoption.record.tool {
            let (next_graph, owner) = execute_adopted_publisher_tool(
                graph,
                adoption,
                tool,
                unit,
                supplied_owner_roots,
                product_parent,
                capture_index,
            )?;
            graph = next_graph;
            generated.push(owner);
        }
    }
    for owner in &mut generated {
        let unit = coordinates
            .get(&owner.capture_index)
            .ok_or_else(|| CliError::failure(format!("publisher owner lost capture unit {}", owner.capture_index)))?;
        owner.unit_identity = selected_identity_for_coordinates(graph.graph(), unit)?;
    }
    finalized.unit_identities = coordinates
        .iter()
        .map(|(capture_index, coordinates)| {
            selected_identity_for_coordinates(graph.graph(), coordinates).map(|identity| (*capture_index, identity))
        })
        .collect::<CliResult<_>>()?;
    finalized.graph = graph;
    Ok(generated)
}

/// Execute one adopted native-link record and return the graph rekeyed by its receipt.
#[allow(clippy::too_many_arguments)]
fn execute_adopted_publisher_link(
    graph: ValidatedOvenSelectedRustFacetGraph,
    adoption: &oven_cargo_compat::LoafRegistryAdoption,
    link: &RustFactLink,
    unit: &PublisherUnitCoordinates,
    supplied_owner_roots: &[PathBuf],
    product_parent: &Path,
    capture_index: usize,
) -> CliResult<(ValidatedOvenSelectedRustFacetGraph, PublisherGeneratedOwnerRoot)> {
    let consuming_identity = selected_identity_for_coordinates(graph.graph(), unit)?;
    let owner_paths = std::iter::once(link.executable.path.as_str())
        .chain(link.objects.iter().flat_map(|object| {
            object.arguments.iter().filter_map(|argument| match argument {
                RustFactArgument::Owner { owner } => Some(owner.as_str()),
                _ => None,
            })
        }))
        .collect::<Vec<_>>();
    let executable_owner = resolve_publisher_owner_root(
        &adoption.package,
        &link.name,
        &link.executable.owner,
        &owner_paths,
        supplied_owner_roots,
    )?;
    let product_root = publisher_product_root(product_parent, capture_index, "link", &link.name)?;
    let product = bake_publisher_link(&OvenPublisherLinkBakeRequest {
        link,
        selected_target: &graph.graph().selection.intent.target,
        archive_format: publisher_archive_format(&graph.graph().selection.intent.target),
        toolchain: &graph.graph().selection.intent.toolchain,
        consuming_unit_identity: &consuming_identity,
        executable_owner_root: &executable_owner,
        source_owner_root: &adoption.manifest_root,
        output_root: &product_root,
        limits: publisher_process_limits(),
    })
    .map_err(oven_error)?;
    let owner = PublisherGeneratedOwnerRoot {
        identity: product.receipt.identity.clone(),
        root: product.product_root.clone(),
        binding: publisher_binding(adoption, link.name.as_str()),
        unit_identity: consuming_identity.clone(),
        attestation_reference: product.receipt.identity.clone(),
        capture_index,
    };
    let graph = finalize_publisher_native_link(graph, &consuming_identity, &product)?;
    Ok((graph, owner))
}

/// Execute one adopted publisher-tool record and return the graph rekeyed by its receipt.
#[allow(clippy::too_many_arguments)]
fn execute_adopted_publisher_tool(
    graph: ValidatedOvenSelectedRustFacetGraph,
    adoption: &oven_cargo_compat::LoafRegistryAdoption,
    tool: &RustFactTool,
    unit: &PublisherUnitCoordinates,
    supplied_owner_roots: &[PathBuf],
    product_parent: &Path,
    capture_index: usize,
) -> CliResult<(ValidatedOvenSelectedRustFacetGraph, PublisherGeneratedOwnerRoot)> {
    let consuming_identity = selected_identity_for_coordinates(graph.graph(), unit)?;
    let owner_paths = std::iter::once(tool.executable.path.as_str())
        .chain(tool.arguments.iter().filter_map(|argument| match argument {
            RustFactArgument::Owner { owner } => Some(owner.as_str()),
            _ => None,
        }))
        .collect::<Vec<_>>();
    let executable_owner = resolve_publisher_owner_root(
        &adoption.package,
        &tool.name,
        &tool.executable.owner,
        &owner_paths,
        supplied_owner_roots,
    )?;
    let product_root = publisher_product_root(product_parent, capture_index, "tool", &tool.name)?;
    let consuming_units = [consuming_identity.as_str()];
    let receipt = execute_publisher_tool(&OvenPublisherToolRequest {
        mode: OvenPublisherExecutionMode::Publisher,
        tool,
        fact_owner: OvenPublisherToolOwner {
            identity: adoption.manifest_digest.clone(),
            root: &adoption.manifest_root,
        },
        executable_owner: OvenPublisherToolOwner {
            identity: tool.executable.owner.clone(),
            root: &executable_owner,
        },
        host: &graph.graph().selection.host,
        target: &graph.graph().selection.intent.target,
        consuming_units: &consuming_units,
        product_root: &product_root,
    })
    .map_err(|error| CliError::failure(error.to_string()))?;
    write_publisher_tool_receipt(&receipt, &product_root)?;
    let owner = PublisherGeneratedOwnerRoot {
        identity: receipt.identity.clone(),
        root: product_root,
        binding: publisher_binding(adoption, tool.name.as_str()),
        unit_identity: consuming_identity,
        attestation_reference: receipt.identity.clone(),
        capture_index,
    };
    let graph = finalize_publisher_tool_product(graph, &receipt).map_err(oven_error)?;
    Ok((graph, owner))
}

/// Spell one native/tool fact binding independently of publisher-local paths.
fn publisher_binding(adoption: &oven_cargo_compat::LoafRegistryAdoption, producer: &str) -> String {
    let selection = &adoption.record;
    format!(
        "{} {} {} {} [{}] {}",
        adoption.package,
        adoption.version,
        selection.target,
        selection.profile,
        selection.features.join(","),
        producer
    )
}

/// Write one canonical zero-byte registry asset-event claim per executed native/tool binding.
fn write_publisher_asset_claims(
    owners: &[PublisherGeneratedOwnerRoot],
    archive_digest: &str,
    destination: &Path,
) -> CliResult<()> {
    fs::create_dir_all(destination).map_err(|error| {
        CliError::failure(format!(
            "could not create publisher asset-claim directory {}: {error}",
            destination.display()
        ))
    })?;
    for (index, owner) in owners.iter().enumerate() {
        let claim = PublisherAssetClaim {
            schema: 1,
            event: "asset",
            binding: &owner.binding,
            archive_digest,
            unit_identity: &owner.unit_identity,
            builder_kind: "local",
            attestation_reference: &owner.attestation_reference,
        };
        let mut bytes = serde_json::to_vec_pretty(&claim)
            .map_err(|error| CliError::failure(format!("could not encode publisher asset claim: {error}")))?;
        bytes.push(b'\n');
        let path = destination.join(format!("{index:04}.json"));
        fs::write(&path, bytes).map_err(|error| {
            CliError::failure(format!(
                "could not write publisher asset claim {}: {error}",
                path.display()
            ))
        })?;
    }
    Ok(())
}

/// Write paired Cargo-harvest and Cargo-free Oven-publisher manifests from one finalized release graph.
#[allow(
    clippy::too_many_arguments,
    reason = "the evidence writer names both producers and their shared authority"
)]
fn write_release_equivalence_captures(
    options: &OvenLoafBakeCommandOptions,
    finalized: &oven_cargo_compat::OvenFinalizedCompilerSupportSelectedGraph,
    build: &oven_rustc::rustc::OvenRuntimeFoundationBuild,
    cargo_artifact_root: &Path,
    asset_root: &Path,
    output_root: &Path,
) -> CliResult<()> {
    let compiler =
        finalized.capture.compiler.as_ref().ok_or_else(|| {
            CliError::failure("equivalence capture requires the observed compiler selection".to_string())
        })?;
    let setup = CaptureSetup {
        cargo: capture_executable_identity(&options.cargo)?,
        rustc: capture_executable_identity(&options.rustc)?,
        host: compiler.host.clone(),
        target: compiler.target.clone(),
        profile: finalized.graph.graph().selection.intent.profile.clone(),
    };
    let mut cargo_units = Vec::new();
    let mut oven_units = Vec::new();
    for (ordinal, rebuilt) in build.outputs().iter().enumerate() {
        let (cargo, oven) = release_capture_unit_inputs(
            finalized,
            rebuilt,
            cargo_artifact_root,
            asset_root,
            &setup.profile,
            ordinal,
        )?;
        cargo_units.push(cargo);
        oven_units.push(oven);
    }
    let root = output_root.join("equivalence");
    write_capture_manifest(
        &root.join("cargo-capture.json"),
        CaptureManifestInput {
            producer: "cargo-harvest",
            cargo_free: false,
            setup: setup.clone(),
            units: cargo_units,
        },
    )
    .map_err(|error| CliError::failure(format!("could not write Cargo equivalence capture: {error}")))?;
    write_capture_manifest(
        &root.join("oven-capture.json"),
        CaptureManifestInput {
            producer: "oven-publisher",
            cargo_free: true,
            setup,
            units: oven_units,
        },
    )
    .map_err(|error| CliError::failure(format!("could not write Oven equivalence capture: {error}")))
}

/// Assemble the paired physical inputs for one rebuilt release unit.
#[allow(
    clippy::too_many_arguments,
    reason = "one evidence pair names its two owners and normalized coordinate"
)]
fn release_capture_unit_inputs(
    finalized: &oven_cargo_compat::OvenFinalizedCompilerSupportSelectedGraph,
    rebuilt: &oven_rustc::rustc::OvenRuntimeRebuildOutput,
    cargo_artifact_root: &Path,
    asset_root: &Path,
    profile: &str,
    ordinal: usize,
) -> CliResult<(
    super::equivalence::CaptureUnitInput,
    super::equivalence::CaptureUnitInput,
)> {
    let graph_unit = finalized
        .graph
        .graph()
        .units
        .iter()
        .find(|unit| unit.identity == rebuilt.selected_identity)
        .ok_or_else(|| {
            CliError::failure(format!(
                "rebuilt unit `{}` is absent from the selected graph",
                rebuilt.selected_identity
            ))
        })?;
    let capture_index = finalized
        .unit_identities
        .iter()
        .find_map(|(index, identity)| (identity == &rebuilt.selected_identity).then_some(*index))
        .ok_or_else(|| {
            CliError::failure(format!(
                "rebuilt unit `{}` has no Cargo capture binding",
                rebuilt.selected_identity
            ))
        })?;
    let captured = finalized
        .capture
        .units
        .get(capture_index)
        .ok_or_else(|| CliError::failure(format!("Cargo capture unit {capture_index} is absent")))?;
    let extension = rebuilt
        .artifact
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("bin");
    let cargo_artifact = retained_cargo_artifact(captured, Some(extension), cargo_artifact_root)?;
    let cargo_native = retained_cargo_native_artifacts(captured, cargo_artifact_root)?;
    let oven_native = publisher_native_objects_for_unit(asset_root, &graph_unit.linked_libraries, &cargo_native)?;
    let unit = release_unit_manifest(graph_unit, captured, profile)?;
    let artifact_path = format!("units/{ordinal:04}/artifact.{extension}");
    let inputs = |source, native: Vec<PathBuf>, archive| {
        (
            vec![CaptureFileInput {
                role: equivalence_artifact_role(extension).to_string(),
                path: artifact_path.clone(),
                source,
            }],
            native
                .into_iter()
                .enumerate()
                .map(|(index, source)| CaptureFileInput {
                    role: "native-object".to_string(),
                    path: format!("units/{ordinal:04}/native/{index:04}.o"),
                    source,
                })
                .collect(),
            archive,
        )
    };
    let (cargo_artifacts, cargo_native, _) = inputs(cargo_artifact, cargo_native, None);
    let (oven_artifacts, oven_native, oven_archive) = inputs(
        rebuilt.artifact.clone(),
        oven_native,
        Some(asset_root.join(oven_rustc::rustc::OVEN_RUNTIME_FOUNDATION_ASSET_FILENAME)),
    );
    Ok((
        (unit.clone(), cargo_artifacts, cargo_native, None),
        (unit, oven_artifacts, oven_native, oven_archive),
    ))
}

/// Pair Cargo's retained native probes with the exact generated-output directories selected by the unit.
fn publisher_native_objects_for_unit(
    asset_root: &Path,
    linked_libraries: &[OvenSelectedRustFacetLinkedLibrary],
    cargo_native: &[PathBuf],
) -> CliResult<Vec<PathBuf>> {
    let selected_output_roots = linked_libraries
        .iter()
        .filter_map(|library| match library {
            OvenSelectedRustFacetLinkedLibrary::Archive { artifact, .. } => Path::new(&artifact.path).parent(),
            OvenSelectedRustFacetLinkedLibrary::Provider { .. } => None,
        })
        .collect::<BTreeSet<_>>();
    cargo_native
        .iter()
        .map(|cargo_object| {
            let name = cargo_object.file_name().ok_or_else(|| {
                CliError::failure(format!(
                    "retained Cargo native object has no file name: {}",
                    cargo_object.display()
                ))
            })?;
            let mut matches = selected_output_roots
                .iter()
                .map(|output_root| asset_root.join(output_root).join(name))
                .filter(|candidate| {
                    fs::symlink_metadata(candidate)
                        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
                })
                .collect::<Vec<_>>();
            matches.sort();
            matches.dedup();
            let [matched] = matches.as_slice() else {
                return Err(CliError::failure(format!(
                    "retained Cargo native object `{}` matched {} products under the selected generated-output roots",
                    name.to_string_lossy(),
                    matches.len()
                )));
            };
            Ok((*matched).clone())
        })
        .collect()
}

/// Project one selected graph unit into the shared semantic half of both capture records.
fn release_unit_manifest(
    graph_unit: &oven_rustc::rustc::OvenSelectedRustFacetUnit,
    captured: &oven_cargo_compat::OvenLegacyCargoSelectedUnit,
    profile: &str,
) -> CliResult<UnitManifest> {
    let manifest_digest = captured
        .registry_source
        .as_ref()
        .and_then(|source| source.members.iter().find(|member| member.path == "Cargo.toml"))
        .map(|member| member.digest.clone())
        .unwrap_or_else(|| graph_unit.source.digest.clone());
    let native = graph_unit
        .linked_libraries
        .iter()
        .enumerate()
        .map(|(index, library)| {
            serde_json::to_vec(library)
                .map(|bytes| NameDigest {
                    name: format!("native-{index:04}"),
                    digest: digest_bytes(&bytes),
                })
                .map_err(|error| CliError::failure(format!("could not encode native identity input: {error}")))
        })
        .collect::<CliResult<Vec<_>>>()?;
    Ok(UnitManifest {
        binding: format!(
            "{} {} {:?} {} [{}]",
            graph_unit.package,
            graph_unit.package_version,
            graph_unit.domain,
            profile,
            graph_unit.features.join(",")
        ),
        package: graph_unit.package.clone(),
        version: graph_unit.package_version.clone(),
        unit_identity: graph_unit.identity.clone(),
        features: graph_unit.features.clone(),
        source_digest: graph_unit.source.digest.clone(),
        manifest_digest,
        identity_inputs: IdentityInputs {
            cfg: graph_unit.cfg.clone(),
            out: graph_unit
                .generated_inputs
                .iter()
                .map(|input| super::equivalence::PathDigest {
                    path: input.name.clone(),
                    digest: input.digest.clone(),
                })
                .collect(),
            native,
            tools: Vec::new(),
            dependencies: graph_unit
                .dependencies
                .iter()
                .map(|dependency| DependencyIdentity {
                    name: dependency.alias.clone(),
                    unit_identity: dependency.unit.clone(),
                })
                .collect(),
        },
        native_objects: Vec::new(),
        artifacts: Vec::new(),
        asset_archive: None,
    })
}

/// Resolve and verify the one retained Cargo artifact for a rebuilt product extension.
fn retained_cargo_artifact(
    unit: &oven_cargo_compat::OvenLegacyCargoSelectedUnit,
    extension: Option<&str>,
    loaf_root: &Path,
) -> CliResult<PathBuf> {
    let matches = unit
        .retained_artifacts
        .iter()
        .filter(|artifact| artifact.extension.as_deref() == extension)
        .collect::<Vec<_>>();
    let [artifact] = matches.as_slice() else {
        return Err(CliError::failure(format!(
            "Cargo capture for `{}` has {} retained artifacts with extension {:?}; expected exactly one",
            unit.package,
            matches.len(),
            extension
        )));
    };
    verified_retained_file(&unit.package, &artifact.relative_path, &artifact.digest, loaf_root)
}

/// Resolve every captured native probe output through its retained build-script output inventory.
fn retained_cargo_native_artifacts(
    unit: &oven_cargo_compat::OvenLegacyCargoSelectedUnit,
    loaf_root: &Path,
) -> CliResult<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for script in unit
        .dependencies
        .iter()
        .filter_map(|dependency| dependency.build_script.as_ref())
    {
        for probe in &script.publisher_native_probes {
            let Some(output) = probe.output.as_ref() else {
                continue;
            };
            let retained = script.output.as_ref().ok_or_else(|| {
                CliError::failure(format!(
                    "Cargo capture for `{}` has native probe output without a retained output tree",
                    unit.package
                ))
            })?;
            let members = retained
                .members
                .iter()
                .filter(|member| member.path == output.relative_path && member.digest == output.digest)
                .count();
            if members != 1 {
                return Err(CliError::failure(format!(
                    "Cargo capture for `{}` native probe `{}` matched {members} retained members",
                    unit.package, output.relative_path
                )));
            }
            let relative = Path::new(&retained.relative_root).join(&output.relative_path);
            let relative = relative.to_str().ok_or_else(|| {
                CliError::failure(format!(
                    "Cargo capture for `{}` has a non-UTF-8 retained native path",
                    unit.package
                ))
            })?;
            paths.push(verified_retained_file(
                &unit.package,
                relative,
                &output.digest,
                loaf_root,
            )?);
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Verify one digest-bound, safe Loaf-relative retained file and return its physical path.
fn verified_retained_file(package: &str, relative_path: &str, digest: &str, loaf_root: &Path) -> CliResult<PathBuf> {
    let relative = Path::new(relative_path);
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(CliError::failure(format!(
            "Cargo capture for `{}` has unsafe retained artifact path `{}`",
            package, relative_path
        )));
    }
    let path = loaf_root.join(relative);
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        CliError::failure(format!(
            "could not inspect retained Cargo artifact {}: {error}",
            path.display()
        ))
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(CliError::failure(format!(
            "retained Cargo artifact is not a regular file: {}",
            path.display()
        )));
    }
    let bytes = fs::read(&path).map_err(|error| {
        CliError::failure(format!(
            "could not read retained Cargo artifact {}: {error}",
            path.display()
        ))
    })?;
    let actual = digest_bytes(&bytes);
    if actual != digest {
        return Err(CliError::failure(format!(
            "Cargo capture for `{}` retained artifact digest mismatch: declared {}, found {actual}",
            package, digest
        )));
    }
    Ok(path)
}

/// Record one executable's raw bytes and first version line for equivalence setup identity.
fn capture_executable_identity(executable: &Path) -> CliResult<ExecutableIdentity> {
    let bytes = fs::read(executable).map_err(|error| {
        CliError::failure(format!(
            "could not read equivalence executable {}: {error}",
            executable.display()
        ))
    })?;
    let output = Command::new(executable).arg("--version").output().map_err(|error| {
        CliError::failure(format!(
            "could not identify equivalence executable {}: {error}",
            executable.display()
        ))
    })?;
    if !output.status.success() {
        return Err(CliError::failure(format!(
            "equivalence executable {} refused --version",
            executable.display()
        )));
    }
    let version = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if version.is_empty() {
        return Err(CliError::failure(format!(
            "equivalence executable {} returned no version",
            executable.display()
        )));
    }
    Ok(ExecutableIdentity {
        version,
        digest: digest_bytes(&bytes),
    })
}

/// Map a retained compiler output suffix to the schema-1 artifact role vocabulary.
fn equivalence_artifact_role(extension: &str) -> &'static str {
    match extension {
        "rlib" => "rlib",
        "rmeta" => "rmeta",
        "dylib" | "so" | "dll" => "proc-macro",
        _ => "executable",
    }
}

/// Capture the stable source coordinates of one selected unit before publisher products rekey it.
fn publisher_unit_coordinates(
    graph: &oven_rustc::rustc::OvenSelectedRustFacetGraph,
    identity: &str,
) -> CliResult<PublisherUnitCoordinates> {
    let unit = graph
        .units
        .iter()
        .find(|unit| unit.identity == identity)
        .ok_or_else(|| CliError::failure(format!("selected graph has no publisher unit `{identity}`")))?;
    Ok(PublisherUnitCoordinates {
        package: unit.package.clone(),
        version: unit.package_version.clone(),
        crate_name: unit.crate_name.clone(),
        domain: unit.domain,
        source_identity: unit.source.identity.clone(),
        source_root: unit.source.root.clone(),
    })
}

/// Find the unique current identity of a unit whose source coordinates survive graph rekeying.
fn selected_identity_for_coordinates(
    graph: &oven_rustc::rustc::OvenSelectedRustFacetGraph,
    coordinates: &PublisherUnitCoordinates,
) -> CliResult<String> {
    let matches = graph
        .units
        .iter()
        .filter(|unit| {
            unit.package == coordinates.package
                && unit.package_version == coordinates.version
                && unit.crate_name == coordinates.crate_name
                && unit.domain == coordinates.domain
                && unit.source.identity == coordinates.source_identity
                && unit.source.root == coordinates.source_root
        })
        .map(|unit| unit.identity.clone())
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [identity] => Ok(identity.clone()),
        _ => Err(CliError::failure(format!(
            "publisher binding for `{}` {} crate `{}` matched {} selected units",
            coordinates.package,
            coordinates.version,
            coordinates.crate_name,
            matches.len()
        ))),
    }
}

/// Merge repeatable command-line owner roots with the user-owned `[publisher]` Oven configuration.
///
/// The configuration lives at `$INCAN_HOME/config.toml`, or `~/.incan/config.toml` when `INCAN_HOME` is unset. A
/// missing file means no configured publisher capability; malformed content refuses before any publisher process
/// runs. Project manifests are intentionally never consulted for executable authority.
fn configured_publisher_owner_roots(explicit: &[PathBuf]) -> CliResult<Vec<PathBuf>> {
    let config_root = env::var_os("INCAN_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| user_home().map(|home| PathBuf::from(home).join(".incan")));
    let mut roots = explicit.to_vec();
    if let Some(path) = config_root.map(|root| root.join("config.toml")) {
        match fs::read_to_string(&path) {
            Ok(source) => {
                let config = toml::from_str::<PublisherConfigFile>(&source).map_err(|error| {
                    CliError::failure(format!("Oven publisher config {} is invalid: {error}", path.display()))
                })?;
                roots.extend(config.publisher.link_owners);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(CliError::failure(format!(
                    "could not read Oven publisher config {}: {error}",
                    path.display()
                )));
            }
        }
    }
    roots.sort();
    roots.dedup();
    Ok(roots)
}

/// Select exactly one supplied owner root whose declared closure reproduces the record identity.
fn resolve_publisher_owner_root(
    package: &str,
    binding: &str,
    expected_owner: &str,
    owner_paths: &[&str],
    supplied: &[PathBuf],
) -> CliResult<PathBuf> {
    let mut matches = Vec::new();
    for root in supplied {
        if !root.is_dir() {
            continue;
        }
        let identity = publisher_owner_identity(root, owner_paths.iter().copied()).map_err(|error| {
            CliError::failure(format!(
                "could not inventory publisher owner {}: {error}",
                root.display()
            ))
        })?;
        if identity == expected_owner {
            matches.push(root.clone());
        }
    }
    match matches.as_slice() {
        [root] => Ok(root.clone()),
        [] => Err(CliError::failure(format!(
            "package `{package}` binding `{binding}` requires publisher owner `{expected_owner}`; supply its root with --link-owner or [publisher].link-owner"
        ))),
        _ => Err(CliError::failure(format!(
            "package `{package}` binding `{binding}` owner `{expected_owner}` matches more than one configured root"
        ))),
    }
}

/// Reserve a deterministic private product root below this bake's scratch tree.
fn publisher_product_root(parent: &Path, unit: usize, role: &str, name: &str) -> CliResult<PathBuf> {
    let safe_name = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let root = parent.join(format!("{unit}-{role}-{safe_name}"));
    if root.exists() {
        return Err(CliError::failure(format!(
            "publisher product root already exists: {}",
            root.display()
        )));
    }
    Ok(root)
}

/// Apply one bounded process policy to publisher compilers and generators.
fn publisher_process_limits() -> BoundedProcessLimits {
    BoundedProcessLimits {
        stdout_bytes: 1024 * 1024,
        stderr_bytes: 1024 * 1024,
        timeout: Some(Duration::from_secs(5 * 60)),
    }
}

/// Persist one verified tool receipt beside its generated products without replacing existing bytes.
fn write_publisher_tool_receipt(
    receipt: &oven_store::publisher_execution::OvenPublisherToolReceipt,
    product_root: &Path,
) -> CliResult<()> {
    oven_store::publisher_execution::verify_publisher_tool_receipt(receipt)
        .map_err(|error| CliError::failure(error.to_string()))?;
    let path = product_root.join("publisher-tool-receipt.json");
    let bytes = serde_json::to_vec_pretty(receipt)
        .map_err(|error| CliError::failure(format!("could not encode publisher tool receipt: {error}")))?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| CliError::failure(format!("could not create {}: {error}", path.display())))?;
    use std::io::Write;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| CliError::failure(format!("could not persist {}: {error}", path.display())))?;
    Ok(())
}

/// Bake or exactly reuse one complete compiler-owned Alpha Loaf envelope.
///
/// The command is hidden beneath `legacy_cargo`. Compiler-suite reuse is checked before Cargo; release reuse first
/// captures the physical closure needed to derive registry, runtime-foundation, and generation evidence. Normal
/// build/run/test commands never call this function and never fall back to it.
pub fn oven_legacy_cargo_bake_loafs(options: OvenLoafBakeCommandOptions) -> CliResult<ExitCode> {
    let started = Instant::now();
    let link_owner_roots = configured_publisher_owner_roots(&options.link_owners)?;
    if !options.compiler_root.is_dir() {
        return Err(CliError::failure(format!(
            "Loaf compiler root is not a directory: {}",
            options.compiler_root.display()
        )));
    }
    if !options.sdk_inventory.is_file() {
        return Err(CliError::failure(format!(
            "Loaf SDK inventory is not a regular file: {}",
            options.sdk_inventory.display()
        )));
    }
    if !options.cargo.is_file() || !options.rustc.is_file() {
        return Err(CliError::failure(
            "the explicit Loaf baker requires regular --cargo and --rustc executables".to_string(),
        ));
    }
    fs::create_dir_all(&options.output).map_err(|error| {
        CliError::failure(format!(
            "could not create Loaf output {}: {error}",
            options.output.display()
        ))
    })?;
    let publication_lock = acquire_exclusive_loaf_generation_lock(&options.output).map_err(oven_error)?;
    let output_parent = options
        .output
        .parent()
        .ok_or_else(|| CliError::failure("Loaf output has no parent directory".to_string()))?;
    let scratch = LoafTemporaryDirectory::create(output_parent, ".incan-oven-loaf-envelope-")
        .map_err(|error| CliError::failure(format!("could not allocate Loaf baker scratch directory: {error}")))?;
    let staged_root = scratch.path().join("staged");
    fs::create_dir_all(&staged_root)
        .map_err(|error| CliError::failure(format!("could not create Loaf staging root: {error}")))?;

    let envelope = match options.envelope {
        OvenLoafEnvelopeArgument::Release => OvenLoafEnvelope::Release,
        OvenLoafEnvelopeArgument::CompilerSuite => OvenLoafEnvelope::CompilerSuite,
    };
    if options.harvest_dir.is_some() && envelope != OvenLoafEnvelope::Release {
        return Err(CliError::failure(
            "--harvest-dir harvests the release runtime-foundation capture; the compiler-suite envelope has none"
                .to_string(),
        ));
    }
    // Stage the release compiler before Cargo observes any unit. Rustc folds its executable location into metadata
    // for multi-file crates, so harvesting through the caller's rustup path and rebuilding through this retained copy
    // produces different rlib bytes despite an identical `rustc -vV` identity.
    let release_toolchain = if envelope == OvenLoafEnvelope::Release {
        let relative_path = PathBuf::from("runtime-foundations/rust-toolchain");
        let root = staged_root.join(&relative_path);
        let target = rustc_host_target(&options.rustc).map_err(oven_error)?;
        let (compiler_closure_identity, members) =
            stage_release_runtime_foundation_toolchain(&options.rustc, &target, &root).map_err(oven_error)?;
        Some(StagedReleaseToolchain {
            relative_path,
            root,
            compiler_closure_identity,
            members,
        })
    } else {
        None
    };
    let bake_rustc = release_toolchain
        .as_ref()
        .map(|toolchain| toolchain.root.join("bin/rustc"))
        .unwrap_or_else(|| options.rustc.clone());
    let publisher_input = release_policy_publisher_input(
        envelope,
        options.policy_engine_store.as_deref(),
        options.policy_engine_identity.as_deref(),
        options.policy_engine_target.as_deref(),
    )?;
    let default_limits = loaf_envelope_default_limits(envelope);
    let combined_max_physical_bytes = options.max_physical_bytes.unwrap_or(default_limits.max_physical_bytes);
    let existing_suite_physical_bytes = if envelope == OvenLoafEnvelope::CompilerSuite {
        let suite_store = compiler_suite_store_path(&options)?;
        if suite_store.is_dir() {
            oven_cargo_compat::conservative_directory_reservation(&suite_store).map_err(oven_error)?
        } else {
            0
        }
    } else {
        0
    };
    let max_physical_bytes = combined_max_physical_bytes
        .checked_sub(existing_suite_physical_bytes)
        .filter(|remaining| *remaining > 0)
        .ok_or_else(|| {
            CliError::failure(format!(
                "existing compiler-suite storage uses {existing_suite_physical_bytes} bytes of the complete {combined_max_physical_bytes}-byte baker allowance"
            ))
        })?;
    let max_domain_physical_bytes = options
        .max_domain_physical_bytes
        .unwrap_or(default_limits.max_domain_physical_bytes)
        .min(max_physical_bytes);
    let max_domain_logical_bytes = options
        .max_domain_logical_bytes
        .unwrap_or(default_limits.max_domain_logical_bytes);
    let limits = OvenStoreLimits::new(max_physical_bytes, max_domain_physical_bytes, max_domain_logical_bytes);
    if max_physical_bytes == 0 || max_domain_physical_bytes == 0 || max_domain_logical_bytes == 0 {
        return Err(CliError::failure(
            "Loaf storage limits must be greater than zero".to_string(),
        ));
    }
    if options
        .max_domain_physical_bytes
        .unwrap_or(default_limits.max_domain_physical_bytes)
        > combined_max_physical_bytes
    {
        return Err(CliError::failure(
            "Loaf per-domain physical limit cannot exceed its aggregate physical limit".to_string(),
        ));
    }

    let current_executable = env::current_exe()
        .map_err(|error| CliError::failure(format!("could not resolve the active Incan executable: {error}")))?;
    let evidence = loaf_envelope_evidence(
        envelope,
        &options.compiler_root,
        &current_executable,
        &options.sdk_inventory,
        &bake_rustc,
    )?;
    let mut phase_timing = OvenLoafBakePhaseTiming {
        preflight_elapsed_ms: started.elapsed().as_millis(),
        ..OvenLoafBakePhaseTiming::default()
    };
    let release_store_member = publisher_input.map(|(_, identity, _)| OvenReleaseStoreMember {
        schema_version: OVEN_RELEASE_STORE_MEMBER_SCHEMA_VERSION,
        label: "rust-policy-engine".to_string(),
        store_relative_path: PathBuf::from("project-outputs/rust-policy-engine/oven/store/v2"),
        artifact_identity: identity.to_string(),
    });
    // Release generations bind a runtime-foundation descriptor that is known only after the explicit physical
    // capture and source-policy exchange. Compiler-suite generations have no such member and retain the cheap early
    // mirror/reuse path. Release reuse is checked below once the exact descriptor is available.
    if envelope == OvenLoafEnvelope::CompilerSuite {
        import_loaf_envelope_from_configured_mirrors(
            &options.output,
            scratch.path(),
            envelope,
            &evidence,
            release_store_member.as_ref(),
            None,
            None,
            None,
        )?;
    }
    if envelope == OvenLoafEnvelope::CompilerSuite
        && let Some(report) = reuse_complete_loaf_envelope(CompleteLoafEnvelopeReuseInput {
            output: &options.output,
            scratch: scratch.path(),
            envelope,
            evidence: &evidence,
            release_store_member: release_store_member.as_ref(),
            runtime_foundation: None,
            runtime_closure: None,
            loaf_registry_evidence: None,
            limits,
            started,
        })?
    {
        // Exact envelope validation and retirement require exclusive publication authority. Compiler-suite
        // completion then consumes the committed Loafs through a shared generation lease, so retaining the writer
        // lock across that transition would make this process wait on itself.
        let report = finish_loaf_bake_after_publication(publication_lock, &options, envelope, report, started)?;
        verify_committed_release_policy_output(
            &options.output,
            release_store_member.as_ref(),
            options.policy_engine_target.as_deref(),
        )?;
        print_loaf_bake_report(&report, options.format)?;
        return Ok(ExitCode::SUCCESS);
    }
    // Private fixture analysis may execute a normal Incan command. That command must be able to take a shared lease
    // on the currently committed generation while it determines whether the old Loaf is compatible. Holding the
    // publisher's exclusive lock here would make the parent wait for a child that is waiting for the parent. The
    // staged generation is private and has no publication authority, so release exclusivity until the atomic commit.
    drop(publication_lock);
    let generations_root = options.output.join("generations");
    fs::create_dir_all(&generations_root)
        .map_err(|error| CliError::failure(format!("could not create Loaf generations root: {error}")))?;
    let release_policy_output = if let (Some((source_store, _, expected_target)), Some(member)) =
        (publisher_input, release_store_member.as_ref())
    {
        Some(import_release_policy_output(
            source_store,
            &staged_root,
            member,
            expected_target,
            limits,
        )?)
    } else {
        None
    };
    let (release_member_logical_bytes, release_member_physical_bytes) =
        release_store_member_byte_counts(&staged_root, release_store_member.as_ref(), limits)?;
    let mut pending = Vec::new();
    let mut release_foundation_capture: Option<ReleaseFoundationCapture> = None;
    // TODO(#1561): temporary with `--harvest-dir` (see `harvest_release_entry`).
    // Harvest state: the compiler closure identity is computed once for every harvested entry and later checked
    // against the retained Toolchain owner; the report accumulates over both stdlib profiles.
    let mut harvest_compiler_closure: Option<String> = None;
    let mut harvest_report: Option<OvenLoafHarvestReport> = None;
    let envelope_inspection_packages = loaf_envelope_inspection_packages(envelope).map_err(CliError::failure)?;
    // A cold first bake cannot consume a Loaf that does not exist yet. Resolve its Rust inspection sources once at
    // this already explicit Cargo boundary, then hand the typed locked authority to every no-Cargo fixture child.
    let inspection_authority_started = Instant::now();
    announce_oven_progress("RESOLVE", "Rust inspection authority", None);
    let authority_dir = scratch.path().join("rust-inspect-authority");
    fs::create_dir_all(&authority_dir).map_err(|error| {
        CliError::failure(format!(
            "could not create explicit baker Rust inspection authority directory: {error}"
        ))
    })?;
    let compiler_manifest = loaf_compiler_manifest_path(&options.compiler_root)?;
    let envelope_inspection_sources = match envelope {
        OvenLoafEnvelope::CompilerSuite => {
            legacy_cargo_resolved_registry_sources(&options.cargo, &compiler_manifest, &[], &authority_dir)
        }
        OvenLoafEnvelope::Release => legacy_cargo_inspection_sources(
            &options.cargo,
            &compiler_manifest,
            &[],
            &envelope_inspection_packages,
            &authority_dir,
        ),
    }
    .map_err(oven_error)?;
    #[cfg(feature = "rust_inspect")]
    let baker_inspection_authority = {
        let sources = envelope_inspection_sources
            .iter()
            .map(|source| OvenInspectionRegistrySource {
                package: source.package.clone(),
                version: source.version.clone(),
                registry: source.registry.clone(),
                checksum: source.checksum.clone(),
                features: source.features.clone(),
                source_root: source.source_root.clone(),
                source_digest: source.source_digest.clone(),
            })
            .collect();
        write_sealed_oven_inspection_source_authority(&authority_dir, sources).map_err(|error| {
            CliError::failure(format!(
                "could not write explicit baker Rust inspection authority: {error}"
            ))
        })?
    };
    phase_timing.inspection_authority_elapsed_ms = inspection_authority_started.elapsed().as_millis();
    announce_oven_progress(
        "RESOLVED",
        "Rust inspection authority",
        Some(&elapsed_detail(inspection_authority_started)),
    );
    let cargo_process_started = true;
    let mut transient_peak_physical_bytes = 0_u64;
    let compiler_support_target = scratch.path().join("compiler-support-target");
    let probe_toolchain_data_root = scratch.path().join("probe-toolchain-data");
    fs::create_dir_all(probe_toolchain_data_root.join("share/incan/oven/loafs")).map_err(|error| {
        CliError::failure(format!(
            "could not create isolated Loaf fixture toolchain data: {error}"
        ))
    })?;
    let compiler_lock = loaf_compiler_lock_path(&options.compiler_root)?;
    let fixture_preparation_started = Instant::now();
    let specifications = loaf_envelope_specifications(envelope);
    let specification_count = specifications.len();
    for (position, specification) in specifications.iter().enumerate() {
        let fixture_started = Instant::now();
        let fixture_subject = format!("{}/{}", specification.label, specification.profile);
        announce_oven_progress(
            "BAKE",
            &fixture_subject,
            Some(&format!("{}/{specification_count}", position + 1)),
        );
        let inspection_packages = if specification.role.provides_source_authority() {
            specification.inspection_packages().map_err(CliError::failure)?
        } else {
            Vec::new()
        };
        let inspection_sources: &[OvenLegacyCargoInspectionSource] = if specification.role.provides_source_authority() {
            &envelope_inspection_sources
        } else {
            &[]
        };
        let project_root = scratch
            .path()
            .join("fixtures")
            .join(specification.label)
            .join(specification.profile);
        fs::create_dir_all(&project_root).map_err(|error| {
            CliError::failure(format!(
                "could not create checked Loaf fixture {}: {error}",
                specification.label
            ))
        })?;
        // Project-output authority hashes the conventional `src/` tree. The fixture must use that shape so its
        // deliberate first Oven miss still reaches receipt creation.
        let source_root = project_root.join("src");
        fs::create_dir_all(&source_root).map_err(|error| {
            CliError::failure(format!(
                "could not create checked Loaf fixture source directory {}: {error}",
                source_root.display()
            ))
        })?;
        let source = source_root.join("main.incn");
        fs::write(&source, specification.source).map_err(|error| {
            CliError::failure(format!(
                "could not write checked Loaf fixture {}: {error}",
                source.display()
            ))
        })?;
        fs::write(project_root.join("loaf.toml"), specification.manifest).map_err(|error| {
            CliError::failure(format!(
                "could not write checked Loaf manifest {}: {error}",
                specification.label
            ))
        })?;
        let mut command = Command::new(&current_executable);
        command
            .env_remove("INCAN_STDLIB")
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_SOURCE_ROOT", &options.compiler_root)
            .env("INCAN_SDK_INVENTORY", &options.sdk_inventory)
            .env(OVEN_LOAF_ENV, "1")
            .env("INCAN_HOME", project_root.join(".oven-home"));
        #[cfg(feature = "rust_inspect")]
        command.env(OVEN_LEGACY_CARGO_INSPECTION_AUTHORITY_ENV, &baker_inspection_authority);
        pin_loaf_fixture_rustc(&mut command, &bake_rustc);
        isolate_loaf_fixture_toolchain_data(&mut command, &probe_toolchain_data_root);
        match (specification.action, specification.profile) {
            (OvenLoafFixtureAction::Build, "release") => {
                command.args(["build", "--release"]).arg(&source);
            }
            (OvenLoafFixtureAction::Build, _) => {
                command.arg("build").arg(&source);
            }
            (OvenLoafFixtureAction::Run, "release") => {
                command.args(["run", "--release"]).arg(&source);
            }
            (OvenLoafFixtureAction::Run, _) => {
                command.arg("run").arg(&source);
            }
        }
        let probe = command.output().map_err(|error| {
            CliError::failure(format!(
                "could not analyze checked Loaf fixture {}: {error}",
                specification.label
            ))
        })?;
        let receipt_path = project_root.join(".incan/oven/receipt.json");
        let generated_project = project_root.join("target/incan").join(specification.project_name);
        let probe_stderr = String::from_utf8_lossy(&probe.stderr);
        if !probe.status.success() && !loaf_fixture_probe_is_expected_miss(&probe_stderr) {
            return Err(CliError::failure(format!(
                "checked Loaf fixture `{}` failed before its expected Oven miss:\n{}",
                specification.label,
                probe_stderr.trim()
            )));
        }
        if !receipt_path.is_file() || !generated_project.is_dir() {
            return Err(CliError::failure(format!(
                "checked Loaf fixture `{}` did not produce its receipt and generated project",
                specification.label
            )));
        }
        stage_locked_loaf_fixture(&options.cargo, &generated_project, &compiler_lock).map_err(oven_error)?;
        let receipt = read_receipt(&receipt_path)?;
        let prepared = prepare_loaf_from_generated_project_with_selected_units(
            &staged_root,
            &OvenLoafBakerContext {
                compiler: &incan_oven_facet::compiler_identity(),
                provider_hooks: incan_oven_facet::provider_hooks(),
                compiler_root: &options.compiler_root,
                compiler_support_target: &compiler_support_target,
                capacity_roots: [&options.output, scratch.path()],
                transient_limit: max_physical_bytes,
                cargo: &options.cargo,
                auxiliary_target_rustc: &options.rustc,
                rustc: &bake_rustc,
                cc: &options.cc,
                cxx: &options.cxx,
                c_sysroot: &options.c_sysroot,
                inspection_packages: &inspection_packages,
                inspection_sources,
                retain_complete_registry_leaves: specification.retain_complete_registry_leaves,
                retain_checked_direct_dependencies: specification.retain_checked_direct_dependencies,
                limits,
            },
            receipt.clone(),
            &generated_project,
        )
        .map_err(oven_error)?;
        // The harvest reads the gate's own capture for every entry that binds registry sources (both stdlib
        // profiles): no second Cargo run stands in for the observation, and the retained OUT_DIR members come from
        // the provisional Loaf the publisher just staged.
        if let Some(harvest_dir) = options.harvest_dir.as_deref()
            && specification.role.provides_source_authority()
        {
            let compiler_closure = match harvest_compiler_closure.as_deref() {
                Some(identity) => identity.to_string(),
                None => {
                    let identity = direct_rustc_compiler_closure_identity(&bake_rustc, &receipt.intent.target)
                        .map_err(oven_error)?;
                    harvest_compiler_closure = Some(identity.clone());
                    identity
                }
            };
            let entry_report = harvest_release_entry(
                harvest_dir,
                &prepared,
                &receipt,
                specification.profile,
                &compiler_closure,
                &options.compiler_root,
                &staged_root,
                &fixture_subject,
            )?;
            harvest_report = Some(match harvest_report.take() {
                Some(mut accumulated) => {
                    accumulated.proposals.extend(entry_report.proposals);
                    accumulated.refused += entry_report.refused;
                    accumulated.hazards = entry_report.hazards;
                    accumulated
                }
                None => entry_report,
            });
        }
        if envelope == OvenLoafEnvelope::Release
            && specification.label == "stdlib"
            && specification.profile == "release"
        {
            let selected_units = prepared
                .selected_units
                .clone()
                .ok_or_else(|| CliError::failure("release stdlib publisher produced no exact selected-unit capture"))?;
            if release_foundation_capture
                .replace(ReleaseFoundationCapture {
                    receipt: receipt.clone(),
                    capture: selected_units,
                    sources: inspection_sources.to_vec(),
                    manifest_source: specification.manifest.to_string(),
                    loaf_identity: prepared.preparation.loaf_identity.clone(),
                })
                .is_some()
            {
                return Err(CliError::failure(
                    "release envelope produced more than one runtime-foundation capture".to_string(),
                ));
            }
        }
        let result = prepared.preparation;
        let observed_transient = oven_cargo_compat::conservative_directory_reservation(&options.output)
            .and_then(|owned| {
                oven_cargo_compat::conservative_directory_reservation(scratch.path())
                    .map(|transient| owned.saturating_add(transient))
            })
            .map_err(oven_error)?;
        transient_peak_physical_bytes = transient_peak_physical_bytes
            .max(result.transient_peak_physical_bytes)
            .max(observed_transient);
        if observed_transient > max_physical_bytes {
            return Err(CliError::failure(format!(
                "Loaf baker transient storage reached {observed_transient} bytes, exceeding its {max_physical_bytes}-byte allowance"
            )));
        }
        if result.logical_bytes > max_domain_logical_bytes {
            return Err(CliError::failure(format!(
                "Loaf `{}` uses {} logical bytes, exceeding its {}-byte domain allowance",
                specification.label, result.logical_bytes, max_domain_logical_bytes
            )));
        }
        if result.physical_bytes > max_domain_physical_bytes {
            return Err(CliError::failure(format!(
                "Loaf `{}` uses {} physical bytes, exceeding its {}-byte domain allowance",
                specification.label, result.physical_bytes, max_domain_physical_bytes
            )));
        }
        announce_oven_progress(
            "BAKED",
            &fixture_subject,
            Some(&format!(
                "{}/{specification_count}, {}, {}",
                position + 1,
                human_bytes(result.physical_bytes),
                elapsed_detail(fixture_started)
            )),
        );
        pending.push(OvenLoafBakeEntryReport {
            label: specification.label.to_string(),
            profile: specification.profile.to_string(),
            action: loaf_fixture_action_name(specification.action).to_string(),
            role: specification.role,
            result,
        });
    }
    phase_timing.fixture_preparation_elapsed_ms = fixture_preparation_started.elapsed().as_millis();
    if envelope == OvenLoafEnvelope::Release && release_foundation_capture.is_none() {
        return Err(CliError::failure(
            "release envelope did not retain its runtime-foundation capture".to_string(),
        ));
    }
    // The selected graph names its Toolchain owner by the bounded compiler/sysroot closure digest, which is the
    // identity of the physical members the generation retains for that owner. Stage that closure first so the graph
    // is sealed against exactly what will ship, rather than against the compiler's version string.
    // The harvest named the compiler by the same closure digest the retained Toolchain owner carries; a
    // disagreement would mean the proposals describe a compiler this generation does not ship.
    if let (Some(harvested), Some(toolchain)) = (harvest_compiler_closure.as_deref(), release_toolchain.as_ref())
        && harvested != toolchain.compiler_closure_identity
    {
        return Err(CliError::failure(format!(
            "harvest named compiler closure {harvested}, but the retained Toolchain owner is {}",
            toolchain.compiler_closure_identity
        )));
    }
    let finalized_release_graph = if let (Some(foundation), Some(toolchain)) =
        (release_foundation_capture.as_ref(), release_toolchain.as_ref())
    {
        let ReleaseFoundationCapture {
            receipt,
            capture,
            sources,
            manifest_source,
            ..
        } = foundation;
        let toolchain_owner = toolchain.compiler_closure_identity.as_str();
        let manifest = ProjectManifest::from_str(manifest_source, Path::new("incan.toml"))
            .map_err(|error| CliError::failure(format!("release foundation manifest is invalid: {error}")))?;
        // A registered Loaf registry supplies RFC 119 declarations for captured registry units; where one binds
        // the exact captured source and selection it governs that unit, and the observation must agree with it.
        // A pinned registry opens only at the index commit the release is settled against.
        let mut registry_authority = match (
            options.loaf_registry.as_deref(),
            options.loaf_registry_commit.as_deref(),
        ) {
            (Some(root), Some(commit)) => {
                let registry =
                    LoafRegistry::open_pinned(root, commit).map_err(|error| CliError::failure(error.to_string()))?;
                LoafRegistryAuthority::resolve(capture, &registry, &receipt.intent.profile).map_err(oven_error)?
            }
            (Some(root), None) => {
                let registry = LoafRegistry::open(root).map_err(|error| CliError::failure(error.to_string()))?;
                LoafRegistryAuthority::resolve(capture, &registry, &receipt.intent.profile).map_err(oven_error)?
            }
            (None, _) => LoafRegistryAuthority::none(),
        };
        if let Some(harvest_dir) = options.harvest_dir.as_deref() {
            registry_authority = registry_authority
                .with_same_run_harvest(capture, harvest_dir, &receipt.intent.profile)
                .map_err(oven_error)?;
        }
        let registry_records = registry_authority.registry_records();
        let (_, generated) = legacy_cargo_generated_output_bindings(capture).map_err(oven_error)?;
        let linked = legacy_cargo_generated_archive_bindings(capture, &generated).map_err(oven_error)?;
        let provisional = legacy_cargo_foundation_projection(
            capture,
            receipt,
            sources,
            &receipt.identity,
            toolchain_owner,
            &linked,
            &registry_authority,
        )
        .map_err(oven_error)?;
        let closure_digest =
            legacy_cargo_build_script_closure_digest(capture, &provisional.build_scripts).map_err(oven_error)?;
        let capture_receipt =
            receipt_with_build_unit_input(receipt, OVEN_LEGACY_CARGO_BUILD_SCRIPT_CLOSURE_INPUT, closure_digest)
                .map_err(oven_error)?;
        let projection = legacy_cargo_foundation_projection(
            capture,
            receipt,
            sources,
            &capture_receipt.identity,
            toolchain_owner,
            &linked,
            &registry_authority,
        )
        .map_err(oven_error)?;
        let mut finalized = finalize_compiler_support_selected_graph(
            capture,
            &projection,
            &manifest,
            &BTreeSet::new(),
            toolchain_owner,
            receipt,
        )
        .map_err(oven_error)?;
        let publisher_products = staged_root.join("publisher-products");
        fs::create_dir_all(&publisher_products).map_err(|error| {
            CliError::failure(format!(
                "could not create publisher product root {}: {error}",
                publisher_products.display()
            ))
        })?;
        let generated_owners = execute_adopted_publisher_work(
            &mut finalized,
            &registry_authority,
            &link_owner_roots,
            &publisher_products,
        )?;
        Some((
            finalized,
            registry_authority.evidence_digest(),
            registry_records,
            generated_owners,
        ))
    } else {
        None
    };
    let (finalized_release_graph, loaf_registry_evidence, registry_records, publisher_generated_owners) =
        match finalized_release_graph {
            Some((finalized, evidence, records, generated)) => (Some(finalized), evidence, records, generated),
            None => (None, None, Vec::new(), Vec::new()),
        };
    if let (Some(finalized), Some(foundation)) = (finalized_release_graph.as_ref(), release_foundation_capture.as_ref())
    {
        let expected_capture = &foundation.capture;
        let capture_loaf_identity = &foundation.loaf_identity;
        // Bind each compiled physical unit to the selected identity projected from it. Run-custom-build units are
        // edge endpoints, not graph units; every other unit must map to exactly one distinct identity.
        let compiled_units = finalized
            .capture
            .units
            .iter()
            .enumerate()
            .filter(|(_, unit)| unit.mode != "run-custom-build")
            .collect::<Vec<_>>();
        // Identities come from the units as originally captured: pruning renumbers edges, and the final bake
        // observes the unpruned closure again.
        let selected_unit_bindings = compiled_units
            .iter()
            .map(|(index, captured)| {
                let selected = finalized.unit_identities.get(index).ok_or_else(|| {
                    CliError::failure(format!(
                        "final selected graph has no unit for compiled physical unit `{}`",
                        captured.package_id
                    ))
                })?;
                let original = finalized
                    .source_indices
                    .get(*index)
                    .copied()
                    .filter(|source| *source < expected_capture.units.len())
                    .ok_or_else(|| {
                        CliError::failure(format!(
                            "pruned physical unit `{}` has no source in the original capture",
                            captured.package_id
                        ))
                    })?;
                oven_cargo_compat::legacy_cargo_selected_unit_capture_identity(expected_capture, original)
                    .map(|identity| (identity, selected.clone()))
                    .map_err(oven_error)
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let distinct_identities = selected_unit_bindings.values().collect::<BTreeSet<_>>();
        if selected_unit_bindings.len() != compiled_units.len()
            || distinct_identities.len() != compiled_units.len()
            || finalized.graph.graph().units.len() != compiled_units.len()
        {
            return Err(CliError::failure(
                "final selected graph does not correspond one-to-one with its compiled physical units".to_string(),
            ));
        }
        // The final receipt changes the authority, not the bytes. Republish the provisional Loaf's artifacts under
        // it rather than compiling the closure a second time: the graph then describes exactly what ships, the
        // registry leaves bind through the capture that produced them, and no build-script output has to be
        // byte-reproducible across two stagings.
        let provisional_loaf = staged_root.join(format!(
            "{}.loaf",
            capture_loaf_identity
                .strip_prefix("sha256:")
                .unwrap_or(capture_loaf_identity)
        ));
        let republished = republish_loaf_under_final_receipt(
            &provisional_loaf,
            &finalized.final_receipt,
            expected_capture,
            &selected_unit_bindings,
        )
        .map_err(oven_error)?;
        let final_entry = pending
            .iter_mut()
            .find(|entry| entry.label == "stdlib" && entry.profile == "release")
            .ok_or_else(|| CliError::failure("release stdlib result is absent"))?;
        final_entry.result = republished;
    }
    let (release_policy_inventories, build_script_warnings) =
        if let (Some(finalized), Some(policy), Some(foundation), Some(toolchain)) = (
            finalized_release_graph.as_ref(),
            release_policy_output.as_ref(),
            release_foundation_capture.as_ref(),
            release_toolchain.as_ref(),
        ) {
            let request = encode_selected_graph_policy_request(
                &finalized.graph,
                &foundation.capture,
                &foundation.sources,
                &toolchain.compiler_closure_identity,
            )
            .map_err(oven_error)?;
            let exchange_root = scratch.path().join("rust-policy-exchange");
            let response = run_release_rust_policy(policy, &exchange_root, &request)?;
            match runtime_foundation_inventories_from_policy_response(&finalized.graph, &response) {
                Ok(selection) => {
                    let warnings = selection
                        .warnings
                        .iter()
                        .map(|warning| format!("build script for package `{}` is inert", warning.package))
                        .collect::<Vec<_>>();
                    for warning in &warnings {
                        eprintln!("warning: {warning}");
                    }
                    (Some(selection.inventories), warnings)
                }
                Err(error) => {
                    // The scratch exchange is discarded with the publisher; retain the refused exchange where an
                    // investigation can replay it against the policy engine and its tests.
                    let retained = retain_refused_policy_exchange(&exchange_root).map_err(oven_error)?;
                    return Err(CliError::failure(format!(
                        "{error}; the refused exchange is retained at {}",
                        retained.display()
                    )));
                }
            }
        } else {
            (None, Vec::new())
        };
    if envelope == OvenLoafEnvelope::Release && release_policy_inventories.is_none() {
        return Err(CliError::failure(
            "release envelope did not produce admitted Rust policy inventories".to_string(),
        ));
    }
    let (runtime_foundation, runtime_closure) = if let (Some(finalized), Some(inventories), Some(toolchain)) = (
        finalized_release_graph.as_ref(),
        release_policy_inventories,
        release_toolchain,
    ) {
        let final_entry_index = pending
            .iter()
            .position(|entry| entry.label == "stdlib" && entry.profile == "release")
            .ok_or_else(|| CliError::failure("release stdlib result is absent"))?;
        let loaf_name = pending[final_entry_index]
            .result
            .loaf_identity
            .strip_prefix("sha256:")
            .unwrap_or(&pending[final_entry_index].result.loaf_identity);
        let mut loaf_root = staged_root.join(format!("{loaf_name}.loaf"));
        let mut loaf: OvenLoaf = serde_json::from_slice(
            &fs::read(loaf_root.join("loaf.json"))
                .map_err(|error| CliError::failure(format!("could not read final release Loaf: {error}")))?,
        )
        .map_err(|error| CliError::failure(format!("final release Loaf is invalid: {error}")))?;
        let foundation =
            runtime_foundation_for_publisher_rebuild(finalized, &loaf, &toolchain.compiler_closure_identity)
                .map_err(oven_error)?;
        pending[final_entry_index].result =
            bind_staged_loaf_to_runtime_foundation(&mut loaf_root, &mut loaf, &foundation.artifacts)?;
        let final_result = pending[final_entry_index].result.clone();
        let committed = committed_release_runtime_members(&options.output).map_err(oven_error)?;
        let reusable = if let Some((foundation_member, closure_member)) = committed {
            let candidate_asset = OvenRuntimeFoundationAsset::sealed(foundation.clone(), inventories.clone())
                .map_err(oven_error)?
                .validated()
                .map_err(oven_error)?;
            let exact = foundation_member.foundation_identity == candidate_asset.foundation_identity()
                && foundation_member.compiled_loaf_identity == final_result.loaf_identity
                && foundation_member.compiled_plan_identity == final_result.plan_identity
                && foundation_member.toolchain_owner_identity == toolchain.compiler_closure_identity
                && closure_member.foundation_identity == foundation_member.foundation_identity
                && closure_member.compiler_closure_identity == foundation_member.compiler_closure_identity;
            exact.then_some((foundation_member, closure_member))
        } else {
            None
        };
        if let Some((foundation_member, closure_member)) = reusable {
            let publication_lock = acquire_exclusive_loaf_generation_lock(&options.output).map_err(oven_error)?;
            if let Some(report) = reuse_complete_loaf_envelope(CompleteLoafEnvelopeReuseInput {
                output: &options.output,
                scratch: scratch.path(),
                envelope,
                evidence: &evidence,
                release_store_member: release_store_member.as_ref(),
                runtime_foundation: Some(&foundation_member),
                runtime_closure: Some(&closure_member),
                loaf_registry_evidence: loaf_registry_evidence.as_deref(),
                limits,
                started,
            })? {
                let mut report =
                    finish_loaf_bake_after_publication(publication_lock, &options, envelope, report, started)?;
                report.harvest = harvest_report;
                report.registry_records = registry_records.clone();
                report.warnings = build_script_warnings.clone();
                verify_committed_release_policy_output(
                    &options.output,
                    release_store_member.as_ref(),
                    options.policy_engine_target.as_deref(),
                )?;
                print_loaf_bake_report(&report, options.format)?;
                return Ok(ExitCode::SUCCESS);
            }
            drop(publication_lock);
        }
        {
            let StagedReleaseToolchain {
                relative_path: toolchain_relative,
                root: toolchain_root,
                compiler_closure_identity,
                members: toolchain_members,
            } = toolchain;
            let asset = OvenRuntimeFoundationAsset::sealed(foundation, inventories).map_err(oven_error)?;
            let foundation_relative = PathBuf::from("runtime-foundations/rust-policy-foundation");
            let generated_owner_roots = publisher_generated_owners
                .iter()
                .map(|owner| OvenSelectedRustFacetOwnerRoot {
                    identity: owner.identity.clone(),
                    root: owner.root.clone(),
                })
                .collect::<Vec<_>>();
            let admitted = publish_runtime_foundation_asset_with_generated_owners(
                asset,
                &loaf_root,
                &toolchain_root,
                &generated_owner_roots,
                &staged_root.join(&foundation_relative),
            )
            .map_err(oven_error)?;
            write_publisher_asset_claims(
                &publisher_generated_owners,
                admitted.foundation_identity(),
                &staged_root.join("asset-claims"),
            )?;
            let foundation_member = OvenReleaseRuntimeFoundationMember {
                schema_version: OVEN_RELEASE_RUNTIME_FOUNDATION_MEMBER_SCHEMA_VERSION,
                label: "rust-policy-foundation".to_string(),
                foundation_relative_path: foundation_relative,
                foundation_identity: admitted.foundation_identity().to_string(),
                compiled_loaf_identity: final_result.loaf_identity,
                compiled_plan_identity: final_result.plan_identity,
                toolchain_owner_identity: compiler_closure_identity.clone(),
                compiler_closure_identity: compiler_closure_identity.clone(),
                toolchain_root_relative_path: toolchain_relative,
                toolchain_members,
            };
            let materialized = admitted.materialize_asset_for_publication().map_err(oven_error)?;
            let compiler =
                OvenRuntimeCompilerClosure::new(toolchain_root.join("bin/rustc"), compiler_closure_identity.clone());
            let build = execute_runtime_foundation_rebuild(
                materialized.foundation(),
                materialized.materialized(),
                &compiler,
                &scratch.path().join("runtime-closure-build"),
            )
            .map_err(oven_error)?;
            write_release_equivalence_captures(
                &options,
                finalized,
                &build,
                &loaf_root,
                &staged_root.join(&foundation_member.foundation_relative_path),
                &staged_root,
            )?;
            let closure_store_relative = PathBuf::from("runtime-closures/store");
            let closure_store = OvenStore::new(staged_root.join(&closure_store_relative), limits);
            let closure_manifest = publish_runtime_closure(
                &closure_store,
                &finalized.final_receipt,
                materialized.foundation(),
                &build,
                &foundation_member.foundation_identity,
            )
            .map_err(oven_error)?;
            let closure_payload = oven_rustc::rustc::runtime_closure_payload(
                materialized.foundation(),
                &build,
                &foundation_member.foundation_identity,
            )
            .map_err(oven_error)?;
            let closure_member = OvenReleaseRuntimeClosureMember {
                schema_version: OVEN_RELEASE_RUNTIME_CLOSURE_MEMBER_SCHEMA_VERSION,
                label: "rust-policy-closure".to_string(),
                store_relative_path: closure_store_relative,
                artifact_identity: closure_manifest.identity,
                closure_identity: closure_payload.identity().map_err(oven_error)?,
                foundation_identity: foundation_member.foundation_identity.clone(),
                compiler_closure_identity,
            };
            (Some(foundation_member), Some(closure_member))
        }
    } else {
        (None, None)
    };
    if envelope == OvenLoafEnvelope::Release {
        let publication_lock = acquire_exclusive_loaf_generation_lock(&options.output).map_err(oven_error)?;
        import_loaf_envelope_from_configured_mirrors(
            &options.output,
            scratch.path(),
            envelope,
            &evidence,
            release_store_member.as_ref(),
            runtime_foundation.as_ref(),
            runtime_closure.as_ref(),
            loaf_registry_evidence.as_deref(),
        )?;
        if let Some(report) = reuse_complete_loaf_envelope(CompleteLoafEnvelopeReuseInput {
            output: &options.output,
            scratch: scratch.path(),
            envelope,
            evidence: &evidence,
            release_store_member: release_store_member.as_ref(),
            runtime_foundation: runtime_foundation.as_ref(),
            runtime_closure: runtime_closure.as_ref(),
            loaf_registry_evidence: loaf_registry_evidence.as_deref(),
            limits,
            started,
        })? {
            let report = finish_loaf_bake_after_publication(publication_lock, &options, envelope, report, started)?;
            verify_committed_release_policy_output(
                &options.output,
                release_store_member.as_ref(),
                options.policy_engine_target.as_deref(),
            )?;
            print_loaf_bake_report(&report, options.format)?;
            return Ok(ExitCode::SUCCESS);
        }
        drop(publication_lock);
    }

    let (foundation_logical_bytes, foundation_physical_bytes) = if let Some(member) = runtime_foundation.as_ref() {
        let foundation =
            loaf_directory_byte_counts(&staged_root.join(&member.foundation_relative_path)).map_err(oven_error)?;
        let toolchain =
            loaf_directory_byte_counts(&staged_root.join(&member.toolchain_root_relative_path)).map_err(oven_error)?;
        let closure = runtime_closure
            .as_ref()
            .map(|member| loaf_directory_byte_counts(&staged_root.join(&member.store_relative_path)))
            .transpose()
            .map_err(oven_error)?
            .unwrap_or((0, 0));
        (
            foundation.0.saturating_add(toolchain.0).saturating_add(closure.0),
            foundation.1.saturating_add(toolchain.1).saturating_add(closure.1),
        )
    } else {
        (0, 0)
    };
    let logical_bytes = pending
        .iter()
        .map(|entry| entry.result.logical_bytes)
        .sum::<u64>()
        .saturating_add(release_member_logical_bytes)
        .saturating_add(foundation_logical_bytes);
    let physical_bytes = pending
        .iter()
        .map(|entry| entry.result.physical_bytes)
        .sum::<u64>()
        .saturating_add(release_member_physical_bytes)
        .saturating_add(foundation_physical_bytes);
    if physical_bytes > max_physical_bytes {
        return Err(CliError::failure(format!(
            "Loaf envelope uses {physical_bytes} physical bytes, exceeding its {max_physical_bytes}-byte allowance"
        )));
    }

    let prepared_count = pending.len();
    let envelope_publication_started = Instant::now();
    announce_oven_progress("PUBLISH", "Loaf envelope", Some(&format!("{prepared_count} Loaf(s)")));
    let mut compatibility_evidence = loaf_envelope_compatibility_map_with_release_member(
        &evidence,
        release_store_member.as_ref(),
        loaf_registry_evidence.as_deref(),
    )?;
    if let Some(member) = runtime_foundation.as_ref() {
        oven_rustc::loaf::bind_release_runtime_foundation_evidence(&mut compatibility_evidence, member)
            .map_err(oven_error)?;
    }
    if let Some(member) = runtime_closure.as_ref() {
        oven_rustc::loaf::bind_release_runtime_closure_evidence(&mut compatibility_evidence, member)
            .map_err(oven_error)?;
    }
    let generation_identity =
        loaf_generation_identity_with_release_member(envelope, &compatibility_evidence, release_store_member.as_ref())?;
    let generation_name = generation_identity
        .strip_prefix("sha256:")
        .unwrap_or(&generation_identity);
    let generation_relative = Path::new("generations").join(generation_name);
    let generation_output = options.output.join(&generation_relative);
    let manifest = OvenLoafEnvelopeManifest {
        schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
        envelope: loaf_envelope_name(envelope).to_string(),
        generation_identity: generation_identity.clone(),
        evidence: compatibility_evidence,
        loafs: pending
            .iter()
            .map(|entry| {
                let identity = entry
                    .result
                    .loaf_identity
                    .strip_prefix("sha256:")
                    .unwrap_or(&entry.result.loaf_identity);
                OvenLoafEnvelopeMember {
                    label: entry.label.clone(),
                    profile: entry.profile.clone(),
                    action: entry.action.clone(),
                    role: entry.role,
                    build_unit_identity: entry.result.build_unit_identity.clone(),
                    loaf_identity: entry.result.loaf_identity.clone(),
                    plan_identity: entry.result.plan_identity.clone(),
                    logical_bytes: entry.result.logical_bytes,
                    physical_bytes: entry.result.physical_bytes,
                    path: generation_relative.join(format!("{identity}.loaf/loaf.json")),
                }
            })
            .collect(),
        release_store_member: release_store_member.clone(),
        runtime_foundation,
        runtime_closure,
    };
    let publication_lock = acquire_exclusive_loaf_generation_lock(&options.output).map_err(oven_error)?;
    let replacement_high_water = oven_cargo_compat::conservative_directory_reservation(&options.output)
        .and_then(|owned| {
            oven_cargo_compat::conservative_directory_reservation(scratch.path())
                .map(|transient| owned.saturating_add(transient))
        })
        .map_err(oven_error)?;
    transient_peak_physical_bytes = transient_peak_physical_bytes.max(replacement_high_water);
    if replacement_high_water > max_physical_bytes {
        return Err(CliError::failure(format!(
            "Loaf replacement high water reached {replacement_high_water} bytes, exceeding its {max_physical_bytes}-byte allowance"
        )));
    }
    commit_loaf_generation(
        &options.output,
        &generations_root,
        &generation_output,
        &staged_root,
        &manifest,
        scratch.path(),
        || Ok(()),
    )
    .map_err(oven_error)?;

    // The new manifest is the envelope's single authority. Retire old content-addressed generations only after that
    // authority has committed, so any earlier publication failure leaves the previous complete envelope usable.
    // A retirement failure is safe: the newly committed Loafs remain valid and obsolete unreferenced data can be
    // reclaimed by the next successful bake.
    retire_unreferenced_loaf_generations(&options.output, &generation_identity, scratch.path()).map_err(oven_error)?;

    let (_, owned_physical_bytes) = loaf_directory_byte_counts(&options.output).map_err(oven_error)?;
    let raw_disk_bytes = loaf_raw_disk_bytes(&options.output).map_err(oven_error)?;
    if owned_physical_bytes > max_physical_bytes {
        return Err(CliError::failure(format!(
            "published Loaf output uses {owned_physical_bytes} physical bytes after reclaiming obsolete generations, exceeding its {max_physical_bytes}-byte allowance"
        )));
    }
    phase_timing.envelope_publication_elapsed_ms = envelope_publication_started.elapsed().as_millis();
    announce_oven_progress(
        "PUBLISHED",
        "Loaf envelope",
        Some(&elapsed_detail(envelope_publication_started)),
    );
    let reused_count = 0;
    let report = OvenLoafBakeReport {
        action: "prepared".to_string(),
        envelope: loaf_envelope_name(envelope).to_string(),
        loaf_count: pending.len(),
        prepared_count,
        reused_count,
        logical_bytes,
        physical_bytes,
        owned_physical_bytes,
        raw_disk_bytes,
        // Publication retires every obsolete generation before this report. The active manifest/lock account for
        // owned overhead beyond the referenced Loafs and must not be misreported as reclaimable data.
        reclaimable_physical_bytes: 0,
        active_lease_physical_bytes: 0,
        transient_peak_physical_bytes,
        max_physical_bytes,
        max_domain_physical_bytes,
        max_domain_logical_bytes,
        elapsed_ms: started.elapsed().as_millis(),
        phase_timing,
        cargo_process_started,
        evidence,
        loafs: pending,
        compiler_suite: None,
        harvest: harvest_report,
        registry_records: registry_records.clone(),
        warnings: build_script_warnings,
    };
    // `finish_loaf_bake` opens the committed Loafs as a normal shared-lease consumer. Publication and retirement
    // are complete, so release exclusive authority before crossing into that consumer phase.
    let report = finish_loaf_bake_after_publication(publication_lock, &options, envelope, report, started)?;
    verify_committed_release_policy_output(
        &options.output,
        release_store_member.as_ref(),
        options.policy_engine_target.as_deref(),
    )?;
    print_loaf_bake_report(&report, options.format)?;
    Ok(ExitCode::SUCCESS)
}

/// Reissue one staged compiled Loaf under the artifact manifest rebuilt by its runtime foundation.
///
/// The release envelope binds the compiled Loaf and runtime foundation by both content identities. The publisher
/// foundation deliberately removes Cargo artifacts and replaces them with direct-`rustc` rebuild inputs, so that
/// rebuilt manifest must become the compiled Loaf's final plan before either identity is recorded.
fn bind_staged_loaf_to_runtime_foundation(
    loaf_root: &mut PathBuf,
    loaf: &mut OvenLoaf,
    artifacts: &OvenRustcArtifactManifest,
) -> CliResult<OvenLoafPreparation> {
    loaf.plan = artifacts.clone();
    loaf.registry_leaves = loaf.plan.registry_leaves.clone();
    let plan_identity = digest_bytes(
        &serde_json::to_vec(&loaf.plan)
            .map_err(|error| CliError::failure(format!("could not encode rebuilt Loaf plan identity: {error}")))?,
    );
    let loaf_bytes = serde_json::to_vec_pretty(loaf)
        .map_err(|error| CliError::failure(format!("could not encode rebuilt Loaf: {error}")))?;
    let loaf_identity = digest_bytes(&loaf_bytes);
    let parent = loaf_root
        .parent()
        .ok_or_else(|| CliError::failure(format!("staged Loaf has no parent: {}", loaf_root.display())))?;
    let identity_name = loaf_identity.strip_prefix("sha256:").unwrap_or(&loaf_identity);
    let rebound_root = parent.join(format!("{identity_name}.loaf"));
    if rebound_root == *loaf_root {
        let (logical_bytes, physical_bytes) = loaf_directory_byte_counts(loaf_root).map_err(oven_error)?;
        return Ok(OvenLoafPreparation {
            build_unit_identity: loaf.build_unit_identity.clone(),
            loaf_identity,
            plan_identity,
            logical_bytes,
            physical_bytes,
            transient_peak_physical_bytes: 0,
        });
    }
    if rebound_root.exists() {
        return Err(CliError::failure(format!(
            "content-addressed rebuilt Loaf destination already exists: {}",
            rebound_root.display()
        )));
    }
    fs::write(loaf_root.join("loaf.json"), loaf_bytes)
        .map_err(|error| CliError::failure(format!("could not write rebuilt Loaf: {error}")))?;
    fs::rename(loaf_root.as_path(), &rebound_root)
        .map_err(|error| CliError::failure(format!("could not publish rebuilt Loaf identity: {error}")))?;
    *loaf_root = rebound_root;
    let (logical_bytes, physical_bytes) = loaf_directory_byte_counts(loaf_root).map_err(oven_error)?;
    Ok(OvenLoafPreparation {
        build_unit_identity: loaf.build_unit_identity.clone(),
        loaf_identity,
        plan_identity,
        logical_bytes,
        physical_bytes,
        transient_peak_physical_bytes: 0,
    })
}

/// Validate and copy the exact release policy ProjectOutput into the private generation store.
pub(crate) fn import_release_policy_output(
    source_store: &Path,
    staged_generation: &Path,
    member: &OvenReleaseStoreMember,
    expected_target: &str,
    limits: OvenStoreLimits,
) -> CliResult<OvenStoredProjectOutput> {
    let mut selected = PublishedOvenStore::new(source_store)
        .select_payloads_matching_for_execution(|manifest| manifest.identity == member.artifact_identity)
        .map_err(oven_error)?;
    if selected.len() != 1 {
        return Err(CliError::failure(
            "policy-engine store must contain exactly the declared identity",
        ));
    }
    let selected = selected
        .pop()
        .ok_or_else(|| CliError::failure("policy-engine selection became empty"))?;
    selected.verify_materialized_files().map_err(oven_error)?;
    let receipt = selected
        .original_native_receipt()
        .cloned()
        .ok_or_else(|| CliError::failure("release policy ProjectOutput has no original publisher receipt"))?;
    let admitted_files = selected.admitted_materialized_files().to_vec();
    let admitted_directories = selected.admitted_materialized_directories().to_vec();
    let (manifest, artifact_root, payload_bytes, lease) = selected.into_parts();
    let payload: OvenProjectOutputPayload = serde_json::from_slice(&payload_bytes)
        .map_err(|error| CliError::failure(format!("release policy ProjectOutput payload is invalid: {error}")))?;
    let stored = stored_project_output_from_parts(manifest.clone(), artifact_root.clone(), payload, lease)?;
    validate_release_policy_project_output(&manifest, &stored.payload, &receipt, expected_target)?;
    let destination = OvenStore::new(staged_generation.join(&member.store_relative_path), limits);
    let materialized_files = manifest
        .materialized_files
        .iter()
        .map(|file| OvenArtifactMaterializedFile {
            source_path: artifact_root.join(&file.relative_path),
            relative_path: file.relative_path.clone(),
        })
        .collect();
    let materialized_directories = manifest
        .materialized_directories
        .iter()
        .map(|directory| OvenArtifactMaterializedDirectory {
            source_path: artifact_root.join(&directory.relative_path),
            relative_path: directory.relative_path.clone(),
        })
        .collect();
    let published = destination
        .publish_verified_import(
            &OvenArtifactPublishRequest {
                receipt,
                domain: manifest.domain.clone(),
                kind: manifest.kind,
                payload: payload_bytes,
                materialized_files,
                materialized_directories,
            },
            &admitted_files,
            &admitted_directories,
        )
        .map_err(oven_error)?;
    if published.identity != member.artifact_identity {
        return Err(CliError::failure(
            "embedded policy-engine import changed its exact artifact identity",
        ));
    }
    let mut selected = PublishedOvenStore::new(staged_generation.join(&member.store_relative_path))
        .select_payloads_matching_for_execution(|manifest| manifest.identity == member.artifact_identity)
        .map_err(oven_error)?;
    if selected.len() != 1 {
        return Err(CliError::failure(
            "embedded policy-engine import did not retain exactly its declared identity",
        ));
    }
    let selected = selected
        .pop()
        .ok_or_else(|| CliError::failure("embedded policy-engine selection became empty"))?;
    selected.verify_materialized_files().map_err(oven_error)?;
    let (manifest, artifact_root, payload_bytes, lease) = selected.into_parts();
    let payload = serde_json::from_slice(&payload_bytes)
        .map_err(|error| CliError::failure(format!("embedded policy-engine payload is invalid: {error}")))?;
    stored_project_output_from_parts(manifest, artifact_root, payload, lease)
}

/// Copy a refused Rust policy exchange out of publisher scratch so it can be inspected and replayed.
fn retain_refused_policy_exchange(exchange_root: &Path) -> Result<PathBuf, oven_rustc::loaf::OvenLoafError> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let destination = std::env::temp_dir().join(format!("incan-oven-rust-policy-refusal-{stamp}"));
    fs::create_dir_all(&destination).map_err(|source| oven_rustc::loaf::OvenLoafError::Io {
        path: destination.clone(),
        source,
    })?;
    for name in ["request.json", "response.json"] {
        let from = exchange_root.join(name);
        if from.is_file() {
            fs::copy(&from, destination.join(name))
                .map_err(|source| oven_rustc::loaf::OvenLoafError::Io { path: from, source })?;
        }
    }
    Ok(destination)
}

/// Execute the exact admitted policy engine with one bounded file exchange.
fn run_release_rust_policy(
    policy: &OvenStoredProjectOutput,
    exchange_root: &Path,
    request: &serde_json::Value,
) -> CliResult<serde_json::Value> {
    fs::create_dir(exchange_root).map_err(|error| {
        CliError::failure(format!(
            "could not create private Rust policy exchange directory {}: {error}",
            exchange_root.display()
        ))
    })?;
    let request_path = exchange_root.join("request.json");
    let response_path = exchange_root.join("response.json");
    if response_path.exists() {
        return Err(CliError::failure(
            "private Rust policy exchange already contains a response".to_string(),
        ));
    }
    let encoded = serde_json::to_vec(request)
        .map_err(|error| CliError::failure(format!("could not encode Rust policy request: {error}")))?;
    if encoded.len() as u64 > POLICY_EXCHANGE_MAX_BYTES {
        return Err(CliError::failure(
            "Rust policy request exceeds its bounded exchange allowance",
        ));
    }
    fs::write(&request_path, encoded)
        .map_err(|error| CliError::failure(format!("could not write Rust policy request: {error}")))?;
    let mut command = Command::new(&policy.native_output);
    command.arg(&request_path).arg(&response_path);
    let output = run_bounded_process(
        &mut command,
        BoundedProcessLimits {
            stdout_bytes: 1024 * 1024,
            stderr_bytes: 1024 * 1024,
            timeout: Some(Duration::from_secs(60)),
        },
        None,
    )
    .map_err(|error| CliError::failure(format!("could not execute admitted Rust policy engine: {error}")))?;
    if output.termination != BoundedProcessTermination::Completed || !output.status.success() {
        return Err(CliError::failure(format!(
            "admitted Rust policy engine refused or failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let metadata = fs::symlink_metadata(&response_path)
        .map_err(|error| CliError::failure(format!("Rust policy engine wrote no response: {error}")))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > POLICY_EXCHANGE_MAX_BYTES {
        return Err(CliError::failure(
            "Rust policy response is not a bounded regular file".to_string(),
        ));
    }
    let file = fs::File::from(
        rustix::fs::open(
            &response_path,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW,
            rustix::fs::Mode::empty(),
        )
        .map_err(|error| CliError::failure(format!("could not open Rust policy response: {error}")))?,
    );
    if !file
        .metadata()
        .map_err(|error| CliError::failure(format!("could not inspect Rust policy response: {error}")))?
        .is_file()
    {
        return Err(CliError::failure(
            "Rust policy response changed physical type".to_string(),
        ));
    }
    let mut response = Vec::new();
    file.take(POLICY_EXCHANGE_MAX_BYTES + 1)
        .read_to_end(&mut response)
        .map_err(|error| CliError::failure(format!("could not read Rust policy response: {error}")))?;
    if response.len() as u64 > POLICY_EXCHANGE_MAX_BYTES {
        return Err(CliError::failure(
            "Rust policy response exceeds its bounded allowance".to_string(),
        ));
    }
    serde_json::from_slice(&response)
        .map_err(|error| CliError::failure(format!("Rust policy response is invalid JSON: {error}")))
}

/// Accept the optional publisher input only as one complete release-only pair.
pub(crate) fn release_policy_publisher_input<'a>(
    envelope: OvenLoafEnvelope,
    store: Option<&'a Path>,
    identity: Option<&'a str>,
    target: Option<&'a str>,
) -> CliResult<Option<(&'a Path, &'a str, &'a str)>> {
    match (store, identity, target) {
        (None, None, None) => Ok(None),
        (Some(store), Some(identity), Some(target))
            if envelope == OvenLoafEnvelope::Release && !target.trim().is_empty() =>
        {
            Ok(Some((store, identity, target)))
        }
        (Some(_), Some(_), Some(_)) => Err(CliError::failure(
            "policy-engine inputs are accepted only for the release envelope",
        )),
        _ => Err(CliError::failure(
            "--policy-engine-store, --policy-engine-identity, and --policy-engine-target must be supplied together",
        )),
    }
}

/// Re-prove the committed physical member after publication authority has been released.
pub(crate) fn verify_committed_release_policy_output(
    output: &Path,
    expected: Option<&OvenReleaseStoreMember>,
    expected_target: Option<&str>,
) -> CliResult<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let held = oven_rustc::loaf::acquire_committed_release_store_member(output, &expected.label)
        .map_err(oven_error)?
        .ok_or_else(|| CliError::failure("committed release policy store member is missing"))?;
    if held.payload.manifest.identity != expected.artifact_identity {
        return Err(CliError::failure(
            "committed release policy store member has the wrong artifact identity",
        ));
    }
    let receipt = held
        .payload
        .original_native_receipt()
        .ok_or_else(|| CliError::failure("committed release policy ProjectOutput has no publisher receipt"))?;
    let payload: OvenProjectOutputPayload = serde_json::from_slice(&held.payload.payload).map_err(|error| {
        CliError::failure(format!(
            "committed release policy ProjectOutput payload is invalid: {error}"
        ))
    })?;
    let expected_target = expected_target
        .ok_or_else(|| CliError::failure("committed release policy member has no expected Rust target authority"))?;
    validate_release_policy_project_output(&held.payload.manifest, &payload, receipt, expected_target)?;
    Ok(())
}

/// Apply the Incan-owned semantic contract to either a source or committed generic store entry.
pub(crate) fn validate_release_policy_project_output(
    manifest: &oven_store::store::OvenArtifactManifest,
    payload: &OvenProjectOutputPayload,
    receipt: &oven_store::OvenReceipt,
    expected_target: &str,
) -> CliResult<()> {
    let expected_project_identity =
        incan_driver::build::output_selection::baked_project_owner_identity_for_name("oven_local_intake");
    receipt
        .verify_identity()
        .map_err(|error| CliError::failure(format!("release policy publisher receipt is invalid: {error}")))?;
    if manifest.kind != OvenArtifactKind::ProjectOutput
        || manifest.intent != receipt.intent
        || manifest.intent.profile != "release"
        || receipt.intent.target != expected_target
        || payload.compiler_version != super::INCAN_VERSION
        || payload.project_target != "executable"
        || payload.target_identity != "executable:src/plan_json_main.incn"
        || payload.entrypoint_relative_path != "src/plan_json_main.incn"
        || payload.project_identity != expected_project_identity
        || payload.source_authority_digest.trim().is_empty()
        || payload.receipt_identity != receipt.identity
        || payload.receipt_identity != manifest.receipt_identity
        || payload.build_unit_identity != receipt.build_unit_identity
        || payload.build_unit_identity != manifest.build_unit_identity
    {
        return Err(CliError::failure(
            "release policy input is not the exact release core_engine ProjectOutput authority",
        ));
    }
    payload
        .backend_receipt
        .verify_identity()
        .map_err(|error| CliError::failure(format!("release policy backend receipt is invalid: {error}")))?;
    Ok(())
}

/// Cross from exclusive envelope publication into normal shared-lease consumption.
pub(crate) fn finish_loaf_bake_after_publication(
    publication_lock: oven_rustc::loaf::OvenLoafGenerationLock,
    options: &OvenLoafBakeCommandOptions,
    envelope: OvenLoafEnvelope,
    report: OvenLoafBakeReport,
    started: Instant,
) -> CliResult<OvenLoafBakeReport> {
    drop(publication_lock);
    finish_loaf_bake(options, envelope, report, started)
}

/// Complete the typed compiler-suite envelope with its source-plan/foundation store through the same baker.
pub(crate) fn finish_loaf_bake(
    options: &OvenLoafBakeCommandOptions,
    envelope: OvenLoafEnvelope,
    mut report: OvenLoafBakeReport,
    started: Instant,
) -> CliResult<OvenLoafBakeReport> {
    if envelope != OvenLoafEnvelope::CompilerSuite {
        report.elapsed_ms = started.elapsed().as_millis();
        return Ok(report);
    }
    let compiler_suite_preparation_started = Instant::now();
    // The longest single phase of a cold prewarm: it stages the third-party foundation, which is where the
    // transitional Cargo path still compiles the heavy dependency graph.
    announce_oven_progress("PREPARE", "compiler-suite standard-library family", None);
    let suite_store = compiler_suite_store_path(options)?;
    let default_limits = loaf_envelope_default_limits(envelope);
    let max_physical_bytes = options.max_physical_bytes.unwrap_or(default_limits.max_physical_bytes);
    let remaining_physical_bytes = max_physical_bytes
        .checked_sub(report.owned_physical_bytes)
        .filter(|remaining| *remaining > 0)
        .ok_or_else(|| {
            CliError::failure(format!(
                "Loaf envelope already uses {} bytes of its {max_physical_bytes}-byte combined allowance",
                report.owned_physical_bytes
            ))
        })?;
    let max_domain_physical_bytes = options
        .max_domain_physical_bytes
        .unwrap_or(default_limits.max_domain_physical_bytes)
        .min(remaining_physical_bytes);
    let max_domain_logical_bytes = options
        .max_domain_logical_bytes
        .unwrap_or(default_limits.max_domain_logical_bytes);
    let store_options = OvenStoreCommandOptions {
        root: Some(suite_store.clone()),
        max_physical_bytes: Some(remaining_physical_bytes),
        max_domain_physical_bytes: Some(max_domain_physical_bytes),
        max_domain_logical_bytes: Some(max_domain_logical_bytes),
    };
    let (receipt, receipt_path) =
        compiler_libtests_receipt(&options.compiler_root, &options.rustc, &[], Some(&options.output))?;
    write_receipt(&receipt, &receipt_path).map_err(oven_error)?;
    let store = open_store(&store_options)?;
    let prepare = prepare_compiler_test_suite(&OvenLegacyCargoPrepareRequest {
        compiler: incan_oven_facet::compiler_identity(),
        provider_hooks: incan_oven_facet::provider_hooks(),
        store: &store,
        receipt,
        generated_project: options.compiler_root.clone(),
        cargo: options.cargo.clone(),
        rustc: options.rustc.clone(),
        cc: Some(options.cc.clone()),
        cxx: Some(options.cxx.clone()),
        c_sysroot: Some(options.c_sysroot.clone()),
        sdk_inventory: Some(options.sdk_inventory.clone()),
        compiler_loaf_root: Some(options.output.clone()),
        domain: "compiler-suite".to_string(),
        publication_kind: OvenLegacyCargoPublicationKind::LibraryTests,
        source_evidence_key: oven_store::COMPILER_WORKSPACE_MANIFEST_EVIDENCE_KEY.to_string(),
        compile_environment: BTreeMap::new(),
        inspection_packages: Some(Vec::new()),
        direct_dependency_closure: OvenLegacyCargoDirectDependencyClosure::CheckedDeclared,
        provider_compilations: &[],
        compact_debug_info: false,
        retain_equivalence_artifacts: false,
        source_compiler_vocab_support: false,
        base_loaf: None,
    })
    .map_err(oven_error)?;
    let suite_reused = prepare.cargo_version == "not-run-existing-suite";
    let store_inspection = if suite_reused {
        store.inspect_for_exact_reuse()
    } else {
        store.inspect()
    }
    .map_err(oven_error)?;
    let loaf_owned_physical_bytes = report.owned_physical_bytes;
    report.logical_bytes = report.logical_bytes.saturating_add(store_inspection.logical_bytes);
    report.physical_bytes = report.physical_bytes.saturating_add(store_inspection.physical_bytes);
    report.owned_physical_bytes = report
        .owned_physical_bytes
        .saturating_add(store_inspection.physical_bytes);
    report.raw_disk_bytes = report
        .raw_disk_bytes
        .saturating_add(loaf_raw_disk_bytes(&suite_store).map_err(oven_error)?);
    report.reclaimable_physical_bytes = store_inspection.reclaimable_physical_bytes;
    report.active_lease_physical_bytes = store_inspection.active_lease_physical_bytes;
    report.transient_peak_physical_bytes = report
        .transient_peak_physical_bytes
        .max(loaf_owned_physical_bytes.saturating_add(prepare.transient_reservation_bytes));
    report.max_physical_bytes = max_physical_bytes;
    report.max_domain_physical_bytes = options
        .max_domain_physical_bytes
        .unwrap_or(default_limits.max_domain_physical_bytes);
    report.max_domain_logical_bytes = max_domain_logical_bytes;
    if !suite_reused {
        report.action = "prepared".to_string();
        report.cargo_process_started = true;
    }
    if report.owned_physical_bytes > max_physical_bytes {
        return Err(CliError::failure(format!(
            "combined Loaf and compiler-suite storage uses {} physical bytes, exceeding its {max_physical_bytes}-byte allowance",
            report.owned_physical_bytes
        )));
    }
    report.elapsed_ms = started.elapsed().as_millis();
    report.phase_timing.compiler_suite_preparation_elapsed_ms =
        compiler_suite_preparation_started.elapsed().as_millis();
    announce_oven_progress(
        "PREPARED",
        "compiler-suite standard-library family",
        Some(&elapsed_detail(compiler_suite_preparation_started)),
    );
    report.compiler_suite = Some(OvenCompilerSuiteBakeReport {
        receipt: receipt_path,
        prepare,
        store: store_inspection,
    });
    Ok(report)
}

/// Product-owned storage policy for each built-in Loaf envelope.
pub(crate) fn loaf_envelope_default_limits(envelope: OvenLoafEnvelope) -> OvenStoreLimits {
    match envelope {
        OvenLoafEnvelope::Release => OvenStoreLimits::new(
            DEFAULT_OVEN_MAX_PHYSICAL_BYTES,
            DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES,
            DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES,
        ),
        OvenLoafEnvelope::CompilerSuite => OvenStoreLimits::new(
            DEFAULT_OVEN_COMPILER_SUITE_MAX_PHYSICAL_BYTES,
            DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_PHYSICAL_BYTES,
            DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_LOGICAL_BYTES,
        ),
    }
}

/// The one-line text rendering of the third-party foundation stage, or `None` when an existing suite made the stage
/// unnecessary.
///
/// The JSON report carries the same facts under `compiler_suite.prepare.foundation` and `.timing`; this line is
/// what a developer reads after `make test-prewarm-oven-loafs` to see whether a compiler edit cost a Cargo build.
pub(crate) fn foundation_stage_line(prepare: &OvenLegacyCargoCompilerSuiteResult) -> Option<String> {
    let key = prepare.foundation.key.as_ref()?;
    let origin = match prepare.foundation.selection {
        OvenLegacyCargoFoundationSelection::ExistingSuite => return None,
        OvenLegacyCargoFoundationSelection::ReusedFromStore => "reused by key from the store",
        OvenLegacyCargoFoundationSelection::ReusedFromMirror => "reused by key from a mirror",
        OvenLegacyCargoFoundationSelection::Built => "built by Cargo",
    };
    Some(format!(
        "  Third-party foundation: {origin} ({} partition(s), key {}; Cargo {}, build {} ms, selection {} ms).",
        prepare.foundation.entries,
        key.as_str(),
        if prepare.foundation.cargo_process_started {
            "started"
        } else {
            "not started"
        },
        prepare.timing.foundation_build_elapsed_ms,
        prepare.timing.foundation_selection_elapsed_ms,
    ))
}

/// Resolve the compiler-suite store owned by one typed envelope and reject overlapping policy roots.
pub(crate) fn compiler_suite_store_path(options: &OvenLoafBakeCommandOptions) -> CliResult<PathBuf> {
    let suite_store = match &options.suite_store {
        Some(path) => path.clone(),
        None => options
            .output
            .parent()
            .ok_or_else(|| CliError::failure("Loaf output has no parent for its compiler-suite store".to_string()))?
            .join("compiler-suite-store"),
    };
    if suite_store.starts_with(&options.output) || options.output.starts_with(&suite_store) {
        return Err(CliError::failure(
            "compiler-suite store and Loaf output must be separate non-nested bounded roots".to_string(),
        ));
    }
    Ok(suite_store)
}

/// Harvest incan.pub proposals from one release entry's capture into `harvest_dir`.
///
/// TODO(#1561): temporary. This is the release bake's half of the Cargo-observed harvest: it proposes records from
/// the compatibility publisher's capture of the release family, and retires with that capture once incan.pub records
/// govern the corpus and the bake settles from records alone. The registry pin beside it (`--loaf-registry-commit`,
/// `LoafRegistryAuthority`) is not temporary.
///
/// The evidence names the receipt the publisher ran under, the compiler closure identity the generation will
/// retain as its Toolchain owner, and the hazard tokens this process carries; the note names the compiler checkout
/// the release was baked from. The OUT_DIR members are verified and copied out of the provisional Loaf under
/// `staged_root`.
#[allow(
    clippy::too_many_arguments,
    reason = "one harvest names its capture, receipt, profile, compiler, checkout and staging explicitly"
)]
fn harvest_release_entry(
    harvest_dir: &Path,
    prepared: &oven_cargo_compat::loaf_bake::OvenPreparedLoafWithSelectedUnits,
    receipt: &OvenReceipt,
    profile: &str,
    compiler_closure: &str,
    compiler_root: &Path,
    staged_root: &Path,
    subject: &str,
) -> CliResult<OvenLoafHarvestReport> {
    let capture = prepared.selected_units.as_ref().ok_or_else(|| {
        CliError::failure(format!(
            "release entry {subject} retained no selected-unit capture to harvest"
        ))
    })?;
    let notes = checkout_head_commit(compiler_root)
        .ok()
        .map(|head| harvest_notes_for_checkout(&head));
    let evidence = HarvestEvidenceInputs::from_loaf_publisher(
        &prepared.publisher,
        HarvestPublisherIdentity::new(&receipt.identity, compiler_closure, ambient_harvest_hazards(), notes),
    );
    let loaf_name = prepared
        .preparation
        .loaf_identity
        .strip_prefix("sha256:")
        .unwrap_or(&prepared.preparation.loaf_identity);
    let retained_root = staged_root.join(format!("{loaf_name}.loaf"));
    let report =
        harvest_registry_units_to_dir(capture, &evidence, profile, harvest_dir, &retained_root).map_err(oven_error)?;
    let proposals = proposal_directory_names(&report).map_err(oven_error)?;
    announce_oven_progress(
        "HARVESTED",
        subject,
        Some(&format!(
            "{} proposal(s), {} refusal(s) into {}",
            proposals.len(),
            report.refusals.len(),
            harvest_dir.display()
        )),
    );
    Ok(OvenLoafHarvestReport {
        output: harvest_dir.to_path_buf(),
        proposals,
        refused: report.refusals.len(),
        hazards: report.hazards,
    })
}

/// Render one complete baker result without making Make or CI reconstruct product accounting.
pub(crate) fn print_loaf_bake_report(report: &OvenLoafBakeReport, format: OvenOutputFormat) -> CliResult<()> {
    match format {
        OvenOutputFormat::Text => {
            println!(
                "{} complete standard-library Loaf family ({} profile variants; {} logical, {} physical; Cargo baker {}).",
                if report.action == "reused" {
                    "Reused"
                } else {
                    "Prepared"
                },
                report.loaf_count,
                human_bytes(report.logical_bytes),
                human_bytes(report.physical_bytes),
                if report.cargo_process_started {
                    "used"
                } else {
                    "not used"
                },
            );
            if let Some(harvest) = &report.harvest {
                println!(
                    "Harvested {} incan.pub proposal(s) and {} refusal(s) into {}.",
                    harvest.proposals.len(),
                    harvest.refused,
                    harvest.output.display()
                );
                if !harvest.hazards.is_empty() {
                    println!(
                        "Harvest hazards: {} (admission refuses every proposal of this harvest).",
                        harvest.hazards.join(", ")
                    );
                }
            }
            if let Some(suite) = &report.compiler_suite {
                println!(
                    "Compiler-suite standard-library family: {} ({} logical, {} physical).",
                    if suite.prepare.cargo_version == "not-run-existing-suite" {
                        "reused"
                    } else {
                        "prepared"
                    },
                    human_bytes(suite.store.logical_bytes),
                    human_bytes(suite.store.physical_bytes),
                );
                if let Some(line) = foundation_stage_line(&suite.prepare) {
                    println!("{line}");
                }
            }
        }
        OvenOutputFormat::Json => print_json(report)?,
    }
    Ok(())
}

#[cfg(test)]
mod publisher_tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Build one manifest unit shared by the Cargo and Oven sides of the publisher evidence proof.
    fn evidence_unit() -> UnitManifest {
        UnitManifest {
            binding: "fixture 1.0.0 Target release []".to_string(),
            package: "fixture".to_string(),
            version: "1.0.0".to_string(),
            unit_identity: digest_bytes(b"unit"),
            features: Vec::new(),
            source_digest: digest_bytes(b"source"),
            manifest_digest: digest_bytes(b"manifest"),
            identity_inputs: IdentityInputs {
                cfg: Vec::new(),
                out: Vec::new(),
                native: Vec::new(),
                tools: Vec::new(),
                dependencies: Vec::new(),
            },
            native_objects: Vec::new(),
            artifacts: Vec::new(),
            asset_archive: None,
        }
    }

    /// Build the shared compiler setup required by both sides of one equivalence comparison.
    fn evidence_setup() -> CaptureSetup {
        CaptureSetup {
            cargo: ExecutableIdentity {
                version: "cargo fixture".to_string(),
                digest: digest_bytes(b"cargo"),
            },
            rustc: ExecutableIdentity {
                version: "rustc fixture".to_string(),
                digest: digest_bytes(b"rustc"),
            },
            host: "fixture-host".to_string(),
            target: "fixture-target".to_string(),
            profile: "release".to_string(),
        }
    }

    /// Copy one test evidence tree without preserving any absolute source coordinate.
    fn copy_evidence_tree(source: &Path, destination: &Path) -> TestResult {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            let source_path = entry.path();
            let destination_path = destination.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy_evidence_tree(&source_path, &destination_path)?;
            } else {
                fs::copy(source_path, destination_path)?;
            }
        }
        Ok(())
    }

    /// Compile one deterministic library artifact through the real retained-compiler command boundary.
    fn compile_evidence_artifact(rustc: &Path, source: &Path, output: &Path, remap_root: &Path) -> TestResult {
        fs::create_dir_all(output.parent().ok_or("compiled artifact has no parent")?)?;
        let remap = format!("{}=/fixture", remap_root.display());
        let result = Command::new(rustc)
            .args([
                "--crate-name",
                "equivalence_fixture",
                "--crate-type",
                "rlib",
                "--edition",
                "2024",
            ])
            .args(["-C", "metadata=equivalence-fixture", "-C", "embed-bitcode=no"])
            .arg("--remap-path-prefix")
            .arg(remap)
            .arg(source)
            .arg("-o")
            .arg(output)
            .output()?;
        if !result.status.success() {
            return Err(format!("fixture rustc failed: {}", String::from_utf8_lossy(&result.stderr)).into());
        }
        Ok(())
    }

    /// Resolve the pinned release compiler used by the runtime-foundation publisher tests.
    fn publisher_rebuild_release_rustc() -> Result<PathBuf, Box<dyn std::error::Error>> {
        let output = Command::new("rustup")
            .args(["which", "--toolchain", "1.98.0", "rustc"])
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "rustup could not locate the pinned Rust 1.98.0 compiler: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let path = PathBuf::from(String::from_utf8(output.stdout)?.trim());
        if !path.is_file() {
            return Err(format!("pinned release compiler is not a file: {}", path.display()).into());
        }
        Ok(path)
    }

    /// A produced release envelope binds the reissued compiled Loaf to the runtime foundation's exact manifest.
    #[test]
    fn publisher_rebuilt_foundation_reissues_and_validates_its_compiled_loaf() -> TestResult {
        let root = tempfile::tempdir()?;
        let mut loaf_root = root.path().join("staged/original.loaf");
        fs::create_dir_all(&loaf_root)?;
        let original_plan: OvenRustcArtifactManifest = serde_json::from_value(serde_json::json!({
            "schema_version": oven_rustc::rustc::OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            "intent": {
                "target": "fixture-target",
                "toolchain": "fixture-toolchain",
                "profile": "release",
                "features": []
            }
        }))?;
        let mut rebuilt_plan = original_plan.clone();
        rebuilt_plan
            .compile_environment
            .insert("FIXTURE_REBUILT".to_string(), "1".to_string());
        let mut loaf = OvenLoaf {
            schema_version: oven_rustc::loaf::OVEN_LOAF_SCHEMA_VERSION,
            build_unit_identity: digest_bytes(b"compiled-unit"),
            provenance: Default::default(),
            accounting: Default::default(),
            compatibility: Default::default(),
            registry_leaves: Vec::new(),
            plan: original_plan,
        };
        fs::write(loaf_root.join("loaf.json"), serde_json::to_vec_pretty(&loaf)?)?;

        let prepared = bind_staged_loaf_to_runtime_foundation(&mut loaf_root, &mut loaf, &rebuilt_plan)?;
        let rebound: OvenLoaf = serde_json::from_slice(&fs::read(loaf_root.join("loaf.json"))?)?;
        assert_eq!(rebound.plan, rebuilt_plan);
        assert_eq!(
            prepared.plan_identity,
            digest_bytes(&serde_json::to_vec(&rebuilt_plan)?)
        );
        assert_eq!(
            prepared.loaf_identity,
            digest_bytes(&serde_json::to_vec_pretty(&rebound)?)
        );
        let reused = bind_staged_loaf_to_runtime_foundation(&mut loaf_root, &mut loaf, &rebuilt_plan)?;
        assert_eq!(reused, prepared);

        let rustc_digest = digest_bytes(b"rustc");
        let toolchain_members = vec![OvenReleaseToolchainMember {
            relative_path: PathBuf::from("bin/rustc"),
            digest: rustc_digest.clone(),
        }];
        let compiler_closure_identity =
            oven_rustc::loaf::release_toolchain_compiler_closure_identity(&toolchain_members)?;
        let foundation_member = OvenReleaseRuntimeFoundationMember {
            schema_version: OVEN_RELEASE_RUNTIME_FOUNDATION_MEMBER_SCHEMA_VERSION,
            label: "rust-policy-foundation".to_string(),
            foundation_relative_path: PathBuf::from("runtime-foundations/foundation"),
            foundation_identity: digest_bytes(b"foundation"),
            compiled_loaf_identity: prepared.loaf_identity.clone(),
            compiled_plan_identity: prepared.plan_identity.clone(),
            toolchain_owner_identity: digest_bytes(b"toolchain-owner"),
            compiler_closure_identity,
            toolchain_root_relative_path: PathBuf::from("runtime-foundations/toolchain"),
            toolchain_members,
        };
        let mut evidence = BTreeMap::new();
        oven_rustc::loaf::bind_release_runtime_foundation_evidence(&mut evidence, &foundation_member)?;
        let manifest = OvenLoafEnvelopeManifest {
            schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
            envelope: "release".to_string(),
            generation_identity: digest_bytes(b"generation"),
            evidence,
            loafs: vec![OvenLoafEnvelopeMember {
                label: "stdlib".to_string(),
                profile: "release".to_string(),
                action: "prepared".to_string(),
                role: oven_rustc::loaf::OvenLoafMemberRole::CompiledClosure,
                build_unit_identity: prepared.build_unit_identity,
                loaf_identity: prepared.loaf_identity,
                plan_identity: prepared.plan_identity,
                logical_bytes: prepared.logical_bytes,
                physical_bytes: prepared.physical_bytes,
                path: PathBuf::from("generations/fixture/compiled.loaf/loaf.json"),
            }],
            release_store_member: None,
            runtime_foundation: Some(foundation_member.clone()),
            runtime_closure: None,
        };
        oven_rustc::loaf::validate_release_runtime_foundation_member(&manifest, &foundation_member)?;
        Ok(())
    }

    #[test]
    fn publisher_owner_selection_requires_the_record_identity() -> TestResult {
        let matching = tempfile::tempdir()?;
        let substitute = tempfile::tempdir()?;
        fs::create_dir_all(matching.path().join("bin"))?;
        fs::create_dir_all(substitute.path().join("bin"))?;
        fs::write(matching.path().join("bin/clang"), b"matching compiler")?;
        fs::write(substitute.path().join("bin/clang"), b"substitute compiler")?;
        let expected = publisher_owner_identity(matching.path(), ["bin/clang"])?;
        let roots = vec![substitute.path().to_path_buf(), matching.path().to_path_buf()];

        let selected = resolve_publisher_owner_root("native-sys", "native", &expected, &["bin/clang"], &roots)?;
        assert_eq!(selected, matching.path());
        let missing = resolve_publisher_owner_root(
            "native-sys",
            "native",
            &expected,
            &["bin/clang"],
            &[substitute.path().to_path_buf()],
        )
        .err()
        .ok_or("a missing publisher owner was accepted")?;
        assert!(missing.to_string().contains("native-sys"));
        assert!(missing.to_string().contains("native"));
        assert!(missing.to_string().contains(&expected));
        Ok(())
    }

    #[test]
    fn publisher_config_accepts_repeatable_owner_roots() -> TestResult {
        let parsed = toml::from_str::<PublisherConfigFile>(
            "[publisher]\nlink-owner = [\"/toolchain/clang\", \"/tools/protoc\"]\n",
        )?;
        assert_eq!(
            parsed.publisher.link_owners,
            vec![PathBuf::from("/toolchain/clang"), PathBuf::from("/tools/protoc")]
        );
        Ok(())
    }

    #[test]
    fn publisher_asset_claim_names_binding_unit_builder_and_attestation() -> TestResult {
        let root = tempfile::tempdir()?;
        let owner = PublisherGeneratedOwnerRoot {
            identity: digest_bytes(b"owner"),
            root: root.path().join("owner"),
            binding: "native-sys 1.0 target release [] native".to_string(),
            unit_identity: digest_bytes(b"unit"),
            attestation_reference: digest_bytes(b"receipt"),
            capture_index: 0,
        };
        let archive = digest_bytes(b"archive");
        write_publisher_asset_claims(&[owner], &archive, &root.path().join("claims"))?;
        let claim: serde_json::Value = serde_json::from_slice(&fs::read(root.path().join("claims/0000.json"))?)?;
        assert_eq!(claim["event"], "asset");
        assert_eq!(claim["archive_digest"], archive);
        assert_eq!(claim["builder_kind"], "local");
        assert_eq!(claim["unit_identity"], digest_bytes(b"unit"));
        assert_eq!(claim["attestation_reference"], digest_bytes(b"receipt"));
        Ok(())
    }

    #[test]
    fn publisher_native_evidence_follows_the_selected_archive_owner() -> TestResult {
        let root = tempfile::tempdir()?;
        let matching_root = root.path().join("generated-outputs/fixture");
        let unrelated_root = root.path().join("generated-outputs/unrelated");
        fs::create_dir_all(&matching_root)?;
        fs::create_dir_all(&unrelated_root)?;
        fs::write(matching_root.join("flag_check"), b"matching bytes")?;
        fs::write(unrelated_root.join("flag_check"), b"unrelated bytes")?;
        let linked = [OvenSelectedRustFacetLinkedLibrary::Archive {
            name: "fixture".to_string(),
            kind: oven_rustc::rustc::OvenSelectedRustFacetLinkedLibraryKind::Static,
            artifact: oven_rustc::rustc::OvenSelectedRustFacetPath {
                owner: digest_bytes(b"selected owner"),
                path: "generated-outputs/fixture/libfixture.a".to_string(),
            },
            digest: digest_bytes(b"archive"),
        }];

        assert_eq!(
            publisher_native_objects_for_unit(root.path(), &linked, &[PathBuf::from("cargo/flag_check")],)?,
            vec![matching_root.join("flag_check")]
        );
        Ok(())
    }

    /// The release evidence stage consumes only digest-bound Loaf bytes and refuses absence, mutation, or ambiguity.
    #[test]
    fn publisher_retained_cargo_artifact_is_exact_and_digest_bound() -> TestResult {
        let root = tempfile::tempdir()?;
        let relative = "cargo-capture-artifacts/0000/0000";
        let retained = root.path().join(relative);
        fs::create_dir_all(retained.parent().ok_or("retained artifact has no parent")?)?;
        fs::write(&retained, b"artifact bytes")?;
        let mut unit: oven_cargo_compat::OvenLegacyCargoSelectedUnit = serde_json::from_value(serde_json::json!({
            "package_id": "fixture 1.0.0", "package": "fixture", "package_version": "1.0.0",
            "package_source": null, "target_name": "fixture", "target_kinds": ["lib"], "crate_types": ["lib"],
            "source_path": "/transient/src/lib.rs", "retained_artifacts": [{
                "relative_path": relative, "extension": "rlib", "digest": digest_bytes(b"artifact bytes")
            }],
            "root_module": "src/lib.rs", "edition": "2024", "mode": "build", "platform": "fixture-target",
            "target_is_explicit": true, "cfg": [], "compiler_arguments": [], "compile_environment": {},
            "effective_features": [], "dependencies": [], "sysroot_externs": [], "build_script": null,
            "registry_source": null
        }))?;
        assert_eq!(retained_cargo_artifact(&unit, Some("rlib"), root.path())?, retained);

        fs::write(&retained, b"changed")?;
        assert!(retained_cargo_artifact(&unit, Some("rlib"), root.path()).is_err());
        fs::write(&retained, b"artifact bytes")?;
        unit.retained_artifacts.push(unit.retained_artifacts[0].clone());
        assert!(retained_cargo_artifact(&unit, Some("rlib"), root.path()).is_err());
        unit.retained_artifacts.pop();
        fs::remove_file(&retained)?;
        assert!(retained_cargo_artifact(&unit, Some("rlib"), root.path()).is_err());
        Ok(())
    }

    /// Production capture writing and equivalence validation remain complete after the evidence tree is copied.
    #[test]
    fn publisher_rebuild_evidence_validates_clean_and_relocated_manifests() -> TestResult {
        let root = tempfile::tempdir()?;
        let source = root.path().join("source");
        fs::create_dir(&source)?;
        let source_file = source.join("lib.rs");
        fs::write(&source_file, b"pub fn evidence_fixture() -> u32 { 1561 }\n")?;
        let rustc = publisher_rebuild_release_rustc()?;
        let cargo_artifact = root.path().join("cargo/libfixture.rlib");
        let oven_artifact = root.path().join("oven/libfixture.rlib");
        compile_evidence_artifact(&rustc, &source_file, &cargo_artifact, root.path())?;
        compile_evidence_artifact(&rustc, &source_file, &oven_artifact, root.path())?;
        let archive = source.join("foundation.loaf");
        fs::write(&archive, b"foundation witness")?;
        let evidence = root.path().join("equivalence");
        let cargo_manifest = evidence.join("cargo-capture.json");
        let oven_manifest = evidence.join("oven-capture.json");
        write_capture_manifest(
            &cargo_manifest,
            CaptureManifestInput {
                producer: "cargo-harvest",
                cargo_free: false,
                setup: evidence_setup(),
                units: vec![(
                    evidence_unit(),
                    vec![CaptureFileInput {
                        role: "rlib".to_string(),
                        path: "units/0000/artifact.rlib".to_string(),
                        source: cargo_artifact,
                    }],
                    Vec::new(),
                    None,
                )],
            },
        )?;
        write_capture_manifest(
            &oven_manifest,
            CaptureManifestInput {
                producer: "oven-publisher",
                cargo_free: true,
                setup: evidence_setup(),
                units: vec![(
                    evidence_unit(),
                    vec![CaptureFileInput {
                        role: "rlib".to_string(),
                        path: "units/0000/artifact.rlib".to_string(),
                        source: oven_artifact,
                    }],
                    Vec::new(),
                    Some(archive),
                )],
            },
        )?;
        super::super::equivalence::oven_equivalence(
            &cargo_manifest,
            &oven_manifest,
            &evidence.join("attestation.json"),
        )?;

        let relocated = root.path().join("relocated");
        copy_evidence_tree(&evidence, &relocated)?;
        super::super::equivalence::oven_equivalence(
            &relocated.join("cargo-capture.json"),
            &relocated.join("oven-capture.json"),
            &relocated.join("relocated-attestation.json"),
        )?;
        Ok(())
    }
}
