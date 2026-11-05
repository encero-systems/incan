//! Merging of a body's local checked artifacts into the enclosing artifacts.

use super::*;

/// Relocate one local expression projection into its receiving index space before the exhaustive field join. Native
/// read-only assembly and compatibility publication share this operation so open-value groups cannot drift.
pub(super) fn append_local_expressions(target: &mut ExpressionArtifacts, mut local: ExpressionArtifacts) {
    let offset = target.body_ir_open_calls.len();
    for value in local
        .body_ir_open_values
        .values_mut()
        .chain(local.body_ir_open_bindings.values_mut())
    {
        value.group += offset;
    }
    append_expressions(target, local);
}

/// Consume all expressions fields into the compatibility view; field additions require an explicit ownership choice.
pub(super) fn append_expressions(target: &mut ExpressionArtifacts, local: ExpressionArtifacts) {
    let ExpressionArtifacts {
        body_ir_open_calls,
        body_ir_open_values,
        body_ir_open_bindings,
        body_ir_empty_list_requirements,
        body_ir_callable_name_reads,
        body_ir_expr_types,
        body_ir_binding_types,
        body_ir_nominal_pattern_owners,
        body_ir_parameter_requirements,
        expr_types,
        expression_type_identities,
        assignment_binding_types,
        computed_property_accesses,
        ident_kinds,
        ambient_logger_bindings,
        value_destination_types,
        numeric_conversion_destinations,
        read_only_dict_lookups,
        chained_values_written_per_target,
        validated_newtype_coercions,
        source_targets,
        local_partial_method_targets,
    } = local;
    target.body_ir_open_calls.extend(body_ir_open_calls);
    target.body_ir_open_values.extend(body_ir_open_values);
    target.body_ir_open_bindings.extend(body_ir_open_bindings);
    target
        .body_ir_empty_list_requirements
        .extend(body_ir_empty_list_requirements);
    target.body_ir_callable_name_reads.extend(body_ir_callable_name_reads);
    target.body_ir_expr_types.extend(body_ir_expr_types);
    target.body_ir_binding_types.extend(body_ir_binding_types);
    target
        .body_ir_nominal_pattern_owners
        .extend(body_ir_nominal_pattern_owners);
    target
        .body_ir_parameter_requirements
        .extend(body_ir_parameter_requirements);
    target.expr_types.extend(expr_types);
    target.expression_type_identities.extend(expression_type_identities);
    target.assignment_binding_types.extend(assignment_binding_types);
    target.computed_property_accesses.extend(computed_property_accesses);
    target.ident_kinds.extend(ident_kinds);
    target.ambient_logger_bindings.extend(ambient_logger_bindings);
    target.value_destination_types.extend(value_destination_types);
    target
        .numeric_conversion_destinations
        .extend(numeric_conversion_destinations);
    target.read_only_dict_lookups.extend(read_only_dict_lookups);
    target
        .chained_values_written_per_target
        .extend(chained_values_written_per_target);
    target.validated_newtype_coercions.extend(validated_newtype_coercions);
    target.source_targets.extend(source_targets);
    target.local_partial_method_targets.extend(local_partial_method_targets);
}

/// Consume all references fields into the compatibility view; field additions require an explicit ownership choice.
pub(super) fn append_references(target: &mut ReferenceArtifacts, local: ReferenceArtifacts) {
    let ReferenceArtifacts {
        resolved_identities,
        resolved_write_identities,
        resolved_write_types,
    } = local;
    target.resolved_identities.extend(resolved_identities);
    target.resolved_write_identities.extend(resolved_write_identities);
    target.resolved_write_types.extend(resolved_write_types);
}

