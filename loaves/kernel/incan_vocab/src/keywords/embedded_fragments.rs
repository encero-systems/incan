//! RFC 081 embedded fragment submodes, format hints and descriptors.

use super::*;

// ============================================================================
// RFC 081 — descriptor-gated lexical submodes and typed embedded fragments
// ============================================================================
//
// The types below extend the RFC 040/045 scoped-surface model to language-shaped DSL embeddings: fixed,
// compiler-defined lexical submodes (markup, style, raw text/comments, regex/template, selector/declaration-value,
// type-position) that a descriptor may claim for a specific eligible position, per RFC 081's Reference-level
// explanation. A descriptor never authors its own grammar — it only selects which of the compiler's fixed submode
// kinds it claims, for which position, and names the typed-artifact contribution consumers should expect. This
// keeps every accepted construct enumerable, matching RFC 081's Design Decisions ("No compiler-enforced
// partial-subset threshold", "Descriptors are described only in Incan-owned terms").
//
// Eligibility reuses `ScopedSurfaceEligibility`/`ScopedSurfacePosition` as-is rather than duplicating the
// declaration/clause/call positional model RFC 040 already established.

/// Fixed, compiler-defined lexical submode kind an embedded-fragment descriptor may claim.
///
/// This is the complete submode catalog RFC 081 (`#1023`) implements. It is deliberately closed and enumerable: a
/// descriptor can only ever claim one of these kinds for a given eligible position, never an arbitrary author-defined
/// grammar, so unrecognized syntax inside a claimed position is always a parse error rather than a silent
/// reinterpretation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum EmbeddedFragmentSubmode {
    /// Markup content: tags, attributes, text nodes, entity references, comments, and expression holes.
    #[default]
    Markup,
    /// Style content: selector lists and declaration blocks with custom properties and declaration values.
    Style,
    /// Raw text or comment content preserved verbatim, with expression holes only if the descriptor opts in.
    RawText,
    /// A regex literal (`/pattern/flags`) or a template string with expression-hole interpolation.
    RegexTemplate,
    /// A single declaration-value expression outside a full style block (dimension, color, `var()`, literal, hole).
    SelectorDeclarationValue,
    /// A minimal representative type-shaped grammar position (union, generic, nullable, array, qualified name).
    TypePosition,
}

/// Where a descriptor-gated embedded fragment produces layout-sensitive output.
///
/// This is formatter-facing metadata for `#1022` (not consumed by `#1023`): a `true` value tells the formatter it
/// must preserve the fragment's original source layout verbatim rather than reformatting from the structured
/// artifact, per RFC 081's "No third formatter fallback state" Design Decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct EmbeddedFragmentFormatHint {
    /// Whether the formatter must preserve original source layout verbatim instead of reformatting structurally.
    pub layout_sensitive: bool,
}

/// One DSL-owned descriptor-gated embedded-fragment submode claim.
///
/// A descriptor names the fixed submode kind it claims, the position(s) where that claim applies, and the typed
/// artifact key downstream desugarers/lowering hooks should expect to receive. Per RFC 081's Reference-level
/// explanation, the descriptor must not apply outside its declared eligible positions, and when two same-depth
/// descriptors claim the same submode in the same eligible position the compiler rejects the combination as
/// ambiguous (see `loaves/kernel/incan_syntax/src/parser/expr.rs` same-depth ambiguity handling).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct EmbeddedFragmentDescriptor {
    /// Stable descriptor identity used by artifacts, diagnostics, and tooling.
    pub key: String,
    /// Fixed lexical submode kind claimed by this descriptor.
    pub submode: EmbeddedFragmentSubmode,
    /// Stable name for the typed-artifact contribution this descriptor produces, consumed by the owning DSL's
    /// desugarer or lowering hook. Never rediscovered by matching raw source text at a later compiler phase.
    pub artifact_key: String,
    /// Positive positions where this embedded-fragment submode has DSL-owned meaning.
    #[cfg_attr(feature = "serde", serde(default))]
    pub eligible_in: Vec<ScopedSurfaceEligibility>,
    /// Scope where descriptor-owned misuse diagnostics may fire.
    pub misuse_scope: ScopedSurfaceMisuseScope,
    /// Author-provided diagnostic templates.
    #[cfg_attr(feature = "serde", serde(default))]
    pub diagnostics: Vec<ScopedSurfaceDiagnosticTemplate>,
    /// Formatter-facing metadata consumed by `#1022`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub format_hint: EmbeddedFragmentFormatHint,
}

impl EmbeddedFragmentDescriptor {
    /// Create a new embedded-fragment descriptor for a fixed submode kind and typed-artifact key.
    #[must_use]
    pub fn new(key: &str, submode: EmbeddedFragmentSubmode, artifact_key: &str) -> Self {
        Self {
            key: key.to_string(),
            submode,
            artifact_key: artifact_key.to_string(),
            eligible_in: Vec::new(),
            misuse_scope: ScopedSurfaceMisuseScope::None,
            diagnostics: Vec::new(),
            format_hint: EmbeddedFragmentFormatHint::default(),
        }
    }

    /// Add one positive eligibility position.
    #[must_use]
    pub fn with_eligibility(mut self, eligibility: ScopedSurfaceEligibility) -> Self {
        self.eligible_in.push(eligibility);
        self
    }

    /// Add multiple positive eligibility positions.
    #[must_use]
    pub fn with_eligibilities<I>(mut self, eligibilities: I) -> Self
    where
        I: IntoIterator<Item = ScopedSurfaceEligibility>,
    {
        self.eligible_in.extend(eligibilities);
        self
    }

    /// Mark the fragment as eligible in a declaration body (the common case: the whole block body is DSL-owned).
    #[must_use]
    pub fn in_declaration_body(self, declaration: &str) -> Self {
        self.with_eligibility(ScopedSurfaceEligibility::declaration_body(declaration))
    }

    /// Mark the fragment as eligible in a named clause body.
    #[must_use]
    pub fn in_clause_body(self, declaration: &str, clause: &str) -> Self {
        self.with_eligibility(ScopedSurfaceEligibility::clause_body(declaration, clause))
    }

    /// Mark the fragment as eligible in arguments to a named function or method call.
    #[must_use]
    pub fn in_call_argument(self, declaration: &str, call: &str) -> Self {
        self.with_eligibility(ScopedSurfaceEligibility::call_argument(declaration, call))
    }

    /// Set the misuse diagnostic scope.
    #[must_use]
    pub fn with_misuse_scope(mut self, misuse_scope: ScopedSurfaceMisuseScope) -> Self {
        self.misuse_scope = misuse_scope;
        self
    }

    /// Add one author-provided diagnostic template.
    #[must_use]
    pub fn with_diagnostic(mut self, diagnostic: ScopedSurfaceDiagnosticTemplate) -> Self {
        self.diagnostics.push(diagnostic);
        self
    }

    /// Add multiple author-provided diagnostic templates.
    #[must_use]
    pub fn with_diagnostics<I>(mut self, diagnostics: I) -> Self
    where
        I: IntoIterator<Item = ScopedSurfaceDiagnosticTemplate>,
    {
        self.diagnostics.extend(diagnostics);
        self
    }

    /// Mark this embedded fragment as layout-sensitive for the formatter (`#1022`).
    #[must_use]
    pub fn layout_sensitive(mut self) -> Self {
        self.format_hint.layout_sensitive = true;
        self
    }
}
