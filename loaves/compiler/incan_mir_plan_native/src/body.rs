//! Native body nodes: locals, places, operands, rvalues, statements, calls, terminators and blocks, with builders.
//!
//! Local zero is the return place; parameters occupy the next locals in declaration order. Blocks are indexed from
//! zero. Every source-bearing node keeps its Incan location, including operands and places, so native diagnostics
//! retain provenance.

use serde::{Deserialize, Serialize};

use crate::types::{ListLeaf, PlanType, SourceSpan};

/// A named function parameter; its local index follows declaration order.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    pub ty: PlanType,
    pub span: SourceSpan,
}

/// A typed body local; parameters and the return place are included.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Local {
    pub ty: PlanType,
    pub span: SourceSpan,
}

/// One typed projection step. A negative slot is a dereference-only step; ordinary slots select a field.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldProjection {
    pub slot: i64,
    pub ty: PlanType,
    /// A nonnegative variant selects the checked enum payload before this field; -1 keeps an ordinary field.
    pub variant: i64,
    pub dereferences: i64,
}

impl FieldProjection {
    /// An ordinary field step: no enum payload selection and no extra dereference.
    pub fn new(slot: i64, ty: PlanType) -> Self {
        Self {
            slot,
            ty,
            variant: -1,
            dereferences: 0,
        }
    }
}

/// Whole local, checked component, typed reference dereference, canonical field path, or downcast enum payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Projection {
    Whole,
    NumericValue(PlanType),
    Value,
    Overflow,
    VariantField(i64, i64, PlanType),
    Field(i64, PlanType),
    Deref(PlanType),
    DerefField(i64, PlanType),
    Fields(Vec<FieldProjection>),
    DerefFields(Vec<FieldProjection>),
}

/// An indexed local with a checked arithmetic, nominal field, or typed reference projection.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub local: i64,
    pub projection: Projection,
    pub span: SourceSpan,
}

/// A scalar, zero-sized type token, or borrowed static text literal. `Numeric` retains exact integer digits or IEEE
/// bit digits with its carrier, never a host-width approximation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Constant {
    Numeric(String, PlanType),
    RustItem(String, PlanType),
    Int(i64),
    Float(f64),
    Bool(bool),
    Unit,
    TypeToken(Vec<PlanType>),
    Text(String),
}

/// Explicit copy, move, or constant; ownership intent is never inferred.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum OperandKind {
    Copy(Place),
    Move(Place),
    Literal(Constant),
}

/// A source-located value read by a statement or terminator.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Operand {
    pub kind: OperandKind,
    pub span: SourceSpan,
}

/// Checked integer arithmetic, scalar comparisons, floating arithmetic, and integer bit operations.
///
/// `Add`, `Subtract`, and `Multiply` on int produce `CheckedInt`. Comparisons produce `Bool`. The other arithmetic
/// operations require float operands. `BitAnd`, `BitOr`, and `BitXor` take two operands of one integer type and keep
/// it. A shift keeps its left operand's integer type, counts with any integer type, and masks the count to that width
/// as Rust's unchecked release shift does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    BitAnd,
    BitOr,
    BitXor,
    ShiftLeft,
    ShiftRight,
}

/// Boolean inversion, numeric negation, or the bitwise complement of an integer that keeps its type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum UnaryOp {
    Not,
    Negate,
    Invert,
}

/// An explicit native expression assigned to a checked place.
///
/// Enum construction names the owner and variant indices. `Discriminant` reads without moving the enum;
/// `VariantField` projections are valid only on the selected variant branch. `NumericCast` retains both checked source
/// and destination carriers. `FloatToInt` preserves Rust truncation and saturation; `BoolToInt` produces zero or one.
/// `Tuple` preserves the ordered operand layout of its destination type. `Range` preserves the four scalar operands in
/// Body IR's declared range field order. `FrozenText` wraps a static text literal in the dependency-owned `FrozenStr`
/// carrier. `Array` moves or copies its listed operands into a fixed array. `Borrow` creates a shared reference to a
/// checked place; `MutBorrow` creates a mutable list or nominal receiver reference. `UnsizeSlice` converts an array
/// reference to the corresponding shared slice without changing ownership of its owner.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RvalueKind {
    Use(Operand),
    /// Preserve one native value across two retained plan aliases; rustc must prove their native types identical.
    NativeAlias(Operand, PlanType, PlanType),
    /// Reify a validated zero-argument unit function for the canonical async entry bridge.
    UnitFunction(String),
    FrozenText(Operand),
    FrozenBytes(Vec<i64>),
    FrozenCollection(PlanType, Vec<FrozenValue>),
    Binary(BinaryOp, Operand, Operand),
    Unary(UnaryOp, Operand),
    NumericCast(Operand, PlanType, PlanType),
    IntToFloat(Operand),
    FloatToInt(Operand),
    BoolToInt(Operand),
    Array(Vec<Operand>),
    Enum(i64, i64, Vec<Operand>),
    Discriminant(Place),
    TagToInt(Operand),
    Model(i64, Vec<Operand>),
    Tuple(Vec<Operand>),
    Range(Vec<Operand>),
    Borrow(Place),
    MutBorrow(Place),
    UnsizeSlice(Operand),
    /// A reference to a concrete value, as a reference to a Rust trait object of the same mutability: source type,
    /// then target type. rustc proves the value implements the object's every bound before the conversion is built.
    UnsizeTraitObject(Operand, PlanType, PlanType),
    FunctionItem(Callee),
    ReifyFunction(Operand),
    /// Expose a checked function pointer's address without changing its argument or return signature.
    FunctionAddress(Operand),
    /// Borrow an owned closure's callable object as the shared view a call or lending envelope takes.
    ClosureObject(Place),
    /// Test the selected variant using Rust's actual discriminant type and value, including explicit repr values.
    IsVariant(Place, i64),
    /// Addresses and offsets are emitted only from checked bridge facts, never from source pointer arithmetic.
    RawAddressOf(Place, bool),
    PointerCast(Operand, PlanType),
    PointerOffset(Operand, Operand),
    /// Observe an address for a null guard without permitting integer-to-pointer construction.
    PointerAddress(Operand),
}

