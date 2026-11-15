//! Declaration head, body and clause surfaces, expression item modifiers, and declaration surfaces.

use super::*;

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
    pub(super) fn from_spelling(spelling: &str, body_kind: ClauseBodyKind) -> Self {
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
pub(super) fn default_expression_item_modifiers(body_kind: ClauseBodyKind) -> Vec<ExpressionItemModifierSurface> {
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
    /// Nested declarations owned by this declaration.
    ///
    /// A vocabulary can use this to describe a declaration-shaped member without teaching the host parser a
    /// language-specific keyword. The nested declaration receives the enclosing declaration as its placement rule when
    /// it is registered.
    #[cfg_attr(feature = "serde", serde(default))]
    pub declarations: Vec<DeclarationSurface>,
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
            declarations: Vec::new(),
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
            declarations: Vec::new(),
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

    /// Mark this declaration as lowering to compilation-unit declarations.
    #[must_use]
    pub fn desugars_to_declarations(mut self) -> Self {
        self.desugars_to = DesugarTarget::Declarations;
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

    /// Add one declaration-shaped member owned by this declaration.
    #[must_use]
    pub fn with_declaration(mut self, declaration: DeclarationSurface) -> Self {
        self.declarations.push(declaration.in_block(&self.keyword));
        self
    }

    /// Add multiple declaration-shaped members owned by this declaration.
    #[must_use]
    pub fn with_declarations<I>(mut self, declarations: I) -> Self
    where
        I: IntoIterator<Item = DeclarationSurface>,
    {
        self.declarations.extend(
            declarations
                .into_iter()
                .map(|declaration| declaration.in_block(&self.keyword)),
        );
        self
    }
}
