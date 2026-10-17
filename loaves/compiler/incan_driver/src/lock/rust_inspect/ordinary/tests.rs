//! Genuine native publication and Rust metadata controls for ordinary retained inspection.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use incan_provider::dependency_resolver::ResolvedDependencies;
use incan_provider::requirements::ProjectRequirements;
use oven_model::manifest::{DependencySource, DependencySpec};
use oven_rustc::native_loaf::{NativeLoafFacet, NativeLoafPreparationRequest, prepare_native_loafs};
use oven_rustc::rustc::{resolve_active_rustc, rustc_host_target, rustc_identity};
use rust_inspect::{RustMetadataCache, RustWorkspace};

use crate::lock::{OvenRustInspectSourceAuthorityRequest, RustInspectWorkspaceRequest};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Real Rust Loafs, including a unit outside the consumer's declared roots, and an explicit compiler request.
struct Fixture {
    root: tempfile::TempDir,
    rustc: PathBuf,
    target: String,
    toolchain: String,
    lock: PathBuf,
    output: PathBuf,
    facets: Vec<NativeLoafFacet>,
    dependency_alias: String,
}

impl Fixture {
    /// Create isolated authored inputs without a registry, SDK inventory or fabricated native artifact.
    fn new() -> TestResult<Self> {
        let root = tempfile::tempdir()?;
        let lock = root.path().join("lock.json");
        fs::write(&lock, r#"{"schema":"incan.oven.loaf-resolution/2","units":[]}"#)?;
        for name in ["probe_leaf", "unselected_leaf", "helper_leaf"] {
            let project = root.path().join(name);
            fs::create_dir_all(project.join("src"))?;
            fs::write(
                project.join("loaf.toml"),
                format!(
                    "[project]\nname='{name}'\nversion='1.0.0'\n[rust]\nname='{}'\ntype='lib'\nedition='2024'\n{}",
                    if name == "helper_leaf" {
                        "helper_leaf"
                    } else {
                        "probe_leaf"
                    },
                    if name == "probe_leaf" {
                        "[dependencies]\nhelper_leaf={loaf='helper_leaf',path='../helper_leaf',version='1.0.0'}\n"
                    } else {
                        ""
                    },
                ),
            )?;
            fs::write(
                project.join("src/lib.rs"),
                if name == "probe_leaf" {
                    "pub fn answer() -> i64 { 42 }\npub fn make() -> helper_leaf::Record { helper_leaf::Record { value: 42 } }\n"
                } else if name == "helper_leaf" {
                    "pub struct Record { pub value: i64 }\n"
                } else {
                    "pub fn answer() -> bool { true }\n"
                },
            )?;
        }
        let rustc = resolve_active_rustc()?;
        let target = rustc_host_target(&rustc)?;
        let toolchain = rustc_identity(&rustc)?;
        let output = root.path().join("native");
        Ok(Self {
            root,
            rustc,
            target,
            toolchain,
            lock,
            output,
            dependency_alias: "probe_leaf".to_string(),
            facets: ["probe_leaf", "unselected_leaf", "helper_leaf"]
                .into_iter()
                .map(|name| NativeLoafFacet {
                    target_predicates: Vec::new(),
                    target: None,
                    project: name.into(),
                    features: Vec::new(),
                    domain: "target".to_string(),
                })
                .collect(),
        })
    }

    /// Supply the complete real producer request, retaining both selected and unselected units.
    fn request(&self) -> NativeLoafPreparationRequest<'_> {
        NativeLoafPreparationRequest {
            lock: &self.lock,
            blobs: self.root.path(),
            output: &self.output,
            rustc: &self.rustc,
            index: self.root.path(),
            index_commit: "0000000000000000000000000000000000000000",
            target: &self.target,
            profile: "debug",
            facet_owner: self.root.path(),
            facets: &self.facets,
        }
    }

    /// Exact authored path dependency declared by the consumer; the unselected unit remains in the full request.
    fn dependencies(&self) -> Vec<DependencySpec> {
        vec![DependencySpec {
            crate_name: self.dependency_alias.clone(),
            version: Some("=1.0.0".to_string()),
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path {
                path: self.root.path().join("probe_leaf"),
            },
            optional: false,
            package: Some("probe_leaf".to_string()),
        }]
    }
}

