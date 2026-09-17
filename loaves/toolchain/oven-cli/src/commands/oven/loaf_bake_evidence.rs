//! The evidence a Loaf envelope bake is keyed on, and the reports it publishes.
//!
//! A generation is identified by its envelope name and the evidence map — fixture, lock, runtime-source, rustc and SDK
//! inventory digests — so a committed generation can be reused, imported from a read-only mirror, or rebuilt. The
//! report types here are what `incan oven legacy-cargo bake-loafs` prints; the bake command that uses them is
//! `loaf_bake`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use super::{
    CliError, CliResult, INCAN_VERSION, Instant, LoafEnvelopeExpectation, LoafMemberExpectation, LoafMirrorMiss,
    OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION, OvenLegacyCargoCompilerSuiteResult, OvenLegacyCargoSelectedUnitCapture,
    OvenLoafEnvelope, OvenLoafEnvelopeManifest, OvenLoafFixtureAction, OvenLoafMemberRole, OvenLoafPreparation,
    OvenReleaseRuntimeFoundationMember, OvenReleaseStoreMember, OvenStore, OvenStoreInspection, OvenStoreLimits,
    announce_oven_progress, bind_release_runtime_foundation_evidence, configured_mirrors, digest_bytes,
    digest_runtime_crate_source, elapsed_detail, env, import_loaf_envelope_from_mirrors, loaf_directory_byte_counts,
    loaf_envelope_inspection_packages, loaf_envelope_specifications, loaf_raw_disk_bytes, oven_error,
    retire_unreferenced_loaf_generations, rustc_identity, validate_stored_loaf_for_reuse,
};

/// Result for one checked fixture in a built-in Loaf envelope.
#[derive(Debug, Serialize)]
pub(crate) struct OvenLoafBakeEntryReport {
    pub(crate) label: String,
    pub(crate) profile: String,
    pub(crate) action: String,
    pub(crate) role: OvenLoafMemberRole,
    pub(crate) result: OvenLoafPreparation,
    #[serde(skip)]
    pub(crate) selected_units: Option<OvenLegacyCargoSelectedUnitCapture>,
}

/// Complete result from the hidden, explicit `legacy_cargo` Loaf baker.
#[derive(Debug, Serialize)]
pub(crate) struct OvenLoafBakeReport {
    pub(crate) action: String,
    pub(crate) envelope: String,
    pub(crate) loaf_count: usize,
    pub(crate) prepared_count: usize,
    pub(crate) reused_count: usize,
    pub(crate) logical_bytes: u64,
    pub(crate) physical_bytes: u64,
    pub(crate) owned_physical_bytes: u64,
    pub(crate) raw_disk_bytes: u64,
    pub(crate) reclaimable_physical_bytes: u64,
    pub(crate) active_lease_physical_bytes: u64,
    pub(crate) transient_peak_physical_bytes: u64,
    pub(crate) max_physical_bytes: u64,
    pub(crate) max_domain_physical_bytes: u64,
    pub(crate) max_domain_logical_bytes: u64,
    pub(crate) elapsed_ms: u128,
    /// Cold-baker phase ledger. These phases are measured by Oven itself so CI never has to infer work from shell
    /// command boundaries or Cargo's human output.
    pub(crate) phase_timing: OvenLoafBakePhaseTiming,
    pub(crate) cargo_process_started: bool,
    pub(crate) evidence: OvenLoafEnvelopeEvidence,
    pub(crate) loafs: Vec<OvenLoafBakeEntryReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) compiler_suite: Option<OvenCompilerSuiteBakeReport>,
}

