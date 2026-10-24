//! Cache-only Rust facts for clients that cannot link rustc, including the language server (DD-0009).
//!
//! The unit's shipped ABI cache identifies its installed toolchain. That toolchain's driver extracts the declared
//! Rust externs once; subsequent sessions read the resulting cache without a fallback extractor in the client.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use incan_lang::interop::{RUST_ABI_CACHE_VERSION, RustAbiCache};
use rustix::fs::{FlockOperation, flock};
use serde::Deserialize;
#[cfg(unix)]
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::{CliError, CliResult};
use crate::session::{RustFacts, RustFactsSetup};
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
    let started = std::time::Instant::now();

    // ---- Declared native roots ----
    let mut setup = RustFactsSetup {
        caches: unit.rust_abi.clone(),
        source: None,
    };
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
    if externs.is_empty() {
        return Ok(setup);
    }
    // ---- Shipped namespace coverage ----
    let shipped = unit
        .rust_abi
        .iter()
        .map(|path| RustAbiCache::open(path).map_err(failure))
        .collect::<CliResult<Vec<_>>>()?;
    externs.retain(|name, _| !shipped.iter().any(|cache| cache.covers_crate(name)));
    drop(shipped);
    if externs.is_empty() {
        return Ok(setup);
    }
    // ---- Generation cache ----
    let (directory, toolchain) = installed_toolchain(unit)?;
    let identity = generation_identity(unit, &toolchain, &externs, &directory)?;
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
    let reused = cache.is_file();
    if !reused {
        extract_cache(&toolchain, unit, &externs, &directory, &cache)?;
    }
    if std::env::var_os("INCAN_TRACE_RUST_FACTS").is_some() {
        eprintln!(
            "rust ABI cache: {} {} crates in {:?}",
            if reused { "reused" } else { "compiled" },
            externs.len(),
            started.elapsed()
        );
    }
    setup.caches.push(cache);
    Ok(setup)
}

/// Retain a bounded check-time extraction in the ABI layout, separately from complete-crate caches.
///
/// The exact request and native generation identify the result, including absent paths. These files never enter a
/// unit's shipped cache list and grant no crate-wide coverage. Extraction failures publish nothing; the identity
/// lock and atomic publication prevent concurrent readers from observing incomplete payloads. A standalone unit or
/// an unavailable cache retains ordinary extraction; cache storage is never a prerequisite for checking.
pub fn cached_path_facts(
    unit: &UnitDescription,
    paths: &BTreeSet<String>,
    extract: impl FnOnce() -> Result<RustFacts, String>,
) -> CliResult<RustFacts> {
    let started = std::time::Instant::now();

    // ---- Optional cache availability ----
    let (directory, cache, _lock) = match path_cache_location(unit, paths) {
        Ok(location) => location,
        Err(error) => {
            if std::env::var_os("INCAN_TRACE_RUST_FACTS").is_some() {
                eprintln!("rust facts cache unavailable: {}", error.message);
            }
            return extract().map_err(CliError::failure);
        }
    };
    // ---- Retained facts or genuine extraction ----
    let cached = fs::read(&cache)
        .ok()
        .and_then(|bytes| RustAbiCache::decode(&bytes).and_then(RustAbiCache::into_items).ok());
    let reused = cached.is_some();
    let facts = if let Some(facts) = cached {
        facts
    } else {
        let facts = extract().map_err(CliError::failure)?;
        if let Err(error) = persist_path_facts(&directory, &cache, &facts)
            && std::env::var_os("INCAN_TRACE_RUST_FACTS").is_some()
        {
            eprintln!("rust facts cache not retained: {}", error.message);
        }
        facts
    };
    // ---- Optional retrieval trace ----
    if std::env::var_os("INCAN_TRACE_RUST_FACTS").is_some() {
        eprintln!(
            "rust facts cache: {} {} paths, {} items in {:?}",
            if reused { "reused" } else { "compiled" },
            paths.len(),
            facts.len(),
            started.elapsed()
        );
    }
    Ok(facts)
}

/// Locate and lock a bounded cache without extracting facts, so setup failures can retain ordinary checking.
fn path_cache_location(unit: &UnitDescription, paths: &BTreeSet<String>) -> CliResult<(PathBuf, PathBuf, File)> {
    let (directory, toolchain) = installed_toolchain(unit)?;
    let externs = unit.externs.iter().map(|native| (&native.name, &native.path)).collect();
    let mut identity = generation_identity(unit, &toolchain, &externs, &directory)?;
    identity.update(b"incan.rust-facts.paths/1\0");
    hash_file(
        &mut identity,
        &std::env::current_exe().map_err(failure)?,
        &directory.join("rust-abi").join("observations"),
    )?;
    identity.update(serde_json::to_vec(paths).map_err(failure)?);
    let directory = directory.join("rust-abi").join("units").join("paths");
    fs::create_dir_all(&directory).map_err(failure)?;
    let cache = directory.join(format!("{}.rustabi", hex::encode(identity.finalize())));
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(cache.with_extension("lock"))
        .map_err(failure)?;
    flock(&lock, FlockOperation::LockExclusive).map_err(failure)?;
    Ok((directory, cache, lock))
}

/// Publish only a complete successful extraction, preserving a previous cache if serialization or writing fails.
fn persist_path_facts(directory: &Path, cache: &Path, facts: &RustFacts) -> CliResult<()> {
    let bytes = incan_lang::interop::encode_rust_abi_cache(facts).map_err(failure)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory).map_err(failure)?;
    temporary.write_all(&bytes).map_err(failure)?;
    temporary.flush().map_err(failure)?;
    temporary.persist(cache).map_err(failure)?;
    Ok(())
}