/// Exercise the command's real ordinary entry rather than loading a saved graph directly.
fn prepare(
    fixture: &Fixture,
    project: &Path,
    observation: oven_rustc::native_loaf::NativeLoafRequestObservation,
    query: &str,
    profile: &str,
    derives: &[String],
    toolchain: Option<Arc<oven_rustc::rustc::OvenRustInspectionToolchain>>,
) -> TestResult<crate::lock::PreparedRustInspectWorkspace> {
    fs::create_dir_all(project)?;
    let dependencies = fixture.dependencies();
    let resolved = ResolvedDependencies {
        dependencies: dependencies.clone(),
        dev_dependencies: Vec::new(),
    };
    let requirements = ProjectRequirements::default();
    let inputs = BTreeMap::new();
    let queries = [query.to_string()];
    Ok(super::super::prepare_rust_inspect_workspace_with_ordinary_native(
        RustInspectWorkspaceRequest {
            project_root: project,
            project_name: "ordinary_inspection_consumer",
            cargo_package_name: "ordinary_inspection_consumer",
            rust_edition: Some("2024".to_string()),
            resolved: &resolved,
            project_requirements: &requirements,
            lock_payload: None,
            cargo_lock_projection_root: None,
            clear_cargo_lock: false,
            cargo_policy_flags: Vec::new(),
            cargo_target_dir: &project.join("inspection-target"),
            rust_inspect_query_paths: &queries,
            rust_derive_probe_paths: derives,
            prepare_when_empty: true,
            direct_oven_inspection: true,
            force_direct_prewarm: false,
            oven_source_authority: Some(OvenRustInspectSourceAuthorityRequest {
                project_version: "1.0.0",
                target: &fixture.target,
                toolchain: &fixture.toolchain,
                profile,
                features: &[],
                build_unit_inputs: &inputs,
                registry_dependencies: &dependencies,
            }),
            prepared_project_source_authorities: None,
            explicit_oven_bake: false,
        },
        observation,
        &fixture.rustc,
        toolchain,
    )?
    .ok_or("ordinary inspection did not return its retained workspace")?)
}

