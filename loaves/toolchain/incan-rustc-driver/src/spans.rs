//! Register Incan source files and map one-based scalar plan locations to rustc spans.

use crate::error::PlanError;
use crate::plan::SourceSpan;
use rustc_span::source_map::SourceMap;
use rustc_span::{BytePos, DUMMY_SP, SourceFile, Span, SyntaxContext};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

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
    pub fn span(&self, location: &SourceSpan) -> Result<Span, PlanError> {
        if location.file.starts_with("pub::") {
            return Ok(DUMMY_SP);
        }
        let invalid = || PlanError::Invalid {
            function: location.file.clone(),
            reason: "source coordinate is outside the retained file".into(),
        };
        if !self.files.borrow().contains_key(&location.file) {
            let source = self
                .map
                .load_file(Path::new(&location.file))
                .map_err(|error| PlanError::Invalid {
                    function: location.file.clone(),
                    reason: error.to_string(),
                })?;
            self.files.borrow_mut().insert(location.file.clone(), source);
        }
        let files = self.files.borrow();
        let source = files.get(&location.file).ok_or_else(invalid)?;
        let text = source.src.as_ref().ok_or_else(invalid)?;
        let line = usize::try_from(location.line - 1).map_err(|_| invalid())?;
        let column = usize::try_from(location.column - 1).map_err(|_| invalid())?;
        let mut offset = 0usize;
        let mut lines = text.split_inclusive('\n');
        for _ in 0..line {
            offset += lines.next().ok_or_else(invalid)?.len();
        }
        let text = lines.next().ok_or_else(invalid)?;
        let (within, character) = text.char_indices().nth(column).ok_or_else(invalid)?;
        let start = source.start_pos + BytePos(u32::try_from(offset + within).map_err(|_| invalid())?);
        let width = u32::try_from(character.len_utf8()).map_err(|_| invalid())?;
        Ok(Span::new(start, start + BytePos(width), SyntaxContext::root(), None))
    }
}
