//! Reading a unit's sources: bounded source reads and authored-file discovery.

use std::fs;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};

use crate::diagnostics::CliDiagnosticFailure;
use crate::error::{CliError, CliResult};
use incan_frontend::ast::Span;
use incan_frontend::diagnostics;
use sha2::{Digest, Sha256};

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

/// Take the optional captured-source directory from an activation's private arguments. Durable caller descriptions
/// never point into another request's temporary directory.
pub fn captured_source_options(arguments: &[String]) -> Result<(Option<&Path>, &[String]), String> {
    match arguments {
        [flag, directory, rest @ ..] if flag == "--captured-sources" => {
            let directory = Path::new(directory);
            if !directory.is_absolute() || !directory.is_dir() {
                return Err(format!("invalid captured source directory: {}", directory.display()));
            }
            Ok((Some(directory), rest))
        }
        [flag] if flag == "--captured-sources" => Err("--captured-sources requires a request directory".into()),
        _ => Ok((None, arguments)),
    }
}

/// Read the snapshot for one logical source path from this activation's private directory. Only paths the engine
/// already read have snapshots; a missing member leaves the ordinary source reader in charge of that path.
pub fn read_captured_source(directory: &Path, logical: &str) -> CliResult<Option<String>> {
    let name = format!("{:x}.source", Sha256::digest(logical.as_bytes()));
    let file = match fs::File::open(directory.join(name)) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CliError::failure(format!(
                "cannot open captured source {logical}: {error}"
            )));
        }
    };
    let _span =
        tracing::trace_span!(target: "incan::invocation", "incan_driver::read_captured_source", input = logical)
            .entered();
    let mut source = String::new();
    let bytes = file
        .take(MAX_SOURCE_SIZE + 1)
        .read_to_string(&mut source)
        .map_err(|error| CliError::failure(format!("cannot read captured source {logical}: {error}")))?;
    if bytes as u64 > MAX_SOURCE_SIZE {
        return Err(CliError::failure(format!(
            "captured source {logical} exceeds {MAX_SOURCE_SIZE} bytes"
        )));
    }
    Ok(Some(source))
}

/// Read source once behind both legacy text errors and structured diagnostic reporting.
fn read_source_checked(file_path: &str) -> Result<String, SourceReadFailure> {
    let _span =
        tracing::trace_span!(target: "incan::invocation", "incan_driver::read_authored_source", input = file_path)
            .entered();
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
            // Broad source collection is used for package libraries and test units. Compiler, editor and user tools
            // all place non-source state below hidden directories, and a directory such as `.incan` is no valid
            // module name. A directory with its own `loaf.toml` is another project: its modules are that project's,
            // compiled against its dependencies, so collection stops at it.
            if !name.starts_with('.')
                && name != "target"
                && name != "node_modules"
                && !entry.path().join("loaf.toml").is_file()
            {
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
