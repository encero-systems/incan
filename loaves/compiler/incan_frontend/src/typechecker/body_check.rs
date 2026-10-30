//! One callable's checked output and deferred decisions, before the existing module settlement passes.
//!
//! Declaration inputs remain with G1. The bootstrap scope redirects the existing checker into empty local fact
//! tables, then consumes its result into the invocation-local compatibility view. No body is rechecked to settle it.

use super::mut_arguments::MutBodyLocal;
use super::type_info::{
    CAbiInteropArtifacts, CallArtifacts, ExpressionArtifacts, ProtocolArtifacts, ReferenceArtifacts,
    RustInteropArtifacts, TraitArtifacts, TypeCheckInfo,
};
use super::{TypeChecker, VisibilityPoint};
use crate::ast::{FunctionDecl, MethodDecl, Span, TypeParam};
use crate::diagnostics::CompileError;
use crate::symbols::ResolvedType;
use std::collections::BTreeMap;

/// Owned local checked output. Open-value groups keep body-local indices until the compatibility join relocates
/// them; mutation and dictionary decisions remain pending for their existing module-order settlement owners.
#[derive(Debug, Default)]
pub(super) struct BodyLocal {
    expressions: ExpressionArtifacts,
    references: ReferenceArtifacts,
    calls: CallArtifacts,
    protocols: ProtocolArtifacts,
    rust: RustInteropArtifacts,
    traits: TraitArtifacts,
    c_abi: CAbiInteropArtifacts,
    mutation: MutBodyLocal,
    dictionary: Vec<(Span, String)>,
    iterator_sum_items: BTreeMap<Span, ResolvedType>,
    diagnostics: Vec<CompileError>,
    warnings: Vec<CompileError>,
}

/// One source-order body activation against the current declaration/environment views and local scratch. Earlier
/// output is held outside the active writer, never cloned as a body input or recomputed by the scope.
pub(super) struct BodyCheckScope {
    enclosing: BodyLocal,
    diagnostic_start: usize,
    warning_start: usize,
    visibility: VisibilityPoint,
}

impl BodyCheckScope {
    /// Borrow G1's current visibility point before taking mutable local tables. Declaration/default validation runs
    /// once inside the existing callable operation; declaration-owned tables and symbols remain its read authority.
    pub(super) fn enter(checker: &mut TypeChecker) -> Self {
        let visibility = checker.check_environment().visibility_point();
        let mut enclosing = BodyLocal::take(checker);
        enclosing.install_shared_inputs(checker);
        Self {
            enclosing,
            diagnostic_start: checker.errors.len(),
            warning_start: checker.warnings.len(),
            visibility,
        }
    }

    /// Return the owned body result and restore its enclosing writer, preserving source diagnostic order. Pending
    /// decisions are recorded, not resolved, and the environment cannot advance during this activation.
    pub(super) fn finish(mut self, checker: &mut TypeChecker) -> BodyLocal {
        debug_assert_eq!(self.visibility, checker.check_environment().visibility_point());
        let mut local = BodyLocal::take(checker);
        local.diagnostics = checker.errors.split_off(self.diagnostic_start);
        local.warnings = checker.warnings.split_off(self.warning_start);
        local.restore_shared_inputs(&mut self.enclosing);
        self.enclosing.install(checker);
        local
    }
}

