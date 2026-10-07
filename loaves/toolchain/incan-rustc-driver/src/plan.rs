//! Source-named types crossing the checked Incan caller boundary without conversion.

pub use incan_mir_lowering::caller::incan::{
    BasicBlock, BinaryOp, Callee, CalleeKind, Constant, EnumDeclaration, EnumVariant, ExternalFunction, Function,
    ListLeaf, Local, ModelDeclaration, Operand, OperandKind, Parameter, Place, Plan, PlanType, Projection, Rvalue,
    RvalueKind, SizedNumeric, SourceSpan, Statement, StatementKind, Terminator, TerminatorKind, UnaryOp, Unwind,
};
