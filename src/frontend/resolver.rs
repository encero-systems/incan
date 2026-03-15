//! Module resolution for multi-file Incan projects
//!
//! This module handles discovering and parsing all modules in an Incan project,
//! starting from an entry file and following imports.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use incan::frontend::resolver::{ModuleResolver, ResolvedModule};
//!
//! let resolver = ModuleResolver::new();
//! let modules = resolver.resolve("main.incn")?;
//! ```

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::frontend::ast::{Declaration, ImportKind, Program};
use crate::frontend::diagnostics::CompileError;
use crate::frontend::{diagnostics, lexer, parser};

/// A resolved module with its parsed AST
#[derive(Debug)]
pub struct ResolvedModule {
    /// Module name (e.g., "utils", "models_user")
    pub name: String,
    /// Path segments for nested modules (e.g., ["models", "user"])
    pub path_segments: Vec<String>,
    /// Original source code
    pub source: String,
    /// Parsed AST
    pub ast: Program,
}

/// Error during module resolution
#[derive(Debug)]
pub enum ResolveError {
    /// File could not be read
    FileRead { path: String, error: String },
    /// Lexer errors
    Lexer {
        file: String,
        source: String,
        errors: Vec<CompileError>,
    },
    /// Parser errors
    Parser {
        file: String,
        source: String,
        errors: Vec<CompileError>,
    },
}

impl fmt::Display for ResolveError {
    // Format the error as a string
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::FileRead { path, error } => {
                write!(f, "Error reading '{}': {}", path, error)
            }
            ResolveError::Lexer { file, source, errors } => {
                let mut msg = String::new();
                for err in errors {
                    msg.push_str(&diagnostics::format_error(file, source, err));
                    msg.push('\n');
                }
                write!(f, "{}", msg.trim_end())
            }
            ResolveError::Parser { file, source, errors } => {
                let mut msg = String::new();
                for err in errors {
                    msg.push_str(&diagnostics::format_error(file, source, err));
                    msg.push('\n');
                }
                write!(f, "{}", msg.trim_end())
            }
        }
    }
}

impl std::error::Error for ResolveError {}

/// Module resolver for multi-file projects
///
/// Handles discovering imports and building the dependency graph.
#[derive(Debug, Default)]
pub struct ModuleResolver {
    /// Processed file paths (to avoid cycles)
    processed: HashSet<String>,
}

impl ModuleResolver {
    /// Create a new module resolver
    pub fn new() -> Self {
        Self {
            processed: HashSet::new(),
        }
    }
    /// Resolve all modules starting from an entry file
    ///
    /// This function will resolve all modules starting from an entry file.
    /// It will return a vector of resolved modules.
    ///
    /// # Arguments
    ///
    /// * `entry_path` - The path to the entry file
    ///
    /// # Return value
    /// Vector of resolved modules
    ///
    /// # Errors
    ///
    /// This function will return an error if the entry file cannot be read or if there are errors parsing the modules.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let resolver = ModuleResolver::new();
    /// let modules = resolver.resolve("main.incn")?;
    /// ```
    pub fn resolve(&mut self, entry_path: &str) -> Result<Vec<ResolvedModule>, ResolveError> {
        self.processed.clear();

        let path = Path::new(entry_path);
        let base_dir = path.parent().unwrap_or(Path::new("."));

        let mut modules = Vec::new();
        // (file_path, module_name, path_segments)
        let mut to_process: Vec<(String, String, Vec<String>)> =
            vec![(entry_path.to_string(), "main".to_string(), vec!["main".to_string()])];

        while let Some((file_path, module_name, path_segments)) = to_process.pop() {
            if self.processed.contains(&file_path) {
                continue;
            }
            self.processed.insert(file_path.clone());

            let source = self.read_source(&file_path)?;
            let tokens = lexer::lex(&source).map_err(|errors| ResolveError::Lexer {
                file: file_path.clone(),
                source: source.clone(),
                errors,
            })?;

            let ast = parser::parse_with_module_path(&tokens, Some(file_path.as_str())).map_err(|errors| {
                ResolveError::Parser {
                    file: file_path.clone(),
                    source: source.clone(),
                    errors,
                }
            })?;

            // Find imports and add them to process queue
            for decl in &ast.declarations {
                if let Declaration::Import(import) = &decl.node
                    && let Some(dep_info) = self.resolve_import(&import.kind, base_dir)
                    && !self.processed.contains(&dep_info.0)
                {
                    to_process.push(dep_info);
                }
            }

            modules.push(ResolvedModule {
                name: module_name,
                path_segments,
                source,
                ast,
            });
        }

        Ok(modules)
    }

