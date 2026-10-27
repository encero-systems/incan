//! Retain compiler sessions only while Oven's exact unit and every artifact generation still agree.
//! Oven remains responsible for dependency freshness; authored source is parsed and checked on each analysis.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use incan_driver::error::{CliError, CliResult};
use incan_driver::rust_abi::cached_rust_facts;
use incan_driver::session::CompilationSession;
use incan_driver::unit::UnitDescription;

type Generation = Vec<(PathBuf, Option<String>)>;

/// A connection-local, bounded set of sessions. Each holds its already loaded Rust facts setup.
#[derive(Default)]
pub(super) struct Sessions {
    entries: HashMap<PathBuf, Entry>,
}

/// One exact description and artifact generation, including any generated Rust facts cache.
struct Entry {
    unit: UnitDescription,
    paths: BTreeSet<PathBuf>,
    generation: Generation,
    session: Arc<CompilationSession>,
}

impl Sessions {
    /// Reuse the session only after Oven has prepared this request's dependencies and described the current unit.
    /// Failed observations disable reuse; a generation changed during session construction is never retained.
    pub(super) fn session(&mut self, unit: &UnitDescription) -> CliResult<Arc<CompilationSession>> {
        if let Some(entry) = self.entries.get(&unit.source)
            && entry.unit == *unit
            && observe(&entry.paths).is_ok_and(|current| current == entry.generation)
        {
            return Ok(Arc::clone(&entry.session));
        }
        self.entries.remove(&unit.source);
        let mut paths = artifact_paths(unit);
        let initial = observe(&paths).ok();
        let setup = cached_rust_facts(unit)?;
        paths.extend(setup.caches.iter().cloned());
        let prepared = observe(&paths).ok();
        let session = Arc::new(CompilationSession::new(unit)?.with_rust_facts(&setup)?);
        let stable = initial.is_some_and(|before| observe(&artifact_paths(unit)).is_ok_and(|after| before == after));
        if stable
            && let Some(generation) = prepared
            && observe(&paths).is_ok_and(|after| generation == after)
        {
            if self.entries.len() >= 32 {
                self.entries.clear();
            }
            self.entries.insert(
                unit.source.clone(),
                Entry {
                    unit: unit.clone(),
                    paths,
                    generation,
                    session: Arc::clone(&session),
                },
            );
        }
        Ok(session)
    }
}

/// Observe every published input the session can hold, including optional assets whose later creation invalidates it.
fn artifact_paths(unit: &UnitDescription) -> BTreeSet<PathBuf> {
    let mut paths: BTreeSet<_> = unit.rust_abi.iter().cloned().collect();
    paths.extend(unit.externs.iter().map(|native| native.path.clone()));
    paths.extend(unit.search_index.iter().cloned());
    paths.extend(unit.vocab.iter().cloned());
    for loaf in &unit.loaves {
        paths.insert(loaf.dir.join(format!("lib{}.rlib", loaf.name.replace('-', "_"))));
        for extension in ["incnlib", "incnbody", "native.json", "rustabi", "incngraph"] {
            paths.insert(loaf.dir.join(format!("{}.{extension}", loaf.name)));
        }
        paths.insert(loaf.dir.join("vocab-worker"));
    }
    for cache in &unit.rust_abi {
        if let Some(toolchain) = cache.parent().and_then(Path::parent) {
            paths.insert(toolchain.join("toolchain.json"));
        }
    }
    paths
}

/// A complete observation includes absence; other I/O failures prevent this generation from being retained.
fn observe(paths: &BTreeSet<PathBuf>) -> CliResult<Generation> {
    paths
        .iter()
        .map(|path| Ok((path.clone(), file_identity(path)?)))
        .collect()
}

/// Unix file identity catches equal-size rewrites with restored mtimes and atomic replacements at the same path.
#[cfg(unix)]
fn file_identity(path: &Path) -> CliResult<Option<String>> {
    use std::os::unix::fs::MetadataExt;
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(Some(format!(
            "{}:{}:{}:{}:{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.size(),
            metadata.mode(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(CliError::failure(format!("{}: {error}", path.display()))),
    }
}

/// Platforms without this complete identity keep the ordinary fresh-session behavior.
#[cfg(not(unix))]
fn file_identity(_path: &Path) -> CliResult<Option<String>> {
    Err(CliError::failure(
        "session generation observation is unavailable on this platform",
    ))
}
