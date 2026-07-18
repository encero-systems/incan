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

/// RFC 010 — temporary files and directories.
pub const RFC_010: RfcId = "RFC 010";

/// RFC 013 — Rust crate dependencies and lock files.
pub const RFC_013: RfcId = "RFC 013";

/// RFC 015 — project lifecycle CLI.
pub const RFC_015: RfcId = "RFC 015";

/// RFC 016 — `loop` and `break <value>` loop expressions.
pub const RFC_016: RfcId = "RFC 016";

/// RFC 017 — validated newtypes with implicit coercion.
pub const RFC_017: RfcId = "RFC 017";

/// RFC 018 — testing stdlib and assertion helpers.
pub const RFC_018: RfcId = "RFC 018";

/// RFC 019 — first-class test runner.
pub const RFC_019: RfcId = "RFC 019";

/// RFC 021 — model field metadata and aliases.
pub const RFC_021: RfcId = "RFC 021";

/// RFC 022 — stdlib namespacing and compiler handoff.
pub const RFC_022: RfcId = "RFC 022";

/// RFC 023 — stdlib compilation and `@rust.extern` delegation.
pub const RFC_023: RfcId = "RFC 023";

/// RFC 024 — extensible derive protocol.
pub const RFC_024: RfcId = "RFC 024";

/// RFC 028 — trait-based operator overloading.
pub const RFC_028: RfcId = "RFC 028";

/// RFC 029 — union types and narrowing predicates.
pub const RFC_029: RfcId = "RFC 029";

/// RFC 030 — std.collections and field-overlay reflection.
pub const RFC_030: RfcId = "RFC 030";

/// RFC 031 — library system phase 1.
pub const RFC_031: RfcId = "RFC 031";

/// RFC 032 — value enums.
pub const RFC_032: RfcId = "RFC 032";

/// RFC 035 — first-class function references.
pub const RFC_035: RfcId = "RFC 035";

/// RFC 036 — user-defined decorators.
pub const RFC_036: RfcId = "RFC 036";

/// RFC 038 — variadic positional arguments and keyword capture.
pub const RFC_038: RfcId = "RFC 038";

/// RFC 039 — async race expressions and awaitability.
pub const RFC_039: RfcId = "RFC 039";

/// RFC 040 — scoped DSL surface forms.
pub const RFC_040: RfcId = "RFC 040";

/// RFC 041 — first-class Rust interop authoring.
pub const RFC_041: RfcId = "RFC 041";

/// RFC 042 — traits are always abstract.
pub const RFC_042: RfcId = "RFC 042";

/// RFC 043 — Rust trait impl authoring from Incan.
pub const RFC_043: RfcId = "RFC 043";

/// RFC 044 — open-ended trait methods.
pub const RFC_044: RfcId = "RFC 044";

/// RFC 045 — scoped DSL symbol surfaces.
pub const RFC_045: RfcId = "RFC 045";

/// RFC 046 — computed properties.
pub const RFC_046: RfcId = "RFC 046";

/// RFC 047 — lightweight directed graph types.
pub const RFC_047: RfcId = "RFC 047";

/// RFC 048 — contract-backed models and checked API metadata.
pub const RFC_048: RfcId = "RFC 048";

/// RFC 049 — single-arm conditional pattern control flow.
pub const RFC_049: RfcId = "RFC 049";

/// RFC 050 — enum methods and trait adoption.
pub const RFC_050: RfcId = "RFC 050";

/// RFC 051 — dynamic JSON value surface.
pub const RFC_051: RfcId = "RFC 051";

/// RFC 052 — module static storage.
pub const RFC_052: RfcId = "RFC 052";

/// RFC 053 — formatter vertical spacing buckets.
pub const RFC_053: RfcId = "RFC 053";

/// RFC 054 — explicit call-site generics.
pub const RFC_054: RfcId = "RFC 054";

/// RFC 055 — std.fs.
pub const RFC_055: RfcId = "RFC 055";

