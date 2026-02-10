//! Stdlib/prelude loading utilities
//!
//! This module handles loading the standard library prelude.
//!
//! # Current Status
//!
//! **The prelude is defined but not yet integrated into the compilation pipeline.**
//!
//! The `stdlib/` directory contains trait definitions (Debug, Display, Clone, etc.)
//! but codegen currently recognizes these through heuristics rather than actual
//! trait implementations. The infrastructure here is ready for when prelude
//! integration is implemented.
//!
//! # Future Work
//!
//! - Wire prelude ASTs into typechecking
//! - Validate trait bounds for derives
//! - Replace codegen heuristics with proper trait-based dispatch

use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::frontend::ast::Program;
use crate::frontend::{diagnostics, lexer, parser};

/// Parsed module with its source (for error reporting)
pub struct ParsedModule {
    pub name: String,
    /// Path segments for nested modules (e.g., ["db", "models"] for db::models)
    pub path_segments: Vec<String>,
    /// Absolute path to the module file (for diagnostics).
    pub file_path: PathBuf,
    pub source: String,
    pub ast: Program,
}

/// Error when loading/parsing prelude files.
#[derive(Debug)]
pub struct PreludeError {
    pub file: String,
    pub message: String,
}

impl fmt::Display for PreludeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Error loading prelude file '{}': {}", self.file, self.message)
    }
}

impl std::error::Error for PreludeError {}

/// Find the stdlib directory relative to the compiler or workspace
pub fn find_stdlib_dir() -> Option<PathBuf> {
    // Try relative to current directory (development mode)
    let dev_stdlib = Path::new("stdlib");
    if dev_stdlib.exists() && dev_stdlib.is_dir() {
        return Some(dev_stdlib.to_path_buf());
    }

    // Try relative to executable location
    if let Ok(exe_path) = env::current_exe()
        && let Some(exe_dir) = exe_path.parent()
    {
        // Check exe_dir/stdlib
        let stdlib = exe_dir.join("stdlib");
        if stdlib.exists() && stdlib.is_dir() {
            return Some(stdlib);
        }
        // Check exe_dir/../stdlib (for target/debug or target/release)
        if let Some(parent) = exe_dir.parent() {
            let stdlib = parent.join("stdlib");
            if stdlib.exists() && stdlib.is_dir() {
                return Some(stdlib);
            }
            // Check exe_dir/../../stdlib (for target/debug -> project root)
            if let Some(grandparent) = parent.parent() {
                let stdlib = grandparent.join("stdlib");
                if stdlib.exists() && stdlib.is_dir() {
                    return Some(stdlib);
                }
            }
        }
    }

    // Try INCAN_STDLIB environment variable
    if let Ok(stdlib_path) = env::var("INCAN_STDLIB") {
        let path = PathBuf::from(stdlib_path);
        if path.exists() && path.is_dir() {
            return Some(path);
        }
    }

    None
}

/// Parse a single prelude trait file.
///
/// Returns `Ok(None)` if the file doesn't exist (optional file).
/// Returns `Err` if the file exists but fails to parse (this is an error).
pub fn parse_prelude_file(stdlib_dir: &Path, relative_path: &str) -> Result<Option<ParsedModule>, PreludeError> {
    let path = stdlib_dir.join(relative_path);
    let path_display = path.display().to_string();

    if !path.exists() {
        // File doesn't exist - this is OK for optional prelude files
        return Ok(None);
    }

    let source = fs::read_to_string(&path).map_err(|e| PreludeError {
        file: path_display.clone(),
        message: format!("failed to read: {}", e),
    })?;

    let tokens = lexer::lex(&source).map_err(|errs| {
        let mut msg = String::from("lexer errors:\n");
        for err in &errs {
            msg.push_str(&diagnostics::format_error(&path_display, &source, err));
        }
        PreludeError {
            file: path_display.clone(),
            message: msg,
        }
    })?;

    let ast = parser::parse(&tokens).map_err(|errs| {
        let mut msg = String::from("parser errors:\n");
        for err in &errs {
            msg.push_str(&diagnostics::format_error(&path_display, &source, err));
        }
        PreludeError {
            file: path_display.clone(),
            message: msg,
        }
    })?;

    // Parse path segments from relative path (e.g., "derives/debug.incn" -> ["derives", "debug"])
    let path_segments: Vec<String> = relative_path
        .trim_end_matches(".incn")
        .split('/')
        .map(|s| s.to_string())
        .collect();

    let module_name = path_segments.join("_");

    Ok(Some(ParsedModule {
        name: module_name,
        path_segments,
        file_path: path,
        source,
        ast,
    }))
}

/// Load prelude modules from stdlib.
///
/// The prelude is a **hard dependency**. If the stdlib is found but a prelude
/// file fails to parse, this function returns an error.
///
/// If the stdlib directory cannot be found, returns an empty Vec (graceful degradation
/// for use cases where stdlib isn't available).
pub fn load_prelude() -> Result<Vec<ParsedModule>, PreludeError> {
    let mut prelude_modules = Vec::new();

    let Some(stdlib_dir) = find_stdlib_dir() else {
        // No stdlib found - this is OK, user may be running without stdlib
        return Ok(prelude_modules);
    };

    // Load individual trait files in dependency order
    let prelude_files = [
        "derives/debug.incn",
        "derives/display.incn",
        "derives/eq.incn",
        "derives/ord.incn",
        "derives/clone.incn",
        "derives/default.incn",
    ];

    for file in prelude_files {
        // If file exists and fails to parse, that's an error
        if let Some(module) = parse_prelude_file(&stdlib_dir, file)? {
            prelude_modules.push(module);
        }
    }

    // Load the main prelude file last (it re-exports from the derives)
    if let Some(module) = parse_prelude_file(&stdlib_dir, "prelude.incn")? {
        prelude_modules.push(module);
    }

    Ok(prelude_modules)
}
