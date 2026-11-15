//! Exhaustive unsupported-capability inventory for the initial native checker projection.

use super::super::super::canonical_nodes::NodeBindingError;
use super::super::super::type_info::{
    CAbiInteropArtifacts, CallArtifacts, ExpressionArtifacts, ProtocolArtifacts, ReferenceArtifacts,
    RustInteropArtifacts, TraitArtifacts,
};
use super::super::CheckedBodyView;

/// Retain every nonempty unsupported artifact field so reporting does not depend on the first validation failure.
macro_rules! collect_nonempty {
    ($refusals:expr, $kind:literal; $($field:ident),* $(,)?) => {
        $(if !$field.is_empty() { $refusals.push(concat!($kind, ".", stringify!($field))); })*
    };
}

/// Group every unsupported checked artifact by its canonical family and native field name.
pub(super) fn artifact_refusals(value: &CheckedBodyView) -> Vec<&'static str> {
    let mut refusals = Vec::new();
    collect_expression_artifacts(value.expressions(), &mut refusals);
    collect_protocol_artifacts(value.protocols(), &mut refusals);
    collect_rust_interop_artifacts(value.rust(), &mut refusals);
    collect_trait_artifacts(value.traits(), &mut refusals);
    collect_cabi_interop_artifacts(value.c_abi(), &mut refusals);
    refusals
}

/// Refuse every unrepresented nonempty ExpressionArtifacts field; additions must choose a canonical capability.
pub(super) fn expression_artifacts(value: &ExpressionArtifacts) -> Result<(), NodeBindingError> {
    let mut refusals = Vec::new();
    collect_expression_artifacts(value, &mut refusals);
    if let Some(refusal) = refusals.into_iter().next() {
        return Err(NodeBindingError::Unsupported(refusal));
    }
    Ok(())
}

fn collect_expression_artifacts(value: &ExpressionArtifacts, refusals: &mut Vec<&'static str>) {
    let ExpressionArtifacts {
        body_ir_open_calls,
        body_ir_open_values,
        body_ir_open_bindings,
        body_ir_empty_list_requirements,
        body_ir_callable_name_reads,
        body_ir_expr_types: _,
        body_ir_binding_types: _,
        body_ir_nominal_pattern_owners,
        body_ir_parameter_requirements: _,
        expr_types: _,
        expression_type_identities: _,
        assignment_binding_types: _,
        computed_property_accesses,
        ident_kinds: _,
        value_destination_types: _,
        numeric_conversion_destinations,
        read_only_dict_lookups,
        chained_values_written_per_target,
        validated_newtype_coercions,
        source_targets: _,
        local_partial_method_targets,
    } = value;
    collect_nonempty!(refusals, "ExpressionArtifacts"; body_ir_open_calls, body_ir_open_values, body_ir_open_bindings, body_ir_empty_list_requirements, body_ir_callable_name_reads, body_ir_nominal_pattern_owners, computed_property_accesses, numeric_conversion_destinations, read_only_dict_lookups, chained_values_written_per_target, validated_newtype_coercions, local_partial_method_targets);
}

/// Refuse every unrepresented nonempty ReferenceArtifacts field; additions must choose a canonical capability.
pub(super) fn reference_artifacts(value: &ReferenceArtifacts) -> Result<(), NodeBindingError> {
    let ReferenceArtifacts {
        resolved_identities: _,
        stable_declaration_contexts: _,
        resolved_write_identities: _,
        resolved_write_types: _,
    } = value;
    Ok(())
}

/// Refuse every unrepresented nonempty CallArtifacts field; additions must choose a canonical capability.
pub(super) fn call_artifacts(value: &CallArtifacts) -> Result<(), NodeBindingError> {
    let CallArtifacts {
        body_ir_constructor_field_requirements: _,
        body_ir_reexport_calls: _,
        body_ir_string_slice_members: _,
        body_ir_self_result_methods: _,
        body_ir_async_call_targets: _,
        body_ir_native_constructor_bindings: _,
        body_ir_module_constructor_bindings: _,
        body_ir_native_argument_types: _,
        body_ir_native_argument_identities: _,
        body_ir_call_type_arguments: _,
        body_ir_call_parameter_types: _,
        body_ir_factory_result_conflicts: _,
        body_ir_factory_owner_types: _,
        body_ir_constructor_types: _,
        body_ir_call_result_types: _,
        body_ir_native_codec_members: _,
        inferred_constructor_types: _,
        inferred_call_result_types: _,
        mutable_receiver_method_calls: _,
        resolved_builtin_calls: _,
        resolved_builtin_method_calls: _,
        fixed_unpack_plans: _,
        call_site_monomorph_type_args: _,
        isinstance_targets: _,
        call_site_callable_params: _,
        race_arm_binding_types: _,
        constructor_field_bindings: _,
        resolved_operator_calls: _,
        resolved_method_calls,
        compiler_generated_member_identities,
        resolved_collection_constructors: _,
        closure_holding_params: _,
        closure_returning_callables: _,
        capturing_callable_bindings: _,
        source_display_methods: _,
        error_message_display_types,
    } = value;
    let _ = (
        resolved_method_calls,
        compiler_generated_member_identities,
        error_message_display_types,
    );
    Ok(())
}

