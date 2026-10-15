//! Actual ordinary native library publication with complete original producer requests (#1337/#1698).
//!
//! This deliberately requires a coherent source compiler/engine and explicit real resolved support graph. It
//! exercises plain libraries and scalar sysroot/declared Rust ABI uses, including native source invalidation, not
//! arbitrary semantic/macro coverage or complete SDK removal.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use incan_frontend::library_manifest::published_layout::packaged_library_loaf_manifest_path;
use incan_frontend::provider::source_policy::TrustedStandardSourcePublication;
use incan_frontend::typechecker::stdlib_loader::StdlibAstCache;
use incan_provider::FeatureSelection;
use oven_model::lock::LOCK_FILENAME;
use oven_rustc::native_loaf::NativeLoafRequestObservation;
use oven_rustc::rustc::{resolve_active_rustc, rustc_host_target, rustc_identity};
use oven_store::OvenBuildIntent;

use super::admitted_publication_tests::published_metadata;
use super::bake_admitted_library;
use crate::build::library_dependencies::PreparedLibraryDependencies;
use crate::build::library_metadata::SelectedLibraryMetadata;
use crate::build::library_project::metadata_replay::ordinary_native::take_metadata_request_verifications;
use crate::build::library_project::{
    AdmittedLibraryPreparation, ordinary_library_preparation_branches, reset_ordinary_library_preparation_branches,
};
use crate::build::native_runtime_inputs::bound_compiler_engine;
use crate::build::ordinary_library_native::{OrdinaryLibraryNativeProfiles, OrdinaryLibraryNativeRequest};
use crate::build::ordinary_support::CompilerSupportSources;
use crate::build::{OvenPackagedLibraryLoafManifest, OvenProjectBakeReport};
use crate::lock::{project_lock_collection_counts, reset_project_lock_collection_metrics};
use crate::oven_store::open_default_oven_store;
use crate::session::CompilationSession;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
const CHILD: &str = "INCAN_DEV7_ORDINARY_NATIVE_PUBLICATION_CHILD";
const SELECTOR: &str =
    "build::bake::ordinary_native_publication_tests::ordinary_native_library_publication_first_replay_and_source_edit";
const ABI_SELECTOR: &str =
    "build::bake::ordinary_native_publication_tests::ordinary_native_sysroot_abi_publication_replay_and_runtime";
const DECLARED_SELECTOR: &str =
    "build::bake::ordinary_native_publication_tests::ordinary_native_declared_rust_abi_publication_replay_and_runtime";
const REGISTRY_SELECTOR: &str =
    "build::bake::ordinary_native_publication_tests::ordinary_native_registry_abi_publication_replay_and_runtime";
const SYSTEM_SELECTOR: &str =
    "build::bake::ordinary_native_publication_tests::ordinary_standard_system_publication_and_replay";
const SYSTEM_DEBUG_SELECTOR: &str =
    "build::bake::ordinary_native_publication_tests::ordinary_standard_system_debug_publication_and_replay";
const CORE_SELECTOR: &str =
    "build::bake::ordinary_native_publication_tests::ordinary_standard_core_publication_and_replay";
const CORE_RELEASE_SELECTOR: &str =
    "build::bake::ordinary_native_publication_tests::ordinary_standard_core_release_publication_and_replay";
const CORE_ADMISSION_SELECTOR: &str =
    "build::bake::ordinary_native_publication_tests::ordinary_core_checked_dependency_admission";
const PROFILES: [&str; 2] = ["debug", "release"];

/// Select equivalent ordinary publication journeys with distinct native semantic demand owners.
#[derive(Clone, Copy)]
enum Publication {
    Plain,
    Sysroot,
    Declared,
    Registry,
}

impl Publication {
    /// Bind every nonempty journey to its exact imported ABI item.
    fn abi_query(self) -> Option<&'static str> {
        match self {
            Self::Plain => None,
            Self::Sysroot => Some("std::thread::panicking"),
            Self::Declared => Some("probe_leaf::ready"),
            Self::Registry => Some("serde_json::de::from_slice"),
        }
    }

    /// Produce an observable Incan source edit without changing the native dependency selection.
    fn source(self, edited: bool) -> &'static str {
        match (self, edited) {
            (Self::Plain, false) => "pub def answer() -> int:\n    return 42\n",
            (Self::Plain, true) => "pub def answer() -> int:\n    return 43\n",
            (Self::Sysroot, false) => {
                "from rust::std::thread import panicking\npub def answer() -> bool:\n    return panicking()\n"
            }
            (Self::Sysroot, true) => {
                "from rust::std::thread import panicking\npub def answer() -> bool:\n    return not panicking()\n"
            }
            (Self::Declared, false) => {
                "from rust::probe_leaf import ready\npub def answer() -> bool:\n    return ready()\n"
            }
            (Self::Declared, true) => {
                "from rust::probe_leaf import ready\npub def answer() -> bool:\n    return not ready()\n"
            }
            (Self::Registry, false) => {
                "from rust::serde_json::de import from_slice\npub def answer() -> bool:\n    return from_slice(b\"false\").unwrap_or(False)\n"
            }
            (Self::Registry, true) => {
                "from rust::serde_json::de import from_slice\npub def answer() -> bool:\n    return from_slice(b\"true\").unwrap_or(False)\n"
            }
        }
    }
}

/// Require each explicit genuine graph/registry/compiler coordinate instead of skipping unavailable prerequisites.
fn required_path(name: &str) -> TestResult<PathBuf> {
    let path = PathBuf::from(std::env::var_os(name).ok_or_else(|| format!("required {name} is unavailable"))?);
    Ok(path.canonicalize()?)
}

/// Write a productive legacy source decoy; ordinary admissions must use their retained genuine support instead.
fn decoy(root: &Path) -> TestResult {
    fs::create_dir_all(root.join("system/src"))?;
    fs::write(
        root.join("sdk-components.toml"),
        "[sdk]\nid='incan'\nversion='0.6.0-dev.6'\ncompiler-requirement='>=0.6.0-dev.6,<0.7.0'\n\
         [profiles]\nminimal=['stdlib-system']\ndefault=['stdlib-system']\n\
         [components.stdlib-system]\nproject='system'\nnamespace-roots=['io','fs','environ','tempfile']\n",
    )?;
    fs::write(
        root.join("system/src/io.incn"),
        "pub def ambient_only() -> str:\n    return \"hostile ambient source\"\n",
    )?;
    // An accidental SDK selection must fail rather than silently obtaining a valid unrelated native catalog.
    fs::write(root.join("forged-sdk.json"), b"not an admitted SDK inventory")?;
    fs::write(root.join("forged-native-graph.json"), b"not a resolved native graph")?;
    Ok(())
}

/// Execute only the ignored control beside the real compiler, with a uniquely owned copied libtest lifetime.
fn child(selector: &str) -> TestResult {
    child_profiles(selector, "all")
}

