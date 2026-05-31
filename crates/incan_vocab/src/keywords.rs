//! High-level DSL surface types plus low-level keyword registration DTOs (Data Transfer Object).
//!
//! Companion crates should usually describe their grammar through [`DslSurface`], [`DeclarationSurface`], and
//! [`ClauseSurface`] inside the `library_vocab()` entrypoint. [`crate::VocabRegistration`], [`KeywordRegistration`],
//! and [`KeywordSpec`] remain available for compiler tooling and simple escape-hatch registrations.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// A group of keywords that share one activation rule.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct KeywordRegistration {
    /// Activation rule for this keyword group.
    pub activation: KeywordActivation,
    /// Keywords introduced by this group.
    #[cfg_attr(feature = "serde", serde(default))]
    pub keywords: Vec<KeywordSpec>,
    /// Decorators valid on blocks introduced by these keywords.
    #[cfg_attr(feature = "serde", serde(default))]
    pub valid_decorators: Vec<String>,
}

impl KeywordRegistration {
    /// Create an empty keyword registration for one activation rule.
    #[must_use]
    pub fn new(activation: KeywordActivation) -> Self {
        Self {
            activation,
            keywords: Vec::new(),
            valid_decorators: Vec::new(),
        }
    }

    /// Create an import-activated keyword registration.
    #[must_use]
    pub fn on_import(namespace: &str) -> Self {
        Self::new(KeywordActivation::on_import(namespace))
    }

    /// Create an always-active keyword registration.
    #[must_use]
    pub fn always_on() -> Self {
        Self::new(KeywordActivation::Always)
    }

    /// Add one keyword to the registration.
    #[must_use]
    pub fn with_keyword(mut self, keyword: KeywordSpec) -> Self {
        self.keywords.push(keyword);
        self
    }

    /// Add multiple keywords to the registration.
    #[must_use]
    pub fn with_keywords<I>(mut self, keywords: I) -> Self
    where
        I: IntoIterator<Item = KeywordSpec>,
    {
        self.keywords.extend(keywords);
        self
    }

    /// Add one valid decorator to the registration.
    #[must_use]
    pub fn with_valid_decorator(mut self, decorator: impl Into<String>) -> Self {
        self.valid_decorators.push(decorator.into());
        self
    }

    /// Add multiple valid decorators to the registration.
    #[must_use]
    pub fn with_valid_decorators<I, S>(mut self, decorators: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.valid_decorators.extend(decorators.into_iter().map(Into::into));
        self
    }
}

/// Activation rule for a keyword group.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum KeywordActivation {
    /// Always active in every file.
    #[default]
    Always,
    /// Activated when a matching import path is used.
    ///
    /// The namespace should use the library-facing import spelling, for example `mydsl.routes`.
    OnImport { namespace: String },
}

impl KeywordActivation {
    /// Create an import-activated keyword rule.
    #[must_use]
    pub fn on_import(namespace: &str) -> Self {
        Self::OnImport {
            namespace: namespace.to_string(),
        }
    }
}

/// Specification for a single keyword.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct KeywordSpec {
    /// Primary keyword token as written by the user.
    pub name: String,
    /// Parser surface occupied by the keyword.
    pub surface_kind: KeywordSurfaceKind,
    /// Additional tokens for compound keyword spellings.
    #[cfg_attr(feature = "serde", serde(default))]
    pub compound_tokens: Vec<String>,
    /// Where the keyword is valid.
    pub placement: KeywordPlacement,
}

impl KeywordSpec {
    /// Create a simple top-level keyword.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use incan_vocab::{KeywordPlacement, KeywordSpec, KeywordSurfaceKind};
    ///
    /// let spec = KeywordSpec::new("route", KeywordSurfaceKind::BlockDeclaration);
    /// assert_eq!(spec.name, "route");
    /// assert_eq!(spec.surface_kind, KeywordSurfaceKind::BlockDeclaration);
    /// assert_eq!(spec.placement, KeywordPlacement::TopLevel);
    /// assert!(spec.compound_tokens.is_empty());
    /// ```
    #[must_use]
    pub fn new(name: &str, surface_kind: KeywordSurfaceKind) -> Self {
        Self {
            name: name.to_string(),
            surface_kind,
            compound_tokens: Vec::new(),
            placement: KeywordPlacement::TopLevel,
        }
    }

