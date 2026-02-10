//! Source formatting pipeline for `incan fmt`.
//!
//! Formats `.incn` source files in-place, with check and diff modes for CI.

use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::{CliError, CliResult, ExitCode};
use crate::format::{format_diff, format_source};

/// Format Incan source files.
pub fn format_files(path: &str, check_mode: bool, diff_mode: bool) -> CliResult<ExitCode> {
    let path = Path::new(path);
    let files = collect_incn_files(path);

    if files.is_empty() {
        return Err(CliError::failure("No .incn files found"));
    }

    let mut needs_formatting = false;
    let mut formatted_count = 0;
    let mut error_count = 0;

    for file_path in &files {
        let source = match fs::read_to_string(file_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error reading {}: {}", file_path.display(), e);
                error_count += 1;
                continue;
            }
        };

        match format_source(&source) {
            Ok(formatted) => {
                let changed = source != formatted;

                if diff_mode && changed {
                    println!("--- {}", file_path.display());
                    if let Ok(Some(diff)) = format_diff(&source) {
                        print!("{}", diff);
                    }
                    println!();
                }

                if check_mode {
                    if changed {
                        println!("Would reformat: {}", file_path.display());
                        needs_formatting = true;
                    }
                } else if diff_mode {
                    if changed {
                        needs_formatting = true;
                    }
                } else if changed {
                    if let Err(e) = fs::write(file_path, &formatted) {
                        eprintln!("Error writing {}: {}", file_path.display(), e);
                        error_count += 1;
                    } else {
                        println!("Formatted: {}", file_path.display());
                        formatted_count += 1;
                    }
                }
            }
            Err(e) => {
                eprintln!("Error formatting {}: {}", file_path.display(), e);
                error_count += 1;
            }
        }
    }

    if check_mode || diff_mode {
        if needs_formatting {
            let msg = if diff_mode {
                "need formatting"
            } else {
                "would be reformatted"
            };
            return Err(CliError::failure(format!("\n{} file(s) {}", files.len(), msg)));
        } else {
            println!("✓ {} file(s) already formatted", files.len());
        }
    } else {
        println!("\n✓ {} file(s) formatted, {} error(s)", formatted_count, error_count);
    }

    if error_count > 0 {
        return Err(CliError::new("", ExitCode::FAILURE));
    }

    Ok(ExitCode::SUCCESS)
}

/// Recursively collect all `.incn` files from a path.
///
/// Skips hidden directories, `target/`, and `node_modules/`.
fn collect_incn_files(path: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    if path.is_file() {
        if path.extension().is_some_and(|ext| ext == "incn") {
            files.push(path.to_path_buf());
        }
    } else if path.is_dir()
        && let Ok(entries) = fs::read_dir(path)
    {
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                let name = entry_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !name.starts_with('.') && name != "target" && name != "node_modules" {
                    files.extend(collect_incn_files(&entry_path));
                }
            } else if entry_path.extension().is_some_and(|ext| ext == "incn") {
                files.push(entry_path);
            }
        }
    }

    files
}
