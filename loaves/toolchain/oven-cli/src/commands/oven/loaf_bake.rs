//! `incan oven legacy-cargo bake-loafs`: bake or reuse the compiler-owned Loaf envelope generations.
//!
//! The one place Cargo is allowed to build Incan's own runtime and compiler-suite closures. Everything it publishes
//! is keyed on the evidence in `loaf_bake_evidence` and committed atomically as a generation.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use std::time::Instant;

use incan_driver::build::publication::stored_project_output_from_parts;
use incan_driver::build::{OvenProjectOutputPayload, OvenStoredProjectOutput};
use oven_rustc::loaf::{
    OVEN_RELEASE_STORE_MEMBER_SCHEMA_VERSION, OvenReleaseRuntimeFoundationMember, OvenReleaseStoreMember,
};
use oven_store::process::{BoundedProcessLimits, BoundedProcessTermination, run_bounded_process};
use oven_store::store::{
    OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, PublishedOvenStore,
};

const POLICY_EXCHANGE_MAX_BYTES: u64 = 16 * 1024 * 1024;

use super::{
    CliError, CliResult, CompleteLoafEnvelopeReuseInput, DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_LOGICAL_BYTES,
    DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_PHYSICAL_BYTES, DEFAULT_OVEN_COMPILER_SUITE_MAX_PHYSICAL_BYTES,
    DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES, DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES, DEFAULT_OVEN_MAX_PHYSICAL_BYTES,
    ExitCode, LoafTemporaryDirectory, OVEN_LEGACY_CARGO_INSPECTION_AUTHORITY_ENV, OVEN_LOAF_ENV,
    OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION, OvenCompilerSuiteBakeReport, OvenInspectionRegistrySource,
    OvenLegacyCargoDirectDependencyClosure, OvenLegacyCargoInspectionSource, OvenLegacyCargoPrepareRequest,
    OvenLegacyCargoPublicationKind, OvenLoafBakeCommandOptions, OvenLoafBakeEntryReport, OvenLoafBakePhaseTiming,
    OvenLoafBakeReport, OvenLoafBakerContext, OvenLoafEnvelope, OvenLoafEnvelopeArgument, OvenLoafEnvelopeManifest,
    OvenLoafEnvelopeMember, OvenLoafFixtureAction, OvenOutputFormat, OvenStoreCommandOptions, OvenStoreLimits,
    acquire_exclusive_loaf_generation_lock, announce_oven_progress, commit_loaf_generation, compiler_libtests_receipt,
    elapsed_detail, env, human_bytes, import_loaf_envelope_from_configured_mirrors,
    isolate_loaf_fixture_toolchain_data, legacy_cargo_inspection_sources, legacy_cargo_resolved_registry_sources,
    loaf_compiler_lock_path, loaf_compiler_manifest_path, loaf_directory_byte_counts,
    loaf_envelope_compatibility_map_with_release_member, loaf_envelope_evidence, loaf_envelope_inspection_packages,
    loaf_envelope_name, loaf_envelope_specifications, loaf_fixture_action_name, loaf_fixture_probe_is_expected_miss,
    loaf_generation_identity_with_release_member, loaf_raw_disk_bytes, open_store, oven_error, pin_loaf_fixture_rustc,
    prepare_compiler_test_suite, prepare_loaf_from_generated_project_with_selected_units, print_json, read_receipt,
    release_store_member_byte_counts, retire_unreferenced_loaf_generations, reuse_complete_loaf_envelope,
    stage_locked_loaf_fixture, write_receipt, write_sealed_oven_inspection_source_authority,
};

/// Bake or exactly reuse one complete compiler-owned Alpha Loaf envelope.
///
/// The command is hidden beneath `legacy_cargo` because Cargo may run only for a genuine Loaf miss. Normal
/// build/run/test commands never call this function and never fall back to it.
pub fn oven_legacy_cargo_bake_loafs(options: OvenLoafBakeCommandOptions) -> CliResult<ExitCode> {
    let started = Instant::now();
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
        &options.rustc,
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
    let _release_policy_output = if let (Some((source_store, _, expected_target)), Some(member)) =
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
    let mut release_foundation_capture = None;
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
        pin_loaf_fixture_rustc(&mut command, &options.rustc);
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
                rustc: &options.rustc,
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
        if envelope == OvenLoafEnvelope::Release
            && specification.label == "stdlib"
            && specification.profile == "release"
        {
            let selected_units = prepared
                .selected_units
                .clone()
                .ok_or_else(|| CliError::failure("release stdlib publisher produced no exact selected-unit capture"))?;
            if release_foundation_capture
                .replace((receipt.clone(), selected_units))
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

    let logical_bytes = pending
        .iter()
        .map(|entry| entry.result.logical_bytes)
        .sum::<u64>()
        .saturating_add(release_member_logical_bytes);
    let physical_bytes = pending
        .iter()
        .map(|entry| entry.result.physical_bytes)
        .sum::<u64>()
        .saturating_add(release_member_physical_bytes);
    if physical_bytes > max_physical_bytes {
        return Err(CliError::failure(format!(
            "Loaf envelope uses {physical_bytes} physical bytes, exceeding its {max_physical_bytes}-byte allowance"
        )));
    }

    let prepared_count = pending.len();
    let envelope_publication_started = Instant::now();
    announce_oven_progress("PUBLISH", "Loaf envelope", Some(&format!("{prepared_count} Loaf(s)")));
    let runtime_foundation: Option<OvenReleaseRuntimeFoundationMember> = None;
    let mut compatibility_evidence =
        loaf_envelope_compatibility_map_with_release_member(&evidence, release_store_member.as_ref())?;
    if let Some(member) = runtime_foundation.as_ref() {
        oven_rustc::loaf::bind_release_runtime_foundation_evidence(&mut compatibility_evidence, member)
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
    let admitted = selected.admitted_materialized_files().to_vec();
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
    let published = destination
        .publish_verified_import(
            &OvenArtifactPublishRequest {
                receipt,
                domain: manifest.domain.clone(),
                kind: manifest.kind,
                payload: payload_bytes,
                materialized_files,
            },
            &admitted,
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

/// Execute the exact admitted policy engine with one bounded file exchange.
fn run_release_rust_policy(
    policy: &OvenStoredProjectOutput,
    exchange_root: &Path,
    request: &serde_json::Value,
) -> CliResult<serde_json::Value> {
    fs::create_dir_all(exchange_root).map_err(|error| {
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
    let file = fs::File::open(&response_path)
        .map_err(|error| CliError::failure(format!("could not open Rust policy response: {error}")))?;
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
            }
        }
        OvenOutputFormat::Json => print_json(report)?,
    }
    Ok(())
}
