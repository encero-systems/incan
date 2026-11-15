//! Scoped surface families, syntax, eligibility, positions, diagnostics, receivers and descriptors.

use super::*;

/// Family of scoped surface form registered by a DSL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum ScopedSurfaceFamily {
    /// Operator-like glyph such as `>>`, `|>`, or `->`.
    #[default]
    OperatorLike,
    /// Binding-like glyph such as `:=`.
    BindingLike,
    /// Expression-form surface such as a leading-dot path.
    ExpressionForm,
}

/// Concrete syntax shape that activates a scoped surface form.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum ScopedSurfaceSyntax {
    /// Punctuation or symbolic glyph owned by a DSL in eligible positions.
    Glyph { spelling: String },
    /// Leading-dot path with an implicit receiver, for example `.column` or `.order.amount`.
    LeadingDotPath {
        /// Minimum accepted path segment count after the leading dot.
        min_segments: u16,
        /// Optional maximum accepted path segment count after the leading dot.
        max_segments: Option<u16>,
    },
}

impl Default for ScopedSurfaceSyntax {
    /// Default to an empty glyph syntax for serde compatibility.
    fn default() -> Self {
        Self::Glyph {
            spelling: String::new(),
        }
    }
}

/// DSL grammar position where a scoped surface has positive meaning.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ScopedSurfaceEligibility {
    /// Owning declaration keyword, for example `query` or `pipeline`.
    pub declaration: String,
    /// Optional owning clause spelling inside the declaration, for example `SELECT`.
    pub clause: Option<String>,
    /// Optional call target spelling for call-argument scoped surfaces, for example `filter`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub call: Option<String>,
    /// Body position where the surface is eligible.
    pub position: ScopedSurfacePosition,
}

impl ScopedSurfaceEligibility {
    /// Create an eligibility rule for a declaration body.
    #[must_use]
    pub fn declaration_body(declaration: &str) -> Self {
        Self {
            declaration: declaration.to_string(),
            clause: None,
            call: None,
            position: ScopedSurfacePosition::DeclarationBody,
        }
    }

    /// Create an eligibility rule for a declaration head.
    ///
    /// Manifest validation currently rejects this position until declaration-head scoped surfaces are implemented.
    #[must_use]
    pub fn declaration_head(declaration: &str) -> Self {
        Self {
            declaration: declaration.to_string(),
            clause: None,
            call: None,
            position: ScopedSurfacePosition::DeclarationHead,
        }
    }

    /// Create an eligibility rule for a named clause body.
    #[must_use]
    pub fn clause_body(declaration: &str, clause: &str) -> Self {
        Self {
            declaration: declaration.to_string(),
            clause: Some(clause.to_string()),
            call: None,
            position: ScopedSurfacePosition::ClauseBody,
        }
    }

    /// Create an eligibility rule for arguments to a named function or method call.
    #[must_use]
    pub fn call_argument(declaration: &str, call: &str) -> Self {
        Self {
            declaration: declaration.to_string(),
            clause: None,
            call: Some(call.to_string()),
            position: ScopedSurfacePosition::CallArgument,
        }
    }
}

/// Position kind within the owning DSL grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum ScopedSurfacePosition {
    /// Declaration header after the owning keyword. Reserved; rejected by current manifest validation.
    DeclarationHead,
    /// Declaration body item/expression position.
    #[default]
    DeclarationBody,
    /// Body of a named clause.
    ClauseBody,
    /// Argument expression of a named function or method call.
    CallArgument,
}

/// Where a descriptor may produce targeted misuse diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum ScopedSurfaceMisuseScope {
    /// Do not emit descriptor-owned diagnostics outside eligible positions.
    #[default]
    None,
    /// Emit descriptor-owned diagnostics in files where the descriptor is active.
    ActivatingFile,
    /// Emit descriptor-owned diagnostics in modules where the descriptor is active.
    ActivatingModule,
}

/// Diagnostic situation that can use an author-provided template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum ScopedSurfaceDiagnosticKind {
    /// The syntax shape was used outside an eligible DSL position.
    #[default]
    OutsideScope,
    /// Operand or payload type does not match the descriptor contract.
    WrongOperands,
    /// Binding-like surface received an invalid target.
    InvalidBindingTarget,
    /// Multiple descriptors could own the same surface occurrence.
    AmbiguousOwnership,
    /// Expression-form receiver derivation failed.
    InvalidReceiver,
}

/// Author-provided diagnostic text for a compiler-gated scoped-surface failure.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ScopedSurfaceDiagnosticTemplate {
    /// Stable diagnostic identity exposed to tests and tooling.
    pub code: String,
    /// Failure kind this template applies to.
    pub kind: ScopedSurfaceDiagnosticKind,
    /// Primary diagnostic message.
    pub message: String,
    /// Optional help text.
    pub help: Option<String>,
}

