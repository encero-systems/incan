//! Cache-only Rust facts for clients that cannot link rustc, including the language server (DD-0009).
//!
//! The unit's shipped ABI cache identifies its installed toolchain. That toolchain's driver extracts the declared
//! Rust externs once; subsequent sessions read the resulting cache without a fallback extractor in the client.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use incan_frontend::rust_abi_cache::{RUST_ABI_CACHE_VERSION, RustAbiCache};
use rustix::fs::{FlockOperation, flock};
use serde::Deserialize;
#[cfg(unix)]
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::{CliError, CliResult};
use crate::session::{RustFacts, RustFactsSetup};
use crate::unit::UnitDescription;

/// The directory under `<toolchain>/rust-abi` that holds each distinct extracted cache once, named by its SHA-256.
const CONTENT_STORE: &str = "objects";

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
    let store = directory.join("rust-abi").join(CONTENT_STORE);
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
        extract_cache(&toolchain, unit, &externs, &directory, &store, &cache)?;
    }
    if incan_frontend::trace_flags::trace_rust_facts() {
        incan_frontend::trace_line!(
            incan_frontend::trace_flags::Category::RustFacts,
            "rust ABI cache: {} {} crates in {:?}",
            if reused { "reused" } else { "compiled" },
            externs.len(),
            started.elapsed()
        );
    }
    setup.caches.push(cache);
    Ok(setup)
}

/// Read or record bounded check-time extractions, keeping installed crates' facts once per toolchain.
///
/// A path rooted in `std`, `core` or `alloc`, or in a crate whose artifact the installed toolchain holds (its registry
/// entries), describes the same items for every unit: its facts depend on the toolchain and that artifact, never on a
/// unit's own dependencies. A unit whose paths are all such paths extracts them once per toolchain, stored in its
/// content store, where every unit and test case that reaches the same paths reads them. An extraction describes the
/// types a signature names only for the crates it names itself, so a unit that also reaches one of its own crates keeps
/// all of its paths in one extraction: the unit's indexed bakery extraction.
///
/// The facts come back as one ABI cache whose items decode when a checker reads them: a reused extraction is its
/// stored bytes, never its items decoded and encoded into a second cache, and a fresh one is encoded once.
pub fn cached_path_facts(
    unit: &UnitDescription,
    paths: &BTreeSet<String>,
    extract: impl Fn(&BTreeSet<String>) -> Result<RustFacts, String>,
) -> CliResult<RustAbiCache> {
    let Ok((directory, toolchain)) = installed_toolchain(unit) else {
        return unit_path_facts(unit, paths, &extract);
    };
    if paths
        .iter()
        .any(|path| installed_artifact(unit, &directory, path).is_none())
    {
        return unit_path_facts(unit, paths, &extract);
    }
    let rust_abi = directory.join("rust-abi");
    match installed_identity(unit, &directory, &toolchain, paths) {
        Ok(identity) => shared_extraction(
            &rust_abi.join("paths"),
            &rust_abi.join(CONTENT_STORE),
            &identity,
            || extract(paths),
        ),
        Err(_) => encoded_cache(&extract(paths).map_err(CliError::failure)?),
    }
}

/// One fresh extraction's encoded cache bytes, written once and read by the session as they are.
fn encoded_extraction(facts: &RustFacts) -> CliResult<Vec<u8>> {
    incan_frontend::rust_abi_cache::encode_rust_abi_cache(facts).map_err(failure)
}

/// One fresh extraction as the encoded cache a session reads.
fn encoded_cache(facts: &RustFacts) -> CliResult<RustAbiCache> {
    RustAbiCache::decode_owned(encoded_extraction(facts)?).map_err(failure)
}

/// A stored extraction's bytes as its cache, when its sections and blocks are valid.
fn stored_cache(bytes: Vec<u8>) -> Option<RustAbiCache> {
    RustAbiCache::decode_owned(bytes).ok()
}

/// The installed artifact a path's root crate is, when the toolchain holds it: `None` for a unit's own crate, and the
/// sysroot (`Some(None)`) for `std`, `core` and `alloc`.
fn installed_artifact(unit: &UnitDescription, directory: &Path, path: &str) -> Option<Option<PathBuf>> {
    let root = path.split("::").next().unwrap_or(path);
    if matches!(root, "std" | "core" | "alloc") {
        return Some(None);
    }
    let native = unit.externs.iter().find(|native| native.name == root)?;
    native.path.starts_with(directory).then(|| Some(native.path.clone()))
}

