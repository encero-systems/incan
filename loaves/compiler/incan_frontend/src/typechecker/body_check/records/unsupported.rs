//! Exhaustive unsupported-capability inventory for the initial native checker projection.

use super::super::super::canonical_nodes::NodeBindingError;
use super::super::super::type_info::{
    CAbiInteropArtifacts, CallArtifacts, ExpressionArtifacts, ProtocolArtifacts, ReferenceArtifacts,
    RustInteropArtifacts, TraitArtifacts,
};

/// Every unrepresented field is a collection and must be empty; its exact native name identifies the missing codec.
macro_rules! require_empty {
    ($kind:literal; $($field:ident),* $(,)?) => {
        $(if !$field.is_empty() { return Err(NodeBindingError::Unsupported(concat!($kind, ".", stringify!($field)))); })*
    };
}

/// Refuse every unrepresented nonempty ExpressionArtifacts field; additions must choose a canonical capability.
pub(super) fn expression_artifacts(value: &ExpressionArtifacts) -> Result<(), NodeBindingError> {
    let ExpressionArtifacts {
        body_ir_open_calls,
        body_ir_open_values,
        body_ir_open_bindings,
        body_ir_empty_list_requirements,
        body_ir_callable_name_reads,
        body_ir_expr_types: _,
        body_ir_binding_types,
        body_ir_nominal_pattern_owners,
        body_ir_parameter_requirements,
        expr_types: _,
        expression_type_identities,
        assignment_binding_types: _,
        computed_property_accesses,
        ident_kinds: _,
        ambient_logger_bindings,
        value_destination_types: _,
        numeric_conversion_destinations,
        read_only_dict_lookups,
        chained_values_written_per_target,
        validated_newtype_coercions,
        source_targets: _,
        local_partial_method_targets,
    } = value;
    require_empty!("ExpressionArtifacts"; body_ir_open_calls, body_ir_open_values, body_ir_open_bindings, body_ir_empty_list_requirements, body_ir_callable_name_reads, body_ir_binding_types, body_ir_nominal_pattern_owners, body_ir_parameter_requirements, expression_type_identities, computed_property_accesses, ambient_logger_bindings, numeric_conversion_destinations, read_only_dict_lookups, chained_values_written_per_target, validated_newtype_coercions, local_partial_method_targets);
    Ok(())
}

/// Refuse every unrepresented nonempty ReferenceArtifacts field; additions must choose a canonical capability.
pub(super) fn reference_artifacts(value: &ReferenceArtifacts) -> Result<(), NodeBindingError> {
    let ReferenceArtifacts {
        resolved_identities: _,
        resolved_write_identities: _,
        resolved_write_types: _,
    } = value;
    Ok(())
}

/// Refuse every unrepresented nonempty CallArtifacts field; additions must choose a canonical capability.
pub(super) fn call_artifacts(value: &CallArtifacts) -> Result<(), NodeBindingError> {
    let CallArtifacts {
        body_ir_constructor_field_requirements,
        body_ir_reexport_calls,
        body_ir_string_slice_members,
        body_ir_self_result_methods,
        body_ir_async_call_targets,
        body_ir_native_constructor_bindings,
        body_ir_module_constructor_bindings,
        body_ir_native_argument_types: _,
        body_ir_native_argument_identities,
        body_ir_call_type_arguments: _,
        body_ir_call_parameter_types: _,
        body_ir_factory_result_conflicts,
        body_ir_factory_owner_types,
        body_ir_constructor_types,
        body_ir_call_result_types: _,
        body_ir_native_codec_members,
        inferred_constructor_types,
        inferred_call_result_types,
        mutable_receiver_method_calls,
        resolved_builtin_calls: _,
        fixed_unpack_plans,
        call_site_monomorph_type_args: _,
        isinstance_targets,
        call_site_callable_params: _,
        race_arm_binding_types,
        constructor_field_bindings,
        resolved_operator_calls,
        resolved_method_calls,
        compiler_generated_member_identities,
        resolved_collection_constructors,
        resolved_string_helper_calls,
        closure_holding_params,
        closure_returning_callables,
        capturing_callable_bindings,
        source_display_methods,
        error_message_display_types,
    } = value;
    require_empty!("CallArtifacts"; body_ir_constructor_field_requirements, body_ir_reexport_calls, body_ir_string_slice_members, body_ir_self_result_methods, body_ir_async_call_targets, body_ir_native_constructor_bindings, body_ir_module_constructor_bindings, body_ir_native_argument_identities, body_ir_factory_result_conflicts, body_ir_factory_owner_types, body_ir_constructor_types, body_ir_native_codec_members, inferred_constructor_types, inferred_call_result_types, mutable_receiver_method_calls, fixed_unpack_plans, isinstance_targets, race_arm_binding_types, constructor_field_bindings, resolved_operator_calls, resolved_method_calls, compiler_generated_member_identities, resolved_collection_constructors, resolved_string_helper_calls, closure_holding_params, closure_returning_callables, capturing_callable_bindings, source_display_methods, error_message_display_types);
    Ok(())
}

