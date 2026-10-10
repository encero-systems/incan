//! Actual ordinary native library publication with complete original producer requests (#1337/#1698).
//!
//! This deliberately requires a coherent source compiler/engine and explicit real resolved support graph. It
//! exercises the admitted plain-library route, not arbitrary semantic/macro coverage or complete SDK removal.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use incan_frontend::library_manifest::published_layout::packaged_library_loaf_manifest_path;
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
const PROFILES: [&str; 2] = ["debug", "release"];

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
fn child() -> TestResult {
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
        .args(["--exact", SELECTOR, "--ignored", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("CARGO_BIN_EXE_incan", &compiler)
        .env("INCAN_OVEN_BAKE_PROFILES", "all")
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
        let projected = native.runtime_inputs(&profile.receipt.intent, &[], &[], &[], &[], root)?;
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
        return child();
    }
    // ---- Genuine executable/source/compiler prerequisites and productive hostile ambient source ----
    assert_eq!(crate::build::plan_authority::explicit_bake_profiles(), PROFILES);
    let graph = required_path("INCAN_ORDINARY_LIBRARY_GRAPH")?;
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
    let mut digests = BTreeMap::new();
    for (profile, request) in &requests {
        let report = native
            .reports()
            .get(profile)
            .ok_or("actual native preparation report missing")?;
        assert!(!request.graph().units().is_empty());
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
    // ---- Fresh ordinary checked dependency admission, actual publication and checked metadata replay ----
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(
        root.path().join("loaf.toml"),
        "[project]\nname='ordinary_native_publication'\nversion='1.0.0'\n",
    )?;
    let entry = root.path().join("src/lib.incn");
    fs::write(&entry, "pub def answer() -> int:\n    return 42\n")?;
    let dependencies = Arc::new(PreparedLibraryDependencies::admit(
        &[],
        &target,
        &toolchain,
        *open_default_oven_store()?.limits(),
    )?);
    let features = FeatureSelection::default();
    let session =
        CompilationSession::discover_with_admitted_library_dependencies(&entry, &features, Arc::clone(&dependencies))?;
    let input = AdmittedLibraryPreparation::with_ordinary_native(session, Arc::clone(&native))?;
    reset_ordinary_library_preparation_branches();
    let mut first: Option<Arc<SelectedLibraryMetadata>> = None;
    let mut first_outputs = None;
    for iteration in 0..2 {
        reset_project_lock_collection_metrics();
        let started = std::time::Instant::now();
        let report = bake_admitted_library(&input, &features, None)?;
        println!(
            "ordinary-publication-evidence {}",
            serde_json::json!({"phase": if iteration == 0 { "publish-first" } else { "publish-repeat" },
                "seconds": started.elapsed().as_secs_f64(), "profiles": &report.profiles})
        );
        let metadata = published_metadata(root.path(), &report)?;
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
    fs::write(&entry, "pub def answer() -> int:\n    return 43\n")?;
    reset_project_lock_collection_metrics();
    let started = std::time::Instant::now();
    let edited = bake_admitted_library(&input, &features, None)?;
    println!(
        "ordinary-publication-evidence {}",
        serde_json::json!({"phase": "publish-source-edit", "seconds": started.elapsed().as_secs_f64(),
            "profiles": &edited.profiles})
    );
    let edited_metadata = published_metadata(root.path(), &edited)?;
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
    Ok(())
}