impl BodyLocal {
    /// Detach only body-owned artifact families and pending decisions; declaration, imported metadata and signature
    /// inputs are moved back into the active writer by the scope before any semantic operation runs. The exhaustive
    /// root inventory requires every added artifact to choose local, declaration or final-settlement ownership.
    fn take(checker: &mut TypeChecker) -> Self {
        let TypeCheckInfo {
            traits,
            derivations: _,
            expressions,
            references,
            consts: _,
            rust,
            declarations: _,
            registry: _,
            calls,
            protocols,
            c_abi,
            import_bindings: _,
            body_ir_runtime_types: _,
            body_ir_newtypes: _,
            body_ir_iterator_sum_items,
        } = &mut checker.type_info;
        Self {
            expressions: std::mem::take(expressions),
            references: std::mem::take(references),
            calls: std::mem::take(calls),
            protocols: std::mem::take(protocols),
            rust: std::mem::take(rust),
            traits: std::mem::take(traits),
            c_abi: std::mem::take(c_abi),
            mutation: checker.mut_params.take_body_local(),
            dictionary: std::mem::take(&mut checker.pending_uncopyable_dict_lookups),
            iterator_sum_items: std::mem::take(body_ir_iterator_sum_items),
            diagnostics: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Restore an enclosing writer without merging or changing its local group indices.
    fn install(self, checker: &mut TypeChecker) {
        checker.type_info.expressions = self.expressions;
        checker.type_info.references = self.references;
        checker.type_info.calls = self.calls;
        checker.type_info.protocols = self.protocols;
        checker.type_info.rust = self.rust;
        checker.type_info.traits = self.traits;
        checker.type_info.c_abi = self.c_abi;
        checker.mut_params.install_body_local(self.mutation);
        checker.pending_uncopyable_dict_lookups = self.dictionary;
        checker.type_info.body_ir_iterator_sum_items = self.iterator_sum_items;
    }

    /// Keep declaration and interprocedural inputs in the active compatibility writer; local checking never
    /// receives an empty replacement for a known signature or declaration-time native facts.
    fn install_shared_inputs(&mut self, checker: &mut TypeChecker) {
        checker.type_info.c_abi.enum_values = std::mem::take(&mut self.c_abi.enum_values);
        checker.type_info.c_abi.bindings = std::mem::take(&mut self.c_abi.bindings);
        checker.type_info.c_abi.verified_bindings = std::mem::take(&mut self.c_abi.verified_bindings);
        checker.type_info.calls.compiler_generated_member_identities =
            std::mem::take(&mut self.calls.compiler_generated_member_identities);
        checker.type_info.calls.closure_holding_params = std::mem::take(&mut self.calls.closure_holding_params);
        checker.type_info.calls.closure_returning_callables =
            std::mem::take(&mut self.calls.closure_returning_callables);
        checker.type_info.calls.error_message_display_types =
            std::mem::take(&mut self.calls.error_message_display_types);
        checker.type_info.rust.storage_types = std::mem::take(&mut self.rust.storage_types);
        checker.type_info.rust.owner_parameters = std::mem::take(&mut self.rust.owner_parameters);
        checker.type_info.rust.checked_layouts = std::mem::take(&mut self.rust.checked_layouts);
        checker.type_info.rust.checked_enums = std::mem::take(&mut self.rust.checked_enums);
        checker.type_info.rust.declared_conversions = std::mem::take(&mut self.rust.declared_conversions);
        checker.type_info.rust.rusttype_canonical_paths = std::mem::take(&mut self.rust.rusttype_canonical_paths);
        checker.type_info.rust.item_public_paths = std::mem::take(&mut self.rust.item_public_paths);
        checker.type_info.rust.trait_imports = std::mem::take(&mut self.rust.trait_imports);
        checker.type_info.rust.rusttype_forwarded_trait_adoptions =
            std::mem::take(&mut self.rust.rusttype_forwarded_trait_adoptions);
        checker.type_info.rust.native_callable_parameters = std::mem::take(&mut self.rust.native_callable_parameters);
        checker.type_info.rust.function_param_type_displays =
            std::mem::take(&mut self.rust.function_param_type_displays);
        checker.type_info.rust.type_aliases = std::mem::take(&mut self.rust.type_aliases);
        checker.type_info.traits.direct_supertraits = std::mem::take(&mut self.traits.direct_supertraits);
        checker.type_info.traits.type_params = std::mem::take(&mut self.traits.type_params);
        checker.type_info.traits.method_identities = std::mem::take(&mut self.traits.method_identities);
        checker.type_info.traits.default_method_identities = std::mem::take(&mut self.traits.default_method_identities);
        checker.type_info.traits.standard_method_identities =
            std::mem::take(&mut self.traits.standard_method_identities);
        checker.type_info.traits.adoption_type_args = std::mem::take(&mut self.traits.adoption_type_args);
        checker.type_info.traits.body_ir_adoption_type_args =
            std::mem::take(&mut self.traits.body_ir_adoption_type_args);
    }

    /// Return shared inputs, including newly observed requirements, to their original authority before joining locals.
    fn restore_shared_inputs(&mut self, enclosing: &mut Self) {
        enclosing.c_abi.enum_values = std::mem::take(&mut self.c_abi.enum_values);
        enclosing.c_abi.bindings = std::mem::take(&mut self.c_abi.bindings);
        enclosing.c_abi.verified_bindings = std::mem::take(&mut self.c_abi.verified_bindings);
        enclosing.calls.compiler_generated_member_identities =
            std::mem::take(&mut self.calls.compiler_generated_member_identities);
        enclosing.calls.closure_holding_params = std::mem::take(&mut self.calls.closure_holding_params);
        enclosing.calls.closure_returning_callables = std::mem::take(&mut self.calls.closure_returning_callables);
        enclosing.calls.error_message_display_types = std::mem::take(&mut self.calls.error_message_display_types);
        enclosing.rust.storage_types = std::mem::take(&mut self.rust.storage_types);
        enclosing.rust.owner_parameters = std::mem::take(&mut self.rust.owner_parameters);
        enclosing.rust.checked_layouts = std::mem::take(&mut self.rust.checked_layouts);
        enclosing.rust.checked_enums = std::mem::take(&mut self.rust.checked_enums);
        enclosing.rust.declared_conversions = std::mem::take(&mut self.rust.declared_conversions);
        enclosing.rust.rusttype_canonical_paths = std::mem::take(&mut self.rust.rusttype_canonical_paths);
        enclosing.rust.item_public_paths = std::mem::take(&mut self.rust.item_public_paths);
        enclosing.rust.trait_imports = std::mem::take(&mut self.rust.trait_imports);
        enclosing.rust.rusttype_forwarded_trait_adoptions =
            std::mem::take(&mut self.rust.rusttype_forwarded_trait_adoptions);
        enclosing.rust.native_callable_parameters = std::mem::take(&mut self.rust.native_callable_parameters);
        enclosing.rust.function_param_type_displays = std::mem::take(&mut self.rust.function_param_type_displays);
        enclosing.rust.type_aliases = std::mem::take(&mut self.rust.type_aliases);
        enclosing.traits.direct_supertraits = std::mem::take(&mut self.traits.direct_supertraits);
        enclosing.traits.type_params = std::mem::take(&mut self.traits.type_params);
        enclosing.traits.method_identities = std::mem::take(&mut self.traits.method_identities);
        enclosing.traits.default_method_identities = std::mem::take(&mut self.traits.default_method_identities);
        enclosing.traits.standard_method_identities = std::mem::take(&mut self.traits.standard_method_identities);
        enclosing.traits.adoption_type_args = std::mem::take(&mut self.traits.adoption_type_args);
        enclosing.traits.body_ir_adoption_type_args = std::mem::take(&mut self.traits.body_ir_adoption_type_args);
    }

    /// Append one checked body exactly once. Relocate only the local open-value indices; every map value, ordered
    /// diagnostic and deferred decision otherwise reaches the original settlement owners unchanged.
    fn append(mut self, checker: &mut TypeChecker) {
        let offset = checker.type_info.expressions.body_ir_open_calls.len();
        for value in self
            .expressions
            .body_ir_open_values
            .values_mut()
            .chain(self.expressions.body_ir_open_bindings.values_mut())
        {
            value.group += offset;
        }
        append_expressions(&mut checker.type_info.expressions, self.expressions);
        append_references(&mut checker.type_info.references, self.references);
        append_calls(&mut checker.type_info.calls, self.calls);
        append_protocols(&mut checker.type_info.protocols, self.protocols);
        append_rust(&mut checker.type_info.rust, self.rust);
        append_traits(&mut checker.type_info.traits, self.traits);
        checker.mut_params.append_body_local(self.mutation);
        checker.pending_uncopyable_dict_lookups.extend(self.dictionary);
        checker
            .type_info
            .body_ir_iterator_sum_items
            .extend(self.iterator_sum_items);
        append_c_abi(&mut checker.type_info.c_abi, self.c_abi);
        checker.errors.extend(self.diagnostics);
        checker.warnings.extend(self.warnings);
    }
}

/// Consume all expressions fields into the compatibility view; field additions require an explicit ownership choice.
fn append_expressions(target: &mut ExpressionArtifacts, local: ExpressionArtifacts) {
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
fn append_references(target: &mut ReferenceArtifacts, local: ReferenceArtifacts) {
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
fn append_calls(target: &mut CallArtifacts, local: CallArtifacts) {
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
fn append_protocols(target: &mut ProtocolArtifacts, local: ProtocolArtifacts) {
    let ProtocolArtifacts {
        body_ir_truthiness_receivers,
        body_ir_iterations,
        iterations,
        item_taking_iterations,
        comprehension_source_consumption,
        in_place_match_scrutinees,
    } = local;
    target.body_ir_truthiness_receivers.extend(body_ir_truthiness_receivers);
    target.body_ir_iterations.extend(body_ir_iterations);
    target.iterations.extend(iterations);
    target.item_taking_iterations.extend(item_taking_iterations);
    target
        .comprehension_source_consumption
        .extend(comprehension_source_consumption);
    target.in_place_match_scrutinees.extend(in_place_match_scrutinees);
}

/// Consume all rust fields into the compatibility view; field additions require an explicit ownership choice.
fn append_rust(target: &mut RustInteropArtifacts, local: RustInteropArtifacts) {
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
fn append_traits(target: &mut TraitArtifacts, local: TraitArtifacts) {
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
fn append_c_abi(target: &mut CAbiInteropArtifacts, local: CAbiInteropArtifacts) {
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

impl TypeChecker {
    /// Run the existing function operation once with declaration inputs and a fresh body writer.
    pub(super) fn check_function(&mut self, function: &FunctionDecl, span: Span) {
        self.check_work.body_local_checks += 1;
        #[cfg(test)]
        if self.legacy_body_checking {
            self.check_function_in_scope(function, span);
            self.observe_body_local(span);
            return;
        }
        let scope = BodyCheckScope::enter(self);
        self.check_function_in_scope(function, span);
        scope.finish(self).append(self);
        #[cfg(test)]
        self.observe_body_local(span);
    }

    /// Keep abstract method validation outside the body boundary; each concrete method checks one local body.
    pub(super) fn check_method_with_self_ty(
        &mut self,
        method: &MethodDecl,
        span: Span,
        self_ty: ResolvedType,
        owner: &[TypeParam],
    ) {
        if method.body.is_none() {
            self.check_method_in_scope(method, span, self_ty, owner);
            return;
        }
        self.check_work.body_local_checks += 1;
        #[cfg(test)]
        if self.legacy_body_checking {
            self.check_method_in_scope(method, span, self_ty, owner);
            self.observe_body_local(span);
            return;
        }
        let scope = BodyCheckScope::enter(self);
        self.check_method_in_scope(method, span, self_ty, owner);
        scope.finish(self).append(self);
        #[cfg(test)]
        self.observe_body_local(span);
    }
}

#[cfg(test)]
mod tests;
