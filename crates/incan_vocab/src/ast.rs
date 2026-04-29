//! Public AST surface for vocab desugarers.
//!
//! These types intentionally preserve more DSL structure than the previous block-only model. Query-shaped and
//! workflow-shaped DSLs need declaration heads, clause bodies, and expression/statement boundaries to survive the
//! parse -> desugar boundary without being flattened into ordinary statements too early.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// A byte-offset span in source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Span {
    /// Inclusive start offset.
    pub start: usize,
    /// Exclusive end offset.
    pub end: usize,
}

/// A decorator argument value.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum DecoratorArgValue {
    /// String literal argument.
    Str(String),
    /// Integer literal argument.
    Int(i64),
    /// Boolean literal argument.
    Bool(bool),
    /// Nested expression argument.
    Expr(IncanExpr),
}

/// A decorator argument.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DecoratorArg {
    /// Optional argument name for named arguments.
    pub name: Option<String>,
    /// Argument payload.
    pub value: DecoratorArgValue,
}

/// A decorator attached to a block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Decorator {
    /// Path segments for the decorator name.
    #[cfg_attr(feature = "serde", serde(default))]
    pub path: Vec<String>,
    /// Parsed decorator arguments.
    #[cfg_attr(feature = "serde", serde(default))]
    pub args: Vec<DecoratorArg>,
    /// Source span for this decorator.
    pub span: Span,
}

/// Compiler metadata associated with the registered vocab keyword.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct VocabKeywordMetadata {
    /// Dependency key (for example `widgets` from `pub::widgets`).
    pub dependency_key: String,
    /// Activation namespace from keyword registration metadata.
    pub activation_namespace: String,
    /// Surface kind used by the parser.
    pub surface_kind: crate::KeywordSurfaceKind,
    /// Placement rule declared by the companion crate.
    pub placement: crate::KeywordPlacement,
}

/// Opaque host-language type syntax captured inside a DSL-owned position.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct VocabTypeExpr {
    /// Type syntax text as written by the user.
    pub source: String,
    /// Source span for the type expression.
    pub span: Span,
}

/// A parameter parsed from a DSL-owned declaration head.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct VocabParameter {
    /// Parameter name.
    pub name: String,
    /// Optional parameter type annotation.
    pub param_type: Option<VocabTypeExpr>,
    /// Optional default value.
    pub default_value: Option<IncanExpr>,
    /// Source span for this parameter.
    pub span: Span,
}

/// Structured declaration head preserved for DSL-owned declarations.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct VocabDeclarationHead {
    /// Optional declaration name.
    pub name: Option<String>,
    /// Header arguments parsed after the introducing keyword.
    #[cfg_attr(feature = "serde", serde(default))]
    pub header_args: Vec<IncanExpr>,
    /// Signature-style parameters, when the DSL uses a function-like head.
    #[cfg_attr(feature = "serde", serde(default))]
    pub parameters: Vec<VocabParameter>,
    /// Optional declared return type for signature-style forms.
    pub return_type: Option<VocabTypeExpr>,
}

/// One field-like entry parsed inside a DSL-owned clause body.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct VocabFieldSpec {
    /// Field name.
    pub name: String,
    /// Optional field type annotation.
    pub field_type: Option<VocabTypeExpr>,
    /// Optional default value.
    pub default_value: Option<IncanExpr>,
    /// Source span for this field.
    pub span: Span,
}

/// A DSL-owned clause such as `FROM`, `SELECT`, `config`, or `input`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct VocabClause {
    /// The leading clause keyword.
    pub keyword: String,
    /// Additional tokens for compound clause spellings such as `GROUP BY`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub compound_tokens: Vec<String>,
    /// Inline expressions or names captured on the clause head.
    #[cfg_attr(feature = "serde", serde(default))]
    pub head: Vec<IncanExpr>,
    /// Clause body payload.
    pub body: VocabClauseBody,
    /// Source span covering the whole clause.
    pub span: Span,
}

/// Body payload for a DSL-owned clause.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum VocabClauseBody {
    /// No explicit body payload.
    #[default]
    Empty,
    /// A single expression payload.
    Expression(IncanExpr),
    /// A list of expressions, typically separated by commas or lines.
    ExpressionList(Vec<IncanExpr>),
    /// An opaque host-language type payload.
    Type(VocabTypeExpr),
    /// A field/config-style body.
    FieldSet(Vec<VocabFieldSpec>),
    /// Nested clause/declaration/body items.
    Items(Vec<VocabBodyItem>),
}

