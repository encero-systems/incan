//! The field defaults the plan records publish to Incan callers.
//!
//! An Incan caller may leave a field out of a Rust record's construction when the record publishes that field's
//! default as the associated function `__incan_default_<field>`: public, taking no arguments, returning the field's
//! type. The frontend evaluates it after the written arguments. A record that publishes any default states exactly
//! which fields a caller may leave out; every other field stays required. These are the defaults the Incan plan
//! declarations had, field for field.

use crate::body::{Callee, FieldMetadata, FieldProjection, ForeignFunction};
use crate::nominal::{NominalReference, NominalRemap, ProductNominal, UnitNominals};
use crate::plan::{
    Carrier, EnumDeclaration, ExternalFunction, FallibleIteratorMethods, Function, GenericEnumDeclaration, GenericLoan,
    GenericModelDeclaration, IteratorMethods, ModelDeclaration, NativeAttribute, NativeTraitAdoption, Plan, PlanExport,
    StaticCell, ZipCarrier,
};
use crate::positions::{PositionChange, WirePositions};
use crate::record_types::CheckedType;
use crate::records::lowering_events::{
    AdmittedFunction, AdmittedModule, AdmittedPlacement, EventPosition, EventReference, LoweringEvent,
};
use crate::types::PlanType;

/// Publish each `function -> Type = value` as the associated function an Incan construction calls for the field it
/// leaves out. Each function is named `__incan_default_` followed by its field's name.
macro_rules! published_defaults {
    ($owner:ty { $($function:ident -> $field:ty = $value:expr),+ $(,)? }) => {
        impl $owner {
            $(
                #[doc = concat!("Published Incan field default: `", stringify!($value), "`.")]
                pub fn $function() -> $field {
                    $value
                }
            )+
        }
    };
}

published_defaults!(FieldProjection {
    __incan_default_variant -> i64 = -1,
    __incan_default_dereferences -> i64 = 0,
});

published_defaults!(ForeignFunction {
    __incan_default_clove -> String = String::new(),
    __incan_default_configuration -> String = String::new(),
    __incan_default_framework -> bool = false,
});

published_defaults!(Function {
    __incan_default_is_async -> bool = false,
    __incan_default_future_callee -> Option<Callee> = None,
    __incan_default_attributes -> Vec<NativeAttribute> = Vec::new(),
    __incan_default_signature_lifetimes -> Vec<Vec<String>> = Vec::new(),
    __incan_default_generic_loans -> Vec<GenericLoan> = Vec::new(),
    __incan_default_retained_result_source -> i64 = -1,
});

published_defaults!(Plan {
    __incan_default_statics -> Vec<StaticCell> = Vec::new(),
    __incan_default_exports -> Vec<PlanExport> = Vec::new(),
    __incan_default_tests -> Vec<String> = Vec::new(),
    __incan_default_zip_carriers -> Vec<ZipCarrier> = Vec::new(),
    __incan_default_programs -> Vec<String> = Vec::new(),
    __incan_default_generic_models -> Vec<GenericModelDeclaration> = Vec::new(),
    __incan_default_generic_enums -> Vec<GenericEnumDeclaration> = Vec::new(),
});

published_defaults!(StaticCell {
    __incan_default_module_completion -> bool = false,
});

published_defaults!(ModelDeclaration {
    __incan_default_view_lifetime_count -> i64 = 0,
    __incan_default_source_name -> String = String::new(),
    __incan_default_library_item -> String = String::new(),
    __incan_default_library_type_arguments -> Vec<PlanType> = Vec::new(),
    __incan_default_field_serde_names -> Vec<String> = Vec::new(),
    __incan_default_checked_deserialize -> String = String::new(),
    __incan_default_reflection_metadata -> Option<Vec<FieldMetadata>> = None,
    __incan_default_field_metadata_carrier -> bool = false,
    __incan_default_comparison_methods -> Vec<String> = Vec::new(),
    __incan_default_fallible_iterator -> Option<FallibleIteratorMethods> = None,
    __incan_default_iterator -> Option<IteratorMethods> = None,
    __incan_default_callable_method -> String = String::new(),
    __incan_default_callable_signature -> Vec<PlanType> = Vec::new(),
    __incan_default_string_conversion_root -> String = String::new(),
    __incan_default_string_conversion_trait -> String = String::new(),
    __incan_default_native_trait_adoptions -> Vec<NativeTraitAdoption> = Vec::new(),
    __incan_default_unavailable_native_fields -> Vec<i64> = Vec::new(),
});

published_defaults!(ExternalFunction {
    __incan_default_type_arguments -> Vec<PlanType> = Vec::new(),
});

published_defaults!(EnumDeclaration {
    __incan_default_view_lifetime_count -> i64 = 0,
    __incan_default_carrier -> Carrier = Carrier::Declared,
    __incan_default_semantic_key -> Option<CheckedType> = None,
    __incan_default_source_type -> String = String::new(),
    __incan_default_member_debug -> bool = false,
    __incan_default_library_item -> String = String::new(),
    __incan_default_library_type_arguments -> Vec<PlanType> = Vec::new(),
    __incan_default_raw_kind -> String = String::new(),
    __incan_default_raw_values -> Vec<String> = Vec::new(),
    __incan_default_comparison_methods -> Vec<String> = Vec::new(),
    __incan_default_closure_items -> Vec<String> = Vec::new(),
});

published_defaults!(NominalReference {
    __incan_default_name -> String = String::new(),
});

published_defaults!(NominalRemap {
    __incan_default_preserve -> bool = false,
    __incan_default_fold -> i64 = -1,
    __incan_default_crates -> Vec<String> = Vec::new(),
});

published_defaults!(UnitNominals {
    __incan_default_carriers -> Vec<ProductNominal> = Vec::new(),
    __incan_default_carrier_names -> Vec<String> = Vec::new(),
    __incan_default_statics -> Vec<ProductNominal> = Vec::new(),
    __incan_default_static_positions -> std::collections::HashMap<String, i64> = std::collections::HashMap::new(),
});

published_defaults!(WirePositions {
    __incan_default_change -> PositionChange = PositionChange::Preserve,
});

published_defaults!(LoweringEvent {
    __incan_default_admitted -> bool = false,
    __incan_default_module -> String = String::new(),
    __incan_default_role -> String = String::new(),
    __incan_default_demand -> String = String::new(),
    __incan_default_runtime_requirements -> i64 = 0,
    __incan_default_references -> Vec<EventReference> = Vec::new(),
    __incan_default_positions -> Vec<EventPosition> = Vec::new(),
});

published_defaults!(AdmittedFunction {
    __incan_default_placement -> Option<AdmittedPlacement> = None,
});

published_defaults!(AdmittedModule {
    __incan_default_inspected -> bool = false,
});