    /// Create a top-level DSL block declaration keyword.
    #[must_use]
    pub fn block(name: &str) -> Self {
        Self::new(name, KeywordSurfaceKind::BlockDeclaration)
    }

    /// Create a block-context keyword scoped to one parent block.
    #[must_use]
    pub fn block_context(name: &str, parent_keyword: &str) -> Self {
        Self::new(name, KeywordSurfaceKind::BlockContextKeyword).in_block(parent_keyword)
    }

    /// Create a sub-block keyword scoped to one parent block.
    #[must_use]
    pub fn sub_block(name: &str, parent_keyword: &str) -> Self {
        Self::new(name, KeywordSurfaceKind::SubBlock).in_block(parent_keyword)
    }

    /// Replace the compound-token tail for this keyword.
    #[must_use]
    pub fn with_compound_tokens<I, S>(mut self, compound_tokens: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.compound_tokens = compound_tokens.into_iter().map(Into::into).collect();
        self
    }

    /// Set the placement rule for this keyword.
    #[must_use]
    pub fn with_placement(mut self, placement: KeywordPlacement) -> Self {
        self.placement = placement;
        self
    }

    /// Scope this keyword to one parent block.
    #[must_use]
    pub fn in_block(self, parent_keyword: &str) -> Self {
        self.with_placement(KeywordPlacement::in_block([parent_keyword]))
    }

    /// Scope this keyword to one of several parent blocks.
    #[must_use]
    pub fn in_blocks<I, S>(self, parent_keywords: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.with_placement(KeywordPlacement::in_block(parent_keywords))
    }
}

/// Where a keyword is valid.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum KeywordPlacement {
    /// Valid in ordinary statement/declaration positions.
    #[default]
    TopLevel,
    /// Valid only inside one of the provided parent blocks.
    ///
    /// The strings here refer to registered parent keyword names.
    InBlock(Vec<String>),
}

impl KeywordPlacement {
    /// Create a placement rule that is valid inside the provided parent blocks.
    #[must_use]
    pub fn in_block<I, S>(parent_keywords: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::InBlock(parent_keywords.into_iter().map(Into::into).collect())
    }
}

/// High-level shape of a DSL-owned declaration head.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum DeclarationHeadKind {
    /// Keyword-only head with no extra structured payload.
    #[default]
    None,
    /// Header arguments after the leading keyword.
    HeaderArguments,
    /// Function-like signature with name, params, and optional return type.
    Signature,
    /// DSL may use a mixture of header arguments and signature-style fields.
    Mixed,
}

/// High-level shape of a DSL-owned declaration body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum DeclarationBodyKind {
    /// Body is clause-oriented.
    Clauses,
    /// Body is ordinary statement-oriented.
    Statements,
    /// Body may contain a mixture of clauses and ordinary statements.
    #[default]
    Mixed,
}

/// High-level payload kind for a DSL-owned clause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum ClauseBodyKind {
    /// Clause head/body is a single expression.
    #[default]
    Expression,
    /// Clause body is a list of expressions.
    ExpressionList,
    /// Clause body is a type position.
    Type,
    /// Clause body is a field/config specification set.
    FieldSet,
    /// Clause body contains nested clause/declaration items.
    NestedItems,
}

/// Payload parser for a trailing keyword on one expression-list item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum ExpressionItemModifierKind {
    /// The trailing keyword captures an alias identifier, such as `expr as name`.
    #[default]
    Alias,
    /// The trailing keyword captures another expression, such as `expr for target`.
    Expression,
}

