//! Plain data for native bodies, independent of rustc's internal types, and the plan records that retain it.
//!
//! The plan types are what lowering produces and the rustc adapter consumes. Keyed function products, the native
//! interface and the layout records store them as Crumb rows of the compiler-owned native-plan profile.

// Plan and stored-shape enums keep the reference plan's unboxed alternatives, which lowering and the rustc adapter
// construct and match directly.
#![allow(clippy::large_enum_variant)]

mod debug;

pub mod body;
pub mod carriers;
pub mod nominal;
pub mod plan;
pub mod positions;
mod published_defaults;
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
    carrier_position, describe_nominals, is_carrier_index, resolve_nominals, unit_nominals, unresolved_nominal,
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
    restore_record_plan,
};
pub use records::function::{
    NamedProduct, PlacedProduct, PlacedRecord, ProductParts, decode_frames, decode_product, decode_product_parts,
    decode_runtime, decode_scalar, encode_product, encode_runtime, encode_scalar, matches_product, native_crate,
    place_product_record, positioned_product_requirements, product_frame_names, product_identities, product_nominals,
    restore_named_product, transform_product_positions, valid_ordinal_product,
};
pub use records::layout::{
    decode_enums, decode_instance_request, decode_models, decode_zip_carriers, encode_checked_layout, encode_enums,
    encode_instance_request, encode_models, encode_zip_carriers,
};
pub use records::lowering_events::{
    ADMITTED_DEFINITIONS, AdmittedDefinition, AdmittedFunction, AdmittedModule, AdmittedPlacement, EventPosition,
    EventReference, INTERFACE_REPORT, LoweringEvent, LoweringRefusal, NATIVE_DEFINITIONS, NativeInterface,
    admitted_definitions_filename, decode_admitted_definitions, decode_lowering_event, decode_lowering_refusal,
    decode_module_note, decode_native_interface, encode_admitted_definitions, encode_lowering_event,
    encode_lowering_refusal, encode_module_note, encode_native_interface, native_interface_filename, placed_admission,
    read_module_admission, write_module_placement,
};
pub use records::plan_function::{
    LoweringDemands, PlanFunctionRecord, decode_plan_function_record, encode_entry_record, encode_lowering_demand,
    encode_plan_function_record, plan_function_digest, verify_entry_record,
};
pub use records::plan_module::{
    ADMITTED_MODULE_NOTE, PARTIAL_MODULE_NOTE, PLAN_MODULE_FAMILY, PLAN_MODULE_PAGE, POSITIONED_MODULE_PAGE,
    PlanModuleFunction, PlanModuleValue, REPOSITION_MODULE_PAGE, decode_plan_module, encode_plan_module,
};
pub use span_file::SpanFile;
pub use types::{
    ListLeaf, PlanType, SizedNumeric, SourceSpan, TupleElement, list_leaf_type, range_field_types, sized_plan_type,
    source_span, tuple_element_type,
};

#[cfg(test)]
mod tests;
