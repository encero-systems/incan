//! Source-named types crossing the checked Incan caller boundary without conversion.

pub use incan_lowering::owned_capture;
pub use incan_mir_plan_native::{
    AttributeArgument, AttributeValue, BinaryOp, Callee, CalleeKind, Carrier, Constant, EnumDeclaration,
    ExternalFunction, FallibleIteratorMethods, FieldLifecycle, FieldMetadata, ForeignFunction, FrozenValue, Function,
    GenericEnumDeclaration, GenericEnumVariant, GenericLayoutType, GenericLoan, GenericModelDeclaration,
    IteratorMethods, ListLeaf, ModelDeclaration, NativeAttribute, NativeTraitMethod, Operand, OperandKind, Parameter,
    Place, Plan, PlanType, Projection, RvalueKind, SizedNumeric, SourceSpan, Statement, StatementKind, StaticCell,
    Terminator, TerminatorKind, TupleElement, UnaryOp, Unwind, list_leaf_type, range_field_types, tuple_element_type,
};
