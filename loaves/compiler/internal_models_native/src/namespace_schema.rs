//! Crumb descriptions of the compiler-owned editable namespace declaration closure.
//!
//! Translated mechanically from the reference module's generated descriptions; field order, identities and
//! storage spellings are unchanged.

use crate::crumb_rows::{ModelField, model_description};
use crate::plan_wire_rows::{enum_schema, variant_schema};
use internal_models_bindings::Description;

pub const NAMESPACE_SCHEMA: &[u8] = b"\xdf\x0a\xd6\xe4\x38\x80";
pub const NAMESPACE_MODULE_NAME: &[u8] = b"\x2c\x96\x84\xb9\x61\xb6";
pub const NAMESPACE_MODULE_PATH: &[u8] = b"\x81\x51\x94\x99\x5b\xb8";

include!("namespace_schema/shapes_access_mode_to_primitive_type.rs");
include!("namespace_schema/shapes_incan_type_to_type_ref.rs");

pub fn namespace_descriptions() -> Vec<Description> {
    vec![
        shape_access_mode(),
        shape_api_alias(),
        shape_api_callable_metadata(),
        shape_api_class(),
        shape_api_const(),
        shape_api_declaration(),
        shape_api_docstring(),
        shape_api_docstring_entry(),
        shape_api_docstring_return(),
        shape_api_enum(),
        shape_api_enum_variant(),
        shape_api_enum_variant_alias(),
        shape_api_function(),
        shape_api_method(),
        shape_api_model(),
        shape_api_newtype(),
        shape_api_partial(),
        shape_api_projected_function(),
        shape_api_property(),
        shape_api_static(),
        shape_api_trait(),
        shape_api_type_alias(),
        shape_canonical_identity_export(),
        shape_canonical_identity_namespace_export(),
        shape_canonical_identity_origin_export(),
        shape_canonical_identity_span_export(),
        shape_canonical_symbol_id(),
        shape_checked_api_metadata(),
        shape_checked_namespace_interface(),
        shape_decorator_arg_metadata(),
        shape_decorator_call_arg_metadata(),
        shape_decorator_dict_entry(),
        shape_decorator_metadata(),
        shape_decorator_value(),
        shape_default_value(),
        shape_enum_value_export(),
        shape_enum_value_type_export(),
        shape_field_export(),
        shape_hir_source_span(),
        shape_implementation_associated_type_export(),
        shape_implementation_trait_bound_export(),
        shape_implementation_trait_bound_origin_export(),
        shape_implementation_type_param_export(),
        shape_incan_callable_param(),
        shape_incan_callable_param_kind(),
        shape_incan_input_access(),
        shape_incan_primitive_type(),
        shape_incan_type(),
        shape_lifecycle_metadata(),
        shape_loaf_identity(),
        shape_made_private_export(),
        shape_member_visibility_export(),
        shape_namespace_compiled_type(),
        shape_namespace_member_identities(),
        shape_native_union_export(),
        shape_native_union_owner_export(),
        shape_native_union_projection(),
        shape_newtype_constraint_export(),
        shape_newtype_constraint_key_export(),
        shape_nominal_type_origin_export(),
        shape_numeric_type_id(),
        shape_ownership_contract(),
        shape_ownership_export(),
        shape_ownership_mode(),
        shape_param_default_call_arg_export(),
        shape_param_default_call_signature_export(),
        shape_param_default_dict_entry_export(),
        shape_param_default_export(),
        shape_param_export(),
        shape_param_kind_export(),
        shape_partial_preset_export(),
        shape_partial_target_kind_export(),
        shape_preset_dict_entry_export(),
        shape_preset_model_field_export(),
        shape_preset_value_export(),
        shape_receiver_export(),
        shape_result_dependency(),
        shape_runtime_builtin_type(),
        shape_safe_metadata_value(),
        shape_scope_discriminant(),
        shape_semantic_source_target_kind(),
        shape_source_anchor(),
        shape_source_slot(),
        shape_symbol_namespace(),
        shape_symbol_origin(),
        shape_type_alias_export(),
        shape_type_bound_export(),
        shape_type_component(),
        shape_type_param_export(),
        shape_type_ref(),
    ]
}

/// The root selector of a checked namespace interface row.
pub fn namespace_root() -> Vec<u8> {
    b"\x78\xd2\xc9\xd3\x8a\xe9\x00".to_vec()
}

/// These four reserved member selectors carry only source documentation and byte coordinates.
pub fn namespace_field_is_canonical(identity: &[u8]) -> bool {
    const NON_CANONICAL: [&[u8]; 4] = [
        b"\x6e\x50\xd9\x8d\x24\x31",
        b"\xf7\xea\x2f\x6a\xdd\xd6",
        b"\xa2\xf1\xbb\x14\xfa\xae",
        b"\xe6\xb7\xae\xff\x05\xe2",
    ];
    !NON_CANONICAL.contains(&identity)
}