/// One trailing keyword accepted after an expression-list item.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ExpressionItemModifierSurface {
    /// Keyword spelling consumed after the leading item expression.
    pub keyword: String,
    /// Payload shape consumed after the keyword.
    pub kind: ExpressionItemModifierKind,
}

impl ExpressionItemModifierSurface {
    /// Create an alias modifier such as `expr as alias`.
    #[must_use]
    pub fn alias(keyword: &str) -> Self {
        Self {
            keyword: keyword.to_string(),
            kind: ExpressionItemModifierKind::Alias,
        }
    }

    /// Create an expression modifier such as `expr for target`.
    #[must_use]
    pub fn expr(keyword: &str) -> Self {
        Self {
            keyword: keyword.to_string(),
            kind: ExpressionItemModifierKind::Expression,
        }
    }
}

/// Relative placement of one clause within a declaration's clause grammar.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum ClausePlacement {
    /// Clause may appear anywhere the owning declaration accepts clauses.
    #[default]
    Anywhere,
    /// Clause is intended to appear before another named clause.
    Before(String),
    /// Clause is intended to appear after another named clause.
    After(String),
}

/// How often a clause may appear inside a declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum ClauseCardinality {
    /// Clause must appear exactly once.
    Required,
    /// Clause may appear at most once.
    #[default]
    Optional,
    /// Clause may appear multiple times.
    Repeating,
}

/// One DSL-owned clause surface nested under a declaration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ClauseSurface {
    /// Leading clause keyword.
    pub keyword: String,
    /// Additional tokens for compound spellings such as `GROUP BY`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub compound_tokens: Vec<String>,
    /// Structured body payload kind for this clause.
    pub body_kind: ClauseBodyKind,
    /// Trailing keyword payloads accepted after expression-list items.
    #[cfg_attr(feature = "serde", serde(default))]
    pub expression_item_modifiers: Vec<ExpressionItemModifierSurface>,
    /// Whether the clause is required, optional, or repeatable.
    pub cardinality: ClauseCardinality,
    /// Relative ordering guidance within the owning declaration.
    pub placement: ClausePlacement,
}

impl ClauseSurface {
    /// Create a new clause surface with the provided body kind.
    #[must_use]
    pub fn new(keyword: &str, body_kind: ClauseBodyKind) -> Self {
        Self {
            keyword: keyword.to_string(),
            compound_tokens: Vec::new(),
            body_kind,
            expression_item_modifiers: default_expression_item_modifiers(body_kind),
            cardinality: ClauseCardinality::Optional,
            placement: ClausePlacement::Anywhere,
        }
    }

    /// Create an expression clause from its full spelling.
    #[must_use]
    pub fn expr(spelling: &str) -> Self {
        Self::from_spelling(spelling, ClauseBodyKind::Expression)
    }

    /// Create an expression-list clause from its full spelling.
    ///
    /// Expression-list clauses preserve each item as [`crate::VocabExpressionItem`]. They accept SQL-style `expr as
    /// alias` by default and can declare more trailing keyword payloads with
    /// [`Self::with_expression_item_modifier`].
    #[must_use]
    pub fn expr_list(spelling: &str) -> Self {
        Self::from_spelling(spelling, ClauseBodyKind::ExpressionList)
    }

    /// Create a field-set clause from its full spelling.
    #[must_use]
    pub fn fields(spelling: &str) -> Self {
        Self::from_spelling(spelling, ClauseBodyKind::FieldSet)
    }

    /// Create a type-position clause from its full spelling.
    #[must_use]
    pub fn type_ref(spelling: &str) -> Self {
        Self::from_spelling(spelling, ClauseBodyKind::Type)
    }

    /// Create a nested-items clause from its full spelling.
    #[must_use]
    pub fn nested_items(spelling: &str) -> Self {
        Self::from_spelling(spelling, ClauseBodyKind::NestedItems)
    }