    fn read_source(&self, path: &str) -> Result<String, ResolveError> {
        fs::read_to_string(path).map_err(|e| ResolveError::FileRead {
            path: path.to_string(),
            error: e.to_string(),
        })
    }

    fn resolve_import(&self, kind: &ImportKind, base_dir: &Path) -> Option<(String, String, Vec<String>)> {
        let import_path = match kind {
            ImportKind::Module(path) if !path.segments.is_empty() => Some(path),
            ImportKind::From { module, .. } if !module.segments.is_empty() => Some(module),
            _ => None,
        }?;

        // Skip stdlib imports
        if import_path.segments.first() == Some(&"std".to_string()) {
            return None;
        }

        let mut target_dir = base_dir.to_path_buf();

        if import_path.is_absolute {
            // Find project root
            let mut project_root = base_dir.to_path_buf();
            while !project_root.join("Cargo.toml").exists() && !project_root.join("src").exists() {
                if let Some(parent) = project_root.parent() {
                    project_root = parent.to_path_buf();
                } else {
                    break;
                }
            }
            if project_root.join("src").exists() {
                target_dir = project_root.join("src");
            } else {
                target_dir = project_root;
            }
        } else {
            for _ in 0..import_path.parent_levels {
                target_dir = target_dir.parent().map(|p| p.to_path_buf()).unwrap_or(target_dir);
            }
        }

        let module_segments = match kind {
            ImportKind::From { module, .. } => module.segments.clone(),
            ImportKind::Module(p) => {
                if p.segments.len() > 1 {
                    p.segments[..p.segments.len() - 1].to_vec()
                } else {
                    p.segments.clone()
                }
            }
            _ => return None,
        };

        if module_segments.is_empty() {
            return None;
        }

        // Build the file path
        let mut dep_path = target_dir.clone();
        for segment in &module_segments {
            dep_path = dep_path.join(segment);
        }
        dep_path.set_extension("incn");

        // Try different path resolutions
        let found_path = self.find_module_file(&dep_path, &target_dir, &module_segments)?;

        let module_name = module_segments.join("_");
        Some((found_path.to_string_lossy().to_string(), module_name, module_segments))
    }

    fn find_module_file(&self, primary_path: &Path, target_dir: &Path, segments: &[String]) -> Option<PathBuf> {
        // Try primary path
        if primary_path.exists() {
            return Some(primary_path.to_path_buf());
        }

        // Try as directory with mod.incn
        let mut mod_path = target_dir.to_path_buf();
        for segment in segments {
            mod_path = mod_path.join(segment);
        }
        mod_path = mod_path.join("mod.incn");
        if mod_path.exists() {
            return Some(mod_path);
        }

        // Try with __init__.incn (Python style)
        let mut init_path = target_dir.to_path_buf();
        for segment in segments {
            init_path = init_path.join(segment);
        }
        init_path = init_path.join("__init__.incn");
        if init_path.exists() {
            return Some(init_path);
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn must_ok<T, E: std::fmt::Display>(result: Result<T, E>, context: &str) -> T {
        match result {
            Ok(value) => value,
            Err(err) => panic!("{context}: {err}"),
        }
    }

    fn must_some<'a>(value: Option<&'a str>, context: &str) -> &'a str {
        match value {
            Some(v) => v,
            None => panic!("{context}"),
        }
    }

    fn unique_temp_dir() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("incan_resolver_test_{}_{pid}_{id}", ts))
    }

    #[test]
    fn test_resolve_single_file() {
        let tmp_dir = unique_temp_dir();
        must_ok(std::fs::create_dir_all(&tmp_dir), "create tmp dir");

        let main_file = tmp_dir.join("main.incn");
        let mut f = must_ok(std::fs::File::create(&main_file), "create main.incn");
        must_ok(writeln!(f, "def main() -> None:"), "write test main signature");
        must_ok(writeln!(f, "    pass"), "write test main body");

        let mut resolver = ModuleResolver::new();
        let main_file_str = must_some(main_file.to_str(), "main file path should be utf-8");
        let modules = must_ok(resolver.resolve(main_file_str), "resolve single file module");

        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].name, "main");

        must_ok(std::fs::remove_dir_all(&tmp_dir), "cleanup tmp dir");
    }

    #[test]
    fn test_resolver_error_on_missing_file() {
        let mut resolver = ModuleResolver::new();
        let result = resolver.resolve("/nonexistent/path/main.incn");
        assert!(result.is_err());
    }
}
