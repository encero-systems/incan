//! Plan records: keyed function products, layouts, checked carrier types, instance requests and the native
//! interface, each one Crumb row of the compiler-owned native-plan profile.

pub mod body;
pub mod codec;
pub mod function;
pub mod layout;
pub mod plan_function;
mod schema_body_records;
mod schema_function_records;
mod schema_layout_records;
mod schema_layout_type_records;
mod schema_nominal_identities;
mod wire_body_records;
mod wire_function_records;
mod wire_layout_records;
mod wire_layout_type_records;
mod wire_nominal_identities;
pub(crate) mod wire_schema;

/// Everything the profile's shape projections name.
mod prelude {
    pub(crate) use super::layout::{read_source_span, wire_source_span};
    pub(crate) use super::wire_body_records::*;
    pub(crate) use super::wire_layout_type_records::*;
    pub(crate) use super::wire_nominal_identities::*;
    pub(crate) use crate::body::{FieldLifecycle, FieldMetadata, Parameter};
    pub(crate) use crate::nominal::{NominalKind, ProductNominal};
    pub(crate) use crate::plan::{
        Carrier, EnumDeclaration, EnumVariant, FallibleIteratorMethods, IteratorMethods, ModelDeclaration,
        NativeTraitAdoption, NativeTraitMethod, ZipCarrier,
    };
    pub(crate) use crate::positions::WirePositions;
    pub(crate) use crate::record_types::{
        CheckedCallableParam, CheckedCallableParamKind, CheckedInputAccess, CheckedNumericType, CheckedPrimitiveType,
        CheckedRuntimeBuiltin, CheckedType,
    };
    pub(crate) use crate::types::{ListLeaf, PlanType, SizedNumeric, TupleElement};
    pub(crate) use crate::wire::{
        Wire, WireField, alternative, boolean_value, list, number, optional, read_float, read_list, read_optional,
        required, text, unit, unsigned_value, wire_fields, wire_variant,
    };
}
