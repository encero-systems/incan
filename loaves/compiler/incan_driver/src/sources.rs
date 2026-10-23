//! Reading a unit's sources: bounded source reads and authored-file discovery.

use std::fs;
use std::path::{Path, PathBuf};

use crate::diagnostics::CliDiagnosticFailure;
use crate::error::{CliError, CliResult};
use incan_frontend::ast::Span;
use incan_frontend::diagnostics;

/// Maximum source file size (100 MB)
///
/// Files larger than this are rejected to prevent out-of-memory conditions during compilation.
const MAX_SOURCE_SIZE: u64 = 100 * 1024 * 1024;

#[derive(Debug, Clone)]
struct SourceReadFailure {
    message: String,
}

/// Read one source file.
///
/// ## Errors
///
/// Returns an error if:
/// - The file cannot be read (I/O error)
/// - The file exceeds `MAX_SOURCE_SIZE` (100 MB)
pub fn read_source(file_path: &str) -> CliResult<String> {
    read_source_checked(file_path).map_err(|failure| CliError::failure(failure.message))
}

/// Read source for the stable diagnostic path, converting file-system failures into tooling diagnostics.
pub fn read_source_for_diagnostics(file_path: &str) -> Result<String, CliDiagnosticFailure> {
    read_source_checked(file_path).map_err(|failure| {
        CliDiagnosticFailure::single(
            file_path,
            "",
            diagnostics::CompileError::new(failure.message, Span::default()),
            diagnostics::DiagnosticPhase::Tooling,
        )
    })
}

/// Read source once behind both legacy text errors and structured diagnostic reporting.
fn read_source_checked(file_path: &str) -> Result<String, SourceReadFailure> {
    let metadata = fs::metadata(file_path).map_err(|error| SourceReadFailure {
        message: format!("Cannot access file '{}': {}", file_path, error),
    })?;
    if metadata.len() > MAX_SOURCE_SIZE {
        return Err(SourceReadFailure {
            message: format!(
                "Source file '{}' is too large ({} bytes, max {} bytes)",
                file_path,
                metadata.len(),
                MAX_SOURCE_SIZE
            ),
        });
    }
    fs::read_to_string(file_path).map_err(|error| SourceReadFailure {
        message: format!("Error reading file '{}': {}", file_path, error),
    })
}

/// Recursively collect authored `.incn` files without following directory symlinks.
pub fn collect_incan_source_files(directory: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    collect_incan_source_files_with_directories(directory, files, &mut Vec::new(), &mut false)
}

/// Discover the same authored files while retaining traversed directories for source-graph generation guards.
/// Directory observations detect transient added/removed import candidates even when the final file list agrees.
pub(crate) fn collect_incan_source_files_with_directories(
    directory: &Path,
    files: &mut Vec<PathBuf>,
    directories: &mut Vec<PathBuf>,
    contains_symlinks: &mut bool,
) -> std::io::Result<()> {
    directories.push(directory.to_path_buf());
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            let name = entry.file_name();
            let name = name.to_str().unwrap_or("");
            // Broad source collection is used for package libraries and generated test batches. Compiler, editor,
            // and user tools all place non-source state below hidden directories; treating that state as an Incan
            // module can emit invalid Rust such as `pub mod .incan;`.
            if !name.starts_with('.') && name != "target" && name != "node_modules" {
                collect_incan_source_files_with_directories(&entry.path(), files, directories, contains_symlinks)?;
            }
        } else if file_type.is_file() && entry.path().extension().is_some_and(|extension| extension == "incn") {
            files.push(entry.path());
        } else if file_type.is_symlink() {
            *contains_symlinks = true;
        }
    }
    Ok(())
}
