//! `std.testing` marker semantics shared across frontend and CLI.
//!
//! This module owns marker metadata extraction from `stdlib/testing.incn` and provides a stable API for resolving
//! decorator marker kinds.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::frontend::ast;
use crate::frontend::decorator_resolution;
use incan_core::lang::stdlib;

const RUST_EXTERN_NAMESPACE: &str = "rust";
const RUST_EXTERN_DECORATOR: &str = "extern";
const RUST_EXTERN_METADATA_ARG: &str = "metadata";
const TESTING_MARKER_KIND_KEY: &str = "marker_kind";
const TESTING_MARKER_RUNNER_ONLY_KEY: &str = "runner_only";
const TESTING_FIXTURE_SCOPE_ARG_KEY: &str = "scope_arg";
const TESTING_FIXTURE_AUTOUSE_ARG_KEY: &str = "autouse_arg";
const TESTING_FIXTURE_SCOPES_KEY: &str = "scopes";

/// Error type for strict `std.testing` marker metadata loading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestingMarkerLoadError {
    message: String,
}

impl TestingMarkerLoadError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for TestingMarkerLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for TestingMarkerLoadError {}

/// Supported `std.testing` marker kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TestingMarkerKind {
    Fixture,
    Skip,
    XFail,
    Slow,
    Parametrize,
}

impl TestingMarkerKind {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "fixture" => Some(Self::Fixture),
            "skip" => Some(Self::Skip),
            "xfail" => Some(Self::XFail),
            "slow" => Some(Self::Slow),
            "parametrize" => Some(Self::Parametrize),
            _ => None,
        }
    }
}

/// Data-driven marker semantics loaded from `stdlib/testing.incn`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestingMarkerSemantics {
    pub marker_kinds: HashMap<String, TestingMarkerKind>,
    pub fixture_scope_arg: String,
    pub fixture_autouse_arg: String,
    pub fixture_scope_function: String,
    pub fixture_scope_module: String,
    pub fixture_scope_session: String,
}

impl Default for TestingMarkerSemantics {
    fn default() -> Self {
        let mut marker_kinds = HashMap::new();
        marker_kinds.insert("fixture".to_string(), TestingMarkerKind::Fixture);
        marker_kinds.insert("skip".to_string(), TestingMarkerKind::Skip);
        marker_kinds.insert("xfail".to_string(), TestingMarkerKind::XFail);
        marker_kinds.insert("slow".to_string(), TestingMarkerKind::Slow);
        marker_kinds.insert("parametrize".to_string(), TestingMarkerKind::Parametrize);

        Self {
            marker_kinds,
            fixture_scope_arg: "scope".to_string(),
            fixture_autouse_arg: "autouse".to_string(),
            fixture_scope_function: "function".to_string(),
            fixture_scope_module: "module".to_string(),
            fixture_scope_session: "session".to_string(),
        }
    }
}

impl TestingMarkerSemantics {
    pub fn marker_kind(&self, function_name: &str) -> Option<TestingMarkerKind> {
        self.marker_kinds.get(function_name).copied()
    }
}

/// Load and cache marker semantics from `stdlib/testing.incn`.
///
/// Loading is strict: malformed or missing metadata is an error.
pub fn load_testing_marker_semantics() -> Result<TestingMarkerSemantics, TestingMarkerLoadError> {
    static CACHED: OnceLock<Result<TestingMarkerSemantics, TestingMarkerLoadError>> = OnceLock::new();
    CACHED.get_or_init(load_testing_marker_semantics_from_stdlib).clone()
}

/// Resolve a decorator to its testing marker kind, if any.
pub fn resolve_testing_marker_kind(
    dec: &ast::Decorator,
    aliases: &HashMap<String, Vec<String>>,
    semantics: &TestingMarkerSemantics,
) -> Option<TestingMarkerKind> {
    let resolved = decorator_resolution::resolve_decorator_path(dec, aliases);
    if resolved.len() < 3 || resolved[0] != stdlib::STDLIB_ROOT || resolved[1] != "testing" {
        return None;
    }
    semantics.marker_kind(resolved[2].as_str())
}

/// Load testing marker semantics from `stdlib/testing.incn`.
fn load_testing_marker_semantics_from_stdlib() -> Result<TestingMarkerSemantics, TestingMarkerLoadError> {
    let relative = stdlib::stdlib_stub_path(&[stdlib::STDLIB_ROOT.to_string(), "testing".to_string()])
        .ok_or_else(|| TestingMarkerLoadError::new("missing std.testing stub path mapping in stdlib registry"))?;
    let abs_path = find_stdlib_file(&relative).ok_or_else(|| {
        TestingMarkerLoadError::new(format!(
            "could not locate std.testing source at relative path `{relative}`"
        ))
    })?;

    let source = std::fs::read_to_string(&abs_path).map_err(|e| {
        TestingMarkerLoadError::new(format!(
            "failed to read std.testing source `{}`: {e}",
            abs_path.display()
        ))
    })?;

    let tokens = crate::frontend::lexer::lex(&source).map_err(|e| {
        TestingMarkerLoadError::new(format!(
            "failed to lex std.testing source `{}`: {e:?}",
            abs_path.display()
        ))
    })?;

    let path_display = abs_path.to_string_lossy();
    let program =
        crate::frontend::parser::parse_with_module_path(&tokens, Some(path_display.as_ref())).map_err(|e| {
            TestingMarkerLoadError::new(format!(
                "failed to parse std.testing source `{}`: {e:?}",
                abs_path.display()
            ))
        })?;

    extract_testing_marker_semantics(&program)
}

