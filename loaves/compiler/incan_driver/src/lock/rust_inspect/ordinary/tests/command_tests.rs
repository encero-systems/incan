//! Original native ownership at the normal check and native frontend handoff (#1337).

use super::{Fixture, TestResult};
use std::fs;
use std::path::Path;

const CHILD_ROOT: &str = "INCAN_TEST_COMMAND_INSPECTION_ROOT";
const SELECTOR: &str =
    "lock::rust_inspect::ordinary::tests::command_tests::typecheck_inspection_uses_configured_original_native_graph";

/// Invoke the actual typecheck boundary with a conventional project and no completed inspection publication.
fn prepare(root: &Path) -> TestResult<crate::lock::PreparedRustInspectTypecheckWorkspace> {
    let project = root.join("consumer");
    let path = project.join("src/main.incn");
    let source = fs::read_to_string(&path)?;
    let tokens = incan_frontend::lexer::lex(&source).map_err(|errors| format!("lex: {errors:?}"))?;
    let ast = incan_frontend::parser::parse(&tokens).map_err(|errors| format!("parse: {errors:?}"))?;
    let modules = [incan_frontend::ParsedModule {
        name: "main".into(),
        path_segments: vec!["main".into()],
        file_path: path,
        source,
        ast,
    }];
    let manifest = oven_model::manifest::ProjectManifest::load(&project.join("loaf.toml"))?;
    Ok(
        crate::lock::rust_inspect::prepare_rust_inspect_typecheck_workspace(
            crate::lock::RustInspectTypecheckRequest {
                project_root: &project,
                project_name: "consumer",
                manifest: Some(&manifest),
                modules: &modules,
                library_manifest_index: &Default::default(),
                cargo_features: &Default::default(),
                cargo_policy: &Default::default(),
                rust_edition: Some("2024".into()),
                provider_plan: &Default::default(),
            },
        )?
        .ok_or("actual external query did not retain an inspection workspace")?,
    )
}

/// A real configured original graph serves normal typechecking; damaged configuration cannot use an SDK fallback.
#[test]
fn typecheck_inspection_uses_configured_original_native_graph() -> TestResult {
    // ---- Isolate the configured transport without mutating process-global environment ----
    let Some(root) = std::env::var_os(CHILD_ROOT) else {
        let fixture = Fixture::new()?;
        let root = fixture.root.path();
        fs::create_dir_all(root.join("consumer/src"))?;
        fs::write(
            root.join("consumer/loaf.toml"),
            "[project]\nname='consumer'\nversion='1.0.0'\n[dependencies]\nprobe_leaf={loaf='probe_leaf',path='../probe_leaf',version='1.0.0'}\n",
        )?;
        fs::write(
            root.join("consumer/src/main.incn"),
            "from rust::probe_leaf import answer\ndef main() -> None:\n    pass\n",
        )?;
        fs::write(
            root.join("graph.json"),
            serde_json::to_vec(&serde_json::json!({
                "index_commit":"0000000000000000000000000000000000000000",
                "registry_lock":"lock.json",
                "facets":[
                    {"project":"probe_leaf","domain":"target","features":[]},
                    {"project":"helper_leaf","domain":"target","features":[]},
                    {"project":"unselected_leaf","domain":"target","features":[]}
                ]
            }))?,
        )?;
        return run_child(&fixture, SELECTOR);
    };
    let root = Path::new(&root);

    // ---- Exercise the production handoff and its actual callable metadata ----
    let prepared = prepare(root)?;
    prepared.verify_ordinary_native()?;
    let cache = rust_inspect::RustMetadataCache::new();
    let metadata = cache.get_or_extract_complete(prepared.manifest_dir(), "probe_leaf::answer", &|_| {})?;
    let incan_lang::interop::metadata::RustItemKind::Function(signature) = &metadata.kind else {
        return Err("original configured producer did not provide callable metadata".into());
    };
    assert_eq!(signature.return_type, "i64");

    // ---- Refuse changed original graph rather than select a fallback authority ----
    let graph = root.join("graph.json");
    let original = fs::read(&graph)?;
    fs::write(&graph, b"invalid configured native graph")?;
    let error = prepared
        .verify_ordinary_native()
        .err()
        .ok_or("retained typecheck owner ignored graph edit")?;
    assert!(
        error
            .to_string()
            .contains("complete native producer request inputs changed after preparation"),
        "{error}"
    );
    let refused = prepare(root);
    fs::write(&graph, original)?;
    let error = refused.err().ok_or("configured graph corruption fell through")?;
    assert!(
        error
            .to_string()
            .contains("ordinary native Loaf admission failed: expected value"),
        "{error}"
    );
    prepared.verify_ordinary_native()?;
    let source = root.join("unselected_leaf/src/lib.rs");
    let original = fs::read(&source)?;
    fs::write(&source, "pub fn answer() -> bool { false }\n")?;
    let refused = prepared.verify_ordinary_native();
    fs::write(&source, original)?;
    let error = refused
        .err()
        .ok_or("retained typecheck owner ignored unselected source edit")?;
    assert!(
        error
            .to_string()
            .contains("complete native producer request inputs changed after preparation"),
        "{error}"
    );
    prepared.verify_ordinary_native()?;
    let observation = prepared
        .workspace
        ._ordinary_native
        .as_ref()
        .ok_or("typecheck dropped original native owner")?;
    let unit = observation
        .graph()
        .units()
        .values()
        .next()
        .ok_or("original graph is empty")?;
    let retained = std::sync::Arc::downgrade(unit);
    let toolchain = std::sync::Arc::downgrade(
        prepared
            .workspace
            ._ordinary_toolchain
            .as_ref()
            .ok_or("typecheck dropped compiler owner")?,
    );
    let manifest_dir = prepared.manifest_dir().to_path_buf();
    cache.invalidate_manifest_dir(&manifest_dir)?;
    assert!(retained.upgrade().is_some());
    assert!(toolchain.upgrade().is_some());
    drop(prepared);
    assert!(retained.upgrade().is_none());
    assert!(toolchain.upgrade().is_none());
    let restored = prepare(root)?;
    restored.verify_ordinary_native()?;
    let fresh = cache.get_or_extract_complete(restored.manifest_dir(), "probe_leaf::answer", &|_| {})?;
    assert!(
        matches!(&fresh.kind, incan_lang::interop::metadata::RustItemKind::Function(signature) if signature.return_type == "i64")
    );
    drop(restored);
    cache.invalidate_manifest_dir(&manifest_dir)?;
    verify_command_derive_and_empty_dependency_boundaries(root)?;
    Ok(())
}