    /// Create a clause from a full spelling and attach any defaults implied by its body kind.
    fn from_spelling(spelling: &str, body_kind: ClauseBodyKind) -> Self {
        let (keyword, compound_tokens) = split_spelling(spelling);
        Self {
            keyword,
            compound_tokens,
            body_kind,
            expression_item_modifiers: default_expression_item_modifiers(body_kind),
            cardinality: ClauseCardinality::Optional,
            placement: ClausePlacement::Anywhere,
        }
    }

    /// Replace the compound-token tail for this clause.
    #[must_use]
    pub fn with_compound_tokens<I, S>(mut self, compound_tokens: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.compound_tokens = compound_tokens.into_iter().map(Into::into).collect();
        self
    }

    /// Add one trailing keyword parser for expression-list items.
    #[must_use]
    pub fn with_expression_item_modifier(mut self, modifier: ExpressionItemModifierSurface) -> Self {
        if !self
            .expression_item_modifiers
            .iter()
            .any(|existing| existing.keyword == modifier.keyword)
        {
            self.expression_item_modifiers.push(modifier);
        }
        self
    }

    /// Add multiple trailing keyword parsers for expression-list items.
    #[must_use]
    pub fn with_expression_item_modifiers<I>(mut self, modifiers: I) -> Self
    where
        I: IntoIterator<Item = ExpressionItemModifierSurface>,
    {
        for modifier in modifiers {
            self = self.with_expression_item_modifier(modifier);
        }
        self
    }

    /// Mark this clause as required.
    #[must_use]
    pub fn required(mut self) -> Self {
        self.cardinality = ClauseCardinality::Required;
        self
    }

    /// Mark this clause as optional.
    #[must_use]
    pub fn optional(mut self) -> Self {
        self.cardinality = ClauseCardinality::Optional;
        self
    }

    /// Mark this clause as repeatable.
    #[must_use]
    pub fn repeating(mut self) -> Self {
        self.cardinality = ClauseCardinality::Repeating;
        self
    }

    /// Record that this clause should appear before another clause.
    #[must_use]
    pub fn before(mut self, other_clause: &str) -> Self {
        self.placement = ClausePlacement::Before(other_clause.to_string());
        self
    }

    /// Record that this clause should appear after another clause.
    #[must_use]
    pub fn after(mut self, other_clause: &str) -> Self {
        self.placement = ClausePlacement::After(other_clause.to_string());
        self
    }
}

/// Return expression-list item modifiers that are part of the built-in high-level clause contract.
fn default_expression_item_modifiers(body_kind: ClauseBodyKind) -> Vec<ExpressionItemModifierSurface> {
    if matches!(body_kind, ClauseBodyKind::ExpressionList) {
        vec![ExpressionItemModifierSurface::alias("as")]
    } else {
        Vec::new()
    }
}

/// One DSL-owned declaration surface such as a query block, stage, or workflow.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DeclarationSurface {
    /// Leading declaration keyword.
    pub keyword: String,
    /// Additional tokens for compound declaration spellings.
    #[cfg_attr(feature = "serde", serde(default))]
    pub compound_tokens: Vec<String>,
    /// Where the declaration may appear.
    pub placement: KeywordPlacement,
    /// Structured shape of the declaration head.
    pub head_kind: DeclarationHeadKind,
    /// Structured shape of the declaration body.
    pub body_kind: DeclarationBodyKind,
    /// Whether the declaration desugars as an expression or statements.
    pub desugars_to: DesugarTarget,
    /// Nested clauses owned by this declaration.
    #[cfg_attr(feature = "serde", serde(default))]
    pub clauses: Vec<ClauseSurface>,
}

impl DeclarationSurface {
    /// Create a declaration surface with top-level placement by default.
    #[must_use]
    pub fn new(keyword: &str) -> Self {
        Self {
            keyword: keyword.to_string(),
            compound_tokens: Vec::new(),
            placement: KeywordPlacement::TopLevel,
            head_kind: DeclarationHeadKind::None,
            body_kind: DeclarationBodyKind::Mixed,
            desugars_to: DesugarTarget::Statements,
            clauses: Vec::new(),
        }
    }

