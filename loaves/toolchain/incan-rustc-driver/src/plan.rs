//! Source-named types crossing the checked Incan caller boundary without conversion.

pub use incan_mir_lowering::caller::incan::{
    BasicBlock, BinaryOp, Callee, CalleeKind, Constant, ExternalFunction, Function, ListLeaf, Local, ModelDeclaration,
    Operand, OperandKind, Parameter, Place, Plan, PlanType, Projection, Rvalue, RvalueKind, SourceSpan, Statement,
    StatementKind, Terminator, TerminatorKind, UnaryOp, Unwind,
};