/// Product-owned elapsed-time attribution for one Loaf-baker invocation.
#[derive(Debug, Default, Serialize)]
pub(crate) struct OvenLoafBakePhaseTiming {
    /// Input validation, compatibility evidence, and exact-envelope reuse inspection.
    pub(crate) preflight_elapsed_ms: u128,
    /// Locked registry/inspection authority preparation for a cold envelope.
    pub(crate) inspection_authority_elapsed_ms: u128,
    /// Checked fixture receipt generation and direct-Rustc Loaf preparation.
    pub(crate) fixture_preparation_elapsed_ms: u128,
    /// Atomic generation publication, retirement, and owned-byte accounting.
    pub(crate) envelope_publication_elapsed_ms: u128,
    /// Receipt-bound compiler-suite index/foundation preparation after the envelope is available.
    pub(crate) compiler_suite_preparation_elapsed_ms: u128,
}

/// Source-plan and bounded-store evidence baked with the compiler-suite Loaf envelope.
#[derive(Debug, Serialize)]
pub(crate) struct OvenCompilerSuiteBakeReport {
    pub(crate) receipt: PathBuf,
    pub(crate) prepare: OvenLegacyCargoCompilerSuiteResult,
    pub(crate) store: OvenStoreInspection,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
pub(crate) struct OvenLoafEnvelopeEvidence {
    pub(crate) incan_release_version: String,
    /// Report-only provenance for the executable that performed this baker invocation.
    ///
    /// This must not participate in release-family compatibility: rebuilding the same Incan release locally must
    /// not cause every complete standard-library Loaf to be republished.
    pub(crate) compiler_executable_digest: String,
    pub(crate) sdk_inventory_digest: String,
    pub(crate) rustc_identity: String,
    pub(crate) lock_digest: String,
    /// Complete source evidence for the runtime crates compiled into every standard-library Loaf.
    pub(crate) runtime_source_digest: String,
    pub(crate) fixture_digest: String,
}

/// Return the stable wire name used by one built-in Loaf envelope.
pub(crate) fn loaf_envelope_name(envelope: OvenLoafEnvelope) -> &'static str {
    match envelope {
        OvenLoafEnvelope::Release => "release",
        OvenLoafEnvelope::CompilerSuite => "compiler-suite",
    }
}

/// Return the release-family compatibility evidence committed into `envelope.json`.
///
/// The family is selected by the Incan release plus immutable SDK, Rust toolchain, lock, and checked-fixture
/// contracts. The baker executable digest remains report provenance only, so a development rebuild of the same
/// release does not invalidate a complete standard-library Loaf family.
pub(crate) fn loaf_envelope_compatibility_map(evidence: &OvenLoafEnvelopeEvidence) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "incan_release_version".to_string(),
            evidence.incan_release_version.clone(),
        ),
        (
            "sdk_inventory_digest".to_string(),
            evidence.sdk_inventory_digest.clone(),
        ),
        ("rustc_identity".to_string(), evidence.rustc_identity.clone()),
        ("lock_digest".to_string(), evidence.lock_digest.clone()),
        (
            "runtime_source_digest".to_string(),
            evidence.runtime_source_digest.clone(),
        ),
        ("fixture_digest".to_string(), evidence.fixture_digest.clone()),
    ])
}

/// Add the exact optional release asset to the evidence map that mirrors and reuse compare.
pub(crate) fn loaf_envelope_compatibility_map_with_release_member(
    evidence: &OvenLoafEnvelopeEvidence,
    release_store_member: Option<&OvenReleaseStoreMember>,
) -> CliResult<BTreeMap<String, String>> {
    let mut compatibility = loaf_envelope_compatibility_map(evidence);
    if let Some(member) = release_store_member {
        compatibility.insert(
            "release_store_member_artifact_identity".to_string(),
            member.artifact_identity.clone(),
        );
        compatibility.insert(
            "release_store_member_descriptor_digest".to_string(),
            digest_bytes(&serde_json::to_vec(member).map_err(|error| {
                CliError::failure(format!("could not encode release store member evidence: {error}"))
            })?),
        );
    }
    Ok(compatibility)
}