    /// Create a declaration surface from its full spelling.
    #[must_use]
    pub fn named(spelling: &str) -> Self {
        let (keyword, compound_tokens) = split_spelling(spelling);
        Self {
            keyword,
            compound_tokens,
            placement: KeywordPlacement::TopLevel,
            head_kind: DeclarationHeadKind::None,
            body_kind: DeclarationBodyKind::Mixed,
            desugars_to: DesugarTarget::Statements,
            clauses: Vec::new(),
        }
    }

    /// Replace the compound-token tail for this declaration keyword.
    #[must_use]
    pub fn with_compound_tokens<I, S>(mut self, compound_tokens: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.compound_tokens = compound_tokens.into_iter().map(Into::into).collect();
        self
    }

    /// Set the declaration placement rule.
    #[must_use]
    pub fn with_placement(mut self, placement: KeywordPlacement) -> Self {
        self.placement = placement;
        self
    }

    /// Scope this declaration to one parent declaration.
    #[must_use]
    pub fn in_block(self, parent_keyword: &str) -> Self {
        self.with_placement(KeywordPlacement::in_block([parent_keyword]))
    }

    /// Model a declaration head that accepts header-style arguments.
    #[must_use]
    pub fn with_header_args(mut self) -> Self {
        self.head_kind = DeclarationHeadKind::HeaderArguments;
        self
    }

    /// Model a declaration head that uses a signature-style shape.
    #[must_use]
    pub fn with_signature_head(mut self) -> Self {
        self.head_kind = DeclarationHeadKind::Signature;
        self
    }

    /// Model a declaration body that is clause-oriented.
    #[must_use]
    pub fn with_clause_body(mut self) -> Self {
        self.body_kind = DeclarationBodyKind::Clauses;
        self
    }

    /// Model a declaration body that is statement-oriented.
    #[must_use]
    pub fn with_statement_body(mut self) -> Self {
        self.body_kind = DeclarationBodyKind::Statements;
        self
    }

    /// Model a declaration body that mixes clauses and host statements.
    #[must_use]
    pub fn with_mixed_body(mut self) -> Self {
        self.body_kind = DeclarationBodyKind::Mixed;
        self
    }

    /// Set the declaration head shape.
    #[must_use]
    pub fn with_head_kind(mut self, head_kind: DeclarationHeadKind) -> Self {
        self.head_kind = head_kind;
        self
    }

    /// Set the declaration body shape.
    #[must_use]
    pub fn with_body_kind(mut self, body_kind: DeclarationBodyKind) -> Self {
        self.body_kind = body_kind;
        self
    }

    /// Mark this declaration as desugaring to an expression.
    #[must_use]
    pub fn desugars_to_expression(mut self) -> Self {
        self.desugars_to = DesugarTarget::Expression;
        self
    }

    /// Mark this declaration as desugaring to statements.
    #[must_use]
    pub fn desugars_to_statements(mut self) -> Self {
        self.desugars_to = DesugarTarget::Statements;
        self
    }

    /// Add one owned clause surface.
    #[must_use]
    pub fn with_clause(mut self, clause: ClauseSurface) -> Self {
        self.clauses.push(clause);
        self
    }

    /// Add multiple owned clause surfaces.
    #[must_use]
    pub fn with_clauses<I>(mut self, clauses: I) -> Self
    where
        I: IntoIterator<Item = ClauseSurface>,
    {
        self.clauses.extend(clauses);
        self
    }
}

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

/// Whether a declaration lowers into an expression or a statement list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum DesugarTarget {
    /// Lower into host statements.
    #[default]
    Statements,
    /// Lower into one host expression.
    Expression,
}