/// The identity of installed paths' facts: the extracting toolchain, its shipped caches, the root artifacts, the paths.
fn installed_identity(
    unit: &UnitDescription,
    directory: &Path,
    toolchain: &Toolchain,
    paths: &BTreeSet<String>,
) -> CliResult<String> {
    let observations = directory.join("rust-abi").join("observations");
    fs::create_dir_all(&observations).map_err(failure)?;
    let mut identity = Sha256::new();
    identity.update(RUST_ABI_CACHE_VERSION.to_le_bytes());
    identity.update(b"incan.rust-facts.installed-paths/1\0");
    hash_file(&mut identity, &toolchain.driver, &observations)?;
    identity.update(toolchain.sysroot.as_os_str().as_encoded_bytes());
    for cache in &toolchain.rust_abi {
        hash_file(&mut identity, cache, &observations)?;
    }
    hash_file(&mut identity, &std::env::current_exe().map_err(failure)?, &observations)?;
    let roots = paths
        .iter()
        .filter_map(|path| installed_artifact(unit, directory, path).flatten())
        .collect::<BTreeSet<_>>();
    for artifact in roots {
        hash_file(&mut identity, &artifact, &observations)?;
    }
    identity.update(serde_json::to_vec(paths).map_err(failure)?);
    Ok(hex::encode(identity.finalize()))
}

/// Reuse the complete extraction stored under `identity`, or extract it once under the identity's lock and publish it
/// through the content store. A stored file that no longer decodes is extracted again; storage that cannot be used
/// keeps the ordinary extraction, since a cache is never a prerequisite for checking.
fn shared_extraction(
    directory: &Path,
    store: &Path,
    identity: &str,
    extract: impl FnOnce() -> Result<RustFacts, String>,
) -> CliResult<RustAbiCache> {
    let started = std::time::Instant::now();
    let cache = directory.join(format!("{identity}.rustabi"));
    let lock = fs::create_dir_all(directory).ok().and_then(|_| {
        File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(cache.with_extension("lock"))
            .ok()
    });
    let Some(lock) = lock.filter(|lock| flock(lock, FlockOperation::LockExclusive).is_ok()) else {
        return encoded_cache(&extract().map_err(CliError::failure)?);
    };
    let stored = fs::read(&cache).ok().and_then(stored_cache);
    let reused = stored.is_some();
    let facts = match stored {
        Some(facts) => facts,
        None => {
            let bytes = encoded_extraction(&extract().map_err(CliError::failure)?)?;
            if let Err(error) = store_extraction(directory, store, &cache, &bytes)
                && incan_frontend::trace_flags::trace_rust_facts()
            {
                incan_frontend::trace_line!(
                    incan_frontend::trace_flags::Category::RustFacts,
                    "rust facts not shared: {}",
                    error.message
                );
            }
            RustAbiCache::decode_owned(bytes).map_err(failure)?
        }
    };
    drop(lock);
    if incan_frontend::trace_flags::trace_rust_facts() {
        incan_frontend::trace_line!(
            incan_frontend::trace_flags::Category::RustFacts,
            "rust facts cache: {} {} installed items in {:?}",
            if reused { "shared" } else { "compiled" },
            facts.len(),
            started.elapsed()
        );
    }
    Ok(facts)
}

/// Publish one complete encoded extraction under its request name.
fn store_extraction(directory: &Path, store: &Path, cache: &Path, bytes: &[u8]) -> CliResult<()> {
    let mut temporary = tempfile::NamedTempFile::new_in(directory).map_err(failure)?;
    temporary.write_all(bytes).map_err(failure)?;
    temporary.flush().map_err(failure)?;
    publish_cache(store, temporary, cache)
}