/// Refuse every unrepresented nonempty ProtocolArtifacts field; additions must choose a canonical capability.
pub(super) fn protocol_artifacts(value: &ProtocolArtifacts) -> Result<(), NodeBindingError> {
    let ProtocolArtifacts {
        body_ir_truthiness_receivers,
        body_ir_iterations,
        iterations,
        item_taking_iterations,
        comprehension_source_consumption,
        in_place_match_scrutinees,
        loan_free_scrutinees,
    } = value;
    require_empty!("ProtocolArtifacts"; body_ir_truthiness_receivers, body_ir_iterations, iterations, item_taking_iterations, comprehension_source_consumption, in_place_match_scrutinees, loan_free_scrutinees);
    Ok(())
}

/// Refuse every unrepresented nonempty RustInteropArtifacts field; additions must choose a canonical capability.
pub(super) fn rust_interop_artifacts(value: &RustInteropArtifacts) -> Result<(), NodeBindingError> {
    let RustInteropArtifacts {
        storage_types,
        owner_parameters,
        copy_receivers,
        checked_layouts,
        checked_enums,
        checked_constants,
        checked_calls,
        argument_adjustments,
        declared_conversions,
        element_adjustments,
        field_adjustments,
        field_projection_paths,
        field_values,
        checked_function_values,
        iteration_hooks,
        value_item_paths,
        value_native_types,
        body_ir_diverging_import_calls,
        receiver_contracts,
        rusttype_canonical_paths,
        item_public_paths,
        arg_coercions,
        trait_imports,
        rusttype_forwarded_trait_adoptions,
        return_coercions,
        native_return_consumers,
        closure_param_type_displays,
        closure_storage,
        native_callable_parameters,
        function_param_type_displays,
        async_call_realizations,
        await_conversions,
        type_aliases,
    } = value;
    require_empty!("RustInteropArtifacts"; storage_types, owner_parameters, copy_receivers, checked_layouts, checked_enums, checked_constants, checked_calls, argument_adjustments, declared_conversions, element_adjustments, field_adjustments, field_projection_paths, field_values, checked_function_values, iteration_hooks, value_item_paths, value_native_types, body_ir_diverging_import_calls, receiver_contracts, rusttype_canonical_paths, item_public_paths, arg_coercions, trait_imports, rusttype_forwarded_trait_adoptions, return_coercions, native_return_consumers, closure_param_type_displays, closure_storage, native_callable_parameters, function_param_type_displays, async_call_realizations, await_conversions, type_aliases);
    Ok(())
}

/// Refuse every unrepresented nonempty TraitArtifacts field; additions must choose a canonical capability.
pub(super) fn trait_artifacts(value: &TraitArtifacts) -> Result<(), NodeBindingError> {
    let TraitArtifacts {
        direct_supertraits,
        type_params,
        method_identities,
        default_method_identities,
        standard_method_identities,
        self_method_identities,
        self_method_result_types,
        self_method_params,
        inferred_method_identities,
        adoption_type_args,
        body_ir_adoption_type_args,
    } = value;
    require_empty!("TraitArtifacts"; direct_supertraits, type_params, method_identities, default_method_identities, standard_method_identities, self_method_identities, self_method_result_types, self_method_params, inferred_method_identities, adoption_type_args, body_ir_adoption_type_args);
    Ok(())
}

/// Refuse every unrepresented nonempty CAbiInteropArtifacts field; additions must choose a canonical capability.
pub(super) fn cabi_interop_artifacts(value: &CAbiInteropArtifacts) -> Result<(), NodeBindingError> {
    let CAbiInteropArtifacts {
        verified_bindings,
        bridge_calls,
        bindings,
        raw_calls,
        function_calls,
        facades,
        output_slots,
        enum_accesses,
        enum_values,
    } = value;
    require_empty!("CAbiInteropArtifacts"; verified_bindings, bridge_calls, bindings, raw_calls, function_calls, facades, output_slots, enum_accesses, enum_values);
    Ok(())
}
