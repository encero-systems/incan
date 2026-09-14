//! Rust inspect metadata extraction on top of a generated Cargo workspace (typically `target/incan_lock`).
//!
//! The `Inspector` API separates eager extraction (`prewarm`) from cache-only reads (`get`) so compiler hot paths can
//! remain extraction-free.
//!
//! This crate is a toolchain-locked compiler subsystem. It is responsible for staged Rust interop preparation and
//! metadata cache access, not for ambient semantic analysis. Callers should make workspace loading and extraction
//! explicit before typechecking/codegen paths ask for cached metadata.

#[cfg(feature = "inspector")]
mod cache;
#[cfg(feature = "inspector")]
mod cache_resolve;
#[cfg(feature = "inspector")]
mod cache_timing;
mod digest;
mod digest_tokens;
#[cfg(feature = "inspector")]
mod error;
#[cfg(feature = "inspector")]
mod extractor;
#[cfg(feature = "inspector")]
mod generic_params;
#[cfg(feature = "inspector")]
mod inspector;
#[cfg(feature = "inspector")]
mod loader;
#[cfg(feature = "inspector")]
mod mir_digest;
#[cfg(feature = "inspector")]
mod receiver_contract;

#[cfg(feature = "inspector")]
pub use cache::RustMetadataCache;
pub use digest::{
    RustDigestError, RustDigestItemKind, RustItemDigest, RustItemDigestKey, RustSourceDigest, digest_rust_source,
};
#[cfg(feature = "inspector")]
pub use error::RustMetadataError;
#[cfg(feature = "inspector")]
pub use extractor::{extract_rust_item, rust_type_implements_trait};
#[cfg(feature = "inspector")]
pub use inspector::{Fidelity, InspectError, InspectResult, Inspector, InspectorConfig};
#[cfg(feature = "inspector")]
pub use loader::{
    GeneratedOutDirRecord, OVEN_CARGO_BOOTSTRAP_INSPECTION_MARKER, OVEN_DIRECT_INSPECTION_AUTHORITY_FILE,
    OVEN_DIRECT_INSPECTION_MARKER, OvenInspectionRegistrySource, RustWorkspace, SealedGeneratedOutDir,
    oven_inspection_registry_source_roots, read_generated_out_dirs_map, write_generated_out_dirs_map,
    write_oven_generated_out_dirs, write_oven_inspection_source_authority,
    write_sealed_oven_inspection_source_authority,
};
#[cfg(feature = "inspector")]
pub use mir_digest::{MirDigestError, function_body_digest};

#[cfg(all(test, feature = "inspector"))]
mod tests {
    use std::fs;
    use std::sync::Mutex;

    use incan_core::interop::{RustItemKind, RustItemMetadata, RustTypeInfo, RustVisibility};

    use super::*;

    fn dummy_type_metadata(path: &str) -> RustItemMetadata {
        RustItemMetadata {
            canonical_path: path.to_string(),
            definition_path: None,
            visibility: RustVisibility::Public,
            kind: RustItemKind::Type(RustTypeInfo {
                type_params: Vec::new(),
                type_param_defaults: Vec::new(),
                mutable_reference_type_params: Vec::new(),
                expanded_derive_traits: Vec::new(),
                has_const_params: false,
                alias_target: None,
                metadata_completeness: Default::default(),
                methods: Vec::new(),
                implemented_traits: Vec::new(),
                fields: Vec::new(),
                variants: Vec::new(),
            }),
        }
    }

    #[test]
    fn prewarm_reports_deduped_progress_without_forcing_callers_to_probe() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        let inspector = Inspector::new(InspectorConfig::new(tmp.path()));
        inspector
            .cache()
            .insert_test_item(tmp.path(), dummy_type_metadata("demo::Thing"))?;
        let messages = Mutex::new(Vec::new());

        inspector.prewarm(vec!["demo::Thing".to_string(), "demo::Thing".to_string()], &|message| {
            if let Ok(mut messages) = messages.lock() {
                messages.push(message);
            }
        })?;

        let messages = messages
            .into_inner()
            .map_err(|_| std::io::Error::other("progress message lock poisoned"))?;
        assert!(
            messages
                .iter()
                .any(|message| message == "rust-inspect prewarm start: 1 item(s)"),
            "expected observable prewarm start message, got {messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|message| message == "rust-inspect prewarm item 1/1: demo::Thing"),
            "expected observable prewarm item message, got {messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|message| message.starts_with("rust-inspect prewarm complete:")),
            "expected observable prewarm completion message, got {messages:?}"
        );
        assert!(
            messages.iter().any(|message| {
                message.starts_with("rust-inspect prewarm complete:")
                    && message.contains("warmed=0")
                    && message.contains("reused=1")
                    && message.contains("skipped=0")
            }),
            "expected prewarm completion to distinguish cache reuse from extraction, got {messages:?}"
        );
        assert!(
            messages.iter().all(|message| !message.contains("item 2/")),
            "prewarm progress should report deduped work, got {messages:?}"
        );
        Ok(())
    }

    #[test]
    fn prewarm_reports_disk_reuse_for_synthetic_metadata_fixture() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::create_dir_all(tmp.path().join("src"))?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"rust-inspect-heavy-synthetic\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        fs::write(
            tmp.path().join("src/lib.rs"),
            r#"pub mod bridge {
    pub struct Model0;
    pub struct Model1;
    pub struct Model2;
    pub struct Model3;
}
"#,
        )?;
        let queries = (0..4)
            .map(|idx| format!("rust_inspect_heavy_synthetic::bridge::Model{idx}"))
            .collect::<Vec<_>>();

        let cold_messages = Mutex::new(Vec::new());
        Inspector::new(InspectorConfig::new(tmp.path())).prewarm(queries.clone(), &|message| {
            if let Ok(mut messages) = cold_messages.lock() {
                messages.push(message);
            }
        })?;
        let cold_messages = cold_messages
            .into_inner()
            .map_err(|_| std::io::Error::other("cold progress message lock poisoned"))?;
        assert!(
            cold_messages.iter().any(|message| {
                message.starts_with("rust-inspect prewarm complete:")
                    && message.contains("warmed=4")
                    && message.contains("reused=0")
                    && message.contains("skipped=0")
            }),
            "expected cold synthetic prewarm to extract all items, got {cold_messages:?}"
        );

        let warm_messages = Mutex::new(Vec::new());
        Inspector::new(InspectorConfig::new(tmp.path())).prewarm(queries, &|message| {
            if let Ok(mut messages) = warm_messages.lock() {
                messages.push(message);
            }
        })?;
        let warm_messages = warm_messages
            .into_inner()
            .map_err(|_| std::io::Error::other("warm progress message lock poisoned"))?;
        assert!(
            warm_messages.iter().any(|message| {
                message.starts_with("rust-inspect prewarm complete:")
                    && message.contains("warmed=0")
                    && message.contains("reused=4")
                    && message.contains("skipped=0")
            }),
            "expected warm synthetic prewarm to reuse persisted metadata, got {warm_messages:?}"
        );
        Ok(())
    }
}