/// Effect-free static data: scalars, raw bytes, and nested frozen collections in source order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum FrozenValue {
    Scalar(Constant),
    Bytes(Vec<i64>),
    Collection(PlanType, Vec<FrozenValue>),
}

/// A source-located scalar expression.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Rvalue {
    pub kind: RvalueKind,
    pub span: SourceSpan,
}

/// Assignment and explicit local storage lifetime boundaries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum StatementKind {
    Assign(Place, Rvalue),
    StorageLive(i64),
    StorageDead(i64),
}

/// One source-located operation before a block's terminator.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: SourceSpan,
}

/// A planned function, canonical external path, checked model clone, explicit generic instantiation, exact legacy
/// generator runtime operation, or a local item/pointer operand with its retained type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CalleeKind {
    Planned(String),
    External(String),
    CloneEnum(i64, String),
    CloneModel(i64, String),
    /// Every type argument in the external's checked parameter order, including callable arguments.
    Instantiated(String, Vec<PlanType>),
    /// A checked inherent SDK method future keeps owner arguments on its type path, not on its method segment.
    FutureMethod(String, Vec<PlanType>),
    /// The explicit producer takes its yield handle before the source-ordered owned captures. A zero-capture spawn uses
    /// a function item; a captured spawn is a constructor whose call arguments initialize one native FnOnce
    /// environment.
    SpawnGenerator(String, ListLeaf, i64),
    YieldGenerator(ListLeaf, i64),
    CollectGenerator(ListLeaf, i64),
    Value(Operand),
    /// A synthesized constructor's boxed environment. Its closure forwards the leading captured parameters, then its
    /// own inputs, to the named lifted function; the count says how many of that function's parameters are
    /// captures.
    ClosureBody(String, i64),
    /// A validated bridge transfers a boxed callable into the SDK's cloneable immutable environment carrier.
    SharedCallableBody(Vec<PlanType>),
    /// The exact declaration owner and Incan-owned carrier of a checked generated metadata hook.
    FieldMetadata(i64, i64),
    /// A constructor and lifted body for a consuming environment, forwarded by value exactly once.
    ClosureBodyOnce(String, i64),
    /// Invoke a borrowed callable object of this signature with one packed argument tuple.
    CallClosure(Vec<PlanType>),
    /// Invoke the owned callable object by moving it with one packed argument tuple.
    CallClosureOnce(Vec<PlanType>),
    /// Invoke the shared view of a callable object that holds borrowed captures, with one packed argument tuple.
    CallBorrowingClosure(Vec<PlanType>),
    /// Read a clone of computed static storage slot `n`, running its initializer first if nothing has yet.
    StaticLoad(i64),
    /// Replace the value of computed static storage slot `n`.
    StaticStore(i64),
    /// Run computed static slot `n`'s initializer unless it has already run, keeping the value in its storage.
    StaticForce(i64),
    /// Observe or publish completion of this module's declaration-order initialization, keyed by its first cell.
    ModuleInitialized(i64),
    ModuleComplete(i64),
    /// Call the selected planned method while holding computed slot n's actual shared or exclusive storage guard.
    StaticMethod(i64, String),
    /// Clone the captured concrete environment behind an authenticated shared callable object.
    CloneClosure(Vec<PlanType>),
    /// Format the exact native carrier through rustc's Debug (true) or Display (false) implementation.
    FormatValue(PlanType, bool),
    /// Box a checked noncapturing native callback and install it through Rust's standard panic API.
    InstallPanicHook(Vec<PlanType>),
    /// A target-verified foreign declaration; the physical symbol and link capability are never reconstructed.
    Foreign(ForeignFunction),
}

/// One target-verified C function with its exact symbol, explicit library, and native signature.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ForeignFunction {
    pub name: String,
    pub symbol: String,
    pub library: String,
    pub parameters: Vec<PlanType>,
    pub return_type: PlanType,
    /// The Clove whose static archive provides `library` and the configuration it links; empty for a system library.
    pub clove: String,
    pub configuration: String,
    /// Whether `library` is an Apple framework (`c.framework`) rather than a library.
    pub framework: bool,
}

