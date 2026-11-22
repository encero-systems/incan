//! Small plan builders shared by the ported tests.

use crate::body::{
    BasicBlock, Constant, Local, Operand, OperandKind, Place, Projection, Rvalue, RvalueKind, Statement, StatementKind,
    Terminator, TerminatorKind,
};
use crate::plan::Function;
use crate::types::{PlanType, SourceSpan};

/// One ordinary block.
pub(super) fn block(statements: Vec<Statement>, exit: Terminator, span: &SourceSpan) -> BasicBlock {
    BasicBlock {
        statements,
        terminator: exit,
        cleanup: false,
        span: span.clone(),
    }
}

/// A return terminator.
pub(super) fn returning(span: &SourceSpan) -> Terminator {
    Terminator {
        kind: TerminatorKind::Return,
        span: span.clone(),
    }
}

/// A projected place of one local.
pub(super) fn place_at(local: i64, projection: Projection, span: &SourceSpan) -> Place {
    Place {
        local,
        projection,
        span: span.clone(),
    }
}

/// Copy one projected place.
pub(super) fn copy_at(local: i64, projection: Projection, span: &SourceSpan) -> Operand {
    Operand {
        kind: OperandKind::Copy(place_at(local, projection, span)),
        span: span.clone(),
    }
}

/// One integer literal operand.
pub(super) fn literal(value: i64, span: &SourceSpan) -> Operand {
    Operand {
        kind: OperandKind::Literal(Constant::Int(value)),
        span: span.clone(),
    }
}

/// Assign one operation to a projected local place.
pub(super) fn assign_at(local: i64, projection: Projection, value: RvalueKind, span: &SourceSpan) -> Statement {
    Statement {
        kind: StatementKind::Assign(
            place_at(local, projection, span),
            Rvalue {
                kind: value,
                span: span.clone(),
            },
        ),
        span: span.clone(),
    }
}

/// A local of one type.
pub(super) fn local(ty: PlanType, span: &SourceSpan) -> Local {
    Local { ty, span: span.clone() }
}

/// One ordinary scalar declaration with an empty returning body.
pub(super) fn empty_function(name: &str, return_type: PlanType, span: &SourceSpan) -> Function {
    Function::new(
        name.to_owned(),
        Vec::new(),
        return_type.clone(),
        vec![local(return_type, span)],
        vec![block(Vec::new(), returning(span), span)],
        span.clone(),
    )
}
