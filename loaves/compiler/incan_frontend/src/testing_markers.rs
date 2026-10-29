//! `std.testing` marker semantics shared across frontend and CLI.
//!
//! This module reads marker metadata from the checked manifest of the standard package owning `std.testing` and
//! provides a stable API for resolving decorator marker kinds.

use std::collections::HashMap;
use std::fmt;

use crate::api_metadata::{ApiDeclaration, DecoratorArgMetadata, DecoratorValue, SafeMetadataValue};
use crate::ast;
use crate::decorator_resolution;
use crate::library_manifest::LibraryManifest;
use incan_lang::lang::stdlib;

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
    /// Construct a strict marker-metadata error with one user-facing explanation.
    pub fn new(message: impl Into<String>) -> Self {
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

/// Data-driven marker semantics read from the checked metadata of the standard package owning `std.testing`.
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

/// Recover testing marker semantics from one checked SDK provider manifest without reopening provider source.
pub fn testing_marker_semantics_from_manifest(
    manifest: &LibraryManifest,
) -> Result<Option<TestingMarkerSemantics>, TestingMarkerLoadError> {
    let Some(api) = manifest.contract_metadata.api.as_ref() else {
        return Ok(None);
    };
    let mut semantics = TestingMarkerSemantics::default();
    let mut saw_markers = false;
    for module in &api.modules {
        if module.module_path.as_slice() != ["testing"] {
            continue;
        }
        for declaration in &module.declarations {
            let ApiDeclaration::Function(function) = declaration else {
                continue;
            };
            for decorator in &function.decorators {
                if decorator.path.as_slice() != [RUST_EXTERN_NAMESPACE, RUST_EXTERN_DECORATOR] {
                    continue;
                }
                let Some(metadata) = decorator.args.iter().find_map(|argument| match argument {
                    DecoratorArgMetadata::Named { name, value } if name == RUST_EXTERN_METADATA_ARG => Some(value),
                    _ => None,
                }) else {
                    continue;
                };
                let Some(annotation) = testing_annotation_from_checked_metadata(metadata)? else {
                    continue;
                };
                saw_markers = true;
                apply_testing_marker_annotation(&mut semantics, &function.name, annotation);
            }
        }
    }
    if !saw_markers {
        return Ok(None);
    }
    validate_testing_marker_inventory(&semantics)?;
    Ok(Some(semantics))
}

/// Merge one checked testing decorator annotation into the marker inventory used by the runner.
fn apply_testing_marker_annotation(
    semantics: &mut TestingMarkerSemantics,
    function_name: &str,
    annotation: TestingMarkerAnnotation,
) {
    semantics
        .marker_kinds
        .insert(function_name.to_string(), annotation.kind);
    if annotation.kind != TestingMarkerKind::Fixture {
        return;
    }
    if let Some(scope_arg) = annotation.fixture_scope_arg {
        semantics.fixture_scope_arg = scope_arg;
    }
    if let Some(autouse_arg) = annotation.fixture_autouse_arg {
        semantics.fixture_autouse_arg = autouse_arg;
    }
    if let Some([function_scope, module_scope, session_scope]) = annotation.fixture_scopes {
        semantics.fixture_scope_function = function_scope;
        semantics.fixture_scope_module = module_scope;
        semantics.fixture_scope_session = session_scope;
    }
}

/// Resolve a decorator to its testing marker kind, if any.
pub fn resolve_testing_marker_kind(
    dec: &ast::Decorator,
    aliases: &impl decorator_resolution::DecoratorPrefixLookup,
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
    aliases: &impl decorator_resolution::DecoratorPrefixLookup,
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

/// Validate the compile-time testing marker inventory.
fn validate_testing_marker_inventory(semantics: &TestingMarkerSemantics) -> Result<(), TestingMarkerLoadError> {
    let expected_names = incan_lang::lang::testing::RUNNER_ONLY_MARKER_NAMES;
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

/// Decode one checked `@rust.extern` metadata value into its typed testing-marker annotation.
fn testing_annotation_from_checked_metadata(
    metadata: &DecoratorValue,
) -> Result<Option<TestingMarkerAnnotation>, TestingMarkerLoadError> {
    let DecoratorValue::Dict { entries } = metadata else {
        return Err(TestingMarkerLoadError::new(
            "malformed checked @rust.extern metadata for std.testing marker: expected dict",
        ));
    };
    let mut kind = None;
    let mut runner_only = false;
    let mut fixture_scope_arg = None;
    let mut fixture_autouse_arg = None;
    let mut fixture_scopes = None;
    for entry in entries {
        let Some(key) = checked_metadata_string(&entry.key) else {
            return Err(TestingMarkerLoadError::new(
                "malformed checked std.testing marker metadata: non-string key",
            ));
        };
        match key {
            TESTING_MARKER_KIND_KEY => {
                let Some(kind_name) = checked_metadata_string(&entry.value) else {
                    return Err(TestingMarkerLoadError::new(
                        "malformed checked marker_kind metadata value (expected string)",
                    ));
                };
                kind = TestingMarkerKind::from_str(kind_name);
                if kind.is_none() {
                    return Err(TestingMarkerLoadError::new(format!(
                        "unknown checked marker_kind metadata value `{kind_name}`"
                    )));
                }
            }
            TESTING_MARKER_RUNNER_ONLY_KEY => {
                let Some(value) = checked_metadata_bool(&entry.value) else {
                    return Err(TestingMarkerLoadError::new(
                        "malformed checked runner_only metadata value (expected bool)",
                    ));
                };
                runner_only = value;
            }
            TESTING_FIXTURE_SCOPE_ARG_KEY => {
                fixture_scope_arg = checked_metadata_string(&entry.value).map(str::to_string);
            }
            TESTING_FIXTURE_AUTOUSE_ARG_KEY => {
                fixture_autouse_arg = checked_metadata_string(&entry.value).map(str::to_string);
            }
            TESTING_FIXTURE_SCOPES_KEY => {
                fixture_scopes = checked_metadata_string_triplet(&entry.value);
            }
            _ => {}
        }
    }
    let Some(kind) = kind else {
        return Ok(None);
    };
    if !runner_only {
        return Err(TestingMarkerLoadError::new(
            "checked std.testing marker metadata must declare runner_only=true",
        ));
    }
    Ok(Some(TestingMarkerAnnotation {
        kind,
        fixture_scope_arg,
        fixture_autouse_arg,
        fixture_scopes,
    }))
}

/// Read a string literal from the checked decorator-metadata value domain.
fn checked_metadata_string(value: &DecoratorValue) -> Option<&str> {
    match value {
        DecoratorValue::Literal {
            value: SafeMetadataValue::String(value),
        } => Some(value),
        _ => None,
    }
}

/// Read a boolean literal from the checked decorator-metadata value domain.
fn checked_metadata_bool(value: &DecoratorValue) -> Option<bool> {
    match value {
        DecoratorValue::Literal {
            value: SafeMetadataValue::Bool(value),
        } => Some(*value),
        _ => None,
    }
}

/// Read the exact three-string fixture-scope table from checked decorator metadata.
fn checked_metadata_string_triplet(value: &DecoratorValue) -> Option<[String; 3]> {
    let DecoratorValue::List { items } = value else {
        return None;
    };
    let [first, second, third] = items.as_slice() else {
        return None;
    };
    Some([
        checked_metadata_string(first)?.to_string(),
        checked_metadata_string(second)?.to_string(),
        checked_metadata_string(third)?.to_string(),
    ])
}