/// A function reference, resolved by source name or canonical module path.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Callee {
    pub kind: CalleeKind,
    pub span: SourceSpan,
}

/// Propagate unwinding, enter cleanup, or abort a second panic during cleanup.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Unwind {
    Continue,
    Cleanup(i64),
    Terminate,
}

/// Control flow, calls, drops, overflow checks, and unwinding exits.
///
/// `SwitchBool` records the false and true targets explicitly. `AssertOverflow` checks a checked arithmetic overflow
/// flag against false and retains both inputs for the native panic message. `Call` and `Drop` carry explicit unwinding.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TerminatorKind {
    Goto(i64),
    SwitchBool(Operand, i64, i64),
    Call(Callee, Vec<Operand>, Place, i64, Unwind),
    Drop(Place, i64, Unwind),
    AssertOverflow(Operand, BinaryOp, Operand, Operand, i64, Unwind),
    /// Polling and completed-future destruction have distinct owner cleanup states; cancellation drops suspended
    /// owners.
    Await(Place, Place, i64, i64, Unwind, Unwind),
    CoroutineDrop,
    Return,
    Unreachable,
    UnwindResume,
}

/// The source-located exit of a basic block.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Terminator {
    pub kind: TerminatorKind,
    pub span: SourceSpan,
}

/// An indexed block; cleanup blocks may only run on the unwind path.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct BasicBlock {
    pub statements: Vec<Statement>,
    pub terminator: Terminator,
    pub cleanup: bool,
    pub span: SourceSpan,
}

/// Inert checked rename labels, copied into the public Incan lifecycle carrier at emission.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldLifecycle {
    pub since: Option<String>,
    pub retire_in: Option<String>,
    pub message: Option<String>,
    pub by: Option<String>,
    pub issue: Option<String>,
}

/// Immutable source field presentation values selected by the checked reflection visibility policy.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldMetadata {
    pub name: String,
    pub record_identity: Vec<i64>,
    pub record_canonical: bool,
    pub renamed_from: Vec<String>,
    pub renamed_from_lifecycle: Option<FieldLifecycle>,
    pub position: i64,
    pub default_is_empty: bool,
    pub stored_type: Option<String>,
    pub alias: Option<String>,
    pub description: Option<String>,
    pub type_name: String,
    pub has_default: bool,
}

/// Name a whole local without exposing projection bookkeeping to builders.
pub fn place(local: i64, span: &SourceSpan) -> Place {
    Place {
        local,
        projection: Projection::Whole,
        span: span.clone(),
    }
}

/// Build an int literal, represented natively as a signed 64-bit constant.
pub fn integer(value: i64, span: &SourceSpan) -> Operand {
    Operand {
        kind: OperandKind::Literal(Constant::Int(value)),
        span: span.clone(),
    }
}

/// Read a scalar local while leaving it available to later operations.
pub fn copy(local: i64, span: &SourceSpan) -> Operand {
    Operand {
        kind: OperandKind::Copy(place(local, span)),
        span: span.clone(),
    }
}

/// Assign one scalar expression and retain the statement's source location.
pub fn assign(destination: Place, value: RvalueKind, span: &SourceSpan) -> Statement {
    Statement {
        kind: StatementKind::Assign(
            destination,
            Rvalue {
                kind: value,
                span: span.clone(),
            },
        ),
        span: span.clone(),
    }
}

/// Build a normal block; cleanup blocks are constructed explicitly.
pub fn block(statements: Vec<Statement>, exit: TerminatorKind, span: &SourceSpan) -> BasicBlock {
    BasicBlock {
        statements,
        terminator: Terminator {
            kind: exit,
            span: span.clone(),
        },
        cleanup: false,
        span: span.clone(),
    }
}

crate::structural_debug!(Parameter, "Parameter", name, ty, span);
crate::structural_debug!(Local, "Local", ty, span);
crate::structural_debug!(FieldProjection, "FieldProjection", slot, ty, variant, dereferences);
crate::structural_debug!(Place, "Place", local, projection, span);
crate::structural_debug!(Operand, "Operand", kind, span);
crate::structural_debug!(Rvalue, "Rvalue", kind, span);
crate::structural_debug!(Statement, "Statement", kind, span);
crate::structural_debug!(
    ForeignFunction,
    "ForeignFunction",
    name,
    symbol,
    library,
    parameters,
    return_type,
    clove,
    configuration,
    framework,
);
crate::structural_debug!(Callee, "Callee", kind, span);
crate::structural_debug!(Terminator, "Terminator", kind, span);
crate::structural_debug!(BasicBlock, "BasicBlock", statements, terminator, cleanup, span);
crate::structural_debug!(FieldLifecycle, "FieldLifecycle", since, retire_in, message, by, issue);
crate::structural_debug!(
    FieldMetadata,
    "FieldMetadata",
    name,
    record_identity,
    record_canonical,
    renamed_from,
    renamed_from_lifecycle,
    position,
    default_is_empty,
    stored_type,
    alias,
    description,
    type_name,
    has_default,
);
