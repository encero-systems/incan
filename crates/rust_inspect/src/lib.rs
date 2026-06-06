//! Rust inspect metadata extraction on top of a generated Cargo workspace (typically `target/incan_lock`).
//!
//! The `Inspector` API separates eager extraction (`prewarm`) from cache-only reads (`get`) so compiler hot paths can
//! remain extraction-free.
//!
//! This crate is a toolchain-locked compiler subsystem. It is responsible for staged Rust interop preparation and
//! metadata cache access, not for ambient semantic analysis. Callers should make workspace loading and extraction
//! explicit before typechecking/codegen paths ask for cached metadata.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use incan_core::interop::RustItemMetadata;

mod cache;
mod cache_resolve;
mod cache_timing;
mod error;
mod extractor;
mod loader;

pub use cache::RustMetadataCache;
pub use error::RustMetadataError;
pub use extractor::extract_rust_item;
pub use loader::RustWorkspace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// How faithfully the returned metadata matches the query path.
pub enum Fidelity {
    /// Exact canonical-path cache hit.
    Exact,
    /// Resolved via a normalized alias (for example underscore/hyphen crate-name mapping).
    Normalized,
    /// Metadata exists, but contains unknown shapes/displays (`?`) and should be treated conservatively.
    Unknown,
}

#[derive(Debug, Clone)]
/// Result wrapper for one metadata lookup.
pub struct InspectResult {
    pub metadata: Arc<RustItemMetadata>,
    pub fidelity: Fidelity,
}

#[derive(Debug, thiserror::Error)]
pub enum InspectError {
    #[error(transparent)]
    Backend(#[from] RustMetadataError),
    #[error("rust-inspect cache miss for `{canonical_path}` (run prewarm first)")]
    MetadataMiss { canonical_path: String },
}

#[derive(Debug, Clone)]
/// Immutable configuration for one [`Inspector`] instance.
pub struct InspectorConfig {
    manifest_dir: PathBuf,
}

impl InspectorConfig {
    pub fn new(manifest_dir: impl Into<PathBuf>) -> Self {
        Self {
            manifest_dir: manifest_dir.into(),
        }
    }

    pub fn manifest_dir(&self) -> &Path {
        self.manifest_dir.as_path()
    }
}

pub struct Inspector {
    config: InspectorConfig,
    cache: RustMetadataCache,
}

impl Inspector {
    fn env_flag_enabled(name: &str) -> bool {
        std::env::var_os(name).is_some_and(|value| {
            let value = value.to_string_lossy();
            matches!(value.as_ref(), "1" | "true" | "TRUE" | "on" | "ON")
        })
    }

    /// Create an inspector bound to one generated lock workspace.
    ///
    /// The workspace should be compiler-managed, usually the generated lock workspace used for Rust interop
    /// preparation rather than the user's live application tree.
    pub fn new(config: InspectorConfig) -> Self {
        Self {
            config,
            cache: RustMetadataCache::new(),
        }
    }

    /// Eagerly extract/cache metadata for the supplied canonical query paths.
    ///
    /// This is the expensive path. Callers should do this in explicit preparation phases rather than semantic hot
    /// loops. A missing item that is stable enough to cache negatively is skipped here so later cache-only lookups can
    /// report the miss without reloading the Rust workspace.
    pub fn prewarm<I>(&self, canonical_paths: I, progress: &(dyn Fn(String) + Sync)) -> Result<(), InspectError>
    where
        I: IntoIterator<Item = String>,
    {
        let debug = Self::env_flag_enabled("INCAN_RUST_INSPECT_PREWARM_DEBUG");
        let started_all = Instant::now();
        let mut warmed = 0usize;
        let mut reused = 0usize;
        let mut skipped = 0usize;
        let mut seen = BTreeSet::new();
        let mut paths = Vec::new();
        for canonical_path in canonical_paths {
            if canonical_path.is_empty() {
                continue;
            }
            if !seen.insert(canonical_path.clone()) {
                continue;
            }
            paths.push(canonical_path);
        }
        if paths.is_empty() {
            return Ok(());
        }
        let total = paths.len();
        progress(format!("rust-inspect prewarm start: {total} item(s)"));
        for (idx, canonical_path) in paths.into_iter().enumerate() {
            let started_item = Instant::now();
            progress(format!(
                "rust-inspect prewarm item {}/{}: {canonical_path}",
                idx + 1,
                total
            ));
            if debug {
                tracing::debug!(query = %canonical_path, "rust-inspect prewarm start");
            }
            match self.cache.get_or_extract_deferred_persist(
                self.config.manifest_dir(),
                canonical_path.as_str(),
                progress,
            ) {
                Ok(access) => {
                    if access.outcome.reused() {
                        reused += 1;
                    } else {
                        warmed += 1;
                    }
                    if debug {
                        tracing::debug!(
                            query = %canonical_path,
                            outcome = ?access.outcome,
                            ms = started_item.elapsed().as_secs_f64() * 1000.0,
                            "rust-inspect prewarm done"
                        );
                    }
                }
                Err(
                    RustMetadataError::CrateNotFound(_)
                    | RustMetadataError::PathNotResolved(_)
                    | RustMetadataError::UnsupportedMacro(_),
                ) => {
                    skipped += 1;
                    if debug {
                        tracing::debug!(
                            query = %canonical_path,
                            ms = started_item.elapsed().as_secs_f64() * 1000.0,
                            "rust-inspect prewarm skip"
                        );
                    }
                }
                Err(err) => return Err(err.into()),
            }
        }
        self.cache.persist_manifest_dir(self.config.manifest_dir())?;
        progress(format!(
            "rust-inspect prewarm complete: warmed={warmed} reused={reused} skipped={skipped} elapsed_ms={:.0}",
            started_all.elapsed().as_secs_f64() * 1000.0
        ));
        if debug {
            tracing::debug!(
                warmed,
                reused,
                skipped,
                total_ms = started_all.elapsed().as_secs_f64() * 1000.0,
                "rust-inspect prewarm summary"
            );
        }
        Ok(())
    }