/// A selected same-name crate wins actual metadata queries while the full original request remains retained.
#[test]
fn ordinary_native_source_inspection_retains_original_owners_and_cached_database() -> TestResult {
    let fixture = Fixture::new()?;
    let native = prepare_native_loafs(&fixture.request())?;
    assert_eq!(native.report().compiled.len(), 3);
    let observation = native.request_observation()?;
    let original = observation
        .graph()
        .units()
        .values()
        .next()
        .ok_or("original native owner missing")?;
    let retained = Arc::downgrade(original);
    let repeated = prepare_native_loafs(&fixture.request())?;
    assert!(repeated.report().compiled.is_empty());
    assert_eq!(repeated.report().reused.len(), 3);
    let prepared = prepare(
        &fixture,
        &fixture.root.path().join("consumer"),
        observation.clone(),
        "probe_leaf::answer",
        "debug",
        &[],
        None,
    )?;
    prepared.verify_ordinary_native()?;
    let cache = RustMetadataCache::new();
    let first = cache.get_or_extract_complete(prepared.manifest_dir(), "probe_leaf::answer", &|_| {})?;
    assert_eq!(first.canonical_path, "probe_leaf::answer");
    let incan_lang::interop::metadata::RustItemKind::Function(signature) = &first.kind else {
        return Err("ordinary source did not yield callable metadata".into());
    };
    assert_eq!(signature.return_type, "i64");
    assert!(signature.params.is_empty());
    let second = cache.get_or_extract_complete(prepared.manifest_dir(), "probe_leaf::answer", &|_| {})?;
    assert!(Arc::ptr_eq(&first, &second));
    // A real declared native API exposes a type from its actual forward dependency. Source-only facts cannot
    // authorize that extra root; successful inspection plus complete manifest ABI can retain the checked demand.
    let make = cache.get_or_extract_complete(prepared.manifest_dir(), "probe_leaf::make", &|_| {})?;
    let record = cache.get_or_extract_complete(prepared.manifest_dir(), "helper_leaf::Record", &|_| {})?;
    let consumer = fixture.root.path().join("consumer");
    let declaration = consumer.join("loaf.toml");
    fs::write(
        &declaration,
        "[project]\nname='ordinary'\nversion='1.0.0'\n[dependencies]\nprobe_leaf={loaf='probe_leaf',path='../probe_leaf',version='1.0.0'}\n",
    )?;
    let project = oven_model::manifest::ProjectManifest::load(&declaration)?;
    let source = "from rust::probe_leaf import make\npub def answer() -> int:\n    return 42\n";
    fs::create_dir_all(consumer.join("src"))?;
    fs::write(consumer.join("src/lib.incn"), source)?;
    let tokens = incan_frontend::lexer::lex(source).map_err(|errors| format!("lex: {errors:?}"))?;
    let ast = incan_frontend::parser::parse(&tokens).map_err(|errors| format!("parse: {errors:?}"))?;
    let module = incan_frontend::ParsedModule {
        name: "lib".into(),
        path_segments: vec!["lib".into()],
        file_path: consumer.join("src/lib.incn"),
        source: source.into(),
        ast,
    };
    let queries = ["probe_leaf::make".to_string(), "helper_leaf::Record".to_string()].into();
    let demands = crate::build::library_metadata::requirements::capture_checked_native_demands(
        &project,
        &[module],
        &ProjectRequirements::default(),
        &incan_provider::ProviderPlan::default(),
        &queries,
    )?;
    assert!(demands.require_ordinary_source_inspection(None).is_err());
    let mut checked = incan_frontend::library_manifest::LibraryManifest::new("ordinary", "1.0.0");
    checked.rust_abi =
        incan_frontend::library_manifest::LibraryRustAbi::from_items(vec![(*make).clone(), (*record).clone()]);
    let observed = ["helper_leaf::Record".to_string()].into();
    let captured = demands.clone().with_checked_rust_abi_queries(&checked, &observed)?;
    captured.require_ordinary_source_inspection(None)?;
    let decoded: crate::build::library_metadata::requirements::CheckedNativeDemands =
        serde_json::from_slice(&serde_json::to_vec(&captured)?)?;
    decoded.require_ordinary_source_inspection(None)?;
    checked.rust_abi = None;
    assert!(demands.with_checked_rust_abi_queries(&checked, &observed).is_err());
    prepared.verify_ordinary_native()?;
    let toolchain = prepared
        ._ordinary_toolchain
        .as_ref()
        .ok_or("ordinary compiler sources not retained")?;
    toolchain.verify_compiler(&fixture.rustc, &fixture.target)?;
    assert!(!toolchain.source_root().starts_with(toolchain.sysroot()));
    let frozen: serde_json::Value = serde_json::from_slice(&fs::read(
        prepared
            .manifest_dir()
            .join(rust_inspect::OVEN_DIRECT_LOAF_PROJECT_FILE),
    )?)?;
    assert_eq!(frozen["sysroot_src"], serde_json::json!(toolchain.source_root()));
    let seek = cache.get_or_extract_complete(prepared.manifest_dir(), "std::io::SeekFrom", &|_| {})?;
    assert_eq!(seek.canonical_path, "std::io::SeekFrom");
    let incan_lang::interop::metadata::RustItemKind::Type(info) = &seek.kind else {
        return Err("standard-library source did not yield type metadata".into());
    };
    assert_eq!(info.variants.len(), 3);
    assert_eq!(
        info.metadata_completeness,
        incan_lang::interop::metadata::RustTypeMetadataCompleteness::Complete
    );
    assert!(toolchain.verify_compiler(&fixture.rustc, "wrong-target").is_err());
    let bound = prepare(
        &fixture,
        &fixture.root.path().join("recipe-bound-consumer"),
        observation.clone(),
        "probe_leaf::answer",
        "debug",
        &[],
        Some(Arc::clone(toolchain)),
    )?;
    assert!(Arc::ptr_eq(
        toolchain,
        bound
            ._ordinary_toolchain
            .as_ref()
            .ok_or("recipe-bound source owner lost")?
    ));
    bound.verify_ordinary_native()?;
    cache.invalidate_manifest_dir(bound.manifest_dir())?;
    drop(bound);
    let retained_toolchain = Arc::downgrade(toolchain);
    let reuse = super::OrdinaryInspectionProject {
        toolchain: Arc::clone(toolchain),
        observation: observation.clone(),
        inputs: observation
            .graph()
            .select(&observation.graph().select_dependency_roots(
                &fixture.dependencies(),
                fixture.root.path(),
                "target",
            )?)?
            .inspection_inputs()?,
        roots: observation
            .graph()
            .select_dependency_roots(&fixture.dependencies(), fixture.root.path(), "target")?,
        rustc: fixture.rustc.clone(),
        intent: oven_store::OvenBuildIntent {
            target: fixture.target.clone(),
            toolchain: fixture.toolchain.clone(),
            profile: "debug".to_string(),
            features: Vec::new(),
        },
        manifest_dir: prepared.manifest_dir().to_path_buf(),
        consumer_source: crate::rust_inspect_workspace::direct_rust_inspect_probe_source(&[]),
        native_macros: Vec::new(),
    };
    assert!(cache.prepare_retained_project(
        prepared.manifest_dir(),
        &fixture.root.path().join("reused-inspection-target"),
        Box::new(reuse),
        &|_| {},
    )?);
    assert!(!fixture.root.path().join("reused-inspection-target").exists());
    let refused = RustWorkspace::load_with_options(prepared.manifest_dir(), &|_| {}, false);
    assert!(
        matches!(refused, Err(rust_inspect::RustMetadataError::LoadWorkspace { message, .. }) if message.contains("original retained producer capability"))
    );

    for (name, query, profile, derives) in [
        ("undeclared", "unselected_leaf::answer", "debug", Vec::new()),
        ("wrong-profile", "probe_leaf::answer", "release", Vec::new()),
        (
            "derive",
            "probe_leaf::answer",
            "debug",
            vec!["probe_leaf::UnownedDerive".to_string()],
        ),
    ] {
        assert!(
            prepare(
                &fixture,
                &fixture.root.path().join(name),
                observation.clone(),
                query,
                profile,
                &derives,
                None,
            )
            .is_err()
        );
    }
    // An unselected source edit must invalidate the retained complete request, even though the queried leaf is
    // unchanged.
    let unselected = fixture.root.path().join("unselected_leaf/src/lib.rs");
    let bytes = fs::read(&unselected)?;
    fs::write(&unselected, "pub fn answer() -> i64 { 43 }\n")?;
    assert!(prepared.verify_ordinary_native().is_err());
    fs::write(&unselected, bytes)?;
    prepared.verify_ordinary_native()?;
    let original_source = {
        let sources = observation.graph().inspection_inputs()?;
        sources
            .units()
            .values()
            .next()
            .ok_or("original inspection source missing")?
            .root_module()
            .to_path_buf()
    };
    let moved = original_source.with_extension("retained-original");
    fs::rename(&original_source, &moved)?;
    assert!(prepared.verify_ordinary_native().is_err());
    fs::rename(moved, original_source)?;
    prepared.verify_ordinary_native()?;
    let manifest_dir = prepared.manifest_dir().to_path_buf();
    drop(prepared);
    drop(repeated);
    drop(native);
    drop(observation);
    assert!(retained.upgrade().is_some());
    assert!(retained_toolchain.upgrade().is_some());
    assert!(
        cache
            .get_or_extract_complete(&manifest_dir, "probe_leaf::answer", &|_| {})
            .is_ok()
    );
    cache.invalidate_manifest_dir(&manifest_dir)?;
    assert!(retained.upgrade().is_none());
    assert!(retained_toolchain.upgrade().is_none());
    assert!(
        cache
            .get_or_extract_complete(&manifest_dir, "probe_leaf::answer", &|_| {})
            .is_err()
    );
    Ok(())
}

