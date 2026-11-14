//! Register Incan source files and map one-based scalar plan locations to rustc spans.

use crate::error::PlanError;
use crate::plan::SourceSpan;
use rustc_span::source_map::SourceMap;
use rustc_span::{BytePos, DUMMY_SP, SourceFile, Span, SyntaxContext};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Test files checked with generated case entries (`crate::test_cases`), by the file name plans give them, each with a
/// written copy of the text it was checked as. The entries lie past the file's own last line, so their spans resolve
/// only against that copy.
static CHECKED_SOURCES: Mutex<BTreeMap<String, PathBuf>> = Mutex::new(BTreeMap::new());

/// Record the text `file` was checked as when it differs from the file, or forget an earlier record when it does not.
/// The copy lives in the temporary directory under the file's stem and a digest of its text, so an unchanged text
/// reuses its copy.
pub(crate) fn retain_checked_source(file: &Path, text: Option<&str>) -> Result<(), String> {
    let key = file.to_string_lossy().into_owned();
    let mut sources = CHECKED_SOURCES
        .lock()
        .map_err(|_| "checked source registry is poisoned".to_string())?;
    let Some(text) = text else {
        sources.remove(&key);
        return Ok(());
    };
    let digest: String = Sha256::digest(text.as_bytes())
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let stem = file.file_stem().and_then(|stem| stem.to_str()).unwrap_or("test");
    let copy = std::env::temp_dir().join(format!("{stem}-{digest}.incn"));
    if std::fs::read_to_string(&copy).ok().as_deref() != Some(text) {
        std::fs::write(&copy, text)
            .map_err(|error| format!("cannot write checked test source {}: {error}", copy.display()))?;
    }
    sources.insert(key, copy);
    Ok(())
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
            let path = checked_copy(file).unwrap_or_else(|| PathBuf::from(file));
            let source = self.map.load_file(&path).map_err(|error| PlanError::Invalid {
                function: file.to_owned(),
                reason: error.to_string(),
            })?;
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
