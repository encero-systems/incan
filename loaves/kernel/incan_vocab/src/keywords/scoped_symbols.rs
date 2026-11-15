//! Scoped symbol families, role metadata, eligibility, positions, diagnostics and descriptors.

use super::*;

/// Compiler/tooling-known category for a scoped identifier symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum ScopedSymbolFamily {
    /// Function-like symbol invoked with call syntax.
    #[default]
    FunctionLike,
    /// Aggregate-like symbol such as `sum` or `count` in query DSL positions.
    AggregateLike,
    /// Predicate/filtering symbol.
    PredicateLike,
    /// Projection/selection symbol.
    ProjectionLike,
    /// Grouping/bucketing symbol.
    GroupingLike,
    /// Ordering/ranking symbol.
    OrderingLike,
    /// Window/frame symbol.
    WindowLike,
}

/// Optional DSL-authored role metadata attached to a scoped symbol.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ScopedSymbolRoleMetadata {
    /// Stable role key understood by the DSL or its tooling.
    pub key: String,
    /// Optional short label for editor/tooling surfaces.
    pub label: Option<String>,
    /// Optional prose description for editor/tooling surfaces.
    pub description: Option<String>,
}

impl ScopedSymbolRoleMetadata {
    /// Create role metadata with a stable DSL-authored key.
    #[must_use]
    pub fn new(key: &str) -> Self {
        Self {
            key: key.to_string(),
            label: None,
            description: None,
        }
    }

    /// Attach a short display label.
    #[must_use]
    pub fn with_label(mut self, label: &str) -> Self {
        self.label = Some(label.to_string());
        self
    }

    /// Attach a prose description.
    #[must_use]
    pub fn with_description(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }
}

/// DSL name-resolution position where a scoped symbol has positive meaning.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ScopedSymbolEligibility {
    /// Owning declaration keyword, for example `query` or `pipeline`.
    pub declaration: String,
    /// Optional owning clause spelling inside the declaration, for example `SELECT`.
    pub clause: Option<String>,
    /// Optional call target spelling for nested call-argument positions, for example `filter`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub call: Option<String>,
    /// Name-resolution position where the symbol is eligible.
    pub position: ScopedSymbolPosition,
}

impl ScopedSymbolEligibility {
    /// Create an eligibility rule for a declaration body.
    #[must_use]
    pub fn declaration_body(declaration: &str) -> Self {
        Self {
            declaration: declaration.to_string(),
            clause: None,
            call: None,
            position: ScopedSymbolPosition::DeclarationBody,
        }
    }

    /// Create an eligibility rule for a named clause body.
    #[must_use]
    pub fn clause_body(declaration: &str, clause: &str) -> Self {
        Self {
            declaration: declaration.to_string(),
            clause: Some(clause.to_string()),
            call: None,
            position: ScopedSymbolPosition::ClauseBody,
        }
    }

    /// Create an eligibility rule for arguments to a named function or method call.
    #[must_use]
    pub fn call_argument(declaration: &str, call: &str) -> Self {
        Self {
            declaration: declaration.to_string(),
            clause: None,
            call: Some(call.to_string()),
            position: ScopedSymbolPosition::CallArgument,
        }
    }
}

/// Name-resolution position kind within the owning DSL grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum ScopedSymbolPosition {
    /// Declaration body expression/call position.
    #[default]
    DeclarationBody,
    /// Body of a named clause.
    ClauseBody,
    /// Argument expression of a named function or method call.
    CallArgument,
}

/// Where a scoped symbol descriptor may produce targeted misuse diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum ScopedSymbolMisuseScope {
    /// Do not emit descriptor-owned diagnostics.
    #[default]
    None,
    /// Emit descriptor-owned diagnostics only while inside an active DSL declaration.
    ActiveDsl,
}

/// Diagnostic situation that can use an author-provided scoped-symbol template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum ScopedSymbolDiagnosticKind {
    /// The symbol was used in an active DSL context but outside an eligible position.
    #[default]
    OutsideEligiblePosition,
    /// The symbol overlaps with an ordinary lexical/imported name in an eligible position.
    AmbiguousResolution,
    /// The call payload does not match the descriptor contract.
    InvalidCallPayload,
}

/// Author-provided diagnostic text for a compiler-gated scoped-symbol failure.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ScopedSymbolDiagnosticTemplate {
    /// Stable diagnostic identity exposed to tests and tooling.
    pub code: String,
    /// Failure kind this template applies to.
    pub kind: ScopedSymbolDiagnosticKind,
    /// Primary diagnostic message.
    pub message: String,
    /// Optional help text.
    pub help: Option<String>,
}