/// An actual original host derive expands; serialized coordinates cannot replace it, and changed source refuses.
#[test]
fn ordinary_native_source_inspection_expands_owned_host_derive() -> TestResult {
    let mut fixture = Fixture::new()?;
    let macro_root = fixture.root.path().join("derive_probe");
    fs::create_dir_all(macro_root.join("src"))?;
    fs::write(
        macro_root.join("loaf.toml"),
        "[project]\nname='derive_probe'\nversion='1.0.0'\n[rust]\nname='derive_probe'\ntype='proc-macro'\nedition='2024'\n",
    )?;
    fs::write(
        macro_root.join("src/lib.rs"),
        "extern crate proc_macro;\nuse proc_macro::TokenStream;\n#[proc_macro_derive(Generated)]\npub fn generated(_: TokenStream) -> TokenStream {\nmatch \"impl NativeDerived { pub fn generated() -> i64 { 42 } }\".parse() { Ok(tokens) => tokens, Err(_) => TokenStream::new() }\n}\n",
    )?;
    let leaf = fixture.root.path().join("probe_leaf");
    fs::write(
        leaf.join("loaf.toml"),
        "[project]\nname='probe_leaf'\nversion='1.0.0'\n[rust]\nname='probe_leaf'\ntype='lib'\nedition='2024'\n[dependencies]\nderive_probe={loaf='derive_probe',path='../derive_probe',version='1.0.0'}\n",
    )?;
    fs::write(
        leaf.join("src/lib.rs"),
        "pub use derive_probe::Generated;\n#[derive(Generated)]\npub struct NativeDerived;\n",
    )?;
    fixture.facets.push(NativeLoafFacet {
        target_predicates: Vec::new(),
        target: None,
        project: "derive_probe".into(),
        features: Vec::new(),
        domain: "host".into(),
    });
    let native = prepare_native_loafs(&fixture.request())?;
    let observation = native.request_observation()?;
    let prepared = prepare(
        &fixture,
        &fixture.root.path().join("macro-consumer"),
        observation.clone(),
        "probe_leaf::NativeDerived",
        "debug",
        &["probe_leaf::Generated".into()],
        None,
    )?;
    let cache = RustMetadataCache::new();
    let metadata = cache.get_or_extract_complete(prepared.manifest_dir(), "probe_leaf::NativeDerived", &|_| {})?;
    let incan_lang::interop::metadata::RustItemKind::Type(info) = &metadata.kind else {
        return Err("derived native type metadata missing".into());
    };
    assert!(info.methods.iter().any(|method| method.name == "generated"), "{info:?}");
    let consumer_source = prepared.manifest_dir().join("src/main.rs");
    let original_source = fs::read(&consumer_source)?;
    fs::write(&consumer_source, "fn main() {}\n")?;
    let refused = cache.get_or_extract_complete(prepared.manifest_dir(), "probe_leaf::NativeDerived", &|_| {});
    assert!(
        refused.is_err(),
        "removing admitted derive probes must invalidate retained inspection"
    );
    fs::write(&consumer_source, original_source)?;
    let prepared = prepare(
        &fixture,
        &fixture.root.path().join("macro-consumer"),
        observation.clone(),
        "probe_leaf::NativeDerived",
        "debug",
        &["probe_leaf::Generated".into()],
        None,
    )?;
    let authority = prepared
        .manifest_dir()
        .join(rust_inspect::OVEN_DIRECT_PROC_MACRO_AUTHORITY_FILE);
    let mut changed: serde_json::Value = serde_json::from_slice(&fs::read(&authority)?)?;
    changed["macros"][0]["digest"] = serde_json::json!("sha256:unowned");
    fs::write(&authority, serde_json::to_vec(&changed)?)?;
    assert!(
        cache
            .get_or_extract_complete(prepared.manifest_dir(), "probe_leaf::NativeDerived", &|_| {})
            .is_err(),
        "changed sidecar invalidates the database and cannot reload original native owners"
    );
    let fresh = prepare(
        &fixture,
        &fixture.root.path().join("fresh-macro-consumer"),
        observation,
        "probe_leaf::NativeDerived",
        "debug",
        &["probe_leaf::Generated".into()],
        None,
    )?;
    let current = cache.get_or_extract_complete(fresh.manifest_dir(), "probe_leaf::NativeDerived", &|_| {})?;
    assert!(
        matches!(&current.kind, incan_lang::interop::metadata::RustItemKind::Type(info)
        if info.methods.iter().any(|method| method.name == "generated"))
    );
    fs::write(macro_root.join("src/lib.rs"), "extern crate proc_macro;\n")?;
    // Cached facts describe immutable admitted native sources. Current authored source is revalidated by the
    // owning preparation at its checker/publication handoff, as in the source-only control above.
    assert!(fresh.verify_ordinary_native().is_err());
    Ok(())
}

