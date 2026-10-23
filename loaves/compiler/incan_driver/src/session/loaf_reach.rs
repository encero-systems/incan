//! Optional native-unit dependency footprints, bound to unchanged source, artifact and compiler generations.
//!
//! A record contains only the Loaf keys canonical lookup resolved. It never retains a checking result, diagnostics,
//! Body IR or a test outcome. The trusted-home and file-generation assumptions match manifest identity proofs;
//! malformed, unavailable or changed inputs take the full-index path. Editor sessions never read these records.

use std::collections::BTreeSet;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::sources::collect_incan_source_files_with_directories;
use crate::unit::{UnitDescription, UnitKind};

const VERSION: u32 = 1;
const RECORD_LIMIT: u64 = 4 * 1024 * 1024;

/// The fields that invalidate a retained observation, including ctime for restored-mtime edits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Generation {
    directory: bool,
    device: u64,
    inode: u64,
    size: u64,
    modified: i64,
    modified_ns: i64,
    changed: i64,
    changed_ns: i64,
}

/// One input's presence and exact file or directory generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Input {
    path: PathBuf,
    generation: Option<Generation>,
}

/// The prior lookup footprint and its complete input observation, independently checked for accidental corruption.
#[derive(Serialize, Deserialize)]
struct Record {
    version: u32,
    key: String,
    inputs: Vec<Input>,
    loaves: BTreeSet<String>,
    checksum: String,
}

/// One invocation's initial observations, checked again before retaining its canonical demand.
#[derive(Debug, Clone)]
pub(super) struct NativeLoafReach {
    key: String,
    path: PathBuf,
    inputs: Vec<Input>,
    previous: Option<BTreeSet<String>>,
}

impl NativeLoafReach {
    /// Observe ordinary native source programs only. Guest, editor, C-header and contract-model input domains need
    /// their own complete dependency observations and keep the eager index rather than guess at those inputs.
    pub(super) fn capture(unit: &UnitDescription) -> Option<Self> {
        if unit.kind != UnitKind::Program || !unit.c_headers.is_empty() || !unit.contract_models.is_empty() {
            return None;
        }
        if !unit
            .source
            .canonicalize()
            .ok()?
            .starts_with(unit.source_root.canonicalize().ok()?)
        {
            return None;
        }
        let home = PathBuf::from(std::env::var_os("INCAN_HOME")?);
        if !home.is_absolute() {
            return None;
        }
        let descriptor = serde_json::to_vec(&(VERSION, unit)).ok()?;
        let key = format!("{:x}", Sha256::digest(descriptor));
        Some(Self {
            path: home.join("cache/native-loaf-reach").join(format!("{key}.json")),
            key,
            inputs: observe(unit).ok()?,
            previous: None,
        })
    }

    /// Admit only a complete, unchanged, integrity-checked footprint whose keys all belong to this unit.
    pub(super) fn selected(&mut self, unit: &UnitDescription) -> Option<BTreeSet<String>> {
        let metadata = fs::symlink_metadata(&self.path).ok()?;
        if !metadata.is_file() || metadata.len() > RECORD_LIMIT {
            return None;
        }
        let mut bytes = Vec::new();
        File::open(&self.path)
            .ok()?
            .take(RECORD_LIMIT)
            .read_to_end(&mut bytes)
            .ok()?;
        let record: Record = serde_json::from_slice(&bytes).ok()?;
        if record.version != VERSION || record.key != self.key || record.inputs != self.inputs {
            return None;
        }
        let available: BTreeSet<_> = unit
            .loaves
            .iter()
            .map(|loaf| if loaf.standard { &loaf.name } else { &loaf.key })
            .collect();
        if !record.loaves.iter().all(|key| available.contains(key)) {
            return None;
        }
        if record.checksum != checksum(&record.key, &record.inputs, &record.loaves).ok()? {
            return None;
        }
        self.previous = Some(record.loaves.clone());
        Some(record.loaves)
    }