/// One activated DSL surface contributed by a library.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DslSurface {
    /// Activation rule shared by declarations in this surface.
    pub activation: KeywordActivation,
    /// Declarations contributed by this activated surface.
    #[cfg_attr(feature = "serde", serde(default))]
    pub declarations: Vec<DeclarationSurface>,
    /// Scoped surface forms contributed by this activated surface.
    #[cfg_attr(feature = "serde", serde(default))]
    pub scoped_surfaces: Vec<ScopedSurfaceDescriptor>,
    /// Scoped identifier symbols contributed by this activated surface.
    #[cfg_attr(feature = "serde", serde(default))]
    pub scoped_symbols: Vec<ScopedSymbolDescriptor>,
}

impl DslSurface {
    /// Create an empty activated surface.
    #[must_use]
    pub fn new(activation: KeywordActivation) -> Self {
        Self {
            activation,
            declarations: Vec::new(),
            scoped_surfaces: Vec::new(),
            scoped_symbols: Vec::new(),
        }
    }

    /// Create an import-activated DSL surface.
    #[must_use]
    pub fn on_import(namespace: &str) -> Self {
        Self::new(KeywordActivation::on_import(namespace))
    }

    /// Create an always-active DSL surface.
    #[must_use]
    pub fn always_on() -> Self {
        Self::new(KeywordActivation::Always)
    }

    /// Add one declaration to this surface.
    #[must_use]
    pub fn with_declaration(mut self, declaration: DeclarationSurface) -> Self {
        self.declarations.push(declaration);
        self
    }

    /// Add multiple declarations to this surface.
    #[must_use]
    pub fn with_declarations<I>(mut self, declarations: I) -> Self
    where
        I: IntoIterator<Item = DeclarationSurface>,
    {
        self.declarations.extend(declarations);
        self
    }

    /// Add one scoped surface descriptor to this activated surface.
    #[must_use]
    pub fn with_scoped_surface(mut self, scoped_surface: ScopedSurfaceDescriptor) -> Self {
        self.scoped_surfaces.push(scoped_surface);
        self
    }

    /// Add multiple scoped surface descriptors to this activated surface.
    #[must_use]
    pub fn with_scoped_surfaces<I>(mut self, scoped_surfaces: I) -> Self
    where
        I: IntoIterator<Item = ScopedSurfaceDescriptor>,
    {
        self.scoped_surfaces.extend(scoped_surfaces);
        self
    }

    /// Add one scoped symbol descriptor to this activated surface.
    #[must_use]
    pub fn with_scoped_symbol(mut self, scoped_symbol: ScopedSymbolDescriptor) -> Self {
        self.scoped_symbols.push(scoped_symbol);
        self
    }

    /// Add multiple scoped symbol descriptors to this activated surface.
    #[must_use]
    pub fn with_scoped_symbols<I>(mut self, scoped_symbols: I) -> Self
    where
        I: IntoIterator<Item = ScopedSymbolDescriptor>,
    {
        self.scoped_symbols.extend(scoped_symbols);
        self
    }
}

fn split_spelling(spelling: &str) -> (String, Vec<String>) {
    let mut parts = spelling.split_whitespace();
    let keyword = parts.next().unwrap_or_default().to_string();
    let compound_tokens = parts.map(str::to_string).collect();
    (keyword, compound_tokens)
}

/// Parser surface shape for a keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum KeywordSurfaceKind {
    /// `def name(...):`
    #[default]
    FunctionDecl,
    /// `class/model/trait/enum Name:`
    TypeDecl,
    /// `if ...:`
    ConditionalChain,
    /// `for ... in ...:`
    ForLoop,
    /// `while ...:`
    WhileLoop,
    /// `match ...:`
    MatchBlock,
    /// `try: ...`
    TryBlock,
    /// `import ...` / `from ... import ...`
    ImportStatement,
    /// `return`, `break`, `continue`, `pass`, `raise`, `yield`.
    ControlFlow,
    /// `let ... = ...`
    BindingDecl,
    /// Library/DSL block declaration (for example `routes:`).
    BlockDeclaration,
    /// Keyword only valid inside a parent block (for example `GET` in `routes`).
    BlockContextKeyword,
    /// Sub-block in a DSL block.
    SubBlock,
}
