//! Source formatting with the compiler's shared authored-file discovery and unit vocabulary.

use std::fs;
use std::path::{Path, PathBuf};

use incan_driver::error::{CliError, CliResult, ExitCode};
use incan_driver::session::CompilationSession;
use incan_driver::sources::collect_incan_source_files;
use incan_driver::unit::UnitDescription;
use incan_format::{FormatConfig, format_parsed_source_with_config};
use incan_frontend::diagnostics;

/// Format the authored files at `path`, or report required changes without writing in check mode.
pub(crate) fn format_files(path: &str, unit: &UnitDescription, check_mode: bool) -> CliResult<ExitCode> {
    let session = CompilationSession::for_formatting(unit)?;
    let files = source_files(Path::new(path))?;
    if files.is_empty() {
        return Err(CliError::failure("No .incn files found"));
    }
    let mut needs_formatting = false;
    let mut formatted_count = 0;
    let mut error_count = 0;
    for file in &files {
        let result = fs::read_to_string(file)
            .map_err(|error| CliError::failure(format!("Error reading {}: {error}", file.display())))
            .and_then(|source| format_source(file, &source, &session).map(|formatted| (source, formatted)));
        let (source, formatted) = match result {
            Ok(value) => value,
            Err(error) => {
                eprintln!("Error formatting {}: {}", file.display(), error.message);
                error_count += 1;
                continue;
            }
        };
        if source == formatted {
            continue;
        }
        if check_mode {
            println!("Would reformat: {}", file.display());
            needs_formatting = true;
            continue;
        }
        match fs::write(file, formatted) {
            Ok(()) => {
                println!("Formatted: {}", file.display());
                formatted_count += 1;
            }
            Err(error) => {
                eprintln!("Error writing {}: {error}", file.display());
                error_count += 1;
            }
        }
    }
    if check_mode {
        if needs_formatting {
            return Err(CliError::failure(format!(
                "\n{} file(s) would be reformatted",
                files.len()
            )));
        }
        println!("✓ {} file(s) already formatted", files.len());
    } else {
        println!("\n✓ {formatted_count} file(s) formatted, {error_count} error(s)");
    }
    if error_count > 0 {
        return Err(CliError::new("", ExitCode::FAILURE));
    }
    Ok(ExitCode::SUCCESS)
}

/// Preserve explicit single-file formatting while directory discovery follows the compiler's common policy.
pub(crate) fn source_files(path: &Path) -> CliResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    if path.is_file() {
        if path.extension().is_some_and(|extension| extension == "incn") {
            files.push(path.to_path_buf());
        }
    } else {
        collect_incan_source_files(path, &mut files)
            .map_err(|error| CliError::failure(format!("cannot discover {}: {error}", path.display())))?;
        files.sort();
    }
    Ok(files)
}

/// Parse a file's syntax with its owning unit's admitted vocabulary before formatting its authored source; source
/// placement is checked only during compilation.
fn format_source(file: &Path, source: &str, session: &CompilationSession) -> CliResult<String> {
    let ast = session.parse_source_for_formatting(source).map_err(|errors| {
        CliError::failure(
            errors
                .iter()
                .map(|error| diagnostics::format_error(file.to_string_lossy().as_ref(), source, error))
                .collect::<String>(),
        )
    })?;
    format_parsed_source_with_config(source, &ast, FormatConfig::default())
        .map_err(|error| CliError::failure(error.to_string()))
}
