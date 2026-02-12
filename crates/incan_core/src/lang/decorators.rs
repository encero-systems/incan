//! Decorator vocabulary registry.
//!
//! This module centralizes recognized decorator spellings so downstream code
//! doesn't need stringly-typed comparisons.
//!
//! ## Namespaces
//!
//! Decorators are organized into namespaces separated by `.`:
//!
//! - `rust.*` — Rust interop decorators (`@rust.extern`, future `@rust.function`, etc.)
//! - `std.*` — Standard library decorators (`@std.web.route`, `@std.testing.fixture`)
//! - Top-level — `@derive`, `@requires`
//!
//! Known namespace prefixes are registered in [`DECORATOR_NAMESPACES`] so that the validator can distinguish "unknown
//! decorator in the `rust` namespace" from "completely unknown decorator".

use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

/// Stable identifier for supported decorators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecoratorId {
    Derive,
    RustExtern,
    Route,
    Fixture,
    Requires,
}

// ---- Decorator namespace constants ----

/// The `rust` decorator namespace — covers all `@rust.*` decorators.
///
/// Current members: `@rust.extern`. Future: `@rust.function`, etc.
pub const RUST_NAMESPACE: &str = "rust";

/// Known decorator namespace prefixes.
///
/// The validator uses this list to give targeted errors when a user writes e.g. `@rust.blah` instead of "unknown
/// decorator `rust.blah`", it says "unknown decorator `blah` in namespace `rust`".
///
/// Each entry is a top-level namespace root; nested namespaces like `std.web` are handled by matching `std`.
pub const DECORATOR_NAMESPACES: &[&str] = &[RUST_NAMESPACE, "std"];

/// Check whether a leading segment is a known decorator namespace prefix.
pub fn is_known_decorator_namespace(prefix: &str) -> bool {
    DECORATOR_NAMESPACES.contains(&prefix)
}

/// Return all known decorators under a given namespace prefix.
///
/// For example, `decorators_in_namespace("rust")` returns `["rust.extern"]`.
pub fn decorators_in_namespace(prefix: &str) -> Vec<&'static str> {
    let prefix_dot = format!("{}.", prefix);
    DECORATORS
        .iter()
        .filter(|d| d.canonical.starts_with(&prefix_dot))
        .map(|d| d.canonical)
        .collect()
}

/// Named argument for `@route(methods=[...])`.
pub const ROUTE_METHODS_ARG: &str = "methods";

/// Named argument for `@fixture(scope=...)`.
pub const FIXTURE_SCOPE_ARG: &str = "scope";

/// Named argument for `@fixture(autouse=...)`.
pub const FIXTURE_AUTOUSE_ARG: &str = "autouse";

/// Fixture scope value: per-function.
pub const FIXTURE_SCOPE_FUNCTION: &str = "function";

/// Fixture scope value: per-module.
pub const FIXTURE_SCOPE_MODULE: &str = "module";

/// Fixture scope value: per-session.
pub const FIXTURE_SCOPE_SESSION: &str = "session";

/// Metadata entry for a decorator.
pub type DecoratorInfo = LangItemInfo<DecoratorId>;

/// Registry of supported decorators.
pub const DECORATORS: &[DecoratorInfo] = &[
    info(
        DecoratorId::Derive,
        "derive",
        &[],
        "Derive common trait implementations.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        DecoratorId::RustExtern,
        "rust.extern",
        &[],
        "Mark functions whose body is provided by a Rust module.",
        RFC::_022,
        Since(0, 2),
    ),
    info(
        DecoratorId::Route,
        "std.web.route",
        &[],
        "Declare a web route handler.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        DecoratorId::Fixture,
        "std.testing.fixture",
        &[],
        "Declare a test fixture.",
        RFC::_001,
        Since(0, 1),
    ),
    info(
        DecoratorId::Requires,
        "requires",
        &[],
        "Declare required fields for trait default methods.",
        RFC::_000,
        Since(0, 1),
    ),
];

/// Resolve a decorator path to its stable id.
pub fn from_str(name: &str) -> Option<DecoratorId> {
    if let Some(info) = DECORATORS.iter().find(|d| d.canonical == name) {
        return Some(info.id);
    }
    DECORATORS
        .iter()
        .find(|d| {
            let aliases: &[&str] = d.aliases;
            aliases.contains(&name)
        })
        .map(|d| d.id)
}

/// Resolve a decorator path segments to its stable id.
pub fn from_segments(segments: &[String]) -> Option<DecoratorId> {
    let path = segments.join(".");
    from_str(path.as_str())
}

/// Return the canonical spelling for a decorator.
pub fn as_str(id: DecoratorId) -> &'static str {
    info_for(id).canonical
}

/// Return the metadata entry for a decorator.
pub fn info_for(id: DecoratorId) -> &'static DecoratorInfo {
    DECORATORS.iter().find(|d| d.id == id).expect("decorator info missing")
}

const fn info(
    id: DecoratorId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> DecoratorInfo {
    LangItemInfo {
        id,
        canonical,
        aliases,
        description,
        introduced_in_rfc,
        since,
        stability: Stability::Stable,
        examples: &[],
    }
}