/// Find the absolute path for a stdlib file given its relative path (e.g. `"stdlib/testing.incn"`).
///
/// Search order:
/// 1. `$INCAN_STDLIB_DIR/<relative>` if the env var is set (runtime)
/// 2. `$CARGO_MANIFEST_DIR/crates/incan_stdlib/<relative>` (compile-time workspace path)
/// 3. `$CWD/crates/incan_stdlib/<relative>`
/// 4. `$CWD/<relative>`
fn find_stdlib_file(relative: &str) -> Option<PathBuf> {
    // 1. Explicit override root (runtime).
    if let Ok(dir) = std::env::var("INCAN_STDLIB_DIR") {
        let p = PathBuf::from(dir).join(relative);
        if p.exists() {
            return Some(p);
        }
    }

    // 2. Development build: workspace-relative (compile-time path).
    // CARGO_MANIFEST_DIR is captured at compile time and points to the workspace root.
    let workspace_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("crates/incan_stdlib")
        .join(relative);
    if workspace_path.exists() {
        return Some(workspace_path);
    }

    // 3-4. Relative to current working directory.
    if let Ok(cwd) = std::env::current_dir() {
        let crate_local = cwd.join("crates/incan_stdlib").join(relative);
        if crate_local.exists() {
            return Some(crate_local);
        }
        let local = cwd.join(relative);
        if local.exists() {
            return Some(local);
        }
    }

    tracing::debug!(relative_path = %relative, "stdlib file not found in any search path");
    None
}