/// Return a generation identity that also binds any exact generic release-store member descriptor.
///
/// Release packaging must call this form when it publishes a member; otherwise a swapped descriptor could retain
/// the identity of a generation computed only from compatibility evidence.
pub(crate) fn loaf_generation_identity_with_release_member(
    envelope: OvenLoafEnvelope,
    evidence: &BTreeMap<String, String>,
    release_store_member: Option<&OvenReleaseStoreMember>,
) -> CliResult<String> {
    let encoded = match release_store_member {
        Some(member) => serde_json::to_vec(&(loaf_envelope_name(envelope), evidence, member)),
        None => serde_json::to_vec(&(loaf_envelope_name(envelope), evidence)),
    };
    Ok(digest_bytes(&encoded.map_err(|error| {
        CliError::failure(format!("could not encode Loaf generation identity: {error}"))
    })?))
}

/// Measure the one exact embedded generic store entry included in envelope payload totals.
pub(crate) fn release_store_member_byte_counts(
    generation_root: &Path,
    member: Option<&OvenReleaseStoreMember>,
    limits: OvenStoreLimits,
) -> CliResult<(u64, u64)> {
    let Some(member) = member else {
        return Ok((0, 0));
    };
    let inspection = OvenStore::new(generation_root.join(&member.store_relative_path), limits)
        .inspect_for_exact_reuse()
        .map_err(oven_error)?;
    if inspection.entries.len() != 1 || inspection.entries[0].manifest.identity != member.artifact_identity {
        return Err(CliError::failure(
            "release policy store accounting did not find its one exact artifact",
        ));
    }
    Ok((inspection.logical_bytes, inspection.physical_bytes))
}

/// Return the wire spelling of one checked fixture action.
pub(crate) fn loaf_fixture_action_name(action: OvenLoafFixtureAction) -> &'static str {
    match action {
        OvenLoafFixtureAction::Build => "build",
        OvenLoafFixtureAction::Run => "run",
    }
}

/// Commit the expected generation from a configured mirror when the local root has none.
///
/// Runs under the exclusive publication lock, right before ordinary reuse, and only when `output/envelope.json` is
/// absent: a root that already holds a generation is either reused or rebaked by the existing rules, never replaced
/// from a mirror. A miss is silent; a corrupt mirror is reported as a note and otherwise ignored.
pub(crate) fn import_loaf_envelope_from_configured_mirrors(
    output: &Path,
    scratch: &Path,
    envelope: OvenLoafEnvelope,
    evidence: &OvenLoafEnvelopeEvidence,
    release_store_member: Option<&OvenReleaseStoreMember>,
    runtime_foundation: Option<&OvenReleaseRuntimeFoundationMember>,
) -> CliResult<()> {
    if output.join("envelope.json").is_file() {
        return Ok(());
    }
    let mirrors = configured_mirrors(|name| env::var_os(name));
    if mirrors.is_empty() {
        return Ok(());
    }
    import_loaf_envelope_from_mirror_roots(
        output,
        scratch,
        envelope,
        evidence,
        release_store_member,
        runtime_foundation,
        &mirrors,
    )
}

