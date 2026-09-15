//! Test discovery for `incan test` and for the lock: which files are tests, which modules a test pulls in, and
//! the fixture and marker facts the runner schedules by.
//!
//! Discovery is driver work — it needs the compilation session and the frontend, never the terminal — so the test
//! runner in `cli` consumes it and the lock collector reads test imports through it without naming a command.

pub mod discovery;
pub mod module_graph;
pub mod types;

use std::path::{Path, PathBuf};

use discovery::{discover_test_file_candidates, discover_test_files_with_session};

use crate::session::CompilationSession;

/// Discover a test inventory through a compilation session already owned by the caller.
pub fn discover_test_files_with_compilation_session(path: &Path, session: &CompilationSession) -> Vec<PathBuf> {
    let candidates = discover_test_file_candidates(path);
    let authority_root = session
        .manifest
        .as_ref()
        .map(|manifest| {
            std::fs::canonicalize(manifest.project_root()).unwrap_or_else(|_| manifest.project_root().to_path_buf())
        })
        .or_else(|| candidates.command_authority_root().ok());
    authority_root.map_or_else(Vec::new, |authority_root| {
        discover_test_files_with_session(&candidates, &authority_root, session)
    })
}

/// Conventional `tests/` and `src/` anchors belong to their parent project. An unanchored source owns its containing
/// directory so implementation paths above it never leak into generated module names or runtime working directories.
pub fn infer_test_project_root_without_manifest(test_path: &Path) -> PathBuf {
    let absolute_test_path = if test_path.is_absolute() {
        test_path.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(test_path)
    } else {
        test_path.to_path_buf()
    };
    let absolute_test_path = std::fs::canonicalize(&absolute_test_path).unwrap_or(absolute_test_path);

    for ancestor in absolute_test_path.ancestors().skip(1) {
        if ancestor
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| matches!(name, "tests" | "src"))
            && let Some(parent) = ancestor.parent()
        {
            return parent.to_path_buf();
        }
    }

    absolute_test_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}
