//! Rust-inspect workspace preparation (feature `rust_inspect`): the prewarm queue coalesces and keeps follow-up
//! requests, and the LSP workspace combines inline Rust imports with provider-derived requirements under a test-owned
//! generated-cache root.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{
    LspRustInspectContext, PrewarmQueueEntry, enqueue_prewarm_paths,
    prepare_lsp_rust_inspect_workspace_with_target_resolver, take_next_prewarm_batch,
};
use incan_driver::generated_cache::resolve_generated_cargo_target_in_cache_root;
use incan_driver::session::CompilationSession;
use incan_frontend::library_manifest_index::LibraryManifestIndex;
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::{lexer, parser};
use incan_provider::ProviderPlan;
use oven_model::manifest::ProjectManifest;

#[cfg(all(test, feature = "rust_inspect"))]
/// Prepare an LSP rust-inspect workspace against a test-owned generated-cache root.
fn prepare_lsp_rust_inspect_workspace_in_cache_root(
    manifest: &ProjectManifest,
    modules: &[ParsedModule],
    library_manifest_index: &LibraryManifestIndex,
    provider_plan: &ProviderPlan,
    cache_root: &Path,
) -> std::result::Result<LspRustInspectContext, String> {
    prepare_lsp_rust_inspect_workspace_with_target_resolver(
        manifest,
        modules,
        library_manifest_index,
        provider_plan,
        |cargo_package_name, lock_payload, cargo_features, cargo_flags| {
            resolve_generated_cargo_target_in_cache_root(
                cache_root,
                manifest.project_root(),
                cargo_package_name,
                "rust-inspect",
                lock_payload,
                cargo_features,
                cargo_flags,
            )
        },
    )
}

#[test]
fn prewarm_queue_coalesces_followup_requests_for_same_workspace() {
    let mut queue = HashMap::<PathBuf, PrewarmQueueEntry>::new();
    let root = PathBuf::from("/tmp/project");
    assert!(enqueue_prewarm_paths(
        &mut queue,
        &root,
        vec!["a::f".to_string(), "b::g".to_string()]
    ));
    assert!(!enqueue_prewarm_paths(
        &mut queue,
        &root,
        vec!["b::g".to_string(), "c::h".to_string()]
    ));

    let first = take_next_prewarm_batch(&mut queue, &root);
    assert_eq!(
        first,
        Some(vec!["a::f".to_string(), "b::g".to_string(), "c::h".to_string()])
    );
    assert!(take_next_prewarm_batch(&mut queue, &root).is_none());
    assert!(!queue.contains_key(&root));
}

#[test]
fn prewarm_queue_keeps_new_paths_arriving_while_worker_active() {
    let mut queue = HashMap::<PathBuf, PrewarmQueueEntry>::new();
    let root = PathBuf::from("/tmp/project2");
    assert!(enqueue_prewarm_paths(&mut queue, &root, vec!["a::f".to_string()]));
    let first = take_next_prewarm_batch(&mut queue, &root);
    assert_eq!(first, Some(vec!["a::f".to_string()]));

    assert!(!enqueue_prewarm_paths(&mut queue, &root, vec!["z::k".to_string()]));
    let second = take_next_prewarm_batch(&mut queue, &root);
    assert_eq!(second, Some(vec!["z::k".to_string()]));
    assert!(take_next_prewarm_batch(&mut queue, &root).is_none());
    assert!(!queue.contains_key(&root));
}

#[test]
/// Prove LSP Rust inspection combines inline Rust imports with provider-derived implementation requirements.
fn lsp_rust_inspect_workspace_includes_resolved_inline_and_stdlib_requirements()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let manifest_path = tmp.path().join("loaf.toml");
    std::fs::write(&manifest_path, "[project]\nname = \"demo\"\n")?;
    let manifest = ProjectManifest::from_str("[project]\nname = \"demo\"\n", &manifest_path)?;

    let source = r#"
import std.serde.json
from rust::serde import Serialize

def use_it(x: Serialize) -> None:
  pass
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse_with_context(&tokens, Some("src/main.incn"), Some(&std::collections::HashMap::new()))
        .map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let module = ParsedModule {
        name: "main".to_string(),
        path_segments: vec!["main".to_string()],
        file_path: tmp.path().join("src").join("main.incn"),
        source: source.to_string(),
        ast,
    };
    std::fs::create_dir_all(tmp.path().join("src"))?;
    std::fs::write(&module.file_path, source)?;
    let session = CompilationSession::discover_with_feature_selection(&module.file_path, &Default::default())?;
    let provider_plan = session.provider_plan_for_modules(std::slice::from_ref(&module))?;

    let cache_root = tmp.path().join("managed-cache");
    let context = prepare_lsp_rust_inspect_workspace_in_cache_root(
        &manifest,
        &[module],
        &LibraryManifestIndex::default(),
        &provider_plan,
        &cache_root,
    )
    .map_err(std::io::Error::other)?;
    assert!(context.target_dir.starts_with(&cache_root));
    assert!(context.typecheck_lease.is_some());
    assert!(context.prewarm_lease.is_some());
    let cargo_toml = std::fs::read_to_string(context.manifest_dir.join("Cargo.toml"))?;
    let cargo_config = std::fs::read_to_string(context.manifest_dir.join(".cargo/config.toml"))?;

    assert!(
        cargo_toml.contains("serde"),
        "expected inline rust import dependency in generated Cargo.toml, got:\n{cargo_toml}"
    );
    // `std.serde.json` is implemented by the data component and the Rust facet it declares; the Cargo
    // feature that once spelled `json` no longer exists.
    assert!(
        cargo_toml.contains("incan_stdlib_data") && cargo_toml.contains("incan_std_data"),
        "expected provider implementation facts in generated Cargo.toml, got:\n{cargo_toml}"
    );
    assert!(
        cargo_config.contains(context.target_dir.to_string_lossy().as_ref()),
        "expected LSP rust-inspect Cargo output to use its leased target, got:\n{cargo_config}"
    );
    Ok(())
}
