//! Decorator vocabulary registry.
//!
//! This module centralizes recognized decorator spellings so downstream code
//! doesn't need stringly-typed comparisons.
//!
//! ## Namespaces
//!
//! Decorators are organized into namespaces separated by `.`:
//!
//! - `rust.*` — Rust interop decorators (`@rust.extern`, `@rust.allow`, future `@rust.function`, etc.)
//! - `std.*` — Standard library decorators
//! - Top-level — `@derive`, `@requires`
//!
//! Known namespace prefixes are registered in [`DECORATOR_NAMESPACES`] so that the validator can distinguish "unknown
//! decorator in the `rust` namespace" from "completely unknown decorator".
use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

/// Stable identifier for supported decorators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecoratorId {
    Derive,
    RustDerive,
    RustExtern,
    RustAllow,
    NoImplicitCoercion,
    StaticMethod,
    ClassMethod,
    Requires,
    Describe,
}

// ---- Decorator namespace constants ----

/// The `rust` decorator namespace — covers all `@rust.*` decorators.
///
/// Current members: `@rust.extern`, `@rust.allow`. Future: `@rust.function`, etc.
pub const RUST_NAMESPACE: &str = "rust";

/// The `std` decorator namespace — covers all `@std.*` decorators.
pub const STD_NAMESPACE: &str = "std";

/// Known decorator namespace prefixes.
///
/// The validator uses this list to give targeted errors when a user writes e.g. `@rust.blah` instead of "unknown
/// decorator `rust.blah`", it says "unknown decorator `blah` in namespace `rust`".
///
/// Each entry is a top-level namespace root; nested namespaces like `std.web` are handled by matching `std`.
pub const DECORATOR_NAMESPACES: &[&str] = &[RUST_NAMESPACE, STD_NAMESPACE];

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
        DecoratorId::RustDerive,
        "rust.derive",
        &[],
        "Declare a Rust derive path required by a derivable Incan trait.",
        RFC::_024,
        Since(0, 3),
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
        DecoratorId::RustAllow,
        "rust.allow",
        &[],
        "Emit targeted Rust #[allow(...)] lint suppressions on a generated item.",
        RFC::_057,
        Since(0, 3),
    ),
    info(
        DecoratorId::NoImplicitCoercion,
        "no_implicit_coercion",
        &[],
        "Disable RFC 017 implicit newtype coercion for this type.",
        RFC::_017,
        Since(0, 3),
    ),
    info(
        DecoratorId::StaticMethod,
        "staticmethod",
        &[],
        "Mark a method as static (no self receiver).",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        DecoratorId::ClassMethod,
        "classmethod",
        &[],
        "Mark a method as a class method (no implicit self receiver).",
        RFC::_000,
        Since(0, 2),
    ),
    info(
        DecoratorId::Requires,
        "requires",
        &[],
        "Declare required fields for trait default methods.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        DecoratorId::Describe,
        "std.registry.describe",
        &[],
        "Attach a compiler-checked typed registry descriptor to a declaration.",
        RFC::_113,
        Since(0, 5),
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
    let Some(info) = DECORATORS.iter().find(|d| d.id == id) else {
        panic!("decorator info missing for {id:?}");
    };
    info
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
