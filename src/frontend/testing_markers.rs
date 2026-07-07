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
const TESTING_FIXTURE_TIMEOUT_ARG_KEY: &str = "timeout";

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
    Test,
    Fixture,
    Skip,
    SkipIf,
    XFail,
    XFailIf,
    Slow,
    Mark,
    Resource,
    Serial,
    Timeout,
    Parametrize,
}

impl TestingMarkerKind {
    /// Convert the `kind` value from std.testing marker metadata into the runner enum.
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "test" => Some(Self::Test),
            "fixture" => Some(Self::Fixture),
            "skip" => Some(Self::Skip),
            "skipif" => Some(Self::SkipIf),
            "xfail" => Some(Self::XFail),
            "xfailif" => Some(Self::XFailIf),
            "slow" => Some(Self::Slow),
            "mark" => Some(Self::Mark),
            "resource" => Some(Self::Resource),
            "serial" => Some(Self::Serial),
            "timeout" => Some(Self::Timeout),
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

/// Scope accepted by `std.testing.fixture` declarations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TestingFixtureScope {
    /// Recreate the fixture for each expanded test case.
    Function,
    /// Reuse the fixture across tests in one source module.
    Module,
    /// Reuse the fixture across the whole test session.
    Session,
}

impl TestingFixtureScope {
    /// Resolve a source-level scope value using the data-driven spellings loaded from `std.testing`.
    pub fn from_marker_value(value: &str, semantics: &TestingMarkerSemantics) -> Option<Self> {
        match value {
            value if value == semantics.fixture_scope_function.as_str() => Some(Self::Function),
            value if value == semantics.fixture_scope_module.as_str() => Some(Self::Module),
            value if value == semantics.fixture_scope_session.as_str() => Some(Self::Session),
            _ => None,
        }
    }
}

impl Default for TestingFixtureScope {
    /// Return the default fixture scope used when `@fixture` omits `scope=...`.
    fn default() -> Self {
        Self::Function
    }
}

/// Parsed declaration-time arguments from one `@fixture(...)` marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestingFixtureMarkerArgs {
    /// Declared fixture scope after applying the `std.testing` metadata spelling table.
    pub scope: TestingFixtureScope,
    /// Whether `@fixture(autouse=true)` was set.
    pub autouse: bool,
    /// Span of an unsupported `timeout=` fixture argument, when present.
    pub unsupported_timeout_span: Option<ast::Span>,
}

impl Default for TestingMarkerSemantics {
    /// Return fixture defaults used while strict marker metadata is loaded from stdlib source.
    fn default() -> Self {
        Self {
            marker_kinds: HashMap::new(),
            fixture_scope_arg: "scope".to_string(),
            fixture_autouse_arg: "autouse".to_string(),
            fixture_scope_function: "function".to_string(),
            fixture_scope_module: "module".to_string(),
            fixture_scope_session: "session".to_string(),
        }
    }
}