impl ScopedSymbolDiagnosticTemplate {
    /// Create a new diagnostic template.
    #[must_use]
    pub fn new(code: &str, kind: ScopedSymbolDiagnosticKind, message: &str) -> Self {
        Self {
            code: code.to_string(),
            kind,
            message: message.to_string(),
            help: None,
        }
    }

    /// Add help text to the diagnostic template.
    #[must_use]
    pub fn with_help(mut self, help: &str) -> Self {
        self.help = Some(help.to_string());
        self
    }
}

/// One DSL-owned scoped identifier symbol descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ScopedSymbolDescriptor {
    /// Stable descriptor identity used by artifacts, diagnostics, and tooling.
    pub key: String,
    /// Identifier spelling that may receive DSL-owned meaning in eligible positions.
    pub symbol: String,
    /// Compiler/tooling-known symbol family.
    pub family: ScopedSymbolFamily,
    /// Optional DSL-authored role metadata.
    pub role: Option<ScopedSymbolRoleMetadata>,
    /// Positive positions where this identifier has DSL-owned meaning.
    #[cfg_attr(feature = "serde", serde(default))]
    pub eligible_in: Vec<ScopedSymbolEligibility>,
    /// Scope where descriptor-owned misuse diagnostics may fire.
    pub misuse_scope: ScopedSymbolMisuseScope,
    /// Author-provided diagnostic templates.
    #[cfg_attr(feature = "serde", serde(default))]
    pub diagnostics: Vec<ScopedSymbolDiagnosticTemplate>,
}

impl ScopedSymbolDescriptor {
    /// Create a scoped symbol descriptor from a key, identifier spelling, and family.
    #[must_use]
    pub fn new(key: &str, symbol: &str, family: ScopedSymbolFamily) -> Self {
        Self {
            key: key.to_string(),
            symbol: symbol.to_string(),
            family,
            role: None,
            eligible_in: Vec::new(),
            misuse_scope: ScopedSymbolMisuseScope::None,
            diagnostics: Vec::new(),
        }
    }

    /// Create a function-like scoped symbol descriptor.
    #[must_use]
    pub fn function(key: &str, symbol: &str) -> Self {
        Self::new(key, symbol, ScopedSymbolFamily::FunctionLike)
    }

    /// Create an aggregate-like scoped symbol descriptor.
    #[must_use]
    pub fn aggregate(key: &str, symbol: &str) -> Self {
        Self::new(key, symbol, ScopedSymbolFamily::AggregateLike)
    }

    /// Attach optional DSL-authored role metadata.
    #[must_use]
    pub fn with_role(mut self, role: ScopedSymbolRoleMetadata) -> Self {
        self.role = Some(role);
        self
    }

    /// Add one positive eligibility position.
    #[must_use]
    pub fn with_eligibility(mut self, eligibility: ScopedSymbolEligibility) -> Self {
        self.eligible_in.push(eligibility);
        self
    }

    /// Add multiple positive eligibility positions.
    #[must_use]
    pub fn with_eligibilities<I>(mut self, eligibilities: I) -> Self
    where
        I: IntoIterator<Item = ScopedSymbolEligibility>,
    {
        self.eligible_in.extend(eligibilities);
        self
    }

    /// Mark the symbol as eligible in a named clause body.
    #[must_use]
    pub fn in_clause_body(self, declaration: &str, clause: &str) -> Self {
        self.with_eligibility(ScopedSymbolEligibility::clause_body(declaration, clause))
    }

    /// Mark the symbol as eligible in a declaration body.
    #[must_use]
    pub fn in_declaration_body(self, declaration: &str) -> Self {
        self.with_eligibility(ScopedSymbolEligibility::declaration_body(declaration))
    }

    /// Mark the symbol as eligible in arguments to a named function or method call.
    #[must_use]
    pub fn in_call_argument(self, declaration: &str, call: &str) -> Self {
        self.with_eligibility(ScopedSymbolEligibility::call_argument(declaration, call))
    }

    /// Set the misuse diagnostic scope.
    #[must_use]
    pub fn with_misuse_scope(mut self, misuse_scope: ScopedSymbolMisuseScope) -> Self {
        self.misuse_scope = misuse_scope;
        self
    }

    /// Add one author-provided diagnostic template.
    #[must_use]
    pub fn with_diagnostic(mut self, diagnostic: ScopedSymbolDiagnosticTemplate) -> Self {
        self.diagnostics.push(diagnostic);
        self
    }

    /// Add multiple author-provided diagnostic templates.
    #[must_use]
    pub fn with_diagnostics<I>(mut self, diagnostics: I) -> Self
    where
        I: IntoIterator<Item = ScopedSymbolDiagnosticTemplate>,
    {
        self.diagnostics.extend(diagnostics);
        self
    }
}
