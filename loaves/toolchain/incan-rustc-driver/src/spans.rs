//! Register Incan source files and map one-based scalar plan locations to rustc spans.

use crate::error::PlanError;
use crate::plan::SourceSpan;
use rustc_span::source_map::SourceMap;
use rustc_span::{BytePos, DUMMY_SP, FileName, SourceFile, Span, SyntaxContext};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// The test files of the unit this process compiles that were checked with generated case entries
/// (`crate::test_cases`), by the file name plans give them, each with a written copy of the text it was checked as.
/// The entries lie past the file's own last line, so their spans resolve only against that copy. The registry is this
/// process's alone: a plan compiled in another process takes the texts with it ([`checked_texts`]).
static CHECKED_SOURCES: Mutex<BTreeMap<String, PathBuf>> = Mutex::new(BTreeMap::new());
static CAPTURED_SOURCES: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Keep an invocation's authored source directory visible to rustc query threads until native compilation ends.
pub(crate) struct CapturedSourceScope(Option<PathBuf>);

impl Drop for CapturedSourceScope {
    /// Restore the previous capture so a subsequent resident invocation cannot see these bytes.
    fn drop(&mut self) {
        if let Ok(mut directory) = CAPTURED_SOURCES.lock() {
            *directory = self.0.take();
        }
    }
}

/// Select an invocation's captured directory for native span translation.
pub(crate) fn captured_source_scope(directory: Option<PathBuf>) -> Result<CapturedSourceScope, String> {
    let mut active = CAPTURED_SOURCES
        .lock()
        .map_err(|_| "captured source registry is poisoned")?;
    Ok(CapturedSourceScope(std::mem::replace(&mut *active, directory)))
}

/// Record the texts the unit's files were checked as where they differ from the files, replacing the previous unit's
/// records, so a file checked as itself resolves against itself. Each copy lives in the temporary directory under its
/// file's stem and a digest of its text, so an unchanged text reuses its copy.
pub(crate) fn retain_checked_sources(checked: &[(PathBuf, String)]) -> Result<(), String> {
    let mut copies = BTreeMap::new();
    for (file, text) in checked {
        let digest: String = Sha256::digest(text.as_bytes())
            .iter()
            .take(8)
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let stem = file.file_stem().and_then(|stem| stem.to_str()).unwrap_or("test");
        let copy = std::env::temp_dir().join(format!("{stem}-{digest}.incn"));
        if std::fs::read_to_string(&copy).ok().as_deref() != Some(text.as_str()) {
            std::fs::write(&copy, text)
                .map_err(|error| format!("cannot write checked test source {}: {error}", copy.display()))?;
        }
        copies.insert(file.to_string_lossy().into_owned(), copy);
    }
    *CHECKED_SOURCES
        .lock()
        .map_err(|_| "checked source registry is poisoned".to_string())? = copies;
    Ok(())
}

/// The recorded files with the texts they were checked as, for a plan that another process compiles: that process
/// records them again ([`retain_checked_sources`]), so it depends on neither this process nor its temporary directory.
pub(crate) fn checked_texts() -> Result<Vec<(PathBuf, String)>, String> {
    let copies = CHECKED_SOURCES
        .lock()
        .map_err(|_| "checked source registry is poisoned".to_string())?
        .clone();
    copies
        .into_iter()
        .map(|(file, copy)| {
            let text = std::fs::read_to_string(&copy)
                .map_err(|error| format!("cannot read checked test source {}: {error}", copy.display()))?;
            Ok((PathBuf::from(file), text))
        })
        .collect()
}

/// The written copy a file was checked as, when it was checked with generated entries.
fn checked_copy(file: &str) -> Option<PathBuf> {
    CHECKED_SOURCES.lock().ok()?.get(file).cloned()
}

/// Source files retained for the invocation; no generated Rust file enters this map.
pub struct Sources<'a> {
    map: &'a SourceMap,
    files: RefCell<BTreeMap<String, Arc<SourceFile>>>,
}

