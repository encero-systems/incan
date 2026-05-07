//! Shareable metadata for `incan_core::lang` registries.
//!
//! The `incan_core::lang` module is a set of **registry-first** vocabularies: keywords,
//! operators, builtin functions, builtin types, etc. This submodule provides the small,
//! dependency-free metadata types that are reused across all registries.
//!
//! ## Notes
//! - These types are intentionally lightweight and `Copy`-friendly so registries can live in `const` tables.
//! - Metadata is meant for tooling/docs/diagnostics; enforcement of syntax rules still lives in the lexer/parser.
//!
//! ## See also
//! - [`crate::lang::keywords`]
//! - [`crate::lang::operators`]
//! - [`crate::lang::builtins`]
//! - [`crate::lang::types`]

/// Identify the RFC that introduced a vocabulary item.
///
/// ## Notes
/// - The canonical format is `"RFC 000"` (three-digit id).
///
/// ## Examples
/// ```rust
/// use incan_core::lang::registry::RfcId;
///
/// let rfc: RfcId = "RFC 000";
/// assert!(rfc.starts_with("RFC "));
/// ```
pub type RfcId = &'static str;

/// RFC 000 — core language RFC.
pub const RFC_000: RfcId = "RFC 000";

/// RFC 001 — test fixtures (`yield` for setup/teardown).
pub const RFC_001: RfcId = "RFC 001";

/// RFC 004 — async fixtures (Tokio integration; async runtime primitives).
pub const RFC_004: RfcId = "RFC 004";

/// RFC 005 — Rust interop (`rust::...` imports).
pub const RFC_005: RfcId = "RFC 005";

/// RFC 006 — Python-style generators.
pub const RFC_006: RfcId = "RFC 006";

/// RFC 008 — const bindings (`const NAME = ...`).
pub const RFC_008: RfcId = "RFC 008";

/// RFC 009 — sized integers & builtin type registry (builtin type methods; frozen containers).
pub const RFC_009: RfcId = "RFC 009";

/// RFC 016 — `loop` and `break <value>` loop expressions.
pub const RFC_016: RfcId = "RFC 016";

/// RFC 018 — testing stdlib and assertion helpers.
pub const RFC_018: RfcId = "RFC 018";

/// RFC 021 — model field metadata and aliases.
pub const RFC_021: RfcId = "RFC 021";

/// RFC 022 — stdlib namespacing and compiler handoff.
pub const RFC_022: RfcId = "RFC 022";

/// RFC 023 — stdlib compilation and `@rust.extern` delegation.
pub const RFC_023: RfcId = "RFC 023";

/// RFC 028 — trait-based operator overloading.
pub const RFC_028: RfcId = "RFC 028";

/// RFC 029 — union types and narrowing predicates.
pub const RFC_029: RfcId = "RFC 029";

/// RFC 040 — scoped DSL surface forms.
pub const RFC_040: RfcId = "RFC 040";

/// RFC 052 — module static storage.
pub const RFC_052: RfcId = "RFC 052";

/// RFC 057 — targeted Rust lint suppression.
pub const RFC_057: RfcId = "RFC 057";

/// RFC 069 — `list.repeat` helper for fixed-length initialization.
pub const RFC_069: RfcId = "RFC 069";

/// RFC 083 — symbol and method aliases.
pub const RFC_083: RfcId = "RFC 083";

/// RFC 088 — iterator adapter surface.
pub const RFC_088: RfcId = "RFC 088";

/// Namespace-style access to RFC ids.
///
/// This exists purely for ergonomics at call sites so individual registries don’t need to import
/// dozens of `RFC_###` constants into their `use` lists.
///
/// ## Notes
/// - Rust identifiers cannot start with digits, so the style is `RFC::_000` (not `RFC::000`).
/// - The underlying value is still an [`RfcId`] (`&'static str`).
pub struct RFC;