/// Exercise the shared test-command adapter with exact declared roots and explicit compiler intent.
fn command_workspace(
    root: &Path,
    dependencies: &[oven_model::manifest::DependencySpec],
    queries: &[String],
    derives: &[String],
    profile: &str,
) -> TestResult<Option<crate::lock::PreparedRustInspectWorkspace>> {
    let rustc = oven_rustc::rustc::resolve_active_rustc()?;
    let target = oven_rustc::rustc::rustc_host_target(&rustc)?;
    let toolchain = oven_rustc::rustc::rustc_identity(&rustc)?;
    let configured = crate::build::native_loaf_plan::ConfiguredNativeLoafInputs {
        graph: root.join("graph.json"),
        index: root.to_path_buf(),
        blobs: root.to_path_buf(),
    };
    let resolved = incan_provider::dependency_resolver::ResolvedDependencies {
        dependencies: dependencies.to_vec(),
        dev_dependencies: Vec::new(),
    };
    let project = root.join("command-consumer");
    let requirements = Default::default();
    let inputs = Default::default();
    Ok(crate::lock::rust_inspect::prepare_rust_inspect_workspace_for_command(
        crate::lock::RustInspectWorkspaceRequest {
            project_root: &project,
            project_name: "command_consumer",
            cargo_package_name: "command_consumer",
            rust_edition: Some("2024".into()),
            resolved: &resolved,
            project_requirements: &requirements,
            lock_payload: None,
            cargo_lock_projection_root: None,
            clear_cargo_lock: false,
            cargo_policy_flags: Vec::new(),
            cargo_target_dir: &root.join("command-inspection"),
            rust_inspect_query_paths: queries,
            rust_derive_probe_paths: derives,
            prepare_when_empty: false,
            direct_oven_inspection: true,
            force_direct_prewarm: false,
            oven_source_authority: Some(crate::lock::OvenRustInspectSourceAuthorityRequest {
                project_version: "1.0.0",
                target: &target,
                toolchain: &toolchain,
                profile,
                features: &[],
                build_unit_inputs: &inputs,
                registry_dependencies: dependencies,
            }),
            prepared_project_source_authorities: None,
            explicit_oven_bake: false,
        },
        Some(&configured),
        None,
        &rustc,
    )?)
}

