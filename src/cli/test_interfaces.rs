//! Test runner I/O boundary interfaces
//!
//! This module defines trait-based abstractions for the key test runner operations:
//! - Test discovery (filesystem scan + parse)
//! - Harness generation (Incan → Rust compilation + project setup)
//! - Test execution (cargo test invocation + result capture)
//!
//! These interfaces allow for future customization (e.g., dry-run, remote execution)
//! without breaking the current test_runner behavior.

use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::dependency_resolver::InlineRustImport;
use crate::frontend::diagnostics;

/// Errors that occur during test operations
#[derive(Debug, Error)]
pub enum TestError {
    #[error("failed to discover tests: {0}")]
    Discovery(String),

    #[error("lexer error: {0}")]
    Lexer(String),

    #[error("parser error: {0}")]
    Parser(String),

    #[error("code generation failed: {0}")]
    Codegen(String),

    #[error("project generation failed: {0}")]
    ProjectGeneration(String),

    #[error("test execution failed: {0}")]
    Execution(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[allow(unused_imports)]
use super::test_runner::{DiscoveryResult, TestInfo, TestResult};

// ============================================================================
// Test Discovery Interface
// ============================================================================

/// Discover test files and parse them for test/fixture information.
///
/// This trait separates filesystem concerns from test execution logic,
/// allowing for mocking, caching, or remote discovery in the future.
pub trait TestDiscovery {
    /// Find all test files in a path (recursive).
    /// Returns a list of absolute paths to `.incn` test files.
    fn discover_test_files(&self, path: &Path) -> Result<Vec<PathBuf>, TestError>;

    /// Parse a test file for tests and fixtures.
    /// Returns (tests, fixtures).
    fn discover_tests_and_fixtures(&self, file_path: &Path) -> Result<DiscoveryResult, TestError>;
}

// ============================================================================
// Harness Generator Interface
// ============================================================================

/// Code generation and project setup for a single test.
///
/// This trait separates the Incan → Rust compilation and Cargo project
/// creation from execution, allowing for dry-run, inspection, or
/// alternative backends in the future.
pub struct HarnessInput {
    pub source_file: PathBuf,
    pub test_function_name: String,
    pub source_code: String,
}

pub struct HarnessOutput {
    pub rust_code: String,
    pub project_dir: PathBuf,
    pub generated_at: std::time::SystemTime,
}

pub trait HarnessGenerator {
    /// Generate Rust test harness and setup Cargo project.
    /// Returns the project directory and generated code.
    fn generate_harness(&self, input: &HarnessInput) -> Result<HarnessOutput, TestError>;
}

// ============================================================================
// Test Executor Interface
// ============================================================================

/// Execute a compiled test and capture results.
///
/// This trait separates the cargo invocation and output parsing from
/// the test runner orchestration, allowing for custom execution strategies
/// (e.g., timeout, resource limits, custom runner).
pub trait TestExecutor {
    /// Run a compiled test in the given project directory.
    /// Returns (passed, output).
    fn execute_test(&self, project_dir: &Path, test_name: &str) -> Result<(bool, String), TestError>;
}

// ============================================================================
// Default Implementations (Current Behavior)
// ============================================================================

/// Filesystem-based test discovery (current behavior).
pub struct DefaultTestDiscovery;

impl TestDiscovery for DefaultTestDiscovery {
    fn discover_test_files(&self, path: &Path) -> Result<Vec<PathBuf>, TestError> {
        use super::test_runner::discover_test_files;
        Ok(discover_test_files(path))
    }

    fn discover_tests_and_fixtures(&self, file_path: &Path) -> Result<DiscoveryResult, TestError> {
        use super::test_runner::discover_tests_and_fixtures;
        discover_tests_and_fixtures(file_path).map_err(TestError::Discovery)
    }
}

/// Incan → Rust compilation via IrCodegen and ProjectGenerator (current behavior).
pub struct DefaultHarnessGenerator;

impl HarnessGenerator for DefaultHarnessGenerator {
    fn generate_harness(&self, input: &HarnessInput) -> Result<HarnessOutput, TestError> {
        use crate::backend::{IrCodegen, ProjectGenerator};
        use crate::dependency_resolver::resolve_dependencies;
        use crate::frontend::{lexer, parser};
        use crate::lockfile::CargoFeatureSelection;
        use crate::manifest::ProjectManifest;

        let tokens = lexer::lex(&input.source_code).map_err(|e| TestError::Lexer(format!("{:?}", e)))?;

        let ast = parser::parse(&tokens).map_err(|e| TestError::Parser(format!("{:?}", e)))?;

        let inline_imports = collect_inline_rust_imports(&ast, &input.source_file);
        let manifest = ProjectManifest::discover(input.source_file.parent().unwrap_or_else(|| Path::new(".")))
            .map_err(|e| TestError::ProjectGeneration(e.to_string()))?;
        let cargo_features = CargoFeatureSelection::default();
        let resolved = resolve_dependencies(manifest.as_ref(), &inline_imports, true, &cargo_features)
            .map_err(|errors| format_dependency_errors(&errors, &input.source_file, &input.source_code))?;

        let mut codegen = IrCodegen::new();
        codegen.set_test_mode(true);
        codegen.set_test_function(&input.test_function_name);

        let rust_code = codegen
            .try_generate(&ast)
            .map_err(|e| TestError::Codegen(format!("{}", e)))?;

        let project_dir = PathBuf::from(format!("target/incan_tests/{}", input.test_function_name));

        let mut generator = ProjectGenerator::new(&project_dir, "test_runner", true);
        generator.set_include_dev_dependencies(true);
        generator.set_dependencies(resolved.dependencies);
        generator.set_dev_dependencies(resolved.dev_dependencies);
        generator
            .generate(&rust_code)
            .map_err(|e| TestError::ProjectGeneration(e.to_string()))?;

        Ok(HarnessOutput {
            rust_code,
            project_dir,
            generated_at: std::time::SystemTime::now(),
        })
    }
}

/// Collect inline Rust crate imports from an AST.
fn collect_inline_rust_imports(ast: &crate::frontend::ast::Program, file_path: &Path) -> Vec<InlineRustImport> {
    let mut imports = Vec::new();
    for decl in &ast.declarations {
        let crate::frontend::ast::Declaration::Import(import) = &decl.node else {
            continue;
        };
        match &import.kind {
            crate::frontend::ast::ImportKind::RustCrate {
                crate_name,
                version,
                features,
                ..
            } => {
                imports.push(InlineRustImport {
                    crate_name: crate_name.clone(),
                    version: version.clone(),
                    features: features.clone(),
                    span: decl.span,
                    file_path: file_path.to_path_buf(),
                    is_test_context: true,
                });
            }
            crate::frontend::ast::ImportKind::RustFrom {
                crate_name,
                version,
                features,
                ..
            } => {
                imports.push(InlineRustImport {
                    crate_name: crate_name.clone(),
                    version: version.clone(),
                    features: features.clone(),
                    span: decl.span,
                    file_path: file_path.to_path_buf(),
                    is_test_context: true,
                });
            }
            _ => {}
        }
    }
    imports
}

/// Format dependency errors for a test.
fn format_dependency_errors(
    errors: &[crate::dependency_resolver::DependencyError],
    file_path: &Path,
    source: &str,
) -> TestError {
    let mut msg = String::new();
    for err in errors {
        if err.file_path == file_path {
            msg.push_str(&diagnostics::format_error(
                &file_path.to_string_lossy(),
                source,
                &err.error,
            ));
            continue;
        }

        if let Ok(other_source) = std::fs::read_to_string(&err.file_path) {
            msg.push_str(&diagnostics::format_error(
                &err.file_path.to_string_lossy(),
                &other_source,
                &err.error,
            ));
            continue;
        }

        msg.push_str(&format!(
            "error: {}\n  --> {}\n",
            err.error.message,
            err.file_path.display()
        ));
    }
    TestError::ProjectGeneration(msg.trim_end().to_string())
}

/// Cargo test execution with output capture (current behavior).
pub struct DefaultTestExecutor;

impl TestExecutor for DefaultTestExecutor {
    fn execute_test(&self, project_dir: &Path, _test_name: &str) -> Result<(bool, String), TestError> {
        let output = std::process::Command::new("cargo")
            .arg("test")
            .arg("--")
            .arg("--nocapture")
            .current_dir(project_dir)
            .output()
            .map_err(|e| TestError::Execution(format!("Failed to run cargo test: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let combined = format!("{}\n{}", stdout, stderr);

        Ok((output.status.success(), combined))
    }
}
