//! Actual Web publisher inspection roots, retained through original source and native admissions (#1337).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use incan_frontend::provider::source_policy::TrustedStandardSourcePublication;
use incan_provider::FeatureSelection;
use incan_provider::dependency_resolver::{ResolvedDependencies, resolve_reachable_dependencies};
use incan_provider::requirements::ProjectRequirements;
use oven_model::manifest::{DependencySource, DependencySpec, ProjectManifest};
use oven_rustc::rustc::{resolve_active_rustc, rustc_host_target, rustc_identity};

use crate::build::library_dependencies::PreparedLibraryDependencies;
use crate::build::library_project::AdmittedLibraryPreparation;
use crate::build::ordinary_library_native::{OrdinaryLibraryNativeProfiles, OrdinaryLibraryNativeRequest};
use crate::build::ordinary_support::CompilerSupportSources;
use crate::lock::{OvenRustInspectSourceAuthorityRequest, RustInspectWorkspaceRequest};
use crate::oven_store::open_default_oven_store;
use crate::session::CompilationSession;

use super::{CHILD, TestResult, child_profiles, required_path};

const SELECTOR: &str =
    "build::bake::ordinary_native_publication_tests::web::ordinary_standard_web_inspection_retains_declared_macro_root";

/// Follow the real Web source-owner handoff; forward membership alone must not lose its declared macro alias.
#[test]
#[ignore = "requires coherent source compiler and the genuine standard native family containing Web"]
fn ordinary_standard_web_inspection_retains_declared_macro_root() -> TestResult {
    if std::env::var_os(CHILD).is_none() {
        return child_profiles(SELECTOR, "debug");
    }

    // ---- Original Web source issuer and actual route demand ----
    let source =
        Arc::new(TrustedStandardSourcePublication::discover("incan_stdlib_web")?.ok_or("original Web source missing")?);
    let root = source.verified_package_root()?.to_path_buf();
    let rustc = resolve_active_rustc()?;
    let target = rustc_host_target(&rustc)?;
    let toolchain = rustc_identity(&rustc)?;
    let dependencies = Arc::new(PreparedLibraryDependencies::admit(
        &[],
        &target,
        &toolchain,
        *open_default_oven_store()?.limits(),
    )?);
    let session = CompilationSession::discover_with_admitted_standard_source(
        &root.join("src/lib.incn"),
        &FeatureSelection::default(),
        dependencies,
        Arc::clone(&source),
    )?;
    assert!(session.sdk_inventory().is_none());
    let manifest = ProjectManifest::load(&root.join("loaf.toml"))?;
    let path = root.join("src/web/routing.incn");
    let text = fs::read_to_string(&path)?;
    let tokens = incan_frontend::lexer::lex(&text).map_err(|errors| format!("Web lex: {errors:?}"))?;
    let ast = incan_frontend::parser::parse(&tokens).map_err(|errors| format!("Web parse: {errors:?}"))?;
    let module = incan_frontend::ParsedModule {
        name: "web.routing".into(),
        path_segments: vec!["web".into(), "routing".into()],
        file_path: path,
        source: text,
        ast,
    };
    let modules = std::slice::from_ref(&module);
    let imports = crate::modules::collect_rust_dependency_uses(&module, false);
    assert!(
        imports.is_empty(),
        "the canonical collector deliberately omits path companions"
    );
    let resolved = resolve_reachable_dependencies(Some(&manifest), &imports, true, &Default::default())
        .map_err(|errors| format!("Web dependencies: {errors:?}"))?;
    assert!(resolved.dependencies.is_empty());
    let queries = crate::build::library_exports::collect_ordinary_library_rust_abi_query_paths(
        modules,
        &crate::build::rust_extern::collect_rust_extern_contexts(modules),
        session.provider_plan(),
    )?;
    assert_eq!(queries, ["incan_web_macros::route"]);

    // ---- Genuine native family and production session-to-inspection projection ----
    let output = tempfile::tempdir()?;
    let native = Arc::new(OrdinaryLibraryNativeProfiles::prepare(OrdinaryLibraryNativeRequest {
        support: Arc::new(CompilerSupportSources::discover()?.ok_or("original support missing")?),
        graph: &required_path("INCAN_ORDINARY_LIBRARY_GRAPH")?,
        index: &required_path("INCAN_ORDINARY_LIBRARY_INDEX")?,
        blobs: &required_path("INCAN_ORDINARY_LIBRARY_BLOBS")?,
        output: output.path(),
        rustc: &rustc,
        target: &target,
        profiles: &["debug"],
    })?);
    let unbound = native.inspection_dependencies(&resolved.dependencies, &root)?;
    assert!(
        !unbound
            .iter()
            .any(|dependency| dependency.crate_name == "incan_web_macros")
    );
    let input = AdmittedLibraryPreparation::with_ordinary_native(session, native)?;
    let native = input.ordinary_native().ok_or("original native request lost")?;
    let inspection = native.inspection_dependencies(&resolved.dependencies, &root)?;
    let declared = manifest
        .rust_dependencies()
        .get("incan_web_macros")
        .ok_or("Web companion declaration missing")?;
    let selected = inspection
        .iter()
        .find(|dependency| dependency.crate_name == "incan_web_macros")
        .ok_or("production inspection projection lost Web's original declared macro root")?;
    assert!(incan_provider::requirements::dependency_specs_match(selected, declared));
    assert!(
        !inspection
            .iter()
            .any(|dependency| dependency.crate_name == "foreign_macro")
    );
    let (_, observation) = native.metadata().inspection_request()?;
    let roots = observation
        .graph()
        .select_dependency_roots(&inspection, &root, "target")?;
    let selected = roots
        .iter()
        .find(|root| root.alias == "incan_web_macros")
        .ok_or("declared alias not selected")?;
    let original = observation
        .graph()
        .units()
        .get(&selected.record_identity)
        .ok_or("original native root missing")?;
    assert_eq!(original.record().source.domain, "host");
    assert_eq!(original.record().source.loaf, "incan_web_macros");
    let workspace = inspect(native, &root, output.path(), &inspection, &queries)?;
    let metadata = rust_inspect::RustMetadataCache::new().get_or_extract_complete(
        workspace.manifest_dir(),
        &queries[0],
        &|_| {},
    )?;
    assert!(matches!(
        metadata.kind,
        incan_lang::interop::metadata::RustItemKind::Macro(_)
    ));

    // ---- The same original macro root must reach executable plans ----
    verify_execution_roots(native, &root, declared)?;

    // ---- Names and conflicting declarations cannot replace the original companion ----
    let missing = inspect(
        native,
        &root,
        output.path(),
        &inspection,
        &["foreign_macro::route".into()],
    );
    let error = missing.err().ok_or("undeclared query unexpectedly admitted")?;
    assert!(error.to_string().contains("has no declared native root"), "{error}");
    let mut conflicting = declared.clone();
    conflicting.package = Some("incan_derive".into());
    conflicting.version = None;
    conflicting.source = DependencySource::Path {
        path: root.join("../derive/incan_derive"),
    };
    assert!(native.inspection_dependencies(&[conflicting], &root).is_err());
    source.verify()?;
    workspace.verify_ordinary_native()?;
    println!(
        "ordinary-web-inspection-evidence {}",
        serde_json::json!({
            "query": queries[0], "profile": "debug", "macro_domain": "host", "original_alias_retained": true,
            "undeclared_query_refused": true, "conflicting_companion_refused": true, "sdk_discovered": false,
            "execution_macro_root_retained": true, "runtime_and_execution_roots_match": true,
        })
    );
    Ok(())
}