/// RFC 056 — std.io.
pub const RFC_056: RfcId = "RFC 056";

/// RFC 057 — targeted Rust lint suppression.
pub const RFC_057: RfcId = "RFC 057";

/// RFC 058 — std.datetime.
pub const RFC_058: RfcId = "RFC 058";

/// RFC 059 — std.regex.
pub const RFC_059: RfcId = "RFC 059";

/// RFC 060 — std.uuid.
pub const RFC_060: RfcId = "RFC 060";

/// RFC 061 — std.compression.
pub const RFC_061: RfcId = "RFC 061";

/// RFC 064 — std.encoding.
pub const RFC_064: RfcId = "RFC 064";

/// RFC 065 — std.hash.
pub const RFC_065: RfcId = "RFC 065";

/// RFC 068 — protocol hooks for core syntax.
pub const RFC_068: RfcId = "RFC 068";

/// RFC 069 — `list.repeat` helper for fixed-length initialization.
pub const RFC_069: RfcId = "RFC 069";

/// RFC 070 — `Result[T, E]` combinators.
pub const RFC_070: RfcId = "RFC 070";

/// RFC 071 — pattern alternation.
pub const RFC_071: RfcId = "RFC 071";

/// RFC 072 — std.logging.
pub const RFC_072: RfcId = "RFC 072";

/// RFC 083 — symbol and method aliases.
pub const RFC_083: RfcId = "RFC 083";

/// RFC 084 — callable presets.
pub const RFC_084: RfcId = "RFC 084";

/// RFC 088 — iterator adapter surface.
pub const RFC_088: RfcId = "RFC 088";

/// RFC 089 — std.environ runtime environment access.
pub const RFC_089: RfcId = "RFC 089";

/// RFC 102 — semantic layer inspection surface.
pub const RFC_102: RfcId = "RFC 102";

/// RFC 106 — compiler-backed agent context graph.
pub const RFC_106: RfcId = "RFC 106";

/// RFC 107 — type-directed library APIs and compile-time type tokens.
pub const RFC_107: RfcId = "RFC 107";