/// A DSL-owned declaration such as a query block, step, or pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct VocabDeclaration {
    /// The leading keyword that introduced the declaration.
    pub keyword: String,
    /// Optional parser-provided keyword metadata for resolver/runtime routing.
    pub keyword_metadata: Option<VocabKeywordMetadata>,
    /// Structured declaration head preserved across the parse/desugar boundary.
    pub head: VocabDeclarationHead,
    /// Decorators applied to the declaration, if any.
    #[cfg_attr(feature = "serde", serde(default))]
    pub decorators: Vec<Decorator>,
    /// Child clauses, nested declarations, or ordinary host statements inside the body.
    #[cfg_attr(feature = "serde", serde(default))]
    pub body: Vec<VocabBodyItem>,
    /// Source span covering the whole declaration.
    pub span: Span,
}

/// One item contained inside a [`VocabDeclaration`] or nested clause body.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum VocabBodyItem {
    /// A nested DSL-owned clause.
    Clause(VocabClause),
    /// A nested DSL-owned declaration.
    Declaration(VocabDeclaration),
    /// An ordinary statement inside the block body.
    Statement(IncanStatement),
}

/// One DSL syntax node that may be handed to a desugarer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum VocabSyntaxNode {
    /// A DSL-owned declaration.
    Declaration(VocabDeclaration),
    /// A standalone DSL-owned clause.
    Clause(VocabClause),
    /// A host-language statement that was preserved inside a DSL position.
    Statement(IncanStatement),
    /// A host-language expression that was preserved inside a DSL position.
    Expression(IncanExpr),
}

/// A minimal public statement surface for desugaring contracts.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum IncanStatement {
    /// A no-op statement.
    Pass,
    /// A statement represented by an expression.
    Expr(IncanExpr),
    /// Return from the current function.
    Return(Option<IncanExpr>),
    /// A simple assignment.
    Assign { target: String, value: IncanExpr },
    /// A new binding declaration.
    Let {
        name: String,
        mutable: bool,
        value: IncanExpr,
    },
    /// Conditional branch.
    If {
        condition: IncanExpr,
        then_body: Vec<IncanStatement>,
        else_body: Vec<IncanStatement>,
    },
    /// While loop.
    While {
        condition: IncanExpr,
        body: Vec<IncanStatement>,
    },
    /// For loop.
    For {
        binding: String,
        iter: IncanExpr,
        body: Vec<IncanStatement>,
    },
}

/// Binary expression operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum IncanBinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    MatMul,
    PipeForward,
    PipeBackward,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Eq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    And,
    Or,
}

/// Unary expression operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum IncanUnaryOp {
    Neg,
    Not,
    Invert,
}

/// A minimal public expression surface for desugaring contracts.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum IncanExpr {
    /// An identifier reference.
    Name(String),
    /// A symbolic helper reference resolved through the provider manifest.
    ///
    /// Desugarers should prefer this over hard-coded bare names when they need to call a library
    /// helper such as `filter` or `project`.
    Helper(String),
    /// A string literal.
    Str(String),
    /// An integer literal.
    Int(i64),
    /// A boolean literal.
    Bool(bool),
    /// A field on the current relational input (for example `.amount`).
    CurrentField(String),
    /// A field on a named relation (for example `orders.amount`).
    RelationField { relation: String, field: String },
    /// A list literal.
    List(Vec<IncanExpr>),
    /// A tuple literal.
    Tuple(Vec<IncanExpr>),
    /// A dictionary literal.
    Dict(Vec<(IncanExpr, IncanExpr)>),
    /// Binary expression.
    Binary(Box<IncanExpr>, IncanBinaryOp, Box<IncanExpr>),
    /// Unary expression.
    Unary(IncanUnaryOp, Box<IncanExpr>),
    /// Function call.
    Call {
        callee: Box<IncanExpr>,
        args: Vec<IncanExpr>,
    },
    /// Field access.
    Field { object: Box<IncanExpr>, field: String },
    /// DSL-owned scoped surface expression accepted by the compiler.
    ScopedSurface(IncanScopedSurfaceExpr),
}

/// Public desugarer-facing representation of an accepted scoped-surface expression.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct IncanScopedSurfaceExpr {
    pub dependency_key: String,
    pub descriptor_key: String,
    pub payload: IncanScopedSurfacePayload,
}

/// Public desugarer-facing payload for an accepted scoped-surface expression.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum IncanScopedSurfacePayload {
    /// Leading-dot path with an implicit receiver.
    LeadingDotPath {
        segments: Vec<String>,
        receiver: crate::ScopedSurfaceReceiver,
        owner: IncanScopedSurfaceOwner,
    },
    /// DSL-owned binary glyph.
    ScopedGlyph {
        glyph: String,
        left: Box<IncanExpr>,
        right: Box<IncanExpr>,
        owner: IncanScopedSurfaceOwner,
    },
}

/// Public owner context for an accepted scoped-surface expression.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct IncanScopedSurfaceOwner {
    pub declaration: String,
    pub clause: Option<String>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub call: Option<String>,
}
