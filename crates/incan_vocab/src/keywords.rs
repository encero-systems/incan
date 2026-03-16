//! Keyword registration surface shared by vocab providers and compiler tooling.
//!
//! These types describe *what* a library contributes to the parser surface, not how the compiler internally implements
//! those keywords.

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

    fn from_spelling(spelling: &str, body_kind: ClauseBodyKind) -> Self {
        let (keyword, compound_tokens) = split_spelling(spelling);
        Self {
            keyword,
            compound_tokens,
            body_kind,
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
}

impl DslSurface {
    /// Create an empty activated surface.
    #[must_use]
    pub fn new(activation: KeywordActivation) -> Self {
        Self {
            activation,
            declarations: Vec::new(),
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