impl TestingMarkerSemantics {
    /// Return the marker kind associated with a `std.testing` function name.
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

/// Resolve and parse the first `@fixture(...)` marker on a declaration.
///
/// The returned metadata is intentionally declaration-only: it captures static fixture configuration and flags
/// unsupported per-fixture timeout spelling, but it does not evaluate fixture bodies or runner lifecycle behavior.
pub fn resolve_testing_fixture_marker_args(
    decorators: &[ast::Spanned<ast::Decorator>],
    aliases: &HashMap<String, Vec<String>>,
    semantics: &TestingMarkerSemantics,
) -> Option<TestingFixtureMarkerArgs> {
    for dec in decorators {
        if resolve_testing_marker_kind(&dec.node, aliases, semantics) != Some(TestingMarkerKind::Fixture) {
            continue;
        }

        let mut scope = TestingFixtureScope::default();
        let mut autouse = false;
        let mut unsupported_timeout_span = None;

        for arg in &dec.node.args {
            let ast::DecoratorArg::Named(name, value) = arg else {
                continue;
            };
            if name == &semantics.fixture_scope_arg {
                if let ast::DecoratorArgValue::Expr(expr) = value
                    && let ast::Expr::Literal(ast::Literal::String(value)) = &expr.node
                    && let Some(parsed_scope) = TestingFixtureScope::from_marker_value(value, semantics)
                {
                    scope = parsed_scope;
                }
            } else if name == &semantics.fixture_autouse_arg {
                if let ast::DecoratorArgValue::Expr(expr) = value
                    && let ast::Expr::Literal(ast::Literal::Bool(value)) = &expr.node
                {
                    autouse = *value;
                }
            } else if name == TESTING_FIXTURE_TIMEOUT_ARG_KEY {
                unsupported_timeout_span = Some(match value {
                    ast::DecoratorArgValue::Expr(expr) => expr.span,
                    ast::DecoratorArgValue::Type(ty) => ty.span,
                });
            }
        }

        return Some(TestingFixtureMarkerArgs {
            scope,
            autouse,
            unsupported_timeout_span,
        });
    }

    None
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
/// 3. Toolchain-relative paths from the current executable, including symlinked launchers
/// 4. `$CWD/crates/incan_stdlib/<relative>`
/// 5. `$CWD/<relative>`
/// 6. `$INCAN_STDLIB_PATH/<relative>` for installed layouts
fn find_stdlib_file(relative: &str) -> Option<PathBuf> {
    // 1. Explicit override root (runtime).
    if let Ok(dir) = std::env::var("INCAN_STDLIB_DIR")
        && let Some(path) = find_stdlib_file_in_root(relative, PathBuf::from(dir))
    {
        return Some(path);
    }

    // 2. Development build: workspace-relative (compile-time path).
    // CARGO_MANIFEST_DIR is captured at compile time and points to the workspace root.
    let workspace_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("crates/incan_stdlib")
        .join(relative);
    if workspace_path.exists() {
        return Some(workspace_path);
    }

    // 3. Relative to executable location, covering installed toolchains, symlinked launchers, and local target builds.
    if let Some(path) = find_stdlib_file_in_bases(relative, crate::toolchain_layout::current_executable_search_bases())
    {
        return Some(path);
    }

    // 4-5. Relative to current working directory.
    if let Ok(cwd) = std::env::current_dir()
        && let Some(path) = find_stdlib_file_in_bases(relative, [cwd])
    {
        return Some(path);
    }

    // 6. Installed stdlib path (runtime, for production installs).
    if let Ok(stdlib_root) = std::env::var("INCAN_STDLIB_PATH")
        && let Some(path) = find_stdlib_file_in_root(relative, PathBuf::from(stdlib_root))
    {
        return Some(path);
    }

    tracing::debug!(relative_path = %relative, "stdlib file not found in any search path");
    None
}

/// Find a stdlib file under one explicit root directory.
fn find_stdlib_file_in_root(relative: &str, root: PathBuf) -> Option<PathBuf> {
    let path = root.join(relative);
    path.exists().then_some(path)
}

/// Find a stdlib file under candidate base directories.
fn find_stdlib_file_in_bases(relative: &str, bases: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    for base in bases {
        let crate_local = base.join("crates/incan_stdlib").join(relative);
        if crate_local.exists() {
            return Some(crate_local);
        }
        let local = base.join(relative);
        if local.exists() {
            return Some(local);
        }
    }
    None
}

/// Extract compile-time semantics from a testing marker expression.
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
    validate_testing_marker_inventory(&semantics)?;
    Ok(semantics)
}

/// Validate the compile-time testing marker inventory.
fn validate_testing_marker_inventory(semantics: &TestingMarkerSemantics) -> Result<(), TestingMarkerLoadError> {
    let expected_names = incan_core::lang::testing::RUNNER_ONLY_MARKER_NAMES;
    let mut missing = Vec::new();
    let mut mismatched = Vec::new();

    for expected_name in expected_names {
        let Some(actual_kind) = semantics.marker_kinds.get(*expected_name) else {
            missing.push(*expected_name);
            continue;
        };
        let expected_kind = TestingMarkerKind::from_str(expected_name).ok_or_else(|| {
            TestingMarkerLoadError::new(format!(
                "runtime marker inventory contains unknown marker `{expected_name}`"
            ))
        })?;
        if actual_kind != &expected_kind {
            mismatched.push(format!(
                "{expected_name} declares {actual_kind:?}, expected {expected_kind:?}"
            ));
        }
    }

    let unexpected = semantics
        .marker_kinds
        .keys()
        .filter(|name| !expected_names.contains(&name.as_str()))
        .cloned()
        .collect::<Vec<_>>();

    if !missing.is_empty() || !unexpected.is_empty() || !mismatched.is_empty() {
        return Err(TestingMarkerLoadError::new(format!(
            "std.testing marker metadata does not match runtime marker inventory; missing={missing:?}, unexpected={unexpected:?}, mismatched={mismatched:?}"
        )));
    }

    Ok(())
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
    let mut runner_only = false;
    let mut fixture_scope_arg: Option<String> = None;
    let mut fixture_autouse_arg: Option<String> = None;
    let mut fixture_scopes: Option<[String; 3]> = None;

    for entry in entries {
        let ast::DictEntry::Pair(key_expr, value_expr) = entry else {
            return Err(TestingMarkerLoadError::new(
                "malformed @rust.extern metadata for std.testing marker: spread entries are not supported",
            ));
        };
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
            TESTING_MARKER_RUNNER_ONLY_KEY => {
                let Some(value) = expr_as_bool_literal(value_expr) else {
                    return Err(TestingMarkerLoadError::new(
                        "malformed runner_only metadata value (expected bool)",
                    ));
                };
                runner_only = value;
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

    if !runner_only {
        return Err(TestingMarkerLoadError::new(
            "std.testing marker metadata must declare runner_only=true",
        ));
    }

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

    let ast::ListEntry::Element(first_expr) = &items[0] else {
        return None;
    };
    let ast::ListEntry::Element(second_expr) = &items[1] else {
        return None;
    };
    let ast::ListEntry::Element(third_expr) = &items[2] else {
        return None;
    };
    let first = expr_as_string_literal(first_expr)?;
    let second = expr_as_string_literal(second_expr)?;
    let third = expr_as_string_literal(third_expr)?;
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
    fn test_std_testing_metadata_matches_runtime_marker_names() -> Result<(), Box<dyn std::error::Error>> {
        let semantics = load_testing_marker_semantics_from_stdlib()?;
        let mut metadata_names: Vec<&str> = semantics.marker_kinds.keys().map(String::as_str).collect();
        metadata_names.sort_unstable();

        let mut runtime_names = incan_core::lang::testing::RUNNER_ONLY_MARKER_NAMES.to_vec();
        runtime_names.sort_unstable();

        assert_eq!(metadata_names, runtime_names);
        Ok(())
    }

    #[test]
    fn testing_marker_source_lookup_accepts_installed_toolchain_base() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let toolchain_root = tmp.path().join("toolchains/0.4.0-test");
        let stdlib_dir = toolchain_root.join("stdlib");
        std::fs::create_dir_all(&stdlib_dir)?;
        std::fs::write(stdlib_dir.join("testing.incn"), "")?;

        let found = find_stdlib_file_in_bases("stdlib/testing.incn", [toolchain_root])
            .ok_or("expected installed stdlib/testing.incn to be resolved")?;

        assert!(found.ends_with("stdlib/testing.incn"));
        Ok(())
    }

    #[test]
    fn testing_marker_source_lookup_accepts_explicit_installed_stdlib_root() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let stdlib_root = tmp.path().join("installed");
        let stdlib_dir = stdlib_root.join("stdlib");
        std::fs::create_dir_all(&stdlib_dir)?;
        std::fs::write(stdlib_dir.join("testing.incn"), "")?;

        let found = find_stdlib_file_in_root("stdlib/testing.incn", stdlib_root)
            .ok_or("expected explicit installed stdlib root to resolve stdlib/testing.incn")?;

        assert!(found.ends_with("stdlib/testing.incn"));
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

    #[test]
    fn test_testing_marker_semantics_rejects_non_runner_only_marker() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
@rust.extern(metadata={"marker_kind": "skip", "runner_only": false})
def skip(reason: str = "") -> None:
    ...
"#;
        let tokens = match crate::frontend::lexer::lex(source) {
            Ok(tokens) => tokens,
            Err(errs) => return Err(format!("lex failed for non-runner-only annotation fixture: {errs:?}").into()),
        };
        let program = match crate::frontend::parser::parse(&tokens) {
            Ok(program) => program,
            Err(errs) => return Err(format!("parse failed for non-runner-only annotation fixture: {errs:?}").into()),
        };

        let extracted = extract_testing_marker_semantics(&program);
        assert!(
            extracted
                .as_ref()
                .is_err_and(|err| err.to_string().contains("runner_only=true")),
            "non-runner-only marker annotation should fail extraction; got: {extracted:?}"
        );
        Ok(())
    }

    #[test]
    fn test_testing_marker_semantics_rejects_incomplete_marker_inventory() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
@rust.extern(metadata={"marker_kind": "skip", "runner_only": true})
def skip(reason: str = "") -> None:
    ...
"#;
        let tokens = match crate::frontend::lexer::lex(source) {
            Ok(tokens) => tokens,
            Err(errs) => return Err(format!("lex failed for incomplete marker inventory fixture: {errs:?}").into()),
        };
        let program = match crate::frontend::parser::parse(&tokens) {
            Ok(program) => program,
            Err(errs) => return Err(format!("parse failed for incomplete marker inventory fixture: {errs:?}").into()),
        };

        let extracted = extract_testing_marker_semantics(&program);
        assert!(
            extracted
                .as_ref()
                .is_err_and(|err| err.to_string().contains("runtime marker inventory")),
            "incomplete marker inventory should fail extraction; got: {extracted:?}"
        );
        Ok(())
    }

    #[test]
    fn test_testing_marker_semantics_rejects_function_kind_mismatch() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
@rust.extern(metadata={"marker_kind": "xfail", "runner_only": true})
def skip(reason: str = "") -> None:
    ...
"#;
        let tokens = match crate::frontend::lexer::lex(source) {
            Ok(tokens) => tokens,
            Err(errs) => return Err(format!("lex failed for mismatched marker fixture: {errs:?}").into()),
        };
        let program = match crate::frontend::parser::parse(&tokens) {
            Ok(program) => program,
            Err(errs) => return Err(format!("parse failed for mismatched marker fixture: {errs:?}").into()),
        };

        let extracted = extract_testing_marker_semantics(&program);
        assert!(
            extracted
                .as_ref()
                .is_err_and(|err| err.to_string().contains("mismatched")),
            "mismatched marker inventory should fail extraction; got: {extracted:?}"
        );
        Ok(())
    }
}
