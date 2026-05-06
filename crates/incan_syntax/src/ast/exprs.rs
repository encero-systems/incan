//! Expression AST types: literals, operators, calls, match/if expressions, comprehensions, and surface expressions.

use std::fmt;

use incan_semantics_core::SurfaceFeatureKey;

use super::{Ident, Param, Spanned, Statement, Type};

// ============================================================================
// Expressions
// ============================================================================

/// Slice expression: represents `start:end` or `start:end:step`
/// All components are optional, e.g., `[:5]`, `[2:]`, `[::2]`
#[derive(Debug, Clone, PartialEq)]
pub struct SliceExpr {
    pub start: Option<Box<Spanned<Expr>>>,
    pub end: Option<Box<Spanned<Expr>>>,
    pub step: Option<Box<Spanned<Expr>>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Identifier
    Ident(Ident),
    /// Literal
    Literal(Literal),
    /// `self`
    SelfExpr,
    /// Binary operation: `a + b`
    Binary(Box<Spanned<Expr>>, BinaryOp, Box<Spanned<Expr>>),
    /// Unary operation: `-x`, `not x`
    Unary(UnaryOp, Box<Spanned<Expr>>),
    /// Function/method call: `f(a, b)` or `f[T](a, b)`
    Call(Box<Spanned<Expr>>, Vec<Spanned<Type>>, Vec<CallArg>),
    /// Index: `x[i]`
    Index(Box<Spanned<Expr>>, Box<Spanned<Expr>>),
    /// Slice: `x[start:end]` or `x[start:end:step]`
    Slice(Box<Spanned<Expr>>, SliceExpr),
    /// Field access: `x.field`
    Field(Box<Spanned<Expr>>, Ident),
    /// Method call: `x.method(args)` or `x.method[T](args)`
    MethodCall(Box<Spanned<Expr>>, Ident, Vec<Spanned<Type>>, Vec<CallArg>),
    /// `expr?` (try/propagate)
    Try(Box<Spanned<Expr>>),
    /// Match expression
    Match(Box<Spanned<Expr>>, Vec<Spanned<MatchArm>>),
    /// If expression
    If(Box<IfExpr>),
    /// Loop expression
    Loop(Box<LoopExpr>),
    /// List comprehension: `[expr for x in iter if cond]`
    ListComp(Box<ListComp>),
    /// Dict comprehension: `{k: v for x in iter if cond}`
    DictComp(Box<DictComp>),
    /// Generator expression: `(expr for x in iter if cond)`
    Generator(Box<GeneratorExpr>),
    /// Closure: `(x, y) => expr` (a lot like python's lambda)
    Closure(Vec<Spanned<Param>>, Box<Spanned<Expr>>),
    /// Tuple: `(a, b)`
    Tuple(Vec<Spanned<Expr>>),
    /// List literal: `[a, *b, c]`
    List(Vec<ListEntry>),
    /// Dict literal: `{k: v, **other}`
    Dict(Vec<DictEntry>),
    /// Set literal: `{a, b, c}`
    Set(Vec<Spanned<Expr>>),
    /// Parenthesized expression
    Paren(Box<Spanned<Expr>>),
    /// Type constructor: `Some(x)`, `Ok(x)`, `User(id=1, name="Ada")`
    Constructor(Ident, Vec<CallArg>),
    /// f-string: `f"Hello {name}"`
    FString(Vec<FStringPart>),
    /// `yield expr` (for fixtures/generators)
    Yield(Option<Box<Spanned<Expr>>>),
    /// Range expression: `start..end` (exclusive) or `start..=end` (inclusive)
    Range {
        start: Box<Spanned<Expr>>,
        end: Box<Spanned<Expr>>,
        inclusive: bool,
    },
    /// Generic surface expression routed to semantics handlers.
    Surface(Box<SurfaceExpr>),
}

/// One entry in a list literal.
#[derive(Debug, Clone, PartialEq)]
pub enum ListEntry {
    /// Direct element expression.
    Element(Spanned<Expr>),
    /// Spread another list into the literal at this position.
    Spread(Spanned<Expr>),
}

/// One entry in a dict literal.
#[derive(Debug, Clone, PartialEq)]
pub enum DictEntry {
    /// Direct key/value pair.
    Pair(Spanned<Expr>, Spanned<Expr>),
    /// Spread another dict into the literal at this position.
    Spread(Spanned<Expr>),
}

/// Generic surface expression node emitted by parser handoff.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceExpr {
    pub key: SurfaceFeatureKey,
    pub payload: SurfaceExprPayload,
}