/// Execute the same isolated control with an exact explicit native profile selection.
fn child_profiles(selector: &str, profiles: &str) -> TestResult {
    let compiler = required_path("CARGO_BIN_EXE_incan")?;
    for name in [
        "INCAN_ORDINARY_LIBRARY_GRAPH",
        "INCAN_ORDINARY_LIBRARY_INDEX",
        "INCAN_ORDINARY_LIBRARY_BLOBS",
    ] {
        required_path(name)?;
    }
    let parent = compiler.parent().ok_or("actual compiler has no executable owner")?;
    // TempPath removes only this exclusive task-created file. Dropping its writable handle before execution also
    // avoids running an executable that this process still has open for writing. Its name deliberately is not incan:
    // the identity engine must remain bound to CARGO_BIN_EXE_incan and that compiler's real source receipt.
    let copied = tempfile::Builder::new()
        .prefix("incan-ordinary-native-publication-libtest-")
        .tempfile_in(parent)?
        .into_temp_path();
    fs::copy(std::env::current_exe()?, &copied)?;
    assert_ne!(copied.file_name(), Some(std::ffi::OsStr::new("incan")));
    let isolated = tempfile::tempdir()?;
    let ambient = isolated.path().join("ambient");
    decoy(&ambient)?;
    let mut command = std::process::Command::new(copied.as_os_str());
    command
        .args(["--exact", selector, "--ignored", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        // The outer FIRST flag requires reuse of the native test binary. This isolated control intentionally
        // creates fresh projects and edits source; its own executor counters enforce unchanged publication reuse.
        .env_remove("INCAN_TEST_REQUIRE_STORED_NATIVE_REUSE")
        .env("CARGO_BIN_EXE_incan", &compiler)
        .env("INCAN_OVEN_BAKE_PROFILES", profiles)
        .env("INCAN_HOME", isolated.path().join("home"))
        .env("INCAN_STDLIB", &ambient)
        .env("INCAN_SOURCE_ROOT", &ambient)
        .env("INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT", &ambient)
        .env("INCAN_SDK_INVENTORY", ambient.join("forged-sdk.json"))
        .env(
            "INCAN_SDK_NATIVE_COMPILER_GRAPH",
            ambient.join("forged-native-graph.json"),
        )
        .env("INCAN_SDK_NATIVE_INDEX", &ambient)
        .env("INCAN_SDK_NATIVE_BLOBS", &ambient)
        .env("INCAN_INTERNAL_SDK_PROVIDER_STORE", ambient.join("sdk-store"))
        .current_dir(&ambient);
    // Normal consumer discovery needs genuine producer coordinates; the SDK inventory remains a poisoned decoy.
    if selector == CORE_ADMISSION_SELECTOR || selector == REGISTRY_SELECTOR {
        command
            .env(
                "INCAN_SDK_NATIVE_COMPILER_GRAPH",
                required_path("INCAN_ORDINARY_LIBRARY_GRAPH")?,
            )
            .env("INCAN_SDK_NATIVE_INDEX", required_path("INCAN_ORDINARY_LIBRARY_INDEX")?)
            .env("INCAN_SDK_NATIVE_BLOBS", required_path("INCAN_ORDINARY_LIBRARY_BLOBS")?);
    }
    let output = command.output()?;
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

/// Publish the unchanged executable-owned system package through the ordinary producer and checked replay.
#[test]
#[ignore = "requires coherent source compiler/engine and explicit real standard-system native graph/index/blobs"]
fn ordinary_standard_system_publication_and_replay() -> TestResult {
    if std::env::var_os(CHILD).is_none() {
        return child(SYSTEM_SELECTOR);
    }
    standard_source_publication(
        "incan_stdlib_system",
        "INCAN_ORDINARY_SYSTEM_GRAPH",
        &["std::io::Read", "std::io::SeekFrom"],
        &PROFILES,
    )
}

/// Reach actual system source/publication gates independently of missing release-only native facts.
#[test]
#[ignore = "requires coherent source compiler/engine and explicit real standard-system native graph/index/blobs"]
fn ordinary_standard_system_debug_publication_and_replay() -> TestResult {
    if std::env::var_os(CHILD).is_none() {
        return child_profiles(SYSTEM_DEBUG_SELECTOR, "debug");
    }
    standard_source_publication(
        "incan_stdlib_system",
        "INCAN_ORDINARY_SYSTEM_GRAPH",
        &["std::io::Read", "std::io::SeekFrom"],
        &["debug"],
    )
}

/// Publish the actual core source and its own Rust facet before a dependent standard package is admitted.
#[test]
#[ignore = "requires coherent source compiler/engine and explicit real ordinary native graph/index/blobs"]
fn ordinary_standard_core_publication_and_replay() -> TestResult {
    if std::env::var_os(CHILD).is_none() {
        return child(CORE_SELECTOR);
    }
    standard_source_publication(
        "incan_stdlib_core",
        "INCAN_ORDINARY_LIBRARY_GRAPH",
        &["incan_std_core::errors::raise_value_error"],
        &PROFILES,
    )
}

/// A release-only ordinary request must publish and replay without preparing or inspecting a debug profile.
#[test]
#[ignore = "requires coherent source compiler/engine and explicit real ordinary native graph/index/blobs"]
fn ordinary_standard_core_release_publication_and_replay() -> TestResult {
    if std::env::var_os(CHILD).is_none() {
        return child_profiles(CORE_RELEASE_SELECTOR, "release");
    }
    standard_source_publication(
        "incan_stdlib_core",
        "INCAN_ORDINARY_LIBRARY_GRAPH",
        &["incan_std_core::errors::raise_value_error"],
        &["release"],
    )
}

/// An ordinary consumer publishes with core's checked namespace and reuses core output without SDK access.
#[test]
#[ignore = "requires coherent source compiler/engine and explicit real ordinary native graph/index/blobs"]
fn ordinary_core_checked_dependency_admission() -> TestResult {
    if std::env::var_os(CHILD).is_none() {
        return child_profiles(CORE_ADMISSION_SELECTOR, "debug");
    }
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(
        root.path().join("loaf.toml"),
        "[project]\nname='ordinary_core_consumer'\nversion='1.0.0'\n",
    )?;
    let entry = root.path().join("src/lib.incn");
    fs::write(
        &entry,
        "pub def count(values: List[int]) -> int:\n    return values.iter().count()\n",
    )?;
    let manifest = oven_model::manifest::ProjectManifest::load(&root.path().join("loaf.toml"))?;
    let input = AdmittedLibraryPreparation::discover_ordinary_native(&manifest, &FeatureSelection::default(), None)?
        .ok_or("configured ordinary preparation absent")?;
    let modules = crate::modules::collect_modules_detailed_with_session(entry, input.session())
        .map_err(|failure| failure.render_human())?;
    assert_eq!(modules.len(), 1, "core source must remain in its checked loaf");
    assert!(input.session().sdk_inventory().is_none());
    assert!(input.session().sdk_components().is_none());
    let core_provider = input
        .session()
        .provider_plan()
        .active_records()
        .find(|provider| provider.identity.name == "incan_stdlib_core")
        .ok_or("admitted core provider absent")?;
    assert_eq!(
        core_provider
            .artifact
            .as_ref()
            .ok_or("core artifact absent")?
            .dependency_key,
        "incan_stdlib_core"
    );
    let core_dependency = core_provider
        .artifact
        .as_ref()
        .ok_or("core artifact absent")?
        .to_dependency_spec();
    let owners = input
        .session()
        .admitted_library_dependencies()
        .ok_or("ordinary dependencies absent")?;
    assert!(
        owners
            .physical_native_dependencies(input.session().provider_plan(), std::slice::from_ref(&core_dependency))?
            .is_empty()
    );
    let mut changed_dependency = core_dependency;
    changed_dependency.features.push("unadmitted".into());
    assert_eq!(
        owners.physical_native_dependencies(
            input.session().provider_plan(),
            std::slice::from_ref(&changed_dependency)
        )?,
        vec![changed_dependency]
    );
    assert!(matches!(
        input
            .session()
            .provider_plan()
            .resolve_module(&["std".into(), "derives".into(), "collection".into()]),
        incan_provider::ProviderModuleResolution::Active(_)
    ));
    input
        .session()
        .admitted_library_dependencies()
        .ok_or("ordinary dependencies absent")?
        .verify()?;
    // One successful observation produces its semantic identity through one complete native verification.
    let native = input.ordinary_native().ok_or("ordinary native authority absent")?;
    take_metadata_request_verifications();
    let observation =
        crate::build::library_project::metadata_replay::MetadataPreparation::observe_with_ordinary_native_authority(
            &manifest,
            input.session(),
            &root.path().join("target/lib"),
            Arc::clone(native.metadata()),
        )?;
    assert!(observation.is_some(), "ordinary metadata observation must be available");
    assert_eq!(
        take_metadata_request_verifications(),
        1,
        "one metadata observation must verify its full native request exactly once"
    );
    let report = bake_admitted_library(&input, &FeatureSelection::default(), None)?;
    for output in &report.outputs {
        let receipt = crate::build::source_authority::project_bake_receipt_path(
            root.path(),
            crate::build::OvenBakeProjectTarget::Library,
            &root.path().join("src/lib.incn"),
            &output.profile,
        )?;
        assert_eq!(
            crate::build::output_selection::project_output_locator(&receipt).as_deref(),
            Some(output.artifact_identity.as_str()),
            "fresh publication must retain its exact output address before the first unchanged command"
        );
        let selected = crate::build::output_selection::located_project_output(
            &crate::oven_store::open_default_oven_store()?,
            &receipt,
            None,
            |_, _| true,
        )?
        .ok_or("fresh published output cannot be admitted")?;
        assert_eq!(selected.identity, output.artifact_identity);
        assert!(
            crate::build::output_materialization::project_output_projection_is_current(root.path(), &selected)?,
            "fresh publication must retain its current projection before the first unchanged command"
        );
    }
    super::admitted_publication_tests::published_metadata_profiles(root.path(), &report, &["debug"])?;
    let core = TrustedStandardSourcePublication::discover("incan_stdlib_core")?
        .ok_or("compiler-owned core source unavailable")?;
    let handoff_path = packaged_library_loaf_manifest_path(&core.verified_package_root()?.join("target/lib"));
    let handoff = fs::read(&handoff_path)?;
    let package: OvenPackagedLibraryLoafManifest = serde_json::from_slice(&handoff)?;
    for profile in package.profiles.values() {
        assert_eq!(
            profile
                .receipt
                .sources
                .build_unit_inputs
                .get("compiler-version")
                .map(String::as_str),
            Some(incan_lang::version::INCAN_VERSION)
        );
    }
    reset_ordinary_library_preparation_branches();
    let repeated = AdmittedLibraryPreparation::discover_ordinary_native(&manifest, &FeatureSelection::default(), None)?
        .ok_or("repeated ordinary preparation absent")?;
    assert_eq!(
        ordinary_library_preparation_branches(),
        (0, 0),
        "unchanged core must reuse completed output before source preparation or metadata replay"
    );
    assert_eq!(fs::read(&handoff_path)?, handoff);
    assert!(repeated.session().sdk_inventory().is_none());
    assert!(repeated.session().sdk_components().is_none());
    repeated
        .session()
        .admitted_library_dependencies()
        .ok_or("repeated ordinary dependencies absent")?
        .verify()?;
    assert!(matches!(
        repeated
            .session()
            .provider_plan()
            .resolve_module(&["std".into(), "derives".into(), "collection".into()]),
        incan_provider::ProviderModuleResolution::Active(_)
    ));
    // ---- A public dependency and its consumer must share the exact published core instance ----
    let downstream = tempfile::tempdir()?;
    fs::create_dir_all(downstream.path().join("src"))?;
    fs::write(
        downstream.path().join("loaf.toml"),
        format!(
            "[project]\nname='ordinary_core_downstream'\nversion='1.0.0'\n[dependencies]\ncounts={{path={:?}}}\n",
            root.path().to_string_lossy(),
        ),
    )?;
    fs::write(
        downstream.path().join("src/lib.incn"),
        "from pub::counts import count\n\npub def score() -> int:\n    return count([1, 2, 3])\n",
    )?;
    let downstream_manifest = oven_model::manifest::ProjectManifest::load(&downstream.path().join("loaf.toml"))?;
    let downstream_input =
        AdmittedLibraryPreparation::discover_ordinary_native(&downstream_manifest, &FeatureSelection::default(), None)?
            .ok_or("downstream ordinary preparation absent")?;
    let rustc = resolve_active_rustc()?;
    let target = rustc_host_target(&rustc)?;
    let toolchain = rustc_identity(&rustc)?;
    let plan = downstream_input.session().provider_plan();
    let checked = crate::build::provider_compilation::checked_packaged_provider_profiles(
        plan,
        &["debug"],
        &target,
        &toolchain,
        None,
    )?;
    let attachments =
        crate::build::caller_owned::oven_caller_owned_libraries(plan, "debug", &target, &toolchain, Some(&checked))?;
    let counts = attachments
        .iter()
        .find(|library| library.crate_name == "counts")
        .ok_or("counts output missing")?;
    let original = fs::read(&counts.output)?;
    fs::write(&counts.output, b"not the admitted counts output")?;
    let tampered =
        crate::build::caller_owned::oven_caller_owned_libraries(plan, "debug", &target, &toolchain, Some(&checked));
    fs::write(&counts.output, &original)?;
    assert!(tampered.is_err_and(|error| error.message.contains("differs from its admitted")));
    let moved = counts.output.with_extension("retained-test-rlib");
    fs::rename(&counts.output, &moved)?;
    let missing =
        crate::build::caller_owned::oven_caller_owned_libraries(plan, "debug", &target, &toolchain, Some(&checked));
    fs::rename(&moved, &counts.output)?;
    assert!(missing.is_err_and(|error| error.message.contains("missing its admitted")));
    let downstream_report = bake_admitted_library(&downstream_input, &FeatureSelection::default(), None)?;
    super::admitted_publication_tests::published_metadata_profiles(downstream.path(), &downstream_report, &["debug"])?;
    assert_eq!(
        fs::read(&handoff_path)?,
        handoff,
        "public consumption must preserve the exact core publication"
    );
    Ok(())
}

/// Keep every requested profile, original source grant and published checked metadata owner in one journey.
fn standard_source_publication(package: &str, graph: &str, required_abi: &[&str], profiles: &[&str]) -> TestResult {
    let source =
        Arc::new(TrustedStandardSourcePublication::discover(package)?.ok_or("standard package source unavailable")?);
    let root = source.verified_package_root()?.to_path_buf();
    let before = oven_store::project_source_tree_evidence(&root)?;
    let rustc = resolve_active_rustc()?;
    let target = rustc_host_target(&rustc)?;
    let toolchain = rustc_identity(&rustc)?;
    let dependencies = Arc::new(PreparedLibraryDependencies::admit(
        &[],
        &target,
        &toolchain,
        *open_default_oven_store()?.limits(),
    )?);
    let features = FeatureSelection::default();
    let session = CompilationSession::discover_with_admitted_standard_source(
        &root.join("src/lib.incn"),
        &features,
        dependencies,
        Arc::clone(&source),
    )?;
    assert!(session.sdk_inventory().is_none());
    assert!(session.sdk_components().is_none());
    assert!(Arc::ptr_eq(
        session
            .provider_plan()
            .standard_source_publication()
            .ok_or("own source authority lost")?,
        &source,
    ));
    let manifest = session
        .manifest
        .as_ref()
        .ok_or("standard package declaration missing")?;
    let support = Arc::new(CompilerSupportSources::discover()?.ok_or("compiler support unavailable")?);
    let output = tempfile::tempdir()?;
    let native = Arc::new(OrdinaryLibraryNativeProfiles::prepare(OrdinaryLibraryNativeRequest {
        support,
        graph: &required_path(graph)?,
        index: &required_path("INCAN_ORDINARY_LIBRARY_INDEX")?,
        blobs: &required_path("INCAN_ORDINARY_LIBRARY_BLOBS")?,
        output: output.path(),
        rustc: &rustc,
        target: &target,
        profiles,
    })?);
    assert_eq!(
        native.reports().keys().map(String::as_str).collect::<BTreeSet<_>>(),
        profiles.iter().copied().collect()
    );
    native.verify_dependencies(&manifest.rust_dependency_values(), &root)?;
    let input = AdmittedLibraryPreparation::with_ordinary_native(session, native)?;
    let mut original_owner = None;
    for _ in 0..2 {
        let _ = super::take_library_native_output_work();
        let result = bake_admitted_library(&input, &features, None);
        let after = oven_store::project_source_tree_evidence(&root)?;
        if before.digest()? != after.digest()? {
            assert!(before.unchanged_except_exact_file(&after, Path::new("oven.lock"))?);
        }
        let report = result?;
        let (compiled, reused) = super::take_library_native_output_work();
        let metadata = super::admitted_publication_tests::published_metadata_profiles(&root, &report, profiles)?;
        let store = oven_store::store::OvenStore::with_release(
            &report.store,
            *open_default_oven_store()?.limits(),
            &incan_oven_facet::compiler_identity(),
        );
        for profile in profiles {
            let output = crate::build::output_selection::select_baked_project_output(
                &store,
                &root,
                &root.join("src/lib.incn"),
                crate::build::OvenBakeProjectTarget::Library,
                profile,
            )?
            .ok_or("ordinary source-current output missing")?;
            let payload = &output.payload;
            let authority = oven_rustc::rustc::load_project_inspection_authority(
                &store,
                payload
                    .inspection_authority
                    .as_ref()
                    .ok_or("inspection authority missing")?,
                &payload.project_identity,
                &payload.source_authority_digest,
                &payload.compiler_version,
            )?;
            assert_eq!(
                authority.payload.test_dependency_envelope.is_some(),
                profiles.contains(&"debug")
            );
            assert!(!authority.payload.constituents.is_empty());
        }
        crate::build::library_metadata::validate_required_rust_abi(
            metadata.manifest(),
            &required_abi.iter().map(|path| (*path).to_string()).collect(),
        )?;
        let owner = metadata.reference().owner_identity.clone();
        if let Some(original) = &original_owner {
            assert_eq!(original, &owner);
            assert_eq!((compiled, reused), (0, profiles.len()));
        } else {
            original_owner = Some(owner);
        }
    }
    Ok(())
}

/// Actual published artifact, receipt and native content identities for one output profile.
#[derive(Debug, PartialEq, Eq)]
struct PublishedOutput {
    artifact_identity: String,
    receipt_identity: String,
    library_digest: String,
}

/// Verify actual immutable completed-output, receipt and native bytes for each genuinely published profile.
fn outputs(
    root: &Path,
    report: &OvenProjectBakeReport,
    native: &OrdinaryLibraryNativeProfiles,
) -> TestResult<BTreeMap<String, PublishedOutput>> {
    let artifact = root.join("target/lib");
    let package: OvenPackagedLibraryLoafManifest =
        serde_json::from_slice(&fs::read(packaged_library_loaf_manifest_path(&artifact))?)?;
    let mut selected = BTreeMap::new();
    for output in &report.outputs {
        let profile = package
            .profiles
            .get(&output.profile)
            .ok_or("completed profile missing from package")?;
        let digest = oven_store::digest_bytes(&fs::read(artifact.join(&profile.library_relative_path))?);
        assert_eq!(digest, profile.library_digest);
        assert_eq!(output.receipt_identity, profile.receipt.identity);
        let project = oven_model::manifest::ProjectManifest::load(&root.join("loaf.toml"))?;
        let dependencies = project.rust_dependency_values();
        let projected = native.runtime_inputs(&profile.receipt.intent, &[], &[], &dependencies, &dependencies, root)?;
        assert_eq!(
            profile.receipt.sources.build_unit_inputs.get("ordinary-native-roots"),
            projected.get("ordinary-native-roots"),
            "final published receipt must preserve the actual engine's original roots projection"
        );
        assert!(
            selected
                .insert(
                    output.profile.clone(),
                    PublishedOutput {
                        artifact_identity: output.artifact_identity.clone(),
                        receipt_identity: output.receipt_identity.clone(),
                        library_digest: digest,
                    }
                )
                .is_none()
        );
    }
    assert_eq!(
        selected.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        PROFILES.into()
    );
    Ok(selected)
}

/// Verify original full request capabilities survive every actual publication handoff without changing routes.
fn original_requests(
    input: &AdmittedLibraryPreparation,
    original: &Arc<OrdinaryLibraryNativeProfiles>,
    requests: &BTreeMap<String, NativeLoafRequestObservation>,
    digests: &BTreeMap<String, String>,
) -> TestResult {
    assert!(input.temporary_native_sdk_context().is_none());
    let retained = input.ordinary_native().ok_or("ordinary native admission lost")?;
    assert!(Arc::ptr_eq(original, retained));
    assert!(Arc::ptr_eq(original.metadata(), retained.metadata()));
    #[cfg(feature = "rust_inspect")]
    assert!(Arc::ptr_eq(
        original.metadata().inspection_toolchain(),
        retained.metadata().inspection_toolchain()
    ));
    for (profile, request) in requests {
        let current = retained
            .metadata()
            .observations()
            .get(profile)
            .ok_or("original profile request lost")?;
        assert_eq!(
            current.verified_digest()?,
            digests.get(profile).ok_or("original full digest missing")?
        );
        assert_eq!(current.graph().units().len(), request.graph().units().len());
        for (identity, owner) in request.graph().units() {
            assert!(Arc::ptr_eq(
                owner,
                current.graph().units().get(identity).ok_or("original owner lost")?
            ));
        }
    }
    Ok(())
}

/// Publish both native profiles from genuine ordinary requests, replay metadata, then compile an observable edit.
#[test]
#[ignore = "requires coherent source compiler/engine and explicit real ordinary support graph/index/blobs"]
fn ordinary_native_library_publication_first_replay_and_source_edit() -> TestResult {
    if std::env::var_os(CHILD).is_none() {
        return child(SELECTOR);
    }
    publication(Publication::Plain)
}

/// Ship real sysroot callable metadata in the ordinary loaf and run both native profiles before and after an edit.
#[test]
#[ignore = "requires coherent source compiler/engine and explicit real ordinary support graph/index/blobs"]
fn ordinary_native_sysroot_abi_publication_replay_and_runtime() -> TestResult {
    if std::env::var_os(CHILD).is_none() {
        return child(ABI_SELECTOR);
    }
    publication(Publication::Sysroot)
}

/// Ship an authored Rust dependency's ABI in an ordinary loaf and observe replay and source edits in both profiles.
#[test]
#[ignore = "requires coherent source compiler/engine and explicit real ordinary support graph/index/blobs"]
fn ordinary_native_declared_rust_abi_publication_replay_and_runtime() -> TestResult {
    if std::env::var_os(CHILD).is_none() {
        return child(DECLARED_SELECTOR);
    }
    publication(Publication::Declared)
}

/// Publish a registry-backed ABI without a generated Cargo lock, reuse both profiles and execute changed output.
#[test]
#[ignore = "requires coherent source compiler/engine and ordinary support graph with serde_json in both profiles"]
fn ordinary_native_registry_abi_publication_replay_and_runtime() -> TestResult {
    if std::env::var_os(CHILD).is_none() {
        return child(REGISTRY_SELECTOR);
    }
    publication(Publication::Registry)
}

/// Add a real source-owned Rust dependency to the complete support producer request without synthetic owners.
fn declared_graph(root: &Path, graph: &Path, registry: bool) -> TestResult<PathBuf> {
    let project = root.join("probe_leaf");
    fs::create_dir_all(project.join("src"))?;
    let source = if registry {
        "pub use serde_json::Value;\n"
    } else {
        "pub fn ready() -> bool { false }\n"
    };
    fs::write(project.join("src/lib.rs"), source)?;
    let mut manifest =
        "[project]\nname='probe_leaf'\nversion='1.0.0'\n[rust]\nname='probe_leaf'\ntype='lib'\nedition='2024'\n"
            .to_string();
    if registry {
        manifest.push_str("[dependencies]\nserde_json={loaf='crates-io/serde_json',version='1.0'}\n");
    }
    fs::write(project.join("loaf.toml"), manifest)?;
    let owner = graph.parent().ok_or("graph has no owner")?;
    let mut document: serde_json::Value = serde_json::from_slice(&fs::read(graph)?)?;
    let lock = document["registry_lock"].as_str().ok_or("graph lock missing")?;
    document["registry_lock"] = serde_json::json!(owner.join(lock).canonicalize()?);
    for facet in document["facets"].as_array_mut().ok_or("graph facets missing")? {
        let path = facet["project"].as_str().ok_or("facet project missing")?;
        facet["project"] = serde_json::json!(owner.join(path).canonicalize()?);
    }
    document["facets"]
        .as_array_mut()
        .ok_or("graph facets missing")?
        .push(serde_json::json!({"project": project, "features": [], "domain": "target"}));
    let output = root.join("graph.json");
    fs::write(&output, serde_json::to_vec_pretty(&document)?)?;
    Ok(output)
}

/// Run the identical ordinary admission, publication, reuse and source-edit journey for genuine native demands.
fn publication(variant: Publication) -> TestResult {
    // ---- Genuine executable/source/compiler prerequisites and productive hostile ambient source ----
    assert_eq!(crate::build::plan_authority::explicit_bake_profiles(), PROFILES);
    let original_graph = required_path("INCAN_ORDINARY_LIBRARY_GRAPH")?;
    let authored = tempfile::tempdir()?;
    let graph = if matches!(variant, Publication::Declared | Publication::Registry) {
        declared_graph(
            authored.path(),
            &original_graph,
            matches!(variant, Publication::Registry),
        )?
    } else {
        original_graph
    };
    let index = required_path("INCAN_ORDINARY_LIBRARY_INDEX")?;
    let blobs = required_path("INCAN_ORDINARY_LIBRARY_BLOBS")?;
    required_path("CARGO_BIN_EXE_incan")?;
    let (engine, expected_engine) = bound_compiler_engine()?;
    assert_eq!(oven_store::digest_bytes(&fs::read(engine)?), expected_engine);
    let mut legacy = StdlibAstCache::new();
    assert!(
        legacy
            .lookup_function_meta(&["std".to_string(), "io".to_string()], "ambient_only")
            .is_some()
    );
    let support = Arc::new(CompilerSupportSources::discover()?.ok_or("genuine compiler support is unavailable")?);
    assert_ne!(support.verified_standard_source_root()?, required_path("INCAN_STDLIB")?);
    let rustc = resolve_active_rustc()?;
    let target = rustc_host_target(&rustc)?;
    let toolchain = rustc_identity(&rustc)?;
    // ---- Actual complete debug/release native requests, unchanged producer repeat and intent refusals ----
    let native_output = tempfile::tempdir()?;
    let native = Arc::new(OrdinaryLibraryNativeProfiles::prepare(OrdinaryLibraryNativeRequest {
        support: Arc::clone(&support),
        graph: &graph,
        index: &index,
        blobs: &blobs,
        output: native_output.path(),
        rustc: &rustc,
        target: &target,
        profiles: &PROFILES,
    })?);
    assert!(Arc::ptr_eq(&support, native.metadata().support()));
    for (profile, report) in native.reports() {
        println!(
            "ordinary-publication-evidence {}",
            serde_json::json!({"phase": "native-first", "profile": profile,
                "compiled": report.compiled.len(), "reused": report.reused.len(), "seconds": report.seconds})
        );
    }
    assert_eq!(
        native.reports().keys().map(String::as_str).collect::<BTreeSet<_>>(),
        PROFILES.into()
    );
    let requests = native.metadata().observations().clone();
    assert!(!native_output.path().join("store").exists());
    let mut digests = BTreeMap::new();
    for (profile, request) in &requests {
        let report = native
            .reports()
            .get(profile)
            .ok_or("actual native preparation report missing")?;
        assert!(!request.graph().units().is_empty());
        for unit in request.graph().units().values() {
            assert!(!unit.output()?.starts_with(native_output.path()));
        }
        assert_eq!(
            report.compiled.len() + report.reused.len(),
            request.graph().units().len()
        );
        digests.insert(profile.clone(), request.verified_digest()?.to_string());
    }
    let repeated = OrdinaryLibraryNativeProfiles::prepare(OrdinaryLibraryNativeRequest {
        support: Arc::clone(&support),
        graph: &graph,
        index: &index,
        blobs: &blobs,
        output: native_output.path(),
        rustc: &rustc,
        target: &target,
        profiles: &PROFILES,
    })?;
    for (profile, report) in repeated.reports() {
        println!(
            "ordinary-publication-evidence {}",
            serde_json::json!({"phase": "native-repeat", "profile": profile,
                "compiled": report.compiled.len(), "reused": report.reused.len(), "seconds": report.seconds})
        );
        assert!(report.compiled.is_empty());
        let request = repeated
            .metadata()
            .observations()
            .get(profile)
            .ok_or("repeat full request missing")?;
        assert_eq!(report.reused.len(), request.graph().units().len());
        assert_eq!(
            request.verified_digest()?,
            digests.get(profile).ok_or("original full digest missing")?
        );
    }
    drop(repeated);
    // Relocation preserves physical owners; the full producer request still observes its changed output path.
    let relocated_output = tempfile::tempdir()?;
    let relocated = OrdinaryLibraryNativeProfiles::prepare(OrdinaryLibraryNativeRequest {
        support: Arc::clone(&support),
        graph: &graph,
        index: &index,
        blobs: &blobs,
        output: relocated_output.path(),
        rustc: &rustc,
        target: &target,
        profiles: &PROFILES,
    })?;
    assert!(!relocated_output.path().join("store").exists());
    for (profile, report) in relocated.reports() {
        assert!(report.compiled.is_empty());
        let request = relocated
            .metadata()
            .observations()
            .get(profile)
            .ok_or("relocated full request missing")?;
        request.verify()?;
        assert_eq!(report.reused.len(), request.graph().units().len());
        for (identity, unit) in request.graph().units() {
            let original = requests
                .get(profile)
                .and_then(|request| request.graph().units().get(identity))
                .ok_or("relocated original native record missing")?;
            assert_eq!(unit.output()?, original.output()?);
        }
        println!(
            "ordinary-publication-evidence {}",
            serde_json::json!({"phase": "native-relocated", "profile": profile,
                "compiled": report.compiled.len(), "reused": report.reused.len(), "seconds": report.seconds})
        );
    }
    drop(relocated);
    let debug = requests.get("debug").ok_or("debug request missing")?;
    let intent = OvenBuildIntent {
        target: target.clone(),
        toolchain: toolchain.clone(),
        profile: "debug".to_string(),
        features: Vec::new(),
    };
    assert_eq!(
        debug.verify_intent(&rustc, &intent)?,
        digests.get("debug").ok_or("debug full digest missing")?
    );
    for wrong in [
        OvenBuildIntent {
            profile: "release".to_string(),
            ..intent.clone()
        },
        OvenBuildIntent {
            target: "unbound-target".to_string(),
            ..intent.clone()
        },
        OvenBuildIntent {
            toolchain: "unbound-toolchain".to_string(),
            ..intent.clone()
        },
    ] {
        assert!(debug.verify_intent(&rustc, &wrong).is_err());
    }
    // A physical debug handoff needs its entire original request, but does not consume release output bytes.
    // Full metadata authority must still refuse the missing release member rather than pruning that request.
    let release = requests.get("release").ok_or("release request missing")?;
    let release_unit = release
        .graph()
        .units()
        .values()
        .next()
        .ok_or("release native graph is empty")?;
    let original = release_unit.output()?;
    let displaced = original.with_extension("profile-missing");
    let dependency_owner = support.declaration_owner()?;
    fs::rename(&original, &displaced)?;
    let debug_handoff = native.metadata().verify_profile(&intent).map(|_| ());
    let complete_metadata = native.verify();
    let complete_dependencies = native.verify_dependencies(&[], dependency_owner);
    fs::rename(&displaced, &original)?;
    assert!(debug_handoff.is_ok(), "{debug_handoff:?}");
    assert!(complete_metadata.is_err());
    assert!(complete_dependencies.is_err());
    native.verify()?;
    // ---- Fresh ordinary checked dependency admission, actual publication and checked metadata replay ----
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    let mut manifest = "[project]\nname='ordinary_native_publication'\nversion='1.0.0'\n".to_string();
    if matches!(variant, Publication::Declared) {
        manifest.push_str(&format!(
            "[dependencies]\nprobe_leaf={{loaf='probe_leaf',path='{}'}}\n",
            authored.path().join("probe_leaf").display()
        ));
    }
    if matches!(variant, Publication::Registry) {
        manifest.push_str("[dependencies]\nserde_json={loaf='crates-io/serde_json',version='1.0'}\n");
    }
    fs::write(root.path().join("loaf.toml"), manifest)?;
    if matches!(variant, Publication::Declared | Publication::Registry) {
        let project = oven_model::manifest::ProjectManifest::load(&root.path().join("loaf.toml"))?;
        let declared = project.rust_dependency_values();
        native.verify_dependencies(&declared, root.path())?;
        let mut wrong_features = declared.clone();
        wrong_features
            .first_mut()
            .ok_or("declared dependency missing")?
            .features
            .push("unprepared".to_string());
        assert!(native.verify_dependencies(&wrong_features, root.path()).is_err());
        let mut wrong_source = declared;
        wrong_source.first_mut().ok_or("declared dependency missing")?.source =
            oven_model::manifest::DependencySource::Path {
                path: root.path().join("absent-leaf"),
            };
        assert!(native.verify_dependencies(&wrong_source, root.path()).is_err());
    }
    let entry = root.path().join("src/lib.incn");
    fs::write(&entry, variant.source(false))?;
    let namespace_inputs = if matches!(variant, Publication::Registry) {
        let source = Arc::new(
            TrustedStandardSourcePublication::discover("incan_stdlib_core")?
                .ok_or("compiler-bound core publication source unavailable")?,
        );
        vec![crate::build::library_project::prepare_core_checked_dependency(
            source,
            Arc::clone(&native),
            &target,
            &toolchain,
        )?]
    } else {
        Vec::new()
    };
    let dependencies = Arc::new(PreparedLibraryDependencies::admit(
        &namespace_inputs,
        &target,
        &toolchain,
        *open_default_oven_store()?.limits(),
    )?);
    let features = FeatureSelection::default();
    let session =
        CompilationSession::discover_with_admitted_library_dependencies(&entry, &features, Arc::clone(&dependencies))?;
    let provider_plan = session.provider_plan_for_used_module_paths(BTreeSet::new())?;
    let mut provider_requirements = dependencies.projected_requirements(&provider_plan)?;
    assert!(
        native
            .provider_semantic_dependencies(&provider_plan, &provider_requirements, Some(&dependencies))?
            .is_empty()
    );
    if provider_plan.records().next().is_some() {
        assert!(
            native
                .provider_semantic_dependencies(&provider_plan, &provider_requirements, None)
                .is_err()
        );
    }
    provider_requirements.stdlib_facets.push("unadmitted_facet".to_string());
    assert!(
        native
            .provider_semantic_dependencies(&provider_plan, &provider_requirements, Some(&dependencies))
            .is_err()
    );
    let input = AdmittedLibraryPreparation::with_ordinary_native(session, Arc::clone(&native))?;
    reset_ordinary_library_preparation_branches();
    let mut first: Option<Arc<SelectedLibraryMetadata>> = None;
    let mut first_outputs = None;
    for iteration in 0..2 {
        reset_project_lock_collection_metrics();
        super::take_library_native_output_work();
        take_metadata_request_verifications();
        let started = std::time::Instant::now();
        let report = bake_admitted_library(&input, &features, None)?;
        let (compiled, reused) = super::take_library_native_output_work();
        let request_verifications = take_metadata_request_verifications();
        println!(
            "ordinary-publication-evidence {}",
            serde_json::json!({"phase": if iteration == 0 { "publish-first" } else { "publish-repeat" },
                "seconds": started.elapsed().as_secs_f64(), "compiled": compiled, "reused": reused,
                "metadata_request_verifications": request_verifications,
                "profiles": &report.profiles})
        );
        for profile in &report.profiles {
            println!("ordinary-publication-receipt {}", fs::read_to_string(&profile.receipt)?);
        }
        assert_eq!((compiled, reused), if iteration == 0 { (2, 0) } else { (0, 2) });
        let metadata = published_metadata(root.path(), &report)?;
        if matches!(variant, Publication::Declared) {
            verify_declared_contract(&metadata)?;
        }
        if let Some(query) = variant.abi_query() {
            crate::build::library_metadata::validate_required_rust_abi(
                metadata.manifest(),
                &[query.to_string()].into(),
            )?;
            run_consumer(root.path(), &report, &native, &dependencies, false)?;
        }
        let actual_outputs = outputs(root.path(), &report, &native)?;
        assert_eq!(project_lock_collection_counts(), (1, 0));
        assert!(root.path().join(LOCK_FILENAME).is_file());
        assert_eq!(ordinary_library_preparation_branches(), (1, iteration));
        original_requests(&input, &native, &requests, &digests)?;
        assert!(Arc::ptr_eq(
            &dependencies,
            input
                .session()
                .admitted_library_dependencies()
                .ok_or("original dependency admission lost")?
        ));
        if let Some(original) = &first {
            assert_eq!(original.reference().owner_identity, metadata.reference().owner_identity);
            assert_eq!(first_outputs.as_ref(), Some(&actual_outputs));
        } else {
            first = Some(metadata);
            first_outputs = Some(actual_outputs);
        }
    }
    // ---- Source edit invalidates checked and both native output generations; complete request stays current ----
    fs::write(&entry, variant.source(true))?;
    reset_project_lock_collection_metrics();
    super::take_library_native_output_work();
    take_metadata_request_verifications();
    let started = std::time::Instant::now();
    let edited = bake_admitted_library(&input, &features, None)?;
    let (compiled, reused) = super::take_library_native_output_work();
    let request_verifications = take_metadata_request_verifications();
    println!(
        "ordinary-publication-evidence {}",
        serde_json::json!({"phase": "publish-source-edit", "seconds": started.elapsed().as_secs_f64(),
            "compiled": compiled, "reused": reused, "metadata_request_verifications": request_verifications,
            "profiles": &edited.profiles})
    );
    assert_eq!((compiled, reused), (2, 0));
    let edited_metadata = published_metadata(root.path(), &edited)?;
    if variant.abi_query().is_some() {
        run_consumer(root.path(), &edited, &native, &dependencies, true)?;
    }
    if matches!(variant, Publication::Registry) {
        run_ordinary_incan_consumer(root.path())?;
    }
    let original = first.ok_or("original checked metadata missing")?;
    assert_ne!(
        original.reference().owner_identity,
        edited_metadata.reference().owner_identity
    );
    assert_ne!(original.recipe().source_digest, edited_metadata.recipe().source_digest);
    assert_eq!(
        original.recipe().semantic_authority_digest,
        edited_metadata.recipe().semantic_authority_digest
    );
    let edited_outputs = outputs(root.path(), &edited, &native)?;
    for (profile, original_output) in first_outputs.ok_or("original outputs missing")? {
        let edited_output = edited_outputs.get(&profile).ok_or("edited profile output missing")?;
        assert_ne!(original_output.artifact_identity, edited_output.artifact_identity);
        assert_ne!(original_output.receipt_identity, edited_output.receipt_identity);
        assert_ne!(original_output.library_digest, edited_output.library_digest);
    }
    assert_eq!(ordinary_library_preparation_branches(), (2, 1));
    assert_eq!(project_lock_collection_counts(), (1, 0));
    original.verify()?;
    edited_metadata.verify()?;
    original_requests(&input, &native, &requests, &digests)?;
    if matches!(variant, Publication::Declared) {
        // A native source edit makes the retained producer request stale. Reprepare only the changed leaf,
        // then prove that an unchanged Incan consumer observes the new dependency body in both profiles.
        let unchanged_incan_source = fs::read(&entry)?;
        fs::write(
            authored.path().join("probe_leaf/src/lib.rs"),
            "pub fn ready() -> bool { true }\n",
        )?;
        assert!(native.verify().is_err());
        assert!(bake_admitted_library(&input, &features, None).is_err());
        let refreshed = Arc::new(OrdinaryLibraryNativeProfiles::prepare(OrdinaryLibraryNativeRequest {
            support: Arc::clone(&support),
            graph: &graph,
            index: &index,
            blobs: &blobs,
            output: native_output.path(),
            rustc: &rustc,
            target: &target,
            profiles: &PROFILES,
        })?);
        for (profile, report) in refreshed.reports() {
            let request = refreshed
                .metadata()
                .observations()
                .get(profile)
                .ok_or("refreshed request missing")?;
            println!(
                "ordinary-publication-evidence {}",
                serde_json::json!({
                    "phase": "native-dependency-edit", "profile": profile, "compiled": report.compiled.len(),
                    "reused": report.reused.len(), "seconds": report.seconds,
                })
            );
            assert_eq!(report.compiled.len(), 1);
            assert_eq!(report.reused.len() + 1, request.graph().units().len());
            assert_ne!(
                request.verified_digest()?,
                digests.get(profile).ok_or("original full digest missing")?
            );
        }
        let session = CompilationSession::discover_with_admitted_library_dependencies(
            &entry,
            &features,
            Arc::clone(&dependencies),
        )?;
        let refreshed_input = AdmittedLibraryPreparation::with_ordinary_native(session, Arc::clone(&refreshed))?;
        super::take_library_native_output_work();
        let started = std::time::Instant::now();
        let changed = bake_admitted_library(&refreshed_input, &features, None)?;
        let (compiled, reused) = super::take_library_native_output_work();
        println!(
            "ordinary-publication-evidence {}",
            serde_json::json!({
                "phase": "publish-dependency-edit", "seconds": started.elapsed().as_secs_f64(),
                "compiled": compiled, "reused": reused,
            })
        );
        assert_eq!((compiled, reused), (2, 0));
        let changed_metadata = published_metadata(root.path(), &changed)?;
        verify_declared_contract(&changed_metadata)?;
        assert_ne!(
            edited_metadata.reference().owner_identity,
            changed_metadata.reference().owner_identity
        );
        assert_ne!(
            edited_metadata.recipe().semantic_authority_digest,
            changed_metadata.recipe().semantic_authority_digest
        );
        assert_eq!(unchanged_incan_source, fs::read(&entry)?);
        assert_ne!(
            edited_metadata.recipe().source_digest,
            changed_metadata.recipe().source_digest
        );
        run_consumer(root.path(), &changed, &refreshed, &dependencies, false)?;
        super::take_library_native_output_work();
        let repeated = bake_admitted_library(&refreshed_input, &features, None)?;
        assert_eq!(super::take_library_native_output_work(), (0, 2));
        assert_eq!(
            outputs(root.path(), &changed, &refreshed)?,
            outputs(root.path(), &repeated, &refreshed)?
        );
        run_consumer(root.path(), &repeated, &refreshed, &dependencies, false)?;
        changed_metadata.verify()?;
        refreshed.verify()?;
    }
    Ok(())
}

/// Serialized ordinary contracts cannot substitute a crate, omit coverage or inject inline source overrides.
fn verify_declared_contract(metadata: &SelectedLibraryMetadata) -> TestResult {
    use crate::build::library_metadata::requirements::CheckedLibraryRequirements;

    let checked = metadata
        .checked_requirements()
        .ok_or("declared publication lacks checked requirements")?;
    checked.require_source_inspection_native()?;
    assert_eq!(checked.source_inline_crates, ["probe_leaf".to_string()].into());
    let original = serde_json::to_value(checked)?;
    for crates in [
        serde_json::json!([]),
        serde_json::json!(["other"]),
        serde_json::json!(["probe_leaf", "other"]),
    ] {
        let mut payload = original.clone();
        payload["source_inline_crates"] = crates;
        let decoded: CheckedLibraryRequirements = serde_json::from_value(payload)?;
        assert!(decoded.require_source_inspection_native().is_err());
    }
    for (field, value) in [
        ("crate_name", serde_json::json!("other")),
        ("version", serde_json::json!("1")),
        ("features", serde_json::json!(["unobserved"])),
    ] {
        let mut payload = original.clone();
        payload["imports"][0][field] = value;
        let decoded: CheckedLibraryRequirements = serde_json::from_value(payload)?;
        assert!(decoded.require_source_inspection_native().is_err(), "{field}");
    }
    for field in ["scalar_native_imports", "declared_native_crates"] {
        let mut payload = original.clone();
        payload["native_demands"]["observed"]
            .as_object_mut()
            .ok_or("native facts missing")?
            .remove(field);
        let decoded: CheckedLibraryRequirements = serde_json::from_value(payload)?;
        assert!(decoded.require_source_inspection_native().is_err(), "{field}");
    }
    Ok(())
}

/// Link the actually published rlib and its original native dependency owners, then observe the exported value.
fn run_consumer(
    root: &Path,
    report: &OvenProjectBakeReport,
    native: &OrdinaryLibraryNativeProfiles,
    dependencies: &PreparedLibraryDependencies,
    expected: bool,
) -> TestResult {
    let artifact = root.join("target/lib");
    let package: OvenPackagedLibraryLoafManifest =
        serde_json::from_slice(&fs::read(packaged_library_loaf_manifest_path(&artifact))?)?;
    let source_digest = crate::build::library_project::metadata_replay::observe_library_source_digest(root, &[])?;
    // Consumers are separate projects. Their source and binaries must not mutate the publisher's authored tree
    // and legitimately invalidate its unchanged-source metadata recipe before the replay assertion.
    let consumer = tempfile::tempdir()?;
    let source = consumer.path().join("consumer.rs");
    fs::write(
        &source,
        format!("fn main() {{ assert_eq!(ordinary_native_publication::answer(), {expected}); }}\n"),
    )?;
    for output in &report.outputs {
        let profile = package
            .profiles
            .get(&output.profile)
            .ok_or("consumer profile missing")?;
        let executable = consumer.path().join(format!("consumer-{}", output.profile));
        let mut command = std::process::Command::new(native.metadata().rustc());
        command
            .arg(&source)
            .args(["--edition=2024", "--crate-name", "ordinary_consumer", "--extern"])
            .arg(format!(
                "ordinary_native_publication={}",
                artifact.join(&profile.library_relative_path).display()
            ))
            .arg("-o")
            .arg(&executable);
        let observation = native
            .metadata()
            .observations()
            .get(&output.profile)
            .ok_or("consumer original request missing")?;
        let mut paths = BTreeSet::new();
        for unit in observation.graph().units().values() {
            let path = unit.output()?;
            paths.insert(path.parent().ok_or("native output has no parent")?.to_path_buf());
        }
        let checked_dependencies = dependencies.execution_profiles(&[&output.profile])?;
        for profiles in checked_dependencies.values() {
            for checked in profiles {
                let path = checked.artifact_root.join(&checked.package.library_relative_path);
                assert_eq!(
                    oven_store::digest_bytes(&fs::read(&path)?),
                    checked.package.library_digest
                );
                paths.insert(
                    path.parent()
                        .ok_or("checked library output has no parent")?
                        .to_path_buf(),
                );
            }
        }
        for path in paths {
            command.arg("-L").arg(format!("dependency={}", path.display()));
        }
        let built = command.output()?;
        assert!(
            built.status.success(),
            "consumer compile: {}",
            String::from_utf8_lossy(&built.stderr)
        );
        let ran = std::process::Command::new(&executable).output()?;
        assert!(
            ran.status.success(),
            "consumer runtime: {}",
            String::from_utf8_lossy(&ran.stderr)
        );
        println!(
            "ordinary-consumer-evidence {}",
            serde_json::json!({
                "profile": output.profile, "expected": expected, "command": format!("{command:?}"),
                "library_digest": profile.library_digest,
                "executable_digest": oven_store::digest_bytes(&fs::read(&executable)?), "success": true,
            })
        );
    }
    assert_eq!(
        source_digest,
        crate::build::library_project::metadata_replay::observe_library_source_digest(root, &[])?
    );
    Ok(())
}

/// Exercise the ordinary executable lifecycle against the actual registry-backed publication, with hostile SDK inputs.
/// Both profiles must use the original checked package owners; compilation and execution remain canonical.
fn run_ordinary_incan_consumer(library: &Path) -> TestResult {
    let consumer = tempfile::tempdir()?;
    fs::create_dir_all(consumer.path().join("src"))?;
    fs::write(
        consumer.path().join("loaf.toml"),
        format!(
            "[project]\nname='ordinary_incan_consumer'\nversion='1.0.0'\n[dependencies]\nordinary_native_publication={{path='{}'}}\n",
            library.display(),
        ),
    )?;
    let entry = consumer.path().join("src/main.incn");
    fs::write(
        &entry,
        "from pub::ordinary_native_publication import answer\ndef main() -> None:\n    println(answer())\n",
    )?;
    let entry_text = entry.to_str().ok_or("consumer entry is not UTF-8")?;
    for profile in PROFILES {
        let prepared = crate::build::oven_project::prepare_oven_project(
            entry_text,
            None,
            &crate::cargo_policy::CargoPolicy::default(),
            &FeatureSelection::default(),
            None,
            Vec::new(),
            false,
            false,
            profile,
            crate::build::OvenProjectPlanMode::ConsumeOnly,
            None,
        )?;
        assert!(prepared.native_sdk_context.is_none());
        assert!(
            prepared
                .ordinary_session
                .as_ref()
                .is_some_and(|session| session.has_admitted_library_dependencies())
        );
        assert_eq!(
            prepared
                .ordinary_native
                .as_ref()
                .ok_or("native owner missing")?
                .metadata()
                .observations()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec![profile]
        );
        let baked = super::bake_oven_project(&prepared, profile, None)?;
        assert!(!baked.cargo_process_started);
        let output = std::process::Command::new(&baked.output).output()?;
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(String::from_utf8(output.stdout)?.trim(), "true");
        println!(
            "ordinary-incan-consumer-evidence {}",
            serde_json::json!({
                "profile": profile, "success": true, "sdk_discovered": false,
                "output_digest": baked.output_digest,
            })
        );
    }
    Ok(())
}