/// Follow the executable receipt/plan boundary with the actual publisher source and original host macro owner.
fn verify_execution_roots(
    native: &OrdinaryLibraryNativeProfiles,
    root: &Path,
    declared: &DependencySpec,
) -> TestResult {
    // ---- Current source receipt and ordinary execution projection ----
    let request = oven_store::OvenGeneratedProjectRequest::new(
        root,
        "web-native-root-control",
        "0.5.0",
        native.metadata().target(),
        rustc_identity(native.metadata().rustc())?,
        "debug",
        Vec::new(),
    )
    .with_generated_source("publisher-routing", root.join("src/web/routing.incn"));
    let source_receipt = oven_store::receipt_generated_project(&request)?;
    let store = open_default_oven_store()?;
    let (receipt, plan) = native.select_plan(&store, &source_receipt, &[], root)?;
    let macro_artifact = plan
        .artifact_plan()
        .externs
        .iter()
        .find(|(name, _)| name == "incan_web_macros")
        .ok_or("production execution projection lost Web's original declared macro root")?;

    // ---- Explicit original declaration and automatic projection select identical authority ----
    let original = std::slice::from_ref(declared);
    let (_, explicit) = native.select_plan(&store, &source_receipt, original, root)?;
    assert_eq!(
        Some(macro_artifact),
        explicit
            .artifact_plan()
            .externs
            .iter()
            .find(|(name, _)| name == "incan_web_macros")
    );
    let automatic_inputs = native.runtime_inputs(&receipt.intent, &[], &[], &[], &[], root)?;
    let declared_inputs = native.runtime_inputs(&receipt.intent, &[], &[], &[], original, root)?;
    assert_eq!(automatic_inputs, declared_inputs);
    assert_eq!(
        receipt.sources.build_unit_inputs.get("ordinary-native-roots"),
        automatic_inputs.get("ordinary-native-roots")
    );

    // ---- Execution cannot replace the original companion or invent a foreign alias ----
    assert!(
        !plan
            .artifact_plan()
            .externs
            .iter()
            .any(|(name, _)| name == "foreign_macro")
    );
    let mut conflicting = declared.clone();
    conflicting.package = Some("incan_derive".into());
    conflicting.version = None;
    conflicting.source = DependencySource::Path {
        path: root.join("../derive/incan_derive"),
    };
    assert!(
        native
            .select_plan(&store, &source_receipt, &[conflicting], root)
            .is_err()
    );
    native.verify()?;
    Ok(())
}

