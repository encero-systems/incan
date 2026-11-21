//! Plain data for native bodies, independent of rustc's internal types, and the plan records that retain it.
//!
//! The plan types are what lowering produces and the rustc adapter consumes. Keyed function products, the native
//! interface and the layout records store them as Crumb rows of the compiler-owned native-plan profile.

mod debug;

pub mod body;
pub mod carriers;
pub mod nominal;
pub mod plan;
pub mod positions;
pub mod record_types;
pub mod records;
pub mod span_file;
pub mod types;
mod wire;

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
pub use positions::{PositionChange, WirePositions};
pub use records::codec::{
    decode_record_plan, encode_entry_binding, encode_record_plan, encode_record_plan_products, record_plan_products,
};
pub use records::function::{
    NamedProduct, ProductParts, decode_frames, decode_product, decode_product_parts, decode_runtime, decode_scalar,
    encode_product, encode_runtime, encode_scalar, matches_product, positioned_product_requirements,
    product_frame_names, product_identities, product_nominals, restore_named_product, transform_product_positions,
    valid_ordinal_product,
};
pub use records::layout::{
    decode_enums, decode_instance_request, decode_models, decode_zip_carriers, encode_checked_layout, encode_enums,
    encode_instance_request, encode_models, encode_zip_carriers,
};
pub use records::plan_function::{
    LoweringDemands, PlanFunctionRecord, decode_plan_function_record, encode_entry_record, encode_lowering_demand,
    encode_plan_function_record, plan_function_digest, verify_entry_record,
};
pub use span_file::SpanFile;
pub use types::{
    ListLeaf, PlanType, SizedNumeric, SourceSpan, TupleElement, list_leaf_type, range_field_types, sized_plan_type,
    source_span, tuple_element_type,
};

#[cfg(test)]
mod tests;
