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
const SYSTEM_SELECTOR: &str =
    "build::bake::ordinary_native_publication_tests::ordinary_standard_system_publication_and_replay";
const SYSTEM_DEBUG_SELECTOR: &str =
    "build::bake::ordinary_native_publication_tests::ordinary_standard_system_debug_publication_and_replay";
const CORE_SELECTOR: &str =
    "build::bake::ordinary_native_publication_tests::ordinary_standard_core_publication_and_replay";
const CORE_RELEASE_SELECTOR: &str =
    "build::bake::ordinary_native_publication_tests::ordinary_standard_core_release_publication_and_replay";
const PROFILES: [&str; 2] = ["debug", "release"];

/// Select equivalent ordinary publication journeys with distinct native semantic demand owners.
#[derive(Clone, Copy)]
enum Publication {
    Plain,
    Sysroot,
    Declared,
}

impl Publication {
    /// Bind every nonempty journey to its exact imported ABI item.
    fn abi_query(self) -> Option<&'static str> {
        match self {
            Self::Plain => None,
            Self::Sysroot => Some("std::thread::panicking"),
            Self::Declared => Some("probe_leaf::ready"),
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
    let output = std::process::Command::new(copied.as_os_str())
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
        .current_dir(&ambient)
        .output()?;
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
    assert!(session.sdk_inventory.is_none());
    assert!(session.sdk_components.is_none());
    assert!(Arc::ptr_eq(
        session
            .provider_plan
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

/// Add a real source-owned Rust dependency to the complete support producer request without synthetic owners.
fn declared_graph(root: &Path, graph: &Path) -> TestResult<PathBuf> {
    let project = root.join("probe_leaf");
    fs::create_dir_all(project.join("src"))?;
    fs::write(project.join("src/lib.rs"), "pub fn ready() -> bool { false }\n")?;
    fs::write(
        project.join("loaf.toml"),
        "[project]\nname='probe_leaf'\nversion='1.0.0'\n[rust]\nname='probe_leaf'\ntype='lib'\nedition='2024'\n",
    )?;
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
    let graph = if matches!(variant, Publication::Declared) {
        declared_graph(authored.path(), &original_graph)?
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
    fs::rename(&original, &displaced)?;
    let debug_handoff = native.metadata().verify_profile(&intent).map(|_| ());
    let complete_metadata = native.verify();
    fs::rename(&displaced, &original)?;
    assert!(debug_handoff.is_ok(), "{debug_handoff:?}");
    assert!(complete_metadata.is_err());
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
    fs::write(root.path().join("loaf.toml"), manifest)?;
    if matches!(variant, Publication::Declared) {
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
    let dependencies = Arc::new(PreparedLibraryDependencies::admit(
        &[],
        &target,
        &toolchain,
        *open_default_oven_store()?.limits(),
    )?);
    let features = FeatureSelection::default();
    let session =
        CompilationSession::discover_with_admitted_library_dependencies(&entry, &features, Arc::clone(&dependencies))?;
    let empty_plan = session.provider_plan_for_used_module_paths(BTreeSet::new())?;
    let mut provider_requirements = incan_provider::requirements::ProjectRequirements::default();
    assert!(
        native
            .provider_semantic_dependencies(&empty_plan, &provider_requirements)?
            .is_empty()
    );
    provider_requirements.stdlib_facets.push("unadmitted_facet".to_string());
    assert!(
        native
            .provider_semantic_dependencies(&empty_plan, &provider_requirements)
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
            run_consumer(root.path(), &report, &native, false)?;
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
        run_consumer(root.path(), &edited, &native, true)?;
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
        run_consumer(root.path(), &changed, &refreshed, false)?;
        super::take_library_native_output_work();
        let repeated = bake_admitted_library(&refreshed_input, &features, None)?;
        assert_eq!(super::take_library_native_output_work(), (0, 2));
        assert_eq!(
            outputs(root.path(), &changed, &refreshed)?,
            outputs(root.path(), &repeated, &refreshed)?
        );
        run_consumer(root.path(), &repeated, &refreshed, false)?;
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