    /// Read metadata from cache only.
    ///
    /// This method does not trigger rust-analyzer workspace loading/extraction.
    pub fn get(&self, canonical_path: &str) -> Result<InspectResult, InspectError> {
        let lookup = Self::normalize_lookup_path(canonical_path)
            .ok_or_else(|| InspectError::MetadataMiss {
                canonical_path: canonical_path.to_string(),
            })?
            .to_string();
        let Some(hit) = self.cache.get_cached(self.config.manifest_dir(), lookup.as_str())? else {
            return Err(InspectError::MetadataMiss {
                canonical_path: canonical_path.to_string(),
            });
        };
        let mut fidelity = if hit.alias_used {
            Fidelity::Normalized
        } else {
            Fidelity::Exact
        };
        if metadata_has_unknowns(hit.metadata.as_ref()) {
            fidelity = Fidelity::Unknown;
        }
        Ok(InspectResult {
            metadata: hit.metadata,
            fidelity,
        })
    }

    /// Drop all cached state for this manifest directory in-memory.
    pub fn invalidate(&self) -> Result<(), InspectError> {
        self.cache.invalidate_manifest_dir(self.config.manifest_dir())?;
        Ok(())
    }

    /// Access the underlying cache handle.
    pub fn cache(&self) -> &RustMetadataCache {
        &self.cache
    }

    /// Access immutable inspector configuration.
    pub fn config(&self) -> &InspectorConfig {
        &self.config
    }

    /// Normalize a canonical Rust path to the item path used as cache key.
    ///
    /// This strips generic instantiations (`foo::Bar<T>` -> `foo::Bar`) and rejects obviously non-item spellings.
    /// Keep this in one place so all compiler call sites apply identical lookup normalization.
    pub fn normalize_lookup_path(canonical_path: &str) -> Option<&str> {
        let trimmed = canonical_path.trim();
        if trimmed.is_empty() || trimmed == "{unknown}" {
            return None;
        }
        let had_generics = trimmed.contains('<');
        let base = trimmed.split_once('<').map_or(trimmed, |(base, _)| base);
        if had_generics && !base.contains("::") {
            return None;
        }
        if base.is_empty() || base.contains(['{', '}', '(', ')', '[', ']', ',', ' ']) {
            return None;
        }
        Some(base)
    }
}

fn metadata_has_unknowns(metadata: &RustItemMetadata) -> bool {
    fn shape_has_unknown(shape: &incan_core::interop::RustTypeShape) -> bool {
        use incan_core::interop::RustTypeShape;
        match shape {
            RustTypeShape::Unknown => true,
            RustTypeShape::Option(inner) | RustTypeShape::Ref(inner) => shape_has_unknown(inner),
            RustTypeShape::Result(ok, err) => shape_has_unknown(ok) || shape_has_unknown(err),
            RustTypeShape::Tuple(items) => items.iter().any(shape_has_unknown),
            RustTypeShape::RustPath { args, .. } => args.iter().any(shape_has_unknown),
            _ => false,
        }
    }

    match &metadata.kind {
        incan_core::interop::RustItemKind::Function(sig) => {
            sig.params.iter().any(|param| param.type_display.contains('?'))
        }
        incan_core::interop::RustItemKind::Type(info) => {
            info.fields.iter().any(|field| shape_has_unknown(&field.type_shape))
                || info
                    .variants
                    .iter()
                    .flat_map(|variant| variant.fields.iter())
                    .any(shape_has_unknown)
        }
        _ => false,
    }
}

#[cfg(test)]
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
