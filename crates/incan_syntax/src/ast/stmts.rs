//! Statement AST types: assignments, control flow, assertions, and surface statements.

use incan_semantics_core::SurfaceFeatureKey;

use super::{Decorator, Expr, Ident, Pattern, Span, Spanned, Type};

// ============================================================================
// Statements
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    /// `x = value` or `let x = value` or `mut x = value`
    Assignment(AssignmentStmt),
    /// `obj.field = value` or `self.field = value`
    FieldAssignment(FieldAssignmentStmt),
    /// `obj[index] = value`
    IndexAssignment(IndexAssignmentStmt),
    /// `return expr`
    Return(Option<Spanned<Expr>>),
    /// `if expr: ... [else: ...]`
    If(IfStmt),
    /// `while expr: ...`
    While(WhileStmt),
    /// `for x in expr: ...`
    For(ForStmt),
    /// Expression statement
    Expr(Spanned<Expr>),
    /// `pass` or `...`
    Pass,
    /// `break` - exit the innermost loop
    Break,
    /// `continue` - skip to next iteration
    Continue,
    /// Compound assignment: `x += value`, `x -= value`, etc.
    CompoundAssignment(CompoundAssignmentStmt),
    /// Tuple unpacking: `a, b = expr` or `let a, b = expr`
    TupleUnpack(TupleUnpackStmt),
    /// Tuple assignment to lvalues: `arr[i], arr[j] = arr[j], arr[i]`
    TupleAssign(TupleAssignStmt),
    /// Chained assignment: `x = y = z = value`
    ChainedAssignment(ChainedAssignmentStmt),
    /// Generic surface statement routed to semantics handlers.
    Surface(SurfaceStmt),
    /// Raw library vocab block preserved for post-parse desugaring.
    VocabBlock(VocabBlockStmt),
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssignmentStmt {
    pub binding: BindingKind,
    pub name: Ident,
    pub ty: Option<Spanned<Type>>,
    pub value: Spanned<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldAssignmentStmt {
    /// Span of the assignment target (e.g. `self.field`).
    pub target_span: Span,
    pub object: Spanned<Expr>,
    pub field: Ident,
    pub value: Spanned<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexAssignmentStmt {
    pub object: Spanned<Expr>,
    pub index: Spanned<Expr>,
    pub value: Spanned<Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    /// Plain `x = value` - first assignment (becomes `let` in Rust)
    Inferred,
    /// `let x = value`
    Let,
    /// `mut x = value`
    Mutable,
    /// Reassignment to existing mutable variable (no `let` in Rust)
    Reassign,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompoundAssignmentStmt {
    pub name: Ident,
    pub op: CompoundOp,
    pub value: Spanned<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChainedAssignmentStmt {
    pub binding: BindingKind,
    pub targets: Vec<Ident>,
    pub value: Spanned<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TupleUnpackStmt {
    pub binding: BindingKind,
    pub names: Vec<Ident>,
    pub value: Spanned<Expr>,
}

/// Tuple assignment to lvalue expressions: `arr[i], arr[j] = arr[j], arr[i]`
/// Used for swaps and multi-target assignments where targets are not new bindings.
#[derive(Debug, Clone, PartialEq)]
pub struct TupleAssignStmt {
    /// Left-hand side lvalue expressions (index, field, or identifier references)
    pub targets: Vec<Spanned<Expr>>,
    /// Right-hand side expression (typically a tuple)
    pub value: Spanned<Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompoundOp {
    Add,      // +=
    Sub,      // -=
    Mul,      // *=
    Div,      // /=
    FloorDiv, // //=
    Mod,      // %=
}

#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    /// Ordinary boolean condition: `if expr:` / `while expr:`
    Expr(Spanned<Expr>),
    /// Pattern condition: `if let PATTERN = VALUE:` / `while let PATTERN = VALUE:`
    Let {
        pattern: Spanned<Pattern>,
        value: Spanned<Expr>,
    },
}

impl Condition {
    /// Return the source span that covers the full control-flow condition.
    ///
    /// For `if let` / `while let`, this spans from the pattern start through the
    /// scrutinee expression so downstream diagnostics and tooling can treat the
    /// let-pattern condition as one surface unit.
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Expr(expr) => expr.span,
            Self::Let { pattern, value } => Span::new(pattern.span.start, value.span.end),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct IfStmt {
    pub condition: Condition,
    pub then_body: Vec<Spanned<Statement>>,
    pub elif_branches: Vec<(Spanned<Expr>, Vec<Spanned<Statement>>)>,
    pub else_body: Option<Vec<Spanned<Statement>>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WhileStmt {
    pub condition: Condition,
    pub body: Vec<Spanned<Statement>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForStmt {
    pub pattern: Spanned<Pattern>,
    pub iter: Spanned<Expr>,
    pub body: Vec<Spanned<Statement>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssertStmt {
    pub condition: Spanned<Expr>,
    pub message: Option<Spanned<Expr>>,
}

/// Generic surface statement node emitted by parser handoff.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceStmt {
    pub key: SurfaceFeatureKey,
    pub payload: SurfaceStmtPayload,
}

/// Parser metadata describing how a raw vocab block keyword was resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VocabKeywordBinding {
    pub dependency_key: String,
    pub activation_namespace: String,
    pub surface_kind: incan_vocab::KeywordSurfaceKind,
    pub placement: incan_vocab::KeywordPlacement,
}

/// Raw vocab block statement captured before desugaring.
#[derive(Debug, Clone, PartialEq)]
pub struct VocabBlockStmt {
    pub keyword: String,
    pub keyword_binding: VocabKeywordBinding,
    pub decorators: Vec<Spanned<Decorator>>,
    pub header_args: Vec<Spanned<Expr>>,
    pub body: Vec<Spanned<Statement>>,
}

/// Surface statement payload variants.
#[derive(Debug, Clone, PartialEq)]
pub enum SurfaceStmtPayload {
    /// Generic keyword statement args: `kw expr[, expr]`.
    KeywordArgs(Vec<Spanned<Expr>>),
}