/// An invalid source graph used only to prove refusal before rust-analyzer or macro execution can start.
struct UnownedMacroGraph;

impl rust_inspect::RetainedInspectionProject for UnownedMacroGraph {
    /// Return a deliberately unowned nested macro path, never a successful native capability.
    fn verified_project(&self) -> Result<serde_json::Value, rust_inspect::RustMetadataError> {
        Ok(serde_json::json!({"crates": [{"nested": {"proc_macro_dylib_path": null}}]}))
    }
}

/// A retained source request does not grant executable macro authority, even for nested or null paths.
#[test]
fn ordinary_source_loader_refuses_unowned_macro_paths_before_loading() -> TestResult {
    let root = tempfile::tempdir()?;
    let target = root.path().join("never-created");
    let result = RustWorkspace::load_retained_oven_project(root.path(), &target, Box::new(UnownedMacroGraph), &|_| {});
    assert!(
        matches!(result, Err(rust_inspect::RustMetadataError::LoadWorkspace { message, .. }) if message.contains("does not grant native macro execution"))
    );
    assert!(!target.exists());
    let result =
        RustMetadataCache::new().prepare_retained_project(root.path(), &target, Box::new(UnownedMacroGraph), &|_| {});
    assert!(
        matches!(result, Err(rust_inspect::RustMetadataError::LoadWorkspace { message, .. }) if message.contains("projection markers"))
    );
    assert!(!target.exists());
    Ok(())
}

