//! Retain validated manifest facts under exact source and compiler file-generation guards. Missing, malformed,
//! stale or unwritable private records fall back to ordinary JSON admission, never to acceptance of missing facts.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use sha2::{Digest, Sha256};

use super::manifest_input::{self, Generation, generation};
use crate::library_manifest::LibraryManifest;

const LIMIT: u64 = 128 * 1024 * 1024;

/// Read admitted facts once for an unchanged file generation, or enter the ordinary public manifest reader.
pub(super) fn read(path: &Path) -> Result<(Arc<LibraryManifest>, String), String> {
    let canonical =
        fs::canonicalize(path).map_err(|error| format!("cannot resolve Loaf manifest {}: {error}", path.display()))?;
    let state = fs::metadata(&canonical).ok().and_then(|metadata| generation(&metadata));
    let compiler = compiler_generation();
    let key = format!("{:x}", Sha256::digest(canonical.as_os_str().as_encoded_bytes()));
    let cache = manifest_input::cache_path(&key).map(|path| path.with_file_name(format!("{key}.facts.crumb")));
    if let (Some(state), Some(compiler), Some(cache)) = (state, compiler, cache.as_deref())
        && let Some(retained) = load(cache, &canonical, state, compiler)
    {
        tracing::debug!(target: "incan::invocation", cached = true, "decoded Loaf facts");
        return Ok(retained);
    }
    tracing::debug!(target: "incan::invocation", cached = false, "decoded Loaf facts");
    let (content, digest) =
        manifest_input::read(path).map_err(|error| format!("cannot read Loaf manifest {}: {error}", path.display()))?;
    let manifest = LibraryManifest::from_json_str(&content).map_err(|error| error.to_string())?;
    if let (Some(state), Some(compiler), Some(cache)) = (state, compiler, cache.as_deref())
        && fs::metadata(path).ok().and_then(|metadata| generation(&metadata)) == Some(state)
    {
        let _ = store(cache, &canonical, state, compiler, &manifest, &digest);
    }
    Ok((Arc::new(manifest), digest))
}

/// Bind the private encoding and admission rules to this exact running compiler, once per process.
pub(super) fn compiler_generation() -> Option<&'static (PathBuf, Generation)> {
    static COMPILER: OnceLock<Option<(PathBuf, Generation)>> = OnceLock::new();
    COMPILER
        .get_or_init(|| {
            let path = std::env::current_exe().ok()?.canonicalize().ok()?;
            let state = generation(&fs::metadata(&path).ok()?)?;
            Some((path, state))
        })
        .as_ref()
}

/// Refuse an altered record or source path before decoding retained semantic facts; corruption is a cache miss.
fn load(
    cache: &Path,
    path: &Path,
    state: Generation,
    compiler: &(PathBuf, Generation),
) -> Option<(Arc<LibraryManifest>, String)> {
    let (digest, payload) = manifest_input::read_cache(
        cache,
        "facts",
        manifest_input::checked_guard(path, state, compiler),
        LIMIT,
    )?;
    let manifest = LibraryManifest::decode_retained(&payload)
        .map_err(|error| {
            tracing::debug!(target: "incan::invocation", %error, path = %path.display(), "decoded facts cache refused");
            error
        })
        .ok()?;
    if fs::metadata(path).ok().and_then(|metadata| generation(&metadata)) != Some(state) {
        return None;
    }
    Some((Arc::new(manifest), digest))
}

/// Publish an optional complete record atomically after ordinary JSON admission, preserving concurrent readers.
fn store(
    cache: &Path,
    path: &Path,
    state: Generation,
    compiler: &(PathBuf, Generation),
    manifest: &LibraryManifest,
    digest: &str,
) -> io::Result<()> {
    let payload = manifest.encode_retained().map_err(io::Error::other)?;
    if LibraryManifest::decode_retained(&payload).map_err(io::Error::other)? != *manifest {
        return Err(io::Error::other(
            "decoded manifest facts do not preserve the admitted contract",
        ));
    }
    manifest_input::write_cache(
        cache,
        "facts",
        manifest_input::checked_guard(path, state, compiler),
        digest,
        payload,
    )
}