/// Consume all calls fields into the compatibility view; field additions require an explicit ownership choice.
pub(super) fn append_calls(target: &mut CallArtifacts, local: CallArtifacts) {
    let CallArtifacts {
        body_ir_constructor_field_requirements,
        body_ir_reexport_calls,
        body_ir_string_slice_members,
        body_ir_self_result_methods,
        body_ir_async_call_targets,
        body_ir_native_constructor_bindings,
        body_ir_module_constructor_bindings,
        body_ir_native_argument_types,
        body_ir_native_argument_identities,
        body_ir_call_type_arguments,
        body_ir_call_parameter_types,
        body_ir_factory_result_conflicts,
        body_ir_factory_owner_types,
        body_ir_constructor_types,
        body_ir_call_result_types,
        body_ir_native_codec_members,
        inferred_constructor_types,
        inferred_call_result_types,
        mutable_receiver_method_calls,
        resolved_builtin_calls,
        fixed_unpack_plans,
        call_site_monomorph_type_args,
        isinstance_targets,
        call_site_callable_params,
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
    } = local;
    target
        .body_ir_constructor_field_requirements
        .extend(body_ir_constructor_field_requirements);
    target.body_ir_reexport_calls.extend(body_ir_reexport_calls);
    target.body_ir_string_slice_members.extend(body_ir_string_slice_members);
    target.body_ir_self_result_methods.extend(body_ir_self_result_methods);
    target.body_ir_async_call_targets.extend(body_ir_async_call_targets);
    target
        .body_ir_native_constructor_bindings
        .extend(body_ir_native_constructor_bindings);
    target
        .body_ir_module_constructor_bindings
        .extend(body_ir_module_constructor_bindings);
    target
        .body_ir_native_argument_types
        .extend(body_ir_native_argument_types);
    target
        .body_ir_native_argument_identities
        .extend(body_ir_native_argument_identities);
    target.body_ir_call_type_arguments.extend(body_ir_call_type_arguments);
    target.body_ir_call_parameter_types.extend(body_ir_call_parameter_types);
    target
        .body_ir_factory_result_conflicts
        .extend(body_ir_factory_result_conflicts);
    target.body_ir_factory_owner_types.extend(body_ir_factory_owner_types);
    target.body_ir_constructor_types.extend(body_ir_constructor_types);
    target.body_ir_call_result_types.extend(body_ir_call_result_types);
    target.body_ir_native_codec_members.extend(body_ir_native_codec_members);
    target.inferred_constructor_types.extend(inferred_constructor_types);
    target.inferred_call_result_types.extend(inferred_call_result_types);
    target
        .mutable_receiver_method_calls
        .extend(mutable_receiver_method_calls);
    target.resolved_builtin_calls.extend(resolved_builtin_calls);
    target.fixed_unpack_plans.extend(fixed_unpack_plans);
    target
        .call_site_monomorph_type_args
        .extend(call_site_monomorph_type_args);
    target.isinstance_targets.extend(isinstance_targets);
    target.call_site_callable_params.extend(call_site_callable_params);
    target.race_arm_binding_types.extend(race_arm_binding_types);
    target.constructor_field_bindings.extend(constructor_field_bindings);
    target.resolved_operator_calls.extend(resolved_operator_calls);
    target.resolved_method_calls.extend(resolved_method_calls);
    target
        .compiler_generated_member_identities
        .extend(compiler_generated_member_identities);
    target
        .resolved_collection_constructors
        .extend(resolved_collection_constructors);
    target.resolved_string_helper_calls.extend(resolved_string_helper_calls);
    target.closure_holding_params.extend(closure_holding_params);
    target.closure_returning_callables.extend(closure_returning_callables);
    target.capturing_callable_bindings.extend(capturing_callable_bindings);
    target.source_display_methods.extend(source_display_methods);
    target.error_message_display_types.extend(error_message_display_types);
}

/// Consume all protocols fields into the compatibility view; field additions require an explicit ownership choice.
pub(super) fn append_protocols(target: &mut ProtocolArtifacts, local: ProtocolArtifacts) {
    let ProtocolArtifacts {
        explicit_input_calls,
        native_input_contracts,
        explicit_local_modes,
        explicit_input_flow,
        explicit_input_copy,
        body_ir_truthiness_receivers,
        body_ir_iterations,
        iterations,
        item_taking_iterations,
        comprehension_source_consumption,
        in_place_match_scrutinees,
        loan_free_scrutinees,
    } = local;
    target.explicit_input_calls.extend(explicit_input_calls);
    target.native_input_contracts.extend(native_input_contracts);
    target.explicit_local_modes.extend(explicit_local_modes);
    target.explicit_input_flow.extend(explicit_input_flow);
    target.explicit_input_copy.extend(explicit_input_copy);
    target.body_ir_truthiness_receivers.extend(body_ir_truthiness_receivers);
    target.body_ir_iterations.extend(body_ir_iterations);
    target.iterations.extend(iterations);
    target.item_taking_iterations.extend(item_taking_iterations);
    target
        .comprehension_source_consumption
        .extend(comprehension_source_consumption);
    target.in_place_match_scrutinees.extend(in_place_match_scrutinees);
    target.loan_free_scrutinees.extend(loan_free_scrutinees);
}