/// Commit the expected generation from the first of `mirrors` that proves in full; see the configured wrapper.
pub(crate) fn import_loaf_envelope_from_mirror_roots(
    output: &Path,
    scratch: &Path,
    envelope: OvenLoafEnvelope,
    evidence: &OvenLoafEnvelopeEvidence,
    release_store_member: Option<&OvenReleaseStoreMember>,
    runtime_foundation: Option<&OvenReleaseRuntimeFoundationMember>,
    mirrors: &[PathBuf],
) -> CliResult<()> {
    let mut compatibility_evidence =
        loaf_envelope_compatibility_map_with_release_member(evidence, release_store_member)?;
    if let Some(member) = runtime_foundation {
        bind_release_runtime_foundation_evidence(&mut compatibility_evidence, member).map_err(oven_error)?;
    }
    let generation_identity =
        loaf_generation_identity_with_release_member(envelope, &compatibility_evidence, release_store_member)?;
    let members = loaf_envelope_specifications(envelope)
        .iter()
        .map(|specification| LoafMemberExpectation {
            label: specification.label.to_string(),
            profile: specification.profile.to_string(),
            action: loaf_fixture_action_name(specification.action).to_string(),
            role: specification.role,
        })
        .collect::<Vec<_>>();
    let expectation = LoafEnvelopeExpectation {
        schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
        envelope: loaf_envelope_name(envelope),
        generation_identity: &generation_identity,
        evidence: &compatibility_evidence,
        members: &members,
        release_store_member,
        runtime_foundation,
    };
    let started = Instant::now();
    match import_loaf_envelope_from_mirrors(output, scratch, &expectation, mirrors) {
        Ok(admitted) => announce_oven_progress(
            "MIRROR",
            &format!(
                "{} Loaf envelope: {} member(s) proven from {}",
                loaf_envelope_name(envelope),
                admitted.member_count,
                admitted.mirror.display()
            ),
            Some(&elapsed_detail(started)),
        ),
        Err(LoafMirrorMiss::NoCompatibleEnvelope) => {}
        Err(miss @ LoafMirrorMiss::Unreadable { .. }) => eprintln!("note: {miss}"),
    }
    Ok(())
}

/// Gather release-family compatibility evidence and baker provenance for a built-in envelope.
pub(crate) fn loaf_envelope_evidence(
    envelope: OvenLoafEnvelope,
    compiler_root: &Path,
    compiler_executable: &Path,
    sdk_inventory: &Path,
    rustc: &Path,
) -> CliResult<OvenLoafEnvelopeEvidence> {
    let read_digest = |path: &Path, label: &str| -> CliResult<String> {
        let bytes = fs::read(path)
            .map_err(|error| CliError::failure(format!("could not read Loaf {label} {}: {error}", path.display())))?;
        Ok(digest_bytes(&bytes))
    };
    let lock_path = loaf_compiler_lock_path(compiler_root)?;
    let fixture_evidence = loaf_envelope_specifications(envelope)
        .iter()
        .map(|specification| {
            serde_json::json!({
                "label": specification.label,
                "project_name": specification.project_name,
                "profile": specification.profile,
                "action": loaf_fixture_action_name(specification.action),
                "source": specification.source,
                "manifest": specification.manifest,
                "inspection_manifest": specification.inspection_manifest,
                "role": specification.role,
                "retain_complete_registry_leaves": specification.retain_complete_registry_leaves,
                "retain_checked_direct_dependencies": specification.retain_checked_direct_dependencies,
            })
        })
        .collect::<Vec<_>>();
    let inspection_packages = loaf_envelope_inspection_packages(envelope).map_err(CliError::failure)?;
    Ok(OvenLoafEnvelopeEvidence {
        incan_release_version: INCAN_VERSION.to_string(),
        compiler_executable_digest: read_digest(compiler_executable, "compiler executable")?,
        sdk_inventory_digest: read_digest(sdk_inventory, "SDK inventory")?,
        rustc_identity: rustc_identity(rustc).map_err(oven_error)?,
        lock_digest: read_digest(&lock_path, "lock input")?,
        runtime_source_digest: loaf_runtime_source_digest(compiler_root)?,
        fixture_digest: digest_bytes(
            &serde_json::to_vec(&(fixture_evidence, inspection_packages))
                .map_err(|error| CliError::failure(format!("could not encode Loaf fixture evidence: {error}")))?,
        ),
    })
}

