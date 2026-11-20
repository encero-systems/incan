//! Plain data for native bodies, independent of rustc's internal types, and the plan records that retain it.
//!
//! The plan types are what lowering produces and the rustc adapter consumes. Keyed function products, the native
//! interface and the layout records store them as Crumb rows of the compiler-owned native-plan profile.

mod debug;

pub mod body;
pub mod carriers;
pub mod nominal;
pub mod plan;
pub mod record_types;
pub mod span_file;
pub mod types;

pub use body::{
    BasicBlock, BinaryOp, Callee, CalleeKind, Constant, FieldLifecycle, FieldMetadata, FieldProjection,
    ForeignFunction, FrozenValue, Local, Operand, OperandKind, Parameter, Place, Projection, Rvalue, RvalueKind,
    Statement, StatementKind, Terminator, TerminatorKind, UnaryOp, Unwind, assign, block, copy, integer, place,
};
pub use carriers::{fold_carrier_positions, fold_inventories};
pub use nominal::{
    CARRIER_BASE, NominalKind, NominalReference, NominalRemap, ProductNominal, UnitNominals, carrier_index,
    carrier_position, describe_nominals, is_carrier_index, resolve_nominals, unit_nominals,
};
pub use plan::{
    AttributeArgument, AttributeValue, Carrier, EnumDeclaration, EnumVariant, ExternalFunction,
    FallibleIteratorMethods, Function, GenericEnumDeclaration, GenericEnumVariant, GenericLayoutField,
    GenericLayoutType, GenericLoan, GenericModelDeclaration, IteratorMethods, ModelDeclaration, NativeAttribute,
    NativeTraitAdoption, NativeTraitMethod, Plan, PlanExport, StaticCell, ZipCarrier, semantic_type_matches,
};
pub use span_file::SpanFile;
pub use types::{
    ListLeaf, PlanType, SizedNumeric, SourceSpan, TupleElement, list_leaf_type, range_field_types, sized_plan_type,
    source_span, tuple_element_type,
};

#[cfg(test)]
mod tests;