/// Read or record a bounded check-time extraction through the bakery, separately from complete-crate caches.
///
/// The exact request and native generation identify the result, including absent paths. Oven's index names the
/// read-only bakery payload of each extraction the unit used last time; one the index lacks is extracted and written
/// to the staging directory, and every identity the run reads or writes is appended to its `used` list, from which
/// Oven bakes the staged files and registers their roots before the description's next run. These extractions never
/// enter a unit's shipped cache list and grant no crate-wide coverage. A unit Oven does not prepare, or a cache that
/// cannot be read or written, keeps ordinary extraction: cache storage is never a prerequisite for checking.
fn unit_path_facts(
    unit: &UnitDescription,
    paths: &BTreeSet<String>,
    extract: &impl Fn(&BTreeSet<String>) -> Result<RustFacts, String>,
) -> CliResult<RustAbiCache> {
    let started = std::time::Instant::now();

    // ---- Optional cache availability ----
    let Some(directory) = &unit.rust_facts else {
        return encoded_cache(&extract(paths).map_err(CliError::failure)?);
    };
    let (index, staging) = (&directory.join("index"), &directory.join("staging"));
    let identity = match path_identity(unit, paths) {
        Ok(identity) => identity,
        Err(error) => {
            if incan_frontend::trace_flags::trace_rust_facts() {
                incan_frontend::trace_line!(
                    incan_frontend::trace_flags::Category::RustFacts,
                    "rust facts cache unavailable: {}",
                    error.message
                );
            }
            return encoded_cache(&extract(paths).map_err(CliError::failure)?);
        }
    };
    // ---- Indexed bakery payload or genuine extraction ----
    let cached = indexed_extraction(index, &identity)
        .and_then(|payload| fs::read(payload).ok())
        .and_then(stored_cache);
    let reused = cached.is_some();
    let facts = if let Some(facts) = cached {
        facts
    } else {
        let bytes = encoded_extraction(&extract(paths).map_err(CliError::failure)?)?;
        if let Err(error) = stage_extraction(staging, &identity, &bytes)
            && incan_frontend::trace_flags::trace_rust_facts()
        {
            incan_frontend::trace_line!(
                incan_frontend::trace_flags::Category::RustFacts,
                "rust facts cache not staged: {}",
                error.message
            );
        }
        RustAbiCache::decode_owned(bytes).map_err(failure)?
    };
    if let Err(error) = record_used(staging, &identity)
        && incan_frontend::trace_flags::trace_rust_facts()
    {
        incan_frontend::trace_line!(
            incan_frontend::trace_flags::Category::RustFacts,
            "rust facts use not recorded: {}",
            error.message
        );
    }
    // ---- Optional retrieval trace ----
    if incan_frontend::trace_flags::trace_rust_facts() {
        incan_frontend::trace_line!(
            incan_frontend::trace_flags::Category::RustFacts,
            "rust facts cache: {} {} paths, {} items in {:?}",
            if reused { "reused" } else { "compiled" },
            paths.len(),
            facts.len(),
            started.elapsed()
        );
    }
    Ok(facts)
}

/// The request identity of a bounded extraction: the native generation, the extracting executable and the paths.
fn path_identity(unit: &UnitDescription, paths: &BTreeSet<String>) -> CliResult<String> {
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
    Ok(hex::encode(identity.finalize()))
}

/// The bakery payload Oven's index names for `identity`, when the index has one.
fn indexed_extraction(index: &Path, identity: &str) -> Option<PathBuf> {
    let text = fs::read_to_string(index).ok()?;
    text.lines().find_map(|line| {
        let (named, payload) = line.split_once('\t')?;
        (named == identity).then(|| PathBuf::from(payload))
    })
}

/// Write a complete encoded extraction as `<identity>.rustabi`, renaming it into place so Oven never reads a partial
/// file.
fn stage_extraction(staging: &Path, identity: &str, bytes: &[u8]) -> CliResult<()> {
    fs::create_dir_all(staging).map_err(failure)?;
    let mut temporary = tempfile::NamedTempFile::new_in(staging).map_err(failure)?;
    temporary.write_all(bytes).map_err(failure)?;
    temporary.flush().map_err(failure)?;
    temporary
        .persist(staging.join(format!("{identity}.rustabi")))
        .map_err(failure)?;
    Ok(())
}