impl RFC {
    /// RFC 000 — core language RFC.
    pub const _000: RfcId = RFC_000;
    /// RFC 001 — test fixtures (`yield` for setup/teardown).
    pub const _001: RfcId = RFC_001;
    /// RFC 004 — async fixtures (Tokio integration; async runtime primitives).
    pub const _004: RfcId = RFC_004;
    /// RFC 005 — Rust interop (`rust::...` imports).
    pub const _005: RfcId = RFC_005;
    /// RFC 006 — Python-style generators.
    pub const _006: RfcId = RFC_006;
    /// RFC 008 — const bindings (`const NAME = ...`).
    pub const _008: RfcId = RFC_008;
    /// RFC 009 — sized integers & builtin type registry (builtin type methods; frozen containers).
    pub const _009: RfcId = RFC_009;
    /// RFC 016 — `loop` and `break <value>` loop expressions.
    pub const _016: RfcId = RFC_016;
    /// RFC 018 — testing stdlib and assertion helpers.
    pub const _018: RfcId = RFC_018;
    /// RFC 021 — model field metadata and aliases.
    pub const _021: RfcId = RFC_021;
    /// RFC 022 — stdlib namespacing and compiler handoff.
    pub const _022: RfcId = RFC_022;
    /// RFC 023 — stdlib compilation and `@rust.extern` delegation.
    pub const _023: RfcId = RFC_023;
    /// RFC 028 — trait-based operator overloading.
    pub const _028: RfcId = RFC_028;
    /// RFC 029 — union types and narrowing predicates.
    pub const _029: RfcId = RFC_029;
    /// RFC 040 — scoped DSL surface forms.
    pub const _040: RfcId = RFC_040;
    /// RFC 052 — module static storage.
    pub const _052: RfcId = RFC_052;
    /// RFC 057 — targeted Rust lint suppression.
    pub const _057: RfcId = RFC_057;
    /// RFC 069 — `list.repeat` helper for fixed-length initialization.
    pub const _069: RfcId = RFC_069;
    /// RFC 083 — symbol and method aliases.
    pub const _083: RfcId = RFC_083;
    /// RFC 088 — iterator adapter surface.
    pub const _088: RfcId = RFC_088;
}

/// Identify the language/compiler version a vocabulary item is available since.
///
/// This is intentionally **minor-only**: we track `major.minor` and do not model patch versions
/// (patch releases must not introduce new language features).
///
/// ## Examples
/// ```rust
/// use incan_core::lang::registry::Since;
///
/// let since = Since(0, 1);
/// assert_eq!(since.to_string(), "0.1");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Since(pub u16, pub u16);

impl std::fmt::Display for Since {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.0, self.1)
    }
}

/// Describe the lifecycle status of a language vocabulary item.
///
/// ## Notes
/// - This is intended for docs/tooling (e.g. to warn on deprecated spellings), not for feature-gating by itself.
///
/// ## Examples
/// ```rust
/// use incan_core::lang::registry::Stability;
///
/// let s = Stability::Stable;
/// assert_eq!(format!("{s:?}"), "Stable");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stability {
    Stable,
    Draft,
    Deprecated,
}

/// Represent a small example snippet for documentation.
///
/// ## Notes
/// - `code` is the example body, usually in Incan syntax.
/// - `note` is an optional short explanation (one or two sentences).
///
/// ## Examples
/// ```rust
/// use incan_core::lang::registry::Example;
///
/// let ex = Example {
///     code: "if cond:\n  pass",
///     note: Some("Minimal conditional."),
/// };
/// assert!(ex.code.contains("if"));
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Example {
    pub code: &'static str,
    pub note: Option<&'static str>,
}

/// Shared metadata shape for “registry-first” vocabulary items.
///
/// Many language vocabularies share the same core fields:
/// - stable identity (`id`)
/// - accepted spellings (`canonical` + `aliases`)
/// - documentation (`description` + `examples`)
/// - provenance (`introduced_in_rfc`, `since`, `stability`)
///
/// Registries that need extra per-item data (e.g. operator precedence, keyword category/usage) should wrap this
/// struct in an “extension” info type.
///
/// ## Notes
/// - `description` is intentionally mandatory to keep docs/tooling consistent.
/// - This type is `Copy` so it can live in `const` tables.
#[derive(Debug, Clone, Copy)]
pub struct LangItemInfo<Id> {
    pub id: Id,
    pub canonical: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub introduced_in_rfc: RfcId,
    pub since: Since,
    pub stability: Stability,
    pub examples: &'static [Example],
}