/// Refuse every unrepresented nonempty ProtocolArtifacts field; additions must choose a canonical capability.
pub(super) fn protocol_artifacts(value: &ProtocolArtifacts) -> Result<(), NodeBindingError> {
    let mut refusals = Vec::new();
    collect_protocol_artifacts(value, &mut refusals);
    if let Some(refusal) = refusals.into_iter().next() {
        return Err(NodeBindingError::Unsupported(refusal));
    }
    Ok(())
}

fn collect_protocol_artifacts(value: &ProtocolArtifacts, refusals: &mut Vec<&'static str>) {
    let ProtocolArtifacts {
        explicit_input_calls: _,
        // Derived from the checked native calls on restore, so it is never recorded.
        native_input_contracts: _,
        explicit_local_modes: _,
        explicit_input_flow: _,
        // The mutation record transports this graph's source projection; restore rebuilds it at settlement.
        mutation_flows: _,
        explicit_input_copy: _,
        body_ir_truthiness_receivers: _,
        body_ir_iterations,
        iterations,
        item_taking_iterations: _,
        comprehension_source_consumption: _,
        in_place_match_scrutinees: _,
        loan_free_scrutinees: _,
    } = value;
    collect_nonempty!(refusals, "ProtocolArtifacts"; body_ir_iterations, iterations);
}

/// Refuse every unrepresented nonempty RustInteropArtifacts field; additions must choose a canonical capability.
pub(super) fn rust_interop_artifacts(value: &RustInteropArtifacts) -> Result<(), NodeBindingError> {
    let mut refusals = Vec::new();
    collect_rust_interop_artifacts(value, &mut refusals);
    if let Some(refusal) = refusals.into_iter().next() {
        return Err(NodeBindingError::Unsupported(refusal));
    }
    Ok(())
}

fn collect_rust_interop_artifacts(value: &RustInteropArtifacts, refusals: &mut Vec<&'static str>) {
    let RustInteropArtifacts {
        storage_types,
        owner_parameters,
        copy_receivers,
        checked_layouts,
        checked_enums,
        checked_constants: _,
        checked_calls: _,
        argument_adjustments,
        declared_conversions,
        validated_rusttype_edges,
        element_adjustments,
        field_adjustments,
        field_projection_paths,
        field_values,
        checked_function_values,
        iteration_hooks,
        value_item_paths,
        value_native_types: _,
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
    collect_nonempty!(refusals, "RustInteropArtifacts"; storage_types, owner_parameters, copy_receivers, checked_layouts, checked_enums, argument_adjustments, declared_conversions, validated_rusttype_edges, element_adjustments, field_adjustments, field_projection_paths, field_values, checked_function_values, iteration_hooks, value_item_paths, body_ir_diverging_import_calls, receiver_contracts, rusttype_canonical_paths, item_public_paths, arg_coercions, trait_imports, rusttype_forwarded_trait_adoptions, return_coercions, native_return_consumers, closure_param_type_displays, closure_storage, native_callable_parameters, function_param_type_displays, async_call_realizations, await_conversions, type_aliases);
}

/// Refuse every unrepresented nonempty TraitArtifacts field; additions must choose a canonical capability.
pub(super) fn trait_artifacts(value: &TraitArtifacts) -> Result<(), NodeBindingError> {
    let mut refusals = Vec::new();
    collect_trait_artifacts(value, &mut refusals);
    if let Some(refusal) = refusals.into_iter().next() {
        return Err(NodeBindingError::Unsupported(refusal));
    }
    Ok(())
}

fn collect_trait_artifacts(value: &TraitArtifacts, refusals: &mut Vec<&'static str>) {
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
    collect_nonempty!(refusals, "TraitArtifacts"; direct_supertraits, type_params, method_identities, default_method_identities, standard_method_identities, self_method_identities, self_method_result_types, self_method_params, inferred_method_identities, adoption_type_args, body_ir_adoption_type_args);
}

/// Refuse every unrepresented nonempty CAbiInteropArtifacts field; additions must choose a canonical capability.
pub(super) fn cabi_interop_artifacts(value: &CAbiInteropArtifacts) -> Result<(), NodeBindingError> {
    let mut refusals = Vec::new();
    collect_cabi_interop_artifacts(value, &mut refusals);
    if let Some(refusal) = refusals.into_iter().next() {
        return Err(NodeBindingError::Unsupported(refusal));
    }
    Ok(())
}

fn collect_cabi_interop_artifacts(value: &CAbiInteropArtifacts, refusals: &mut Vec<&'static str>) {
    let CAbiInteropArtifacts {
        verified_bindings,
        bridge_calls,
        bindings,
        raw_calls,
        // Reconstructed and loss-checked from the existing ordinary call signatures and source-target fields.
        function_calls: _,
        facades,
        output_slots,
        enum_accesses,
        enum_values,
        clove_libraries,
    } = value;
    collect_nonempty!(refusals, "CAbiInteropArtifacts"; verified_bindings, bridge_calls, bindings, raw_calls, facades, output_slots, enum_accesses, enum_values, clove_libraries);
}