/// Append `identity` to the run's `used` list; one short append per extraction request.
fn record_used(staging: &Path, identity: &str) -> CliResult<()> {
    fs::create_dir_all(staging).map_err(failure)?;
    let mut used = File::options()
        .create(true)
        .append(true)
        .open(staging.join("used"))
        .map_err(failure)?;
    used.write_all(format!("{identity}\n").as_bytes()).map_err(failure)
}

/// Publish a complete cache under its request name through the toolchain's content store.
///
/// Request identities differ whenever any unit input differs, yet many requests produce byte-identical facts. The
/// store keeps each distinct cache once, named by its SHA-256, and every request name is a hard link to it; a
/// filesystem without hard links gets a private copy instead. A stored file whose bytes no longer match its name is
/// replaced, never linked. The caller holds the request's lock and every step renames atomically, so readers observe
/// either the previous cache or the complete new one.
fn publish_cache(store: &Path, temporary: tempfile::NamedTempFile, cache: &Path) -> CliResult<()> {
    // ---- Content-addressed copy ----
    let digest = read_digest(temporary.path())?;
    fs::create_dir_all(store).map_err(failure)?;
    let object = store.join(format!("{}.rustabi", hex::encode(digest)));
    if !object.is_file() || read_digest(&object)? != digest {
        temporary.persist(&object).map_err(failure)?;
    }
    // ---- Request name ----
    let link = cache.with_extension(format!("link-{}", std::process::id()));
    match fs::remove_file(&link) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(failure(error)),
        _ => {}
    }
    if fs::hard_link(&object, &link).is_err() {
        fs::copy(&object, &link).map_err(failure)?;
    }
    fs::rename(&link, cache).map_err(failure)?;
    // Renaming a link over another link to the same file succeeds without removing the source name.
    match fs::remove_file(&link) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(failure(error)),
        _ => Ok(()),
    }
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
/// Resolve its paths relative to the descriptor, as required for relocated release archives; absolute bootstrap paths
/// remain absolute. The owning cache must match after resolution, before any extraction can run.
fn installed_toolchain(unit: &UnitDescription) -> CliResult<(PathBuf, Toolchain)> {
    for cache in &unit.rust_abi {
        let Some(directory) = cache.parent().and_then(Path::parent) else {
            continue;
        };
        let descriptor = directory.join("toolchain.json");
        if !descriptor.is_file() {
            continue;
        }
        let mut toolchain: Toolchain =
            serde_json::from_slice(&fs::read(&descriptor).map_err(failure)?).map_err(failure)?;
        toolchain.driver = directory.join(toolchain.driver);
        toolchain.sysroot = directory.join(toolchain.sysroot);
        for path in &mut toolchain.rust_abi {
            *path = directory.join(&*path);
        }
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
    store: &Path,
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
    RustAbiCache::open(temporary.path()).map_err(failure)?;
    temporary.flush().map_err(failure)?;
    publish_cache(store, temporary, cache)
}

/// Preserve an I/O or serialization failure as an actionable client setup error.
fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(format!("cannot prepare Rust ABI cache: {error}"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::io::Write;
    use std::path::Path;

    use sha2::{Digest, Sha256};

    use super::{RustFacts, UnitDescription, cached_path_facts, publish_cache};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A complete extraction written beside its request names, as both extraction paths write one.
    fn extracted(directory: &Path, bytes: &[u8]) -> Result<tempfile::NamedTempFile, Box<dyn std::error::Error>> {
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        temporary.write_all(bytes)?;
        temporary.flush()?;
        Ok(temporary)
    }

    /// Every file name in `directory`, sorted, so tests can assert that no temporary name survives publication.
    fn names(directory: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let mut names = fs::read_dir(directory)?
            .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
            .collect::<Result<Vec<_>, std::io::Error>>()?;
        names.sort();
        Ok(names)
    }

    #[test]
    fn identical_extractions_share_one_stored_cache() -> TestResult {
        let root = tempfile::tempdir()?;
        let (units, store) = (root.path().join("units"), root.path().join("objects"));
        fs::create_dir_all(&units)?;
        let (first, second) = (units.join("first.rustabi"), units.join("second.rustabi"));
        publish_cache(&store, extracted(&units, b"facts")?, &first)?;
        publish_cache(&store, extracted(&units, b"facts")?, &second)?;
        assert_eq!(fs::read(&first)?, b"facts");
        assert_eq!(fs::read(&second)?, b"facts");
        assert_eq!(names(&store)?.len(), 1);
        assert_eq!(names(&units)?, ["first.rustabi", "second.rustabi"]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(fs::metadata(&first)?.ino(), fs::metadata(&second)?.ino());
        }
        Ok(())
    }

    #[test]
    fn distinct_extractions_keep_their_own_bytes() -> TestResult {
        let root = tempfile::tempdir()?;
        let (units, store) = (root.path().join("units"), root.path().join("objects"));
        fs::create_dir_all(&units)?;
        let (first, second) = (units.join("first.rustabi"), units.join("second.rustabi"));
        publish_cache(&store, extracted(&units, b"first facts")?, &first)?;
        publish_cache(&store, extracted(&units, b"second facts")?, &second)?;
        assert_eq!(fs::read(&first)?, b"first facts");
        assert_eq!(fs::read(&second)?, b"second facts");
        assert_eq!(names(&store)?.len(), 2);
        Ok(())
    }

    #[test]
    fn stored_cache_with_wrong_bytes_is_replaced_before_linking() -> TestResult {
        let root = tempfile::tempdir()?;
        let (units, store) = (root.path().join("units"), root.path().join("objects"));
        fs::create_dir_all(&units)?;
        fs::create_dir_all(&store)?;
        let named = store.join(format!("{}.rustabi", hex::encode(Sha256::digest(b"facts"))));
        fs::write(&named, b"damaged")?;
        let cache = units.join("request.rustabi");
        publish_cache(&store, extracted(&units, b"facts")?, &cache)?;
        assert_eq!(fs::read(&cache)?, b"facts");
        assert_eq!(fs::read(&named)?, b"facts");
        Ok(())
    }

    #[test]
    fn republishing_the_same_cache_leaves_no_temporary_name() -> TestResult {
        let root = tempfile::tempdir()?;
        let (units, store) = (root.path().join("units"), root.path().join("objects"));
        fs::create_dir_all(&units)?;
        let cache = units.join("request.rustabi");
        publish_cache(&store, extracted(&units, b"facts")?, &cache)?;
        publish_cache(&store, extracted(&units, b"facts")?, &cache)?;
        assert_eq!(fs::read(&cache)?, b"facts");
        assert_eq!(names(&units)?, ["request.rustabi"]);
        Ok(())
    }

    /// An installed toolchain whose descriptor owns one shipped cache, as `installed_toolchain` requires.
    fn installed(root: &Path) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
        let toolchain = root.join("toolchain");
        fs::create_dir_all(toolchain.join("rust-abi"))?;
        fs::create_dir_all(toolchain.join("sysroot"))?;
        fs::write(toolchain.join("driver"), b"driver")?;
        fs::write(toolchain.join("rust-abi/shipped.rustabi"), b"shipped")?;
        let descriptor = serde_json::json!({
            "schema": "incan.toolchain/1",
            "driver": "driver",
            "sysroot": "sysroot",
            "rust_abi": ["rust-abi/shipped.rustabi"],
        });
        fs::write(toolchain.join("toolchain.json"), serde_json::to_vec(&descriptor)?)?;
        Ok(toolchain.join("rust-abi/shipped.rustabi"))
    }

    /// A prepared unit of `name` with its own native artifact, as every test case has its own dependency.
    fn unit(root: &Path, name: &str, shipped: &Path) -> Result<UnitDescription, Box<dyn std::error::Error>> {
        let project = root.join(name);
        fs::create_dir_all(&project)?;
        let artifact = project.join(format!("lib{name}.rlib"));
        fs::write(&artifact, name.as_bytes())?;
        let mut unit: UnitDescription = serde_json::from_value(serde_json::json!({
            "schema": "incan.unit/1",
            "source": project.join("main.incn"),
            "project_root": project,
            "source_root": project,
            "package": name,
            "version": "0.1.0",
            "profile": "debug",
            "declared_features": [],
            "active_features": [],
            "loaves": [],
            "rust_crates": [],
            "contract_models": [],
            "c_headers": [],
            "externs": [{"name": name, "path": artifact}],
            "search": [],
            "rust_abi": [shipped],
        }))?;
        unit.rust_facts = Some(project.join("facts"));
        Ok(unit)
    }

    #[test]
    fn a_standard_path_reached_by_two_units_is_extracted_once() -> TestResult {
        let root = tempfile::tempdir()?;
        let shipped = installed(root.path())?;
        let paths = BTreeSet::from(["std::net::Ipv4Addr".to_owned()]);
        let extractions = std::cell::Cell::new(0);
        let extract = |requested: &BTreeSet<String>| -> Result<RustFacts, String> {
            extractions.set(extractions.get() + 1);
            assert_eq!(requested, &paths);
            Ok(RustFacts::new())
        };
        cached_path_facts(&unit(root.path(), "first", &shipped)?, &paths, extract)?;
        cached_path_facts(&unit(root.path(), "second", &shipped)?, &paths, extract)?;
        assert_eq!(
            extractions.get(),
            1,
            "a second unit extracted the same standard path again"
        );
        Ok(())
    }

    #[test]
    fn a_unit_reaching_its_own_crate_and_a_standard_path_extracts_them_together() -> TestResult {
        let root = tempfile::tempdir()?;
        let shipped = installed(root.path())?;
        // An extraction describes the types a signature names only for the crates the same extraction names, so a
        // unit's paths are extracted in one session whenever one of them is rooted in the unit's own crate.
        let paths = BTreeSet::from(["first::Carrier".to_owned(), "std::net::Ipv4Addr".to_owned()]);
        let requests = std::cell::RefCell::new(Vec::new());
        let extract = |requested: &BTreeSet<String>| -> Result<RustFacts, String> {
            requests.borrow_mut().push(requested.clone());
            Ok(RustFacts::new())
        };
        cached_path_facts(&unit(root.path(), "first", &shipped)?, &paths, extract)?;
        assert_eq!(
            requests.into_inner(),
            vec![paths],
            "the unit's paths were split between extractions"
        );
        Ok(())
    }

    /// One Rust record type of the unit's own crate, as an extraction describes it.
    fn carrier() -> incan_lang::interop::RustItemMetadata {
        incan_lang::interop::RustItemMetadata {
            canonical_path: "first::Carrier".into(),
            definition_path: None,
            visibility: incan_lang::interop::RustVisibility::Public,
            kind: incan_lang::interop::RustItemKind::Type(incan_lang::interop::RustTypeInfo::default()),
        }
    }

    /// A unit's extraction that Oven baked into its index is read as the stored cache itself, its items decoded when a
    /// checker reads them: the session receives that cache, not items decoded and encoded into another one.
    #[test]
    fn an_indexed_extraction_is_read_as_its_stored_cache() -> TestResult {
        let root = tempfile::tempdir()?;
        let shipped = installed(root.path())?;
        let unit = unit(root.path(), "first", &shipped)?;
        let paths = BTreeSet::from(["first::Carrier".to_owned()]);
        let extractions = std::cell::Cell::new(0);
        let extract = |_: &BTreeSet<String>| -> Result<RustFacts, String> {
            extractions.set(extractions.get() + 1);
            Ok(RustFacts::from([("first::Carrier".to_owned(), carrier())]))
        };
        let fresh = cached_path_facts(&unit, &paths, extract)?;
        assert!(fresh.contains("first::Carrier"));

        // Oven bakes the staged extraction and indexes it under its request identity before the next run.
        let directory = unit.rust_facts.clone().ok_or("the unit has no facts directory")?;
        let staged = fs::read_dir(directory.join("staging"))?
            .map(|entry| Ok(entry?.path()))
            .collect::<Result<Vec<_>, std::io::Error>>()?
            .into_iter()
            .find(|path| path.extension().is_some_and(|extension| extension == "rustabi"))
            .ok_or("no extraction was staged")?;
        let identity = staged
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or("the staged extraction has no identity")?;
        fs::write(directory.join("index"), format!("{identity}\t{}\n", staged.display()))?;

        let reused = cached_path_facts(&unit, &paths, extract)?;
        assert_eq!(extractions.get(), 1, "the indexed extraction was extracted again");
        assert_eq!(reused.len(), 1);
        assert_eq!(reused.get("first::Carrier")?, Some(carrier()));
        Ok(())
    }
}