/// Consume all rust fields into the compatibility view; field additions require an explicit ownership choice.
pub(super) fn append_rust(target: &mut RustInteropArtifacts, local: RustInteropArtifacts) {
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
    } = local;
    target.storage_types.extend(storage_types);
    target.owner_parameters.extend(owner_parameters);
    target.copy_receivers.extend(copy_receivers);
    target.checked_layouts.extend(checked_layouts);
    target.checked_enums.extend(checked_enums);
    target.checked_constants.extend(checked_constants);
    target.checked_calls.extend(checked_calls);
    target.argument_adjustments.extend(argument_adjustments);
    target.declared_conversions.extend(declared_conversions);
    target.element_adjustments.extend(element_adjustments);
    target.field_adjustments.extend(field_adjustments);
    target.field_projection_paths.extend(field_projection_paths);
    target.field_values.extend(field_values);
    target.checked_function_values.extend(checked_function_values);
    target.iteration_hooks.extend(iteration_hooks);
    target.value_item_paths.extend(value_item_paths);
    target.value_native_types.extend(value_native_types);
    target
        .body_ir_diverging_import_calls
        .extend(body_ir_diverging_import_calls);
    target.receiver_contracts.extend(receiver_contracts);
    target.rusttype_canonical_paths.extend(rusttype_canonical_paths);
    target.item_public_paths.extend(item_public_paths);
    target.arg_coercions.extend(arg_coercions);
    target.trait_imports.extend(trait_imports);
    target
        .rusttype_forwarded_trait_adoptions
        .extend(rusttype_forwarded_trait_adoptions);
    target.return_coercions.extend(return_coercions);
    target.native_return_consumers.extend(native_return_consumers);
    target.closure_param_type_displays.extend(closure_param_type_displays);
    target.closure_storage.extend(closure_storage);
    target.native_callable_parameters.extend(native_callable_parameters);
    target.function_param_type_displays.extend(function_param_type_displays);
    target.async_call_realizations.extend(async_call_realizations);
    target.await_conversions.extend(await_conversions);
    target.type_aliases.extend(type_aliases);
}

/// Consume all traits fields into the compatibility view; field additions require an explicit ownership choice.
pub(super) fn append_traits(target: &mut TraitArtifacts, local: TraitArtifacts) {
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
    } = local;
    target.direct_supertraits.extend(direct_supertraits);
    target.type_params.extend(type_params);
    target.method_identities.extend(method_identities);
    target.default_method_identities.extend(default_method_identities);
    target.standard_method_identities.extend(standard_method_identities);
    target.self_method_identities.extend(self_method_identities);
    target.self_method_result_types.extend(self_method_result_types);
    target.self_method_params.extend(self_method_params);
    target.inferred_method_identities.extend(inferred_method_identities);
    target.adoption_type_args.extend(adoption_type_args);
    target.body_ir_adoption_type_args.extend(body_ir_adoption_type_args);
}

/// Consume the exact checked C bridge, resource, output-slot and raw-call decisions in source order.
pub(super) fn append_c_abi(target: &mut CAbiInteropArtifacts, local: CAbiInteropArtifacts) {
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
    } = local;
    target.verified_bindings.extend(verified_bindings);
    target.bridge_calls.extend(bridge_calls);
    target.bindings.extend(bindings);
    target.raw_calls.extend(raw_calls);
    target.function_calls.extend(function_calls);
    target.facades.extend(facades);
    target.output_slots.extend(output_slots);
    target.enum_accesses.extend(enum_accesses);
    target.enum_values.extend(enum_values);
}