/// Exact target roots and authored aliases win over a same-name host variant without weakening full-request checks.
#[test]
fn ordinary_native_source_inspection_binds_declared_target_with_host_duplicate() -> TestResult {
    let mut fixture = Fixture::new()?;
    let source = fixture.root.path().join("probe_leaf/src/lib.rs");
    fs::write(
        &source,
        "#[cfg(feature = \"host_variant\")] pub fn answer() -> bool { true }\n#[cfg(not(feature = \"host_variant\"))] pub fn answer() -> i64 { 42 }\n",
    )?;
    let manifest = fixture.root.path().join("probe_leaf/loaf.toml");
    fs::write(
        &manifest,
        format!(
            "{}\n[project.features]\nhost_variant=[]\n",
            fs::read_to_string(&manifest)?
        ),
    )?;
    fixture.facets.push(NativeLoafFacet {
        target_predicates: Vec::new(),
        target: None,
        project: "probe_leaf".into(),
        features: vec!["host_variant".to_string()],
        domain: "host".to_string(),
    });
    // The host variant retains the same declared dependency, which therefore needs its own host facet.
    fixture.facets.push(NativeLoafFacet {
        target_predicates: Vec::new(),
        target: None,
        project: "helper_leaf".into(),
        features: Vec::new(),
        domain: "host".to_string(),
    });
    let native = prepare_native_loafs(&fixture.request())?;
    assert_eq!(native.report().compiled.len(), 5);
    let observation = native.request_observation()?;
    assert_eq!(
        observation
            .graph()
            .inspection_inputs()?
            .units()
            .values()
            .filter(|unit| unit.crate_name() == "probe_leaf")
            .count(),
        3,
        "the declared target, its host variant and the unrelated same-name target remain in the full request"
    );
    let cache = RustMetadataCache::new();
    for alias in ["probe_leaf", "renamed_leaf"] {
        fixture.dependency_alias = alias.to_string();
        let query = format!("{alias}::answer");
        let prepared = prepare(
            &fixture,
            &fixture.root.path().join(format!("consumer-{alias}")),
            observation.clone(),
            &query,
            "debug",
            &[],
            None,
        )?;
        let metadata = cache.get_or_extract_complete(prepared.manifest_dir(), &query, &|_| {})?;
        let incan_lang::interop::metadata::RustItemKind::Function(signature) = &metadata.kind else {
            return Err("declared target did not yield callable metadata".into());
        };
        assert_eq!(signature.return_type, "i64");
        let graph: serde_json::Value = serde_json::from_slice(&fs::read(
            prepared
                .manifest_dir()
                .join(rust_inspect::OVEN_DIRECT_LOAF_PROJECT_FILE),
        )?)?;
        let crates = graph["crates"].as_array().ok_or("missing crates")?;
        assert_eq!(crates.len(), 3);
        let consumer_root = crates.last().ok_or("consumer root absent")?;
        assert_eq!(consumer_root["deps"][0]["name"], alias);
        let selected_index = usize::try_from(consumer_root["deps"][0]["crate"].as_u64().ok_or("missing root index")?)?;
        assert_eq!(crates[selected_index]["display_name"], "probe_leaf");
        let consumer = prepared.manifest_dir().join("src/main.rs");
        let bytes = fs::read(&consumer)?;
        fs::write(&consumer, "fn changed() {}\n")?;
        assert!(
            cache
                .get_or_extract_complete(prepared.manifest_dir(), &query, &|_| {})
                .is_err()
        );
        fs::write(&consumer, bytes)?;
        cache.invalidate_manifest_dir(prepared.manifest_dir())?;
    }
    Ok(())
}