/// Hash the complete compiler-runtime source closure that the Loaf fixture links into its sealed artifacts.
///
/// The executable digest is provenance only: rebuilding the same release binary must not invalidate an otherwise
/// compatible envelope. The runtime crates are different: changing their source without selecting a new envelope
/// could pair an updated compiler with stale facet or support-crate archives. Keep this evidence portable by
/// recording named content digests rather than checkout paths.
pub(crate) fn loaf_runtime_source_digest(compiler_root: &Path) -> CliResult<String> {
    let mut records = BTreeMap::new();
    let manifest = loaf_compiler_manifest_path(compiler_root)?;
    let manifest_bytes = fs::read(&manifest).map_err(|error| {
        CliError::failure(format!(
            "could not read Loaf runtime manifest {}: {error}",
            manifest.display()
        ))
    })?;
    records.insert("Cargo.toml".to_string(), digest_bytes(&manifest_bytes));
    for label in oven_model::toolchain_layout::SDK_RUNTIME_CRATES {
        let root = oven_model::toolchain_layout::support_crate_dir_in(compiler_root, label);
        let digest = digest_runtime_crate_source(&root).map_err(CliError::failure)?;
        records.insert(label.to_string(), digest);
    }
    let bytes = serde_json::to_vec(&records)
        .map_err(|error| CliError::failure(format!("could not encode Loaf runtime source evidence: {error}")))?;
    Ok(digest_bytes(&bytes))
}

/// Return the checked Cargo workspace manifest for a compiler checkout or packaged toolchain.
///
/// Source checkouts own the complete workspace at the compiler root. Release archives intentionally ship only the
/// runtime support workspace under `crates/`; the Loaf baker must bind to that staged workspace instead of assuming
/// the archive contains the compiler's development-only root manifest.
pub(crate) fn loaf_compiler_manifest_path(compiler_root: &Path) -> CliResult<PathBuf> {
    [
        compiler_root.join("Cargo.toml"),
        compiler_root.join("crates/Cargo.toml"),
    ]
    .into_iter()
    .find(|path| path.is_file())
    .ok_or_else(|| CliError::failure("Loaf compiler root has no canonical Cargo.toml input".to_string()))
}

/// Return the one checked compiler lock used by both envelope identity and cold fixture publication.
pub(crate) fn loaf_compiler_lock_path(compiler_root: &Path) -> CliResult<PathBuf> {
    [
        compiler_root.join("Cargo.lock"),
        compiler_root.join("crates/Cargo.lock"),
    ]
    .into_iter()
    .find(|path| path.is_file())
    .ok_or_else(|| CliError::failure("Loaf compiler root has no canonical Cargo.lock input".to_string()))
}

/// Inputs that bind exact committed-envelope reuse to its expected evidence and active store limits.
pub(crate) struct CompleteLoafEnvelopeReuseInput<'a> {
    pub(crate) output: &'a Path,
    pub(crate) scratch: &'a Path,
    pub(crate) envelope: OvenLoafEnvelope,
    pub(crate) evidence: &'a OvenLoafEnvelopeEvidence,
    pub(crate) release_store_member: Option<&'a OvenReleaseStoreMember>,
    pub(crate) runtime_foundation: Option<&'a OvenReleaseRuntimeFoundationMember>,
    pub(crate) limits: OvenStoreLimits,
    pub(crate) started: Instant,
}