impl<'a> Sources<'a> {
    /// Start a per-invocation source cache; files are loaded only at their first use.
    pub fn new(map: &'a SourceMap) -> Self {
        Self {
            map,
            files: RefCell::new(BTreeMap::new()),
        }
    }

    /// Keep rustc's logical filename and remapping while reading a captured file from its private placement.
    fn load(&self, file: &str) -> Result<Arc<SourceFile>, PlanError> {
        let failed = |reason: String| PlanError::Invalid {
            function: file.to_owned(),
            reason,
        };
        let path = checked_copy(file).unwrap_or_else(|| PathBuf::from(file));
        if path == PathBuf::from(file) {
            let directory = CAPTURED_SOURCES
                .lock()
                .map_err(|_| failed("captured source registry is poisoned".into()))?;
            if let Some(directory) = directory.as_deref()
                && let Some(text) = incan_driver::sources::read_captured_source(directory, file)
                    .map_err(|error| failed(error.message))?
            {
                let name = FileName::Real(self.map.path_mapping().to_real_filename(self.map.working_dir(), &path));
                return Ok(self.map.new_source_file(name, text));
            }
        }
        let _span = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::load_source_file", input = %path.display()).entered();
        self.map.load_file(&path).map_err(|error| failed(error.to_string()))
    }

    /// Map a character column into its source line's byte offset, refusing out-of-range coordinates.
    /// Published package identities have no source file; preserve that absence with a dummy span instead of loading
    /// a canonical identity as a path or attributing the declaration to the consumer.
    /// Lookups borrow the shared filename; only a new cache entry or an owned refusal copies its text.
    pub fn span(&self, location: &SourceSpan) -> Result<Span, PlanError> {
        let file = location.file.as_str();
        if file.starts_with("pub::") {
            return Ok(DUMMY_SP);
        }
        let invalid = || PlanError::Invalid {
            function: file.to_owned(),
            reason: "source coordinate is outside the retained file".into(),
        };
        if !self.files.borrow().contains_key(file) {
            let source = self.load(file)?;
            self.files.borrow_mut().insert(file.to_owned(), source);
        }
        let files = self.files.borrow();
        let source = files.get(file).ok_or_else(invalid)?;
        let text = source.src.as_ref().ok_or_else(invalid)?;
        let line = usize::try_from(location.line - 1).map_err(|_| invalid())?;
        let column = usize::try_from(location.column - 1).map_err(|_| invalid())?;
        // rustc's line table, computed once per file, locates the line; rescanning the text before it for every
        // span made span construction quadratic in the file's length.
        let starts = source.lines();
        let offset = starts.get(line).ok_or_else(invalid)?.0 as usize;
        let end = starts.get(line + 1).map_or(text.len(), |next| next.0 as usize);
        let text = text.get(offset..end).ok_or_else(invalid)?;
        let (within, character) = text.char_indices().nth(column).ok_or_else(invalid)?;
        let start = source.start_pos + BytePos(u32::try_from(offset + within).map_err(|_| invalid())?);
        let width = u32::try_from(character.len_utf8()).map_err(|_| invalid())?;
        Ok(Span::new(start, start + BytePos(width), SyntaxContext::root(), None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plan compiled in another process takes its checked texts along: recording them where nothing is recorded
    /// restores a copy with the generated entries.
    #[test]
    fn checked_texts_restore_the_copy_in_a_process_without_a_record() -> Result<(), String> {
        let name = "/nowhere/test_handed_off.incn";
        let text = "def test_one() -> None:\n    pass\n\n\ndef generated_entry() -> None:\n    test_one()\n";
        retain_checked_sources(&[(PathBuf::from(name), text.to_string())])?;
        let carried = checked_texts()?;
        retain_checked_sources(&[])?;
        assert!(checked_copy(name).is_none());

        retain_checked_sources(&carried)?;
        let copy = checked_copy(name).ok_or("the carried text restored no copy")?;
        assert_eq!(std::fs::read_to_string(copy).map_err(|error| error.to_string())?, text);
        retain_checked_sources(&[])
    }
}
