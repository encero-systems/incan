//! IR statement definitions

use super::expr::Pattern;
use super::{IrExpr, IrSpan, IrType, Mutability};

/// An IR statement
#[derive(Debug, Clone)]
pub struct IrStmt {
    pub kind: IrStmtKind,
    pub span: IrSpan,
}

impl IrStmt {
    pub fn new(kind: IrStmtKind) -> Self {
        Self {
            kind,
            span: IrSpan::default(),
        }
    }

    pub fn with_span(mut self, span: IrSpan) -> Self {
        self.span = span;
        self
    }
}

/// Statement kinds
#[derive(Debug, Clone)]
pub enum IrStmtKind {
    /// Expression statement (expr;)
    Expr(IrExpr),

    /// Let binding (let x = expr;)
    Let {
        name: String,
        ty: IrType,
        mutability: Mutability,
        value: IrExpr,
    },

    /// Assignment (x = expr;)
    Assign { target: AssignTarget, value: IrExpr },

    /// Compound assignment (x += expr;)
    CompoundAssign {
        target: AssignTarget,
        op: super::expr::BinOp,
        value: IrExpr,
        lhs_ty: IrType,
    },

    /// Return statement
    Return(Option<IrExpr>),

    /// Break statement (with optional label)
    Break(Option<String>),

    /// Continue statement (with optional label)
    Continue(Option<String>),

    /// While loop
    While {
        label: Option<String>,
        condition: IrExpr,
        body: Vec<IrStmt>,
    },

    /// For loop
    For {
        label: Option<String>,
        pattern: Pattern,
        iterable: IrExpr,
        body: Vec<IrStmt>,
    },

    /// Loop (infinite loop)
    Loop { label: Option<String>, body: Vec<IrStmt> },

    /// If statement (no value)
    If {
        condition: IrExpr,
        then_branch: Vec<IrStmt>,
        else_branch: Option<Vec<IrStmt>>,
    },

    /// Match statement (no value)
    Match {
        scrutinee: IrExpr,
        arms: Vec<super::expr::MatchArm>,
    },

    /// Block of statements
    Block(Vec<IrStmt>),
}

/// Target of an assignment
#[derive(Debug, Clone)]
pub enum AssignTarget {
    /// Simple variable
    Var(String),
    /// Field access (obj.field)
    Field { object: Box<IrExpr>, field: String },
    /// Index access (list[i])
    Index { object: Box<IrExpr>, index: Box<IrExpr> },
}