/// Validate and reuse one exact committed envelope without fixture probes or a Cargo process.
pub(crate) fn reuse_complete_loaf_envelope(
    input: CompleteLoafEnvelopeReuseInput<'_>,
) -> CliResult<Option<OvenLoafBakeReport>> {
    let CompleteLoafEnvelopeReuseInput {
        output,
        scratch,
        envelope,
        evidence,
        release_store_member,
        runtime_foundation,
        limits,
        started,
    } = input;
    let manifest_path = output.join("envelope.json");
    if !manifest_path.is_file() {
        return Ok(None);
    }
    let manifest = serde_json::from_slice::<OvenLoafEnvelopeManifest>(&fs::read(&manifest_path).map_err(|error| {
        CliError::failure(format!(
            "could not read Loaf envelope manifest {}: {error}",
            manifest_path.display()
        ))
    })?)
    .map_err(|error| {
        CliError::failure(format!(
            "invalid Loaf envelope manifest {}: {error}",
            manifest_path.display()
        ))
    })?;
    let mut expected_evidence = loaf_envelope_compatibility_map_with_release_member(evidence, release_store_member)?;
    if let Some(member) = runtime_foundation {
        bind_release_runtime_foundation_evidence(&mut expected_evidence, member).map_err(oven_error)?;
    }
    if manifest.schema_version != OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION
        || manifest.envelope != loaf_envelope_name(envelope)
        || manifest.evidence != expected_evidence
        || manifest.release_store_member.as_ref() != release_store_member
        || manifest.runtime_foundation.as_ref() != runtime_foundation
    {
        return Ok(None);
    }
    let specifications = loaf_envelope_specifications(envelope);
    if manifest.loafs.len() != specifications.len() {
        return Err(CliError::failure("Loaf envelope manifest is incomplete".to_string()));
    }
    let mut reports = Vec::with_capacity(manifest.loafs.len());
    for (entry, specification) in manifest.loafs.iter().zip(specifications) {
        let expected_action = loaf_fixture_action_name(specification.action);
        if entry.label != specification.label
            || entry.profile != specification.profile
            || entry.action != expected_action
            || entry.role != specification.role
        {
            return Err(CliError::failure(
                "Loaf envelope manifest does not match its checked specification".to_string(),
            ));
        }
        let loaf_path = output.join(&entry.path);
        let result = validate_stored_loaf_for_reuse(&loaf_path, entry).map_err(oven_error)?;
        if result.logical_bytes > limits.max_domain_logical_bytes
            || result.physical_bytes > limits.max_domain_physical_bytes
        {
            return Err(CliError::failure(format!(
                "stored Loaf `{}` exceeds the active compatibility-domain allowance",
                entry.label
            )));
        }
        reports.push(OvenLoafBakeEntryReport {
            label: entry.label.clone(),
            profile: entry.profile.clone(),
            action: entry.action.clone(),
            role: entry.role,
            result,
            selected_units: None,
        });
    }
    let member_generation = output.join("generations").join(
        manifest
            .generation_identity
            .strip_prefix("sha256:")
            .unwrap_or(&manifest.generation_identity),
    );
    let (member_logical_bytes, member_physical_bytes) =
        release_store_member_byte_counts(&member_generation, release_store_member, limits)?;
    let (foundation_logical_bytes, foundation_physical_bytes) = if let Some(member) = runtime_foundation {
        oven_rustc::loaf::prove_release_runtime_foundation_member(output, &manifest, member).map_err(oven_error)?;
        let foundation = member_generation.join(&member.foundation_relative_path);
        let toolchain = member_generation.join(&member.toolchain_root_relative_path);
        let (foundation_logical, foundation_physical) = loaf_directory_byte_counts(&foundation).map_err(oven_error)?;
        let (toolchain_logical, toolchain_physical) = loaf_directory_byte_counts(&toolchain).map_err(oven_error)?;
        if foundation_logical > limits.max_domain_logical_bytes
            || foundation_physical > limits.max_domain_physical_bytes
            || toolchain_logical > limits.max_domain_logical_bytes
            || toolchain_physical > limits.max_domain_physical_bytes
        {
            return Err(CliError::failure(
                "stored runtime foundation exceeds the active compatibility-domain allowance".to_string(),
            ));
        }
        (
            foundation_logical.saturating_add(toolchain_logical),
            foundation_physical.saturating_add(toolchain_physical),
        )
    } else {
        (0, 0)
    };
    let logical_bytes = reports
        .iter()
        .map(|entry| entry.result.logical_bytes)
        .sum::<u64>()
        .saturating_add(member_logical_bytes)
        .saturating_add(foundation_logical_bytes);
    let physical_bytes = reports
        .iter()
        .map(|entry| entry.result.physical_bytes)
        .sum::<u64>()
        .saturating_add(member_physical_bytes)
        .saturating_add(foundation_physical_bytes);
    if physical_bytes > limits.max_physical_bytes {
        return Err(CliError::failure(format!(
            "stored Loaf envelope uses {physical_bytes} physical bytes, exceeding its {}-byte allowance",
            limits.max_physical_bytes
        )));
    }
    retire_unreferenced_loaf_generations(output, &manifest.generation_identity, scratch).map_err(oven_error)?;
    let (_, owned_physical_bytes) = loaf_directory_byte_counts(output).map_err(oven_error)?;
    let raw_disk_bytes = loaf_raw_disk_bytes(output).map_err(oven_error)?;
    if owned_physical_bytes > limits.max_physical_bytes {
        return Err(CliError::failure(format!(
            "stored Loaf output uses {owned_physical_bytes} physical bytes after reclaiming obsolete generations, exceeding its {}-byte allowance",
            limits.max_physical_bytes
        )));
    }
    Ok(Some(OvenLoafBakeReport {
        action: "reused".to_string(),
        envelope: loaf_envelope_name(envelope).to_string(),
        loaf_count: reports.len(),
        prepared_count: 0,
        reused_count: reports.len(),
        logical_bytes,
        physical_bytes,
        owned_physical_bytes,
        raw_disk_bytes,
        // Exact reuse holds the exclusive generation lock and has already reclaimed every unreferenced generation.
        // The remaining owned overhead is the active envelope manifest/lock, not reclaimable artifact data.
        reclaimable_physical_bytes: 0,
        active_lease_physical_bytes: 0,
        transient_peak_physical_bytes: 0,
        max_physical_bytes: limits.max_physical_bytes,
        max_domain_physical_bytes: limits.max_domain_physical_bytes,
        max_domain_logical_bytes: limits.max_domain_logical_bytes,
        elapsed_ms: started.elapsed().as_millis(),
        phase_timing: OvenLoafBakePhaseTiming {
            preflight_elapsed_ms: started.elapsed().as_millis(),
            ..OvenLoafBakePhaseTiming::default()
        },
        cargo_process_started: false,
        evidence: evidence.clone(),
        loafs: reports,
        compiler_suite: None,
    }))
}

