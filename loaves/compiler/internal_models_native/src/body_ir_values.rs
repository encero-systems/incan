//! Body IR statement, rvalue and callee records retain their kernel-owned leaf types.

use crate::checked_types::CheckedType;
use crate::compiler_records::HirSourceSpan;
use incan_semantics_core::body_ir::{
    AggregateKind, ArgumentElement, AssertionKind, BinOp, CallableParam, CallableTarget, CheckedBridgeCall,
    CheckedForeignCall, ClosureBody, DictEntry, FieldlessEnumVariantTarget, FormatPart, GeneratorBody, HelperOp,
    IsInstanceTarget, IterProtocol, LocalId, MatchArm, MethodTarget, Operand, Place, ProviderOperationPlan, RaceArm,
    ResultVariant, TryErrorRouting, TypeMetadataTarget, UnOp, UnsafeAcknowledgment, ValueEnumVariantTarget,
};

/// Every current native Callee alternative, without reconstructing a selected target from source spelling.
#[derive(Clone, Debug)]
pub enum CompilerCallee {
    Function(CallableTarget),
    Method(MethodTarget),
    Helper(HelperOp),
    ProviderOperation(ProviderOperationPlan),
    TypeMetadata(TypeMetadataTarget),
    CheckedForeign(CheckedForeignCall),
    CheckedBridge(CheckedBridgeCall),
}

/// Every current native Rvalue alternative, retaining checked types and ordered evaluation payloads.
#[derive(Clone, Debug)]
pub enum CompilerRvalue {
    Use(Operand),
    UnaryOp(UnOp, Operand),
    BinaryOp(BinOp, Operand, Operand),
    IsInstance(Operand, CheckedType, IsInstanceTarget),
    Aggregate(AggregateKind, Vec<ArgumentElement>),
    Dict(Vec<DictEntry>),
    ValueEnumVariant(ValueEnumVariantTarget),
    FieldlessEnumVariant(FieldlessEnumVariantTarget),
    ResultVariant(ResultVariant),
    Format(Vec<FormatPart>),
    Closure(Vec<CallableParam>, Vec<Operand>, ClosureBody),
    Generator(Operand, Vec<Operand>, GeneratorBody),
    Match(Operand, Vec<MatchArm>),
}

/// Every current native StatementKind alternative, including normalized control-flow and suspension facts.
#[derive(Clone, Debug)]
pub enum CompilerStatementKind {
    UnsafeRegion(UnsafeAcknowledgment, CompilerBlock),
    Assign(Place, CompilerRvalue),
    Call(Option<Place>, CompilerCallee, Vec<ArgumentElement>, bool),
    Drop(LocalId),
    If(Operand, CompilerBlock, Vec<CompilerBlock>),
    Loop(CompilerBlock),
    Break(Option<Operand>),
    Continue,
    Return(Option<Operand>),
    Await(Option<Place>, Operand),
    Race(Option<Place>, Vec<RaceArm>),
    Yield(Operand),
    Assert(AssertionKind, Option<Operand>, bool),
    Expr(Operand),
    TryPropagate(Place, Operand, TryErrorRouting),
    IterNext(Place, Operand, IterProtocol),
    Unsupported(String),
}

/// One normalized statement and its owned source span.
#[derive(Clone)]
pub struct CompilerStatement {
    pub kind: CompilerStatementKind,
    pub span: HirSourceSpan,
}
crate::incan_debug::incan_debug!(CompilerStatement { kind, span });

/// A scope-bound ordered statement block; nested control-flow stays in the same authored family.
#[derive(Clone)]
pub struct CompilerBlock {
    pub scope: i64,
    pub statements: Vec<CompilerStatement>,
}
crate::incan_debug::incan_debug!(CompilerBlock { scope, statements });