/// Hash the same exact extractor, shipped facts and native artifact generation for complete and bounded caches.
/// Search paths remain ordered because their order participates in rustc's dependency resolution.
fn generation_identity(
    unit: &UnitDescription,
    toolchain: &Toolchain,
    externs: &BTreeMap<&String, &PathBuf>,
    directory: &Path,
) -> CliResult<Sha256> {
    let observations = directory.join("rust-abi").join("observations");
    fs::create_dir_all(&observations).map_err(failure)?;
    let mut identity = Sha256::new();
    identity.update(RUST_ABI_CACHE_VERSION.to_le_bytes());
    hash_file(&mut identity, &toolchain.driver, &observations)?;
    identity.update(toolchain.sysroot.as_os_str().as_encoded_bytes());
    for cache in &toolchain.rust_abi {
        hash_file(&mut identity, cache, &observations)?;
    }
    for (name, artifact) in externs {
        identity.update(name.as_bytes());
        hash_file(&mut identity, artifact, &observations)?;
    }
    identity.update(serde_json::to_vec(&unit.search).map_err(failure)?);
    Ok(identity)
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

/// Add exactly the content-based identity used before observation reuse. An unchanged native file generation may
/// reuse its verified digest; a changed inode, mode, size, mtime or ctime requires reading the bytes again.
fn hash_file(identity: &mut Sha256, path: &Path, observations: &Path) -> CliResult<()> {
    identity.update(serde_json::to_vec(path).map_err(failure)?);
    #[cfg(unix)]
    let digest = observed_digest(path, observations)?;
    #[cfg(not(unix))]
    let digest = read_digest(path)?;
    identity.update(digest);
    Ok(())
}

/// Read one file's SHA-256 without retaining its potentially large artifact in memory.
fn read_digest(path: &Path) -> CliResult<[u8; 32]> {
    let mut content = Sha256::new();
    let mut file = File::open(path).map_err(|error| CliError::failure(format!("{}: {error}", path.display())))?;
    let mut buffer = [0; 65536];
    loop {
        let length = file.read(&mut buffer).map_err(failure)?;
        if length == 0 {
            return Ok(content.finalize().into());
        }
        content.update(&buffer[..length]);
    }
}

/// Native file generation with nanosecond change detection, matching the immutable-artifact observation boundary.
#[cfg(unix)]
#[derive(Deserialize, Serialize, PartialEq, Eq)]
struct ArtifactGeneration {
    device: u64,
    inode: u64,
    mode: u32,
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

#[cfg(unix)]
impl ArtifactGeneration {
    /// Observe the target file so retargeting a symlink to another generation invalidates reuse.
    fn read(path: &Path) -> CliResult<Self> {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::metadata(path).map_err(failure)?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
            length: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }

    /// Require a quiet interval before retaining an observation, including on filesystems with coarse clocks.
    fn settled(&self) -> bool {
        let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else {
            return false;
        };
        let Some(cutoff) = now
            .as_secs()
            .checked_sub(2)
            .and_then(|seconds| i64::try_from(seconds).ok())
        else {
            return false;
        };
        self.modified.0 < cutoff && self.changed.0 < cutoff
    }
}

/// A content digest proved for one native file generation; malformed observations are recomputed.
#[cfg(unix)]
#[derive(Deserialize, Serialize)]
struct DigestObservation {
    generation: ArtifactGeneration,
    digest: [u8; 32],
}

/// Reuse a digest only while the exact file generation still matches. Hashing verifies the generation again
/// before atomic publication, so a concurrent replacement cannot attach old bytes to a new observation.
#[cfg(unix)]
fn observed_digest(path: &Path, directory: &Path) -> CliResult<[u8; 32]> {
    // ---- Verified content observation ----
    let generation = ArtifactGeneration::read(path)?;
    let settled = generation.settled();
    let memo = directory.join(format!(
        "{}.json",
        hex::encode(Sha256::digest(serde_json::to_vec(path).map_err(failure)?))
    ));
    if settled
        && let Ok(bytes) = fs::read(&memo)
        && let Ok(observed) = serde_json::from_slice::<DigestObservation>(&bytes)
        && observed.generation == generation
    {
        return Ok(observed.digest);
    }
    // ---- Changed or unsettled generation ----
    let digest = read_digest(path)?;
    if ArtifactGeneration::read(path)? != generation {
        return Err(CliError::failure(format!(
            "Rust artifact changed while hashing: {}",
            path.display()
        )));
    }
    if !settled {
        return Ok(digest);
    }
    // ---- Atomic observation publication ----
    let observed = DigestObservation { generation, digest };
    let mut temporary = tempfile::NamedTempFile::new_in(directory).map_err(failure)?;
    temporary
        .write_all(&serde_json::to_vec(&observed).map_err(failure)?)
        .map_err(failure)?;
    temporary.flush().map_err(failure)?;
    temporary.persist(&memo).map_err(failure)?;
    Ok(digest)
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
    incan_lang::interop::RustAbiCache::open(temporary.path()).map_err(failure)?;
    temporary.flush().map_err(failure)?;
    temporary.persist(cache).map_err(failure)?;
    Ok(())
}

/// Preserve an I/O or serialization failure as an actionable client setup error.
fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(format!("cannot prepare Rust ABI cache: {error}"))
}