fn extract_testing_marker_semantics(program: &ast::Program) -> Result<TestingMarkerSemantics, TestingMarkerLoadError> {
    let mut semantics = TestingMarkerSemantics::default();
    let mut saw_markers = false;

    for decl in &program.declarations {
        let ast::Declaration::Function(func) = &decl.node else {
            continue;
        };

        for dec in &func.decorators {
            let metadata = match rust_extern_testing_metadata(&dec.node)? {
                Some(metadata) => metadata,
                None => continue,
            };
            saw_markers = true;
            semantics.marker_kinds.insert(func.name.clone(), metadata.kind);

            if metadata.kind == TestingMarkerKind::Fixture {
                if let Some(scope_arg) = metadata.fixture_scope_arg {
                    semantics.fixture_scope_arg = scope_arg;
                }
                if let Some(autouse_arg) = metadata.fixture_autouse_arg {
                    semantics.fixture_autouse_arg = autouse_arg;
                }
                if let Some([function_scope, module_scope, session_scope]) = metadata.fixture_scopes {
                    semantics.fixture_scope_function = function_scope;
                    semantics.fixture_scope_module = module_scope;
                    semantics.fixture_scope_session = session_scope;
                }
            }
        }
    }

    if !saw_markers {
        return Err(TestingMarkerLoadError::new(
            "std.testing does not declare any marker metadata (`@rust.extern(metadata={\"marker_kind\": ...})`)",
        ));
    }
    Ok(semantics)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestingMarkerAnnotation {
    kind: TestingMarkerKind,
    fixture_scope_arg: Option<String>,
    fixture_autouse_arg: Option<String>,
    fixture_scopes: Option<[String; 3]>,
}

/// Extract testing marker metadata from a `@rust.extern` decorator.
fn rust_extern_testing_metadata(
    dec: &ast::Decorator,
) -> Result<Option<TestingMarkerAnnotation>, TestingMarkerLoadError> {
    if !is_rust_extern_decorator(dec) {
        return Ok(None);
    }

    for arg in &dec.args {
        match arg {
            ast::DecoratorArg::Named(name, ast::DecoratorArgValue::Expr(expr)) if name == RUST_EXTERN_METADATA_ARG => {
                return parse_testing_metadata_dict(expr);
            }
            _ => {}
        }
    }
    Ok(None)
}

/// Parse testing marker metadata from a dictionary expression.
fn parse_testing_metadata_dict(
    metadata_expr: &ast::Spanned<ast::Expr>,
) -> Result<Option<TestingMarkerAnnotation>, TestingMarkerLoadError> {
    let ast::Expr::Dict(entries) = &metadata_expr.node else {
        return Err(TestingMarkerLoadError::new(
            "malformed @rust.extern metadata for std.testing marker: expected dict",
        ));
    };

    let mut kind: Option<TestingMarkerKind> = None;
    let mut fixture_scope_arg: Option<String> = None;
    let mut fixture_autouse_arg: Option<String> = None;
    let mut fixture_scopes: Option<[String; 3]> = None;

    for (key_expr, value_expr) in entries {
        let Some(key) = expr_as_string_literal(key_expr) else {
            return Err(TestingMarkerLoadError::new(
                "malformed @rust.extern metadata for std.testing marker: non-string key",
            ));
        };
        match key.as_str() {
            TESTING_MARKER_KIND_KEY => {
                let Some(kind_name) = expr_as_string_literal(value_expr) else {
                    return Err(TestingMarkerLoadError::new(
                        "malformed marker_kind metadata value (expected string)",
                    ));
                };
                let Some(parsed_kind) = TestingMarkerKind::from_str(kind_name.as_str()) else {
                    return Err(TestingMarkerLoadError::new(format!(
                        "unknown marker_kind metadata value `{kind_name}`"
                    )));
                };
                kind = Some(parsed_kind);
            }
            TESTING_MARKER_RUNNER_ONLY_KEY if expr_as_bool_literal(value_expr).is_none() => {
                return Err(TestingMarkerLoadError::new(
                    "malformed runner_only metadata value (expected bool)",
                ));
            }
            TESTING_FIXTURE_SCOPE_ARG_KEY => {
                let Some(value) = expr_as_string_literal(value_expr) else {
                    return Err(TestingMarkerLoadError::new(
                        "malformed scope_arg metadata value (expected string)",
                    ));
                };
                fixture_scope_arg = Some(value);
            }
            TESTING_FIXTURE_AUTOUSE_ARG_KEY => {
                let Some(value) = expr_as_string_literal(value_expr) else {
                    return Err(TestingMarkerLoadError::new(
                        "malformed autouse_arg metadata value (expected string)",
                    ));
                };
                fixture_autouse_arg = Some(value);
            }
            TESTING_FIXTURE_SCOPES_KEY => {
                let Some(scopes) = expr_as_string_triplet(value_expr) else {
                    return Err(TestingMarkerLoadError::new(
                        "malformed scopes metadata value (expected list of three strings)",
                    ));
                };
                fixture_scopes = Some(scopes);
            }
            _ => {}
        }
    }

    let Some(kind) = kind else {
        // Not a testing marker metadata blob.
        return Ok(None);
    };

    Ok(Some(TestingMarkerAnnotation {
        kind,
        fixture_scope_arg,
        fixture_autouse_arg,
        fixture_scopes,
    }))
}

/// Check if a decorator is a `@rust.extern` decorator.
fn is_rust_extern_decorator(dec: &ast::Decorator) -> bool {
    dec.path.parent_levels == 0
        && !dec.path.is_absolute
        && dec.path.segments.len() == 2
        && dec.path.segments[0] == RUST_EXTERN_NAMESPACE
        && dec.path.segments[1] == RUST_EXTERN_DECORATOR
}

/// Convert an expression to a string literal.
fn expr_as_string_literal(expr: &ast::Spanned<ast::Expr>) -> Option<String> {
    if let ast::Expr::Literal(ast::Literal::String(value)) = &expr.node {
        return Some(value.clone());
    }
    None
}

/// Convert an expression to a boolean literal.
fn expr_as_bool_literal(expr: &ast::Spanned<ast::Expr>) -> Option<bool> {
    if let ast::Expr::Literal(ast::Literal::Bool(value)) = &expr.node {
        return Some(*value);
    }
    None
}

/// Convert an expression to a string triplet.
fn expr_as_string_triplet(expr: &ast::Spanned<ast::Expr>) -> Option<[String; 3]> {
    let ast::Expr::List(items) = &expr.node else {
        return None;
    };
    if items.len() != 3 {
        return None;
    }

    let first = expr_as_string_literal(&items[0])?;
    let second = expr_as_string_literal(&items[1])?;
    let third = expr_as_string_literal(&items[2])?;
    Some([first, second, third])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_testing_marker_semantics_from_stdlib() -> Result<(), Box<dyn std::error::Error>> {
        let semantics = load_testing_marker_semantics_from_stdlib()?;
        assert_eq!(semantics.marker_kind("skip"), Some(TestingMarkerKind::Skip));
        assert_eq!(semantics.marker_kind("fixture"), Some(TestingMarkerKind::Fixture));
        assert_eq!(semantics.fixture_scope_arg, "scope");
        assert_eq!(semantics.fixture_autouse_arg, "autouse");
        Ok(())
    }

    #[test]
    fn test_testing_marker_semantics_malformed_annotation_is_error() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
@rust.extern(metadata={"marker_kind": "skip", "runner_only": true})
def skip(reason: str = "") -> None:
    ...

@rust.extern(metadata={"marker_kind": 123})
def xfail(reason: str = "") -> None:
    ...
"#;
        let tokens = match crate::frontend::lexer::lex(source) {
            Ok(tokens) => tokens,
            Err(errs) => return Err(format!("lex failed for malformed annotation fixture: {errs:?}").into()),
        };
        let program = match crate::frontend::parser::parse(&tokens) {
            Ok(program) => program,
            Err(errs) => return Err(format!("parse failed for malformed annotation fixture: {errs:?}").into()),
        };

        let extracted = extract_testing_marker_semantics(&program);
        assert!(extracted.is_err(), "malformed marker annotation should fail extraction");
        Ok(())
    }
}