    /// Publish names only after a stable complete native declaration closure; failure never changes compilation.
    pub(super) fn retain(&self, unit: &UnitDescription, loaves: BTreeSet<String>) -> io::Result<()> {
        // A hit already describes these canonical lookups. Keep its original generation binding: changed inputs
        // invalidate it at the next open, and no facts from an in-flight mutation are published over it.
        if self
            .previous
            .as_ref()
            .is_some_and(|previous| loaves.is_subset(previous))
        {
            return Ok(());
        }
        if observe(unit)? != self.inputs {
            return Ok(());
        }
        let record = Record {
            version: VERSION,
            key: self.key.clone(),
            checksum: checksum(&self.key, &self.inputs, &loaves)?,
            inputs: self.inputs.clone(),
            loaves,
        };
        let bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
        if u64::try_from(bytes.len()).map_err(io::Error::other)? > RECORD_LIMIT {
            return Ok(());
        }
        let parent = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("the reach record has no parent"))?;
        fs::create_dir_all(parent)?;
        let temporary = self.path.with_extension(format!("{}.pending", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary)?;
        let result = file.write_all(&bytes).and_then(|()| fs::rename(&temporary, &self.path));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

/// Observe every authored source, exact artifact, ABI cache and named transitive search artifact the unit can read.
fn observe(unit: &UnitDescription) -> io::Result<Vec<Input>> {
    let mut sources = Vec::new();
    let mut directories = Vec::new();
    let mut contains_symlinks = false;
    collect_incan_source_files_with_directories(
        &unit.source_root,
        &mut sources,
        &mut directories,
        &mut contains_symlinks,
    )?;
    if contains_symlinks {
        return Err(io::Error::other("source symlinks require the full index"));
    }
    let mut paths: BTreeSet<_> = sources.into_iter().collect();
    paths.extend(directories);
    paths.insert(unit.source.clone());
    paths.insert(std::env::current_exe()?.canonicalize()?);
    paths.extend(unit.externs.iter().map(|native| native.path.clone()));
    paths.extend(unit.rust_abi.iter().cloned());
    for loaf in &unit.loaves {
        for name in [
            format!("{}.incnlib", loaf.name),
            format!("{}.incnbody", loaf.name),
            format!("{}.native.json", loaf.name),
            format!("lib{}.rlib", loaf.name),
        ] {
            paths.insert(loaf.dir.join(name));
        }
    }
    for directory in &unit.search {
        paths.insert(directory.clone());
        let entries = fs::read_dir(directory)?;
        for entry in entries {
            let path = entry?.path();
            if matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("rlib" | "rmeta" | "dylib" | "so")
            ) {
                paths.insert(path);
            }
        }
    }
    // Parent generations also guard absent artifacts and transient replacement of search candidates.
    let parents: Vec<_> = paths
        .iter()
        .filter_map(|path| path.parent().map(PathBuf::from))
        .collect();
    paths.extend(parents);
    paths
        .into_iter()
        .map(|path| {
            let generation = match fs::metadata(&path) {
                Ok(metadata) => {
                    Some(generation(&metadata).ok_or_else(|| io::Error::other("unsupported input generation"))?)
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            };
            Ok(Input { path, generation })
        })
        .collect()
}

/// Observe Unix file and directory generations; directory ctime protects source/search membership against ABA edits.
#[cfg(unix)]
fn generation(metadata: &Metadata) -> Option<Generation> {
    (metadata.is_file() || metadata.is_dir()).then(|| Generation {
        directory: metadata.is_dir(),
        device: metadata.dev(),
        inode: metadata.ino(),
        size: metadata.len(),
        modified: metadata.mtime(),
        modified_ns: metadata.mtime_nsec(),
        changed: metadata.ctime(),
        changed_ns: metadata.ctime_nsec(),
    })
}

/// Other platforms retain the full-index path until they have equivalent generation guards.
#[cfg(not(unix))]
fn generation(_metadata: &Metadata) -> Option<Generation> {
    None
}

/// Protect the small dependency footprint independently of the source/artifact generation comparisons.
fn checksum(key: &str, inputs: &[Input], loaves: &BTreeSet<String>) -> io::Result<String> {
    let bytes = serde_json::to_vec(&(VERSION, key, inputs, loaves)).map_err(io::Error::other)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