/// Bind a checked Loaf fixture probe to the compiler selected by the baker.
///
/// The explicit Cargo executable may come from a nightly toolchain solely because the compiler-suite unit graph
/// requires Cargo's unstable `--unit-graph` interface. That must not let the ambient Rustup toolchain choose the
/// compiler recorded in the receipt: `--rustc` is the compatibility authority for both the probe and publication.
pub(crate) fn pin_loaf_fixture_rustc(command: &mut Command, rustc: &Path) {
    command.env("RUSTC", rustc);
}

/// Keep a cold baker probe from selecting an obsolete Loaf generation beside the development executable.
///
/// The probe exists only to derive a fresh receipt and generated project. Giving that maintainer-owned child an
/// empty compiler-data layout makes the intended Oven miss deterministic while the previous committed generation
/// remains intact until its atomic replacement is ready.
pub(crate) fn isolate_loaf_fixture_toolchain_data(command: &mut Command, toolchain_data_root: &Path) {
    command
        .env("INCAN_INTERNAL_OVEN_LOAF_EXECUTION", "1")
        .env("INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT", toolchain_data_root);
}

/// Recognize only the two fail-closed native-plan misses a baker-owned fixture may legitimately produce.
///
/// Both clauses come from the shared constants the messages themselves are built from, so reworded user-facing
/// text stays recognizable here instead of silently turning an intended miss into an unrecognized failure.
pub(crate) fn loaf_fixture_probe_is_expected_miss(stderr: &str) -> bool {
    [
        oven_rustc::loaf::OVEN_DEPENDENCY_MISS_SUMMARY,
        oven_rustc::loaf::OVEN_NESTED_DEPENDENCY_MISS_SUMMARY,
    ]
    .iter()
    .any(|summary| stderr.contains(summary))
        && stderr.contains(oven_rustc::loaf::OVEN_NO_IMPLICIT_DEPENDENCY_BUILD)
}