/// Derive-only requests retain a real host output; empty and Rust-only requests need no external producer.
fn verify_command_derive_and_empty_dependency_boundaries(root: &Path) -> TestResult {
    // ---- Empty and compiler-only queries must not read an invalid external graph ----
    let graph = root.join("graph.json");
    let original = fs::read(&graph)?;
    fs::write(&graph, b"deliberately invalid unused graph")?;
    assert!(command_workspace(root, &[], &[], &[], "debug")?.is_none());
    let standard = command_workspace(root, &[], &["std::mem::size_of".into()], &[], "debug")?;
    assert!(
        standard.is_some(),
        "Rust-only inspection must retain the sealed empty-catalog path"
    );
    fs::write(&graph, original)?;

    // ---- A declared host derive is a real demand even without item queries ----
    let macro_root = root.join("derive_probe");
    fs::create_dir_all(macro_root.join("src"))?;
    fs::write(
        macro_root.join("loaf.toml"),
        "[project]\nname='derive_probe'\nversion='1.0.0'\n[rust]\nname='derive_probe'\ntype='proc-macro'\nedition='2024'\n",
    )?;
    fs::write(
        macro_root.join("src/lib.rs"),
        "extern crate proc_macro;\nuse proc_macro::TokenStream;\n#[proc_macro_derive(Generated)]\npub fn generated(_: TokenStream) -> TokenStream { TokenStream::new() }\n",
    )?;
    fs::write(
        &graph,
        serde_json::to_vec(&serde_json::json!({
            "index_commit":"0000000000000000000000000000000000000000", "registry_lock":"lock.json",
            "facets":[{"project":"derive_probe","domain":"host","features":[]}]
        }))?,
    )?;
    let dependencies = [oven_model::manifest::DependencySpec {
        crate_name: "derive_probe".into(),
        version: Some("=1.0.0".into()),
        features: Vec::new(),
        default_features: true,
        source: oven_model::manifest::DependencySource::Path {
            path: macro_root.clone(),
        },
        optional: false,
        package: Some("derive_probe".into()),
    }];
    let derives = ["derive_probe::Generated".into()];
    let prepared = command_workspace(root, &dependencies, &[], &derives, "release")?
        .ok_or("derive-only command skipped original host authority")?;
    prepared.verify_ordinary_native()?;
    let observation = prepared
        ._ordinary_native
        .as_ref()
        .ok_or("derive command lost native request")?;
    assert!(!observation.graph().units().is_empty());
    for unit in observation.graph().units().values() {
        assert_eq!(unit.record().recipe.intent.profile, "release");
        assert_eq!(unit.record().source.domain, "host");
    }
    assert!(fs::read_to_string(prepared.manifest_dir().join("src/main.rs"))?.contains("derive_probe::Generated"));

    // ---- Neither an undeclared name nor a changed macro source may reuse the admitted owner ----
    let error = command_workspace(root, &[], &[], &derives, "release")
        .err()
        .ok_or("undeclared derive acquired native authority")?;
    assert!(error.to_string().contains("has no declared native root"), "{error}");
    let source = macro_root.join("src/lib.rs");
    let original = fs::read(&source)?;
    fs::write(&source, "extern crate proc_macro;\n")?;
    let refused = prepared.verify_ordinary_native();
    fs::write(&source, original)?;
    let error = refused
        .err()
        .ok_or("retained command owner ignored macro source edit")?;
    assert!(
        error
            .to_string()
            .contains("complete native producer request inputs changed after preparation"),
        "{error}"
    );
    prepared.verify_ordinary_native()?;
    Ok(())
}