/// RFC 114 — compiled providers, SDK components, and package features.
pub const RFC_114: RfcId = "RFC 114";

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
    /// RFC 010 — temporary files and directories.
    pub const _010: RfcId = RFC_010;
    /// RFC 013 — Rust crate dependencies and lock files.
    pub const _013: RfcId = RFC_013;
    /// RFC 015 — project lifecycle CLI.
    pub const _015: RfcId = RFC_015;
    /// RFC 016 — `loop` and `break <value>` loop expressions.
    pub const _016: RfcId = RFC_016;
    /// RFC 017 — validated newtypes with implicit coercion.
    pub const _017: RfcId = RFC_017;
    /// RFC 018 — testing stdlib and assertion helpers.
    pub const _018: RfcId = RFC_018;
    /// RFC 019 — first-class test runner.
    pub const _019: RfcId = RFC_019;
    /// RFC 021 — model field metadata and aliases.
    pub const _021: RfcId = RFC_021;
    /// RFC 022 — stdlib namespacing and compiler handoff.
    pub const _022: RfcId = RFC_022;
    /// RFC 023 — stdlib compilation and `@rust.extern` delegation.
    pub const _023: RfcId = RFC_023;
    /// RFC 024 — extensible derive protocol.
    pub const _024: RfcId = RFC_024;
    /// RFC 028 — trait-based operator overloading.
    pub const _028: RfcId = RFC_028;
    /// RFC 029 — union types and narrowing predicates.
    pub const _029: RfcId = RFC_029;
    /// RFC 030 — std.collections and field-overlay reflection.
    pub const _030: RfcId = RFC_030;
    /// RFC 031 — library system phase 1.
    pub const _031: RfcId = RFC_031;
    /// RFC 032 — value enums.
    pub const _032: RfcId = RFC_032;
    /// RFC 035 — first-class function references.
    pub const _035: RfcId = RFC_035;
    /// RFC 036 — user-defined decorators.
    pub const _036: RfcId = RFC_036;
    /// RFC 038 — variadic positional arguments and keyword capture.
    pub const _038: RfcId = RFC_038;

    /// RFC 039 — async race expressions and awaitability.
    pub const _039: RfcId = RFC_039;
    /// RFC 040 — scoped DSL surface forms.
    pub const _040: RfcId = RFC_040;
    /// RFC 041 — first-class Rust interop authoring.
    pub const _041: RfcId = RFC_041;
    /// RFC 042 — traits are always abstract.
    pub const _042: RfcId = RFC_042;
    /// RFC 043 — Rust trait impl authoring from Incan.
    pub const _043: RfcId = RFC_043;
    /// RFC 044 — open-ended trait methods.
    pub const _044: RfcId = RFC_044;
    /// RFC 045 — scoped DSL symbol surfaces.
    pub const _045: RfcId = RFC_045;
    /// RFC 046 — computed properties.
    pub const _046: RfcId = RFC_046;
    /// RFC 047 — lightweight directed graph types.
    pub const _047: RfcId = RFC_047;
    /// RFC 048 — contract-backed models and checked API metadata.
    pub const _048: RfcId = RFC_048;
    /// RFC 049 — single-arm conditional pattern control flow.
    pub const _049: RfcId = RFC_049;
    /// RFC 050 — enum methods and trait adoption.
    pub const _050: RfcId = RFC_050;
    /// RFC 051 — dynamic JSON value surface.
    pub const _051: RfcId = RFC_051;
    /// RFC 052 — module static storage.
    pub const _052: RfcId = RFC_052;
    /// RFC 053 — formatter vertical spacing buckets.
    pub const _053: RfcId = RFC_053;
    /// RFC 054 — explicit call-site generics.
    pub const _054: RfcId = RFC_054;
    /// RFC 055 — std.fs.
    pub const _055: RfcId = RFC_055;
    /// RFC 056 — std.io.
    pub const _056: RfcId = RFC_056;
    /// RFC 057 — targeted Rust lint suppression.
    pub const _057: RfcId = RFC_057;
    /// RFC 058 — std.datetime.
    pub const _058: RfcId = RFC_058;
    /// RFC 059 — std.regex.
    pub const _059: RfcId = RFC_059;
    /// RFC 060 — std.uuid.
    pub const _060: RfcId = RFC_060;
    /// RFC 061 — std.compression.
    pub const _061: RfcId = RFC_061;
    /// RFC 064 — std.encoding.
    pub const _064: RfcId = RFC_064;
    /// RFC 065 — std.hash.
    pub const _065: RfcId = RFC_065;
    /// RFC 068 — protocol hooks for core syntax.
    pub const _068: RfcId = RFC_068;
    /// RFC 069 — `list.repeat` helper for fixed-length initialization.
    pub const _069: RfcId = RFC_069;
    /// RFC 070 — `Result[T, E]` combinators.
    pub const _070: RfcId = RFC_070;
    /// RFC 071 — pattern alternation.
    pub const _071: RfcId = RFC_071;
    /// RFC 072 — std.logging.
    pub const _072: RfcId = RFC_072;
    /// RFC 083 — symbol and method aliases.
    pub const _083: RfcId = RFC_083;
    /// RFC 084 — callable presets.
    pub const _084: RfcId = RFC_084;
    /// RFC 088 — iterator adapter surface.
    pub const _088: RfcId = RFC_088;
    /// RFC 089 — std.environ runtime environment access.
    pub const _089: RfcId = RFC_089;
    /// RFC 102 — semantic layer inspection surface.
    pub const _102: RfcId = RFC_102;
    /// RFC 106 — compiler-backed agent context graph.
    pub const _106: RfcId = RFC_106;
    /// RFC 107 — type-directed library APIs and compile-time type tokens.
    pub const _107: RfcId = RFC_107;
    /// RFC 114 — compiled providers, SDK components, and package features.
    pub const _114: RfcId = RFC_114;
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