/// Surface expression payload variants.
#[derive(Debug, Clone, PartialEq)]
pub enum SurfaceExprPayload {
    /// Prefix unary keyword expression: `kw expr`.
    PrefixUnary(Box<Spanned<Expr>>),
    /// DSL-owned leading-dot path with an implicit receiver: `.field` or `.relation.field`.
    LeadingDotPath {
        segments: Vec<Ident>,
        receiver: incan_vocab::ScopedSurfaceReceiver,
        owner: ScopedSurfaceOwner,
    },
    /// DSL-owned binary glyph with local block semantics.
    ScopedGlyph {
        glyph: String,
        left: Box<Spanned<Expr>>,
        right: Box<Spanned<Expr>>,
        owner: ScopedSurfaceOwner,
    },
}

/// Source DSL context that accepted a scoped surface expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedSurfaceOwner {
    pub declaration: String,
    pub clause: Option<String>,
    pub call: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FStringPart {
    Literal(String),
    Expr(Spanned<Expr>),
}

/// Parsed integer literal with the **source substring** used for formatting.
///
/// [`IntLiteral::repr`] is the exact `source[start..end]` span from the lexer (including `_` numeric separators).
///
/// [`PartialEq`] compares only [`IntLiteral::value`] so AST equality tests do not depend on `repr`.
#[derive(Debug, Clone)]
pub struct IntLiteral {
    pub value: i64,
    pub repr: String,
}

