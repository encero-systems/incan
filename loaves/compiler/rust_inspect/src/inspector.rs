//! The rust-analyzer-backed inspector: eager extraction (`prewarm`) separated from cache-only reads (`get`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use incan_core::interop::RustItemMetadata;

use crate::cache::RustMetadataCache;
use crate::error::RustMetadataError;

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
    /// Configure an inspector over the generated workspace whose `Cargo.toml` lives in `manifest_dir`.
    pub fn new(manifest_dir: impl Into<PathBuf>) -> Self {
        Self {
            manifest_dir: manifest_dir.into(),
        }
    }

    /// The generated workspace directory this inspector loads and caches metadata for.
    pub fn manifest_dir(&self) -> &Path {
        self.manifest_dir.as_path()
    }
}

/// The rust-analyzer-backed inspector: eager extraction through `prewarm`, cache-only reads through `get`.
pub struct Inspector {
    config: InspectorConfig,
    cache: RustMetadataCache,
}

impl Inspector {
    /// Read a presence-and-truthiness flag (`1`, `true`, `on`) from the environment.
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

/// Whether any type in the item's signature is still `Unknown`, which is what separates a partial prewarm record from
/// complete metadata.
fn metadata_has_unknowns(metadata: &RustItemMetadata) -> bool {
    /// Walk one type shape, including its `Option`, `Ref`, `Result` and collection payloads.
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
