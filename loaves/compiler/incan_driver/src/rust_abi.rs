//! Cache-only Rust facts for clients that cannot link rustc, including the language server (DD-0009).
//!
//! The unit's shipped ABI cache identifies its installed toolchain. That toolchain's driver extracts the declared
//! Rust externs once; subsequent sessions read the resulting cache without a fallback extractor in the client.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use incan_lang::interop::{RUST_ABI_CACHE_VERSION, RustAbiCache};
use rustix::fs::{FlockOperation, flock};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{CliError, CliResult};
use crate::session::RustFactsSetup;
use crate::unit::UnitDescription;

/// The extraction boundary of an installed toolchain; native inputs still come exclusively from the unit.
#[derive(Deserialize)]
struct Toolchain {
    schema: String,
    driver: PathBuf,
    sysroot: PathBuf,
    rust_abi: Vec<PathBuf>,
}

/// Prepare the unit's Rust facts as caches only, with no in-process rustc source.
///
/// Shipped caches live in `<toolchain>/rust-abi`; their owning description must name the same cache. Only the
/// unit's declared Rust dependencies are extracted, using its exact aliases and transitive search directories.
/// Artifact bytes, driver bytes, sysroot identity, search paths and cache layout version identify the cache, so
/// editing a local Rust dependency at the same path cannot retain stale facts. Failed extractions publish nothing.
pub fn cached_rust_facts(unit: &UnitDescription) -> CliResult<RustFactsSetup> {
    let mut setup = RustFactsSetup {
        caches: unit.rust_abi.clone(),
        source: None,
    };
    let shipped = unit
        .rust_abi
        .iter()
        .map(|path| {
            let bytes = fs::read(path).map_err(|error| CliError::failure(format!("{}: {error}", path.display())))?;
            RustAbiCache::decode(&bytes).map_err(failure)
        })
        .collect::<CliResult<Vec<_>>>()?;
    let mut externs = BTreeMap::new();
    for native in &unit.externs {
        if !unit.rust_crates.contains(&native.name) || matches!(native.name.as_str(), "std" | "core" | "alloc") {
            continue;
        }
        if !native.path.is_file() {
            return Err(CliError::failure(format!(
                "unit's `{}` artifact does not exist: {}",
                native.name,
                native.path.display()
            )));
        }
        if externs.insert(&native.name, &native.path).is_some() {
            return Err(CliError::failure(format!(
                "unit names more than one exact `{}` artifact",
                native.name
            )));
        }
    }
    externs.retain(|name, _| !shipped.iter().any(|cache| cache.covers_crate(name)));
    if externs.is_empty() {
        return Ok(setup);
    }
    let (directory, toolchain) = installed_toolchain(unit)?;
    let mut identity = Sha256::new();
    identity.update(RUST_ABI_CACHE_VERSION.to_le_bytes());
    hash_file(&mut identity, &toolchain.driver)?;
    identity.update(toolchain.sysroot.as_os_str().as_encoded_bytes());
    for cache in &toolchain.rust_abi {
        hash_file(&mut identity, cache)?;
    }
    for (name, artifact) in &externs {
        identity.update(name.as_bytes());
        hash_file(&mut identity, artifact)?;
    }
    identity.update(serde_json::to_vec(&unit.search).map_err(failure)?);
    let directory = directory.join("rust-abi").join("units");
    fs::create_dir_all(&directory).map_err(failure)?;
    let cache = directory.join(format!("{}.rustabi", hex::encode(identity.finalize())));
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(cache.with_extension("lock"))
        .map_err(failure)?;
    flock(&lock, FlockOperation::LockExclusive).map_err(failure)?;
    if !cache.is_file() {
        extract_cache(&toolchain, unit, &externs, &directory, &cache)?;
    }
    setup.caches.push(cache);
    Ok(setup)
}

/// Find the installed toolchain that owns one of the ABI caches Oven supplied, without selecting another toolchain.
fn installed_toolchain(unit: &UnitDescription) -> CliResult<(PathBuf, Toolchain)> {
    for cache in &unit.rust_abi {
        let Some(directory) = cache.parent().and_then(Path::parent) else {
            continue;
        };
        let descriptor = directory.join("toolchain.json");
        if !descriptor.is_file() {
            continue;
        }
        let toolchain: Toolchain = serde_json::from_slice(&fs::read(&descriptor).map_err(failure)?).map_err(failure)?;
        if toolchain.schema != "incan.toolchain/1" || !toolchain.rust_abi.contains(cache) {
            return Err(CliError::failure(format!(
                "{} does not own the unit's ABI cache",
                descriptor.display()
            )));
        }
        return Ok((directory.to_path_buf(), toolchain));
    }
    Err(CliError::failure(
        "the unit has no installed toolchain ABI cache; cannot prepare Rust facts",
    ))
}

/// Add a file's identity without retaining its potentially large artifact in memory.
fn hash_file(identity: &mut Sha256, path: &Path) -> CliResult<()> {
    identity.update(serde_json::to_vec(path).map_err(failure)?);
    let mut content = Sha256::new();
    let mut file = File::open(path).map_err(|error| CliError::failure(format!("{}: {error}", path.display())))?;
    let mut buffer = [0; 65536];
    loop {
        let length = file.read(&mut buffer).map_err(failure)?;
        if length == 0 {
            identity.update(content.finalize());
            return Ok(());
        }
        content.update(&buffer[..length]);
    }
}

/// Ask only the installed driver to extract facts and atomically publish a complete, decodable cache.
fn extract_cache(
    toolchain: &Toolchain,
    unit: &UnitDescription,
    externs: &BTreeMap<&String, &PathBuf>,
    directory: &Path,
    cache: &Path,
) -> CliResult<()> {
    let mut temporary = tempfile::NamedTempFile::new_in(directory).map_err(failure)?;
    let mut command = Command::new(&toolchain.driver);
    command.arg("--rust-abi").arg(&toolchain.sysroot).arg(temporary.path());
    for (name, artifact) in externs {
        command
            .arg(name)
            .arg("--extern")
            .arg(format!("{name}={}", artifact.display()));
    }
    for search in &unit.search {
        command.arg("--search").arg(search);
    }
    let output = command
        .output()
        .map_err(|error| CliError::failure(format!("cannot run {}: {error}", toolchain.driver.display())))?;
    if !output.status.success() {
        return Err(CliError::failure(format!(
            "Rust ABI extraction failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    incan_lang::interop::RustAbiCache::decode(&fs::read(temporary.path()).map_err(failure)?).map_err(failure)?;
    temporary.flush().map_err(failure)?;
    temporary.persist(cache).map_err(failure)?;
    Ok(())
}

/// Preserve an I/O or serialization failure as an actionable client setup error.
fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(format!("cannot prepare Rust ABI cache: {error}"))
}
