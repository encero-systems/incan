//! Retain validated manifest facts under exact source and compiler file-generation guards. Missing, malformed,
//! stale or unwritable private records fall back to ordinary JSON admission, never to acceptance of missing facts.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::manifest_input::{self, Generation, generation};
use crate::library_manifest::LibraryManifest;

const VERSION: u32 = 1;
const LIMIT: u64 = 128 * 1024 * 1024;

/// Demand-independent file identity at the front of one decoded record; the checksum binds its facts payload too.
#[derive(Serialize, Deserialize)]
struct Header {
    version: u32,
    path: PathBuf,
    generation: Generation,
    compiler: (PathBuf, Generation),
    digest: String,
    checksum: String,
}

/// Read admitted facts once for an unchanged file generation, or enter the ordinary public manifest reader.
pub(super) fn read(path: &Path) -> Result<(Arc<LibraryManifest>, String), String> {
    let canonical = fs::canonicalize(path).map_err(|error| error.to_string())?;
    let state = fs::metadata(&canonical).ok().and_then(|metadata| generation(&metadata));
    let compiler = compiler_generation();
    let key = format!("{:x}", Sha256::digest(canonical.as_os_str().as_encoded_bytes()));
    let cache = manifest_input::cache_path(&key).map(|path| path.with_extension("facts"));
    if let (Some(state), Some(compiler), Some(cache)) = (state, compiler, cache.as_deref())
        && let Some(retained) = load(cache, &canonical, state, compiler)
    {
        tracing::debug!(target: "incan::invocation", cached = true, "decoded Loaf facts");
        return Ok(retained);
    }
    tracing::debug!(target: "incan::invocation", cached = false, "decoded Loaf facts");
    let (content, digest) = manifest_input::read(path).map_err(|error| error.to_string())?;
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
    let metadata = fs::symlink_metadata(cache).ok()?;
    if !metadata.is_file() || metadata.len() > LIMIT {
        return None;
    }
    let mut file = BufReader::new(File::open(cache).ok()?);
    let mut header = Vec::new();
    file.by_ref().take(8192).read_until(b'\n', &mut header).ok()?;
    let header: Header = serde_json::from_slice(&header).ok()?;
    if header.version != VERSION || header.path != path || header.generation != state || &header.compiler != compiler {
        return None;
    }
    let mut payload = Vec::new();
    file.take(LIMIT).read_to_end(&mut payload).ok()?;
    if header.checksum != checksum(&header, &payload).ok()? {
        return None;
    }
    let manifest = LibraryManifest::decode_retained(&payload)
        .map_err(|error| {
            tracing::debug!(target: "incan::invocation", %error, path = %path.display(), "decoded facts cache refused");
            error
        })
        .ok()?;
    if fs::metadata(path).ok().and_then(|metadata| generation(&metadata)) != Some(state) {
        return None;
    }
    Some((Arc::new(manifest), header.digest))
}

/// Bind every header field except its own checksum to the exact decoded payload bytes.
fn checksum(header: &Header, payload: &[u8]) -> io::Result<String> {
    let mut hash = Sha256::new();
    hash.update(
        serde_json::to_vec(&(
            header.version,
            &header.path,
            header.generation,
            &header.compiler,
            &header.digest,
        ))
        .map_err(io::Error::other)?,
    );
    hash.update(payload);
    Ok(format!("{:x}", hash.finalize()))
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
    let mut header = Header {
        version: VERSION,
        path: path.to_owned(),
        generation: state,
        compiler: compiler.clone(),
        digest: digest.to_owned(),
        checksum: String::new(),
    };
    header.checksum = checksum(&header, &payload)?;
    let mut bytes = serde_json::to_vec(&header).map_err(io::Error::other)?;
    bytes.push(b'\n');
    bytes.extend_from_slice(&payload);
    if u64::try_from(bytes.len()).map_err(io::Error::other)? > LIMIT {
        return Ok(());
    }
    fs::create_dir_all(
        cache
            .parent()
            .ok_or_else(|| io::Error::other("decoded record has no parent"))?,
    )?;
    let temporary = cache.with_extension(format!("{}.pending", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary)?;
    let result = file.write_all(&bytes).and_then(|()| fs::rename(&temporary, cache));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