/// Run one exact control with its own configured graph and Store, preserving the outer harness reuse contract.
fn run_child(fixture: &Fixture, selector: &str) -> TestResult {
    let root = fixture.root.path();
    // Original standard-source ownership follows the actual executable, not ambient source-root variables.
    // Reuse the ordinary publication controls' private copied-libtest lifetime beside the real compiler.
    let compiler =
        std::path::PathBuf::from(std::env::var_os("CARGO_BIN_EXE_incan").ok_or("actual compiler is required")?)
            .canonicalize()?;
    let parent = compiler.parent().ok_or("actual compiler has no executable owner")?;
    let copied = tempfile::Builder::new()
        .prefix("incan-command-inspection-libtest-")
        .tempfile_in(parent)?
        .into_temp_path();
    fs::copy(std::env::current_exe()?, &copied)?;
    let output = std::process::Command::new(copied.as_os_str())
        .args(["--exact", selector, "--nocapture", "--test-threads=1"])
        .env(CHILD_ROOT, root)
        .env("CARGO_BIN_EXE_incan", &compiler)
        .env("INCAN_HOME", root.join("home"))
        .env("INCAN_SDK_NATIVE_COMPILER_GRAPH", root.join("graph.json"))
        .env("INCAN_SDK_NATIVE_INDEX", root)
        .env("INCAN_SDK_NATIVE_BLOBS", root)
        .env("RUSTC", &fixture.rustc)
        .env_remove("INCAN_TEST_REQUIRE_STORED_NATIVE_REUSE")
        .output()?;
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    Ok(())
}

/// Quoted derives keep their declared host dependency through the public typecheck resolver without a Rust import.
#[test]
fn typecheck_inspection_retains_declared_quoted_derive() -> TestResult {
    const DERIVE_SELECTOR: &str =
        "lock::rust_inspect::ordinary::tests::command_tests::typecheck_inspection_retains_declared_quoted_derive";

    // ---- Original declared macro and isolated command transport ----
    let Some(root) = std::env::var_os(CHILD_ROOT) else {
        let fixture = Fixture::new()?;
        let root = fixture.root.path();
        let macros = root.join("derive_probe");
        fs::create_dir_all(macros.join("src"))?;
        fs::write(
            macros.join("loaf.toml"),
            "[project]\nname='derive_probe'\nversion='1.0.0'\n[rust]\nname='derive_probe'\ntype='proc-macro'\nedition='2024'\n",
        )?;
        fs::write(
            macros.join("src/lib.rs"),
            "extern crate proc_macro;\nuse proc_macro::TokenStream;\n#[proc_macro_derive(Generated)]\npub fn generated(_: TokenStream) -> TokenStream { TokenStream::new() }\n",
        )?;
        fs::create_dir_all(root.join("consumer/src"))?;
        fs::write(
            root.join("consumer/loaf.toml"),
            "[project]\nname='consumer'\nversion='1.0.0'\n[dependencies]\nderive_probe={loaf='derive_probe',path='../derive_probe',version='1.0.0'}\n",
        )?;
        fs::write(
            root.join("consumer/src/main.incn"),
            "@rust.derive(\"derive_probe::Generated\")\nmodel Item:\n    value: int\n",
        )?;
        fs::write(
            root.join("graph.json"),
            serde_json::to_vec(&serde_json::json!({
                "index_commit":"0000000000000000000000000000000000000000", "registry_lock":"lock.json",
                "facets":[{"project":"derive_probe","domain":"host","features":[]}]
            }))?,
        )?;
        return run_child(&fixture, DERIVE_SELECTOR);
    };
    let root = Path::new(&root);

    // ---- Actual typecheck entry retains the quoted demand and original executable macro ----
    let prepared = prepare(root)?;
    let cache = rust_inspect::RustMetadataCache::new();
    let metadata = cache.get_or_extract_complete(prepared.manifest_dir(), "derive_probe::Generated", &|_| {})?;
    assert!(matches!(
        metadata.kind,
        incan_lang::interop::metadata::RustItemKind::Macro(_)
    ));
    assert!(fs::read_to_string(prepared.manifest_dir().join("src/main.rs"))?.contains("derive_probe::Generated"));

    // ---- A quote without the original declaration cannot borrow a graph member's authority ----
    let declaration = root.join("consumer/loaf.toml");
    let original = fs::read(&declaration)?;
    fs::write(&declaration, "[project]\nname='consumer'\nversion='1.0.0'\n")?;
    let refused = prepare(root);
    fs::write(&declaration, original)?;
    let error = refused.err().ok_or("undeclared quote acquired host macro authority")?;
    assert!(
        error
            .to_string()
            .contains("ordinary derive `derive_probe::Generated` has no declared native root"),
        "{error}"
    );
    let restored = prepare(root)?;
    let metadata = cache.get_or_extract_complete(restored.manifest_dir(), "derive_probe::Generated", &|_| {})?;
    assert!(matches!(
        metadata.kind,
        incan_lang::interop::metadata::RustItemKind::Macro(_)
    ));
    Ok(())
}