/// Use the same retained ordinary inspection entry as library publication, without a fabricated source sidecar.
fn inspect(
    native: &OrdinaryLibraryNativeProfiles,
    root: &Path,
    output: &Path,
    dependencies: &[DependencySpec],
    queries: &[String],
) -> TestResult<crate::lock::PreparedRustInspectWorkspace> {
    let metadata = native.metadata();
    let (profile, observation) = metadata.inspection_request()?;
    let resolved = ResolvedDependencies {
        dependencies: dependencies.to_vec(),
        dev_dependencies: Vec::new(),
    };
    let requirements = ProjectRequirements::default();
    let toolchain = rustc_identity(metadata.rustc())?;
    let inputs = BTreeMap::new();
    crate::lock::rust_inspect::prepare_rust_inspect_workspace_with_ordinary_native(
        RustInspectWorkspaceRequest {
            project_root: root,
            project_name: "incan_stdlib_web",
            cargo_package_name: "incan_stdlib_web",
            rust_edition: Some("2024".into()),
            resolved: &resolved,
            project_requirements: &requirements,
            lock_payload: None,
            cargo_lock_projection_root: None,
            clear_cargo_lock: false,
            cargo_policy_flags: Vec::new(),
            cargo_target_dir: output,
            rust_inspect_query_paths: queries,
            rust_derive_probe_paths: &[],
            prepare_when_empty: true,
            direct_oven_inspection: true,
            force_direct_prewarm: false,
            oven_source_authority: Some(OvenRustInspectSourceAuthorityRequest {
                project_version: "0.5.0",
                target: metadata.target(),
                toolchain: &toolchain,
                profile,
                features: &[],
                build_unit_inputs: &inputs,
                registry_dependencies: dependencies,
            }),
            prepared_project_source_authorities: None,
            explicit_oven_bake: false,
        },
        observation.clone(),
        metadata.rustc(),
        Some(Arc::clone(metadata.inspection_toolchain())),
    )?
    .ok_or_else(|| "retained Web workspace missing".into())
}