impl PartialEq for IntLiteral {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl IntLiteral {
    /// Canonical decimal spelling — for AST nodes built without source text (tests, vocab bridge).
    pub fn synthetic(value: i64) -> Self {
        Self {
            value,
            repr: value.to_string(),
        }
    }
}

/// Parsed floating-point literal with the **source substring** used for formatting.
///
/// [`FloatLiteral::repr`] is the exact `source[start..end]` span from the lexer (including `_` numeric separators and
/// the author’s `e` / `E` exponent spelling). It avoids `f64` `Display` shortening (for example `120.0` vs `120`) and
/// keeps formatter output aligned with comment reattachment anchors.
///
/// [`PartialEq`] compares only [`FloatLiteral::value`] (IEEE bits) so AST equality tests do not depend on `repr`.
#[derive(Debug, Clone)]
pub struct FloatLiteral {
    pub value: f64,
    pub repr: String,
}

impl PartialEq for FloatLiteral {
    fn eq(&self, other: &Self) -> bool {
        self.value.to_bits() == other.value.to_bits()
    }
}

/// Parsed decimal literal with the **source substring** used for formatting.
///
/// The `body` field is the numeric spelling without `_` separators and without the trailing `d` suffix. Semantic
/// validation of precision and scale belongs to the typechecker.
#[derive(Debug, Clone)]
pub struct DecimalLiteral {
    pub body: String,
    pub repr: String,
}

impl PartialEq for DecimalLiteral {
    /// Compare semantic decimal literal bodies while ignoring source formatting.
    fn eq(&self, other: &Self) -> bool {
        self.body == other.body
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Int(IntLiteral),
    Float(FloatLiteral),
    Decimal(DecimalLiteral),
    String(String),
    Bytes(Vec<u8>),
    Bool(bool),
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv, // // (Python-style floor division)
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
    In,
    NotIn,
    Is,
    IsNot,
}

impl fmt::Display for BinaryOp {
    /// Format a binary operator using its source-level spelling.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BinaryOp::Add => write!(f, "+"),
            BinaryOp::Sub => write!(f, "-"),
            BinaryOp::Mul => write!(f, "*"),
            BinaryOp::Div => write!(f, "/"),
            BinaryOp::FloorDiv => write!(f, "//"),
            BinaryOp::Mod => write!(f, "%"),
            BinaryOp::Pow => write!(f, "**"),
            BinaryOp::MatMul => write!(f, "@"),
            BinaryOp::PipeForward => write!(f, "|>"),
            BinaryOp::PipeBackward => write!(f, "<|"),
            BinaryOp::BitAnd => write!(f, "&"),
            BinaryOp::BitOr => write!(f, "|"),
            BinaryOp::BitXor => write!(f, "^"),
            BinaryOp::Shl => write!(f, "<<"),
            BinaryOp::Shr => write!(f, ">>"),
            BinaryOp::Eq => write!(f, "=="),
            BinaryOp::NotEq => write!(f, "!="),
            BinaryOp::Lt => write!(f, "<"),
            BinaryOp::Gt => write!(f, ">"),
            BinaryOp::LtEq => write!(f, "<="),
            BinaryOp::GtEq => write!(f, ">="),
            BinaryOp::And => write!(f, "and"),
            BinaryOp::Or => write!(f, "or"),
            BinaryOp::In => write!(f, "in"),
            BinaryOp::NotIn => write!(f, "not in"),
            BinaryOp::Is => write!(f, "is"),
            BinaryOp::IsNot => write!(f, "is not"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
    Invert,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CallArg {
    /// Positional argument
    Positional(Spanned<Expr>),
    /// Named argument: `name=value`
    Named(Ident, Spanned<Expr>),
    /// Positional unpack argument: `*expr`.
    PositionalUnpack(Spanned<Expr>),
    /// Keyword unpack argument: `**expr`.
    KeywordUnpack(Spanned<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatternArg {
    /// Positional pattern: `Type(x)`
    Positional(Spanned<Pattern>),
    /// Named pattern: `Type(name=pat)`
    Named(Ident, Spanned<Pattern>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Spanned<Pattern>,
    pub guard: Option<Spanned<Expr>>, // `if condition` guard
    pub body: MatchBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchBody {
    /// `=> expr` (single expression)
    Expr(Spanned<Expr>),
    /// Block of statements
    Block(Vec<Spanned<Statement>>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    /// Wildcard: `_`
    Wildcard,
    /// Binding: `x`
    Binding(Ident),
    /// Literal: `42`, `"hello"`, `true`
    Literal(Literal),
    /// Constructor: `Some(x)`, `Ok(value)`, `Type(name=pat)`
    Constructor(Ident, Vec<PatternArg>),
    /// Tuple: `(a, b)`
    Tuple(Vec<Spanned<Pattern>>),
    /// Parenthesized pattern used for grouping: `(A | B)`
    Group(Box<Spanned<Pattern>>),
    /// Alternation: `A | B`
    Or(Vec<Spanned<Pattern>>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct IfExpr {
    /// Condition that decides whether the `then` body executes.
    pub condition: Spanned<Expr>,
    /// Statements evaluated when the condition is truthy.
    pub then_body: Vec<Spanned<Statement>>,
    /// Optional fallback statements evaluated when the condition is false.
    pub else_body: Option<Vec<Spanned<Statement>>>,
}

/// Explicit infinite-loop expression (`loop:`).
///
/// Unlike `while`, this form is allowed in expression position and may yield a value via `break <expr>`.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopExpr {
    /// Statements that execute for each loop iteration until a `break` exits the loop.
    pub body: Vec<Spanned<Statement>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListComp {
    /// Element expression produced for each accepted binding.
    pub expr: Spanned<Expr>,
    /// First `for` binding mirrored for single-clause comprehension lowering.
    pub pattern: Spanned<Pattern>,
    /// First `for` iterable mirrored for single-clause comprehension lowering.
    pub iter: Spanned<Expr>,
    /// First trailing `if` filter mirrored for single-clause comprehension lowering.
    pub filter: Option<Spanned<Expr>>,
    /// Parsed comprehension clauses in source order.
    pub clauses: Vec<ComprehensionClause>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DictComp {
    /// Key expression produced for each accepted binding.
    pub key: Spanned<Expr>,
    /// Value expression produced for each accepted binding.
    pub value: Spanned<Expr>,
    /// First `for` binding mirrored for single-clause comprehension lowering.
    pub pattern: Spanned<Pattern>,
    /// First `for` iterable mirrored for single-clause comprehension lowering.
    pub iter: Spanned<Expr>,
    /// First trailing `if` filter mirrored for single-clause comprehension lowering.
    pub filter: Option<Spanned<Expr>>,
    /// Parsed comprehension clauses in source order.
    pub clauses: Vec<ComprehensionClause>,
}

/// Generator-expression payload.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratorExpr {
    /// Expression yielded by the generator for each accepted binding.
    pub expr: Spanned<Expr>,
    /// Parsed comprehension clauses in source order.
    pub clauses: Vec<ComprehensionClause>,
}

/// One clause in a comprehension-like expression.
#[derive(Debug, Clone, PartialEq)]
pub enum ComprehensionClause {
    /// `for pattern in iter`
    For {
        /// Binding pattern introduced by the clause.
        pattern: Spanned<Pattern>,
        /// Iterable source consumed by the clause.
        iter: Spanned<Expr>,
    },
    /// `if condition`
    If(Spanned<Expr>),
}