impl ScopedSurfaceDiagnosticTemplate {
    /// Create a new diagnostic template.
    #[must_use]
    pub fn new(code: &str, kind: ScopedSurfaceDiagnosticKind, message: &str) -> Self {
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

/// Receiver/context derivation for expression-form scoped surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum ScopedSurfaceReceiver {
    /// Receiver is supplied by the owning declaration.
    #[default]
    OwningDeclaration,
    /// Receiver is supplied by a named clause in the owning declaration.
    Clause { clause: String },
    /// Receiver derivation is DSL-specific and understood by the desugarer.
    Custom { key: String },
}

impl ScopedSurfaceReceiver {
    /// Receiver supplied by a named clause.
    #[must_use]
    pub fn clause(clause: &str) -> Self {
        Self::Clause {
            clause: clause.to_string(),
        }
    }

    /// DSL-specific receiver derivation key.
    #[must_use]
    pub fn custom(key: &str) -> Self {
        Self::Custom { key: key.to_string() }
    }
}

/// Formatter hint for chain-like scoped surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum ScopedSurfaceChainMode {
    /// Surface is not chain-shaped.
    #[default]
    None,
    /// Repeated surface occurrences should be preserved as pairwise links.
    Pairwise,
}

/// Formatter-facing metadata attached to a scoped surface descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ScopedSurfaceFormatHint {
    /// Whether repeated occurrences carry chain semantics.
    pub chain_mode: ScopedSurfaceChainMode,
}

/// One DSL-owned scoped surface descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ScopedSurfaceDescriptor {
    /// Stable descriptor identity used by artifacts, diagnostics, and tooling.
    pub key: String,
    /// Surface family.
    pub family: ScopedSurfaceFamily,
    /// Syntax shape that activates this surface.
    pub syntax: ScopedSurfaceSyntax,
    /// Positive positions where this surface has DSL-owned meaning.
    #[cfg_attr(feature = "serde", serde(default))]
    pub eligible_in: Vec<ScopedSurfaceEligibility>,
    /// Scope where descriptor-owned misuse diagnostics may fire.
    pub misuse_scope: ScopedSurfaceMisuseScope,
    /// Receiver/context derivation for expression-form surfaces.
    pub receiver: Option<ScopedSurfaceReceiver>,
    /// Author-provided diagnostic templates.
    #[cfg_attr(feature = "serde", serde(default))]
    pub diagnostics: Vec<ScopedSurfaceDiagnosticTemplate>,
    /// Formatter metadata.
    #[cfg_attr(feature = "serde", serde(default))]
    pub format_hint: ScopedSurfaceFormatHint,
}

impl ScopedSurfaceDescriptor {
    /// Create an operator-like glyph descriptor.
    #[must_use]
    pub fn operator(key: &str, glyph: &str) -> Self {
        Self::new(
            key,
            ScopedSurfaceFamily::OperatorLike,
            ScopedSurfaceSyntax::Glyph {
                spelling: glyph.to_string(),
            },
        )
    }

    /// Create a binding-like glyph descriptor.
    #[must_use]
    pub fn binding(key: &str, glyph: &str) -> Self {
        Self::new(
            key,
            ScopedSurfaceFamily::BindingLike,
            ScopedSurfaceSyntax::Glyph {
                spelling: glyph.to_string(),
            },
        )
    }

    /// Create a leading-dot expression-form descriptor.
    #[must_use]
    pub fn leading_dot_path(key: &str) -> Self {
        Self::new(
            key,
            ScopedSurfaceFamily::ExpressionForm,
            ScopedSurfaceSyntax::LeadingDotPath {
                min_segments: 1,
                max_segments: None,
            },
        )
    }

    /// Create a descriptor from explicit family and syntax values.
    #[must_use]
    pub fn new(key: &str, family: ScopedSurfaceFamily, syntax: ScopedSurfaceSyntax) -> Self {
        Self {
            key: key.to_string(),
            family,
            syntax,
            eligible_in: Vec::new(),
            misuse_scope: ScopedSurfaceMisuseScope::None,
            receiver: None,
            diagnostics: Vec::new(),
            format_hint: ScopedSurfaceFormatHint::default(),
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

    /// Mark the surface as eligible in a named clause body.
    #[must_use]
    pub fn in_clause_body(self, declaration: &str, clause: &str) -> Self {
        self.with_eligibility(ScopedSurfaceEligibility::clause_body(declaration, clause))
    }

    /// Mark the surface as eligible in a declaration body.
    #[must_use]
    pub fn in_declaration_body(self, declaration: &str) -> Self {
        self.with_eligibility(ScopedSurfaceEligibility::declaration_body(declaration))
    }

    /// Mark the surface as eligible in arguments to a named function or method call.
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

    /// Set the receiver/context derivation for expression-form surfaces.
    #[must_use]
    pub fn with_receiver(mut self, receiver: ScopedSurfaceReceiver) -> Self {
        self.receiver = Some(receiver);
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

    /// Mark this scoped surface as a pairwise chain for formatter/tooling consumers.
    #[must_use]
    pub fn pairwise_chain(mut self) -> Self {
        self.format_hint.chain_mode = ScopedSurfaceChainMode::Pairwise;
        self
    }

    /// Override formatter metadata.
    #[must_use]
    pub fn with_format_hint(mut self, format_hint: ScopedSurfaceFormatHint) -> Self {
        self.format_hint = format_hint;
        self
    }
}
