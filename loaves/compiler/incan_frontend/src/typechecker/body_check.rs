//! Declaration and callable-statement outputs and deferred decisions, before module settlement.
//!
//! Collected declaration inputs remain with G1. Declaration validation, defaults and initializers own outputs
//! independently from statement bodies. Each operation uses the same local writer and existing semantic rules;
//! settlement consumes those outputs without rechecking an expression or statement. Settled outputs store their
//! diagnostics as anchored semantic values, and the published diagnostics are rendered from those values.

use super::diagnostic_values::{AnchorFault, AnchoredDiagnostic, DiagnosticPositions};
use super::mut_arguments::MutBodyLocal;
use super::type_info::{
    CAbiInteropArtifacts, CallArtifacts, ExpressionArtifacts, ProtocolArtifacts, ReferenceArtifacts,
    RustInteropArtifacts, TraitArtifacts, TypeCheckInfo,
};
use super::{TypeChecker, VisibilityPoint, YieldContext};
use crate::ast::{FunctionDecl, Program, Span, Spanned, Statement};
use crate::diagnostics::CompileError;
use crate::symbols::ResolvedType;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Semantic operation that owns one local output; declaration expressions never count as statement-body checks.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum LocalOwnerKind {
    #[default]
    Body,
    Declaration,
}

/// Owned local checked output. Open-value groups keep body-local indices until the compatibility join relocates
/// them; mutation and dictionary decisions remain pending for their existing module-order settlement owners.
#[derive(Debug, Default, Clone)]
pub(super) struct BodyLocal {
    /// Revision-local owner handle; semantic identity is supplied separately by the record adapter.
    owner: Span,
    /// Declaration/default output is settled independently from callable statement output.
    kind: LocalOwnerKind,
    order: BodyDiagnosticOrder,
    /// Body-owned lexical declarations retained independently of a later whole-symbol-table export.
    nested: BTreeMap<Span, incan_semantics_core::CanonicalSymbolId>,
    expressions: ExpressionArtifacts,
    references: ReferenceArtifacts,
    calls: CallArtifacts,
    protocols: ProtocolArtifacts,
    rust: RustInteropArtifacts,
    traits: TraitArtifacts,
    c_abi: CAbiInteropArtifacts,
    mutation: MutBodyLocal,
    dictionary: Vec<(usize, Span, String)>,
    iterator_sum_items: BTreeMap<Span, ResolvedType>,
    /// This owner's own entries in the active diagnostic writer, excluding nested owners. Settlement captures them
    /// as anchored values once; these invocation-local indices never enter a settled view.
    diagnostic_slots: Vec<usize>,
    warning_slots: Vec<usize>,
}

/// One source-order body activation against the current declaration/environment views and local scratch. Earlier
/// output is held outside the active writer, never cloned as a body input or recomputed by the scope.
pub(super) struct BodyCheckScope {
    enclosing: BodyLocal,
    symbol_start: usize,
    output_start: usize,
    order: BodyDiagnosticOrder,
    diagnostic_start: usize,
    warning_start: usize,
    child_start: usize,
    visibility: VisibilityPoint,
}

impl BodyCheckScope {
    /// Borrow G1's current visibility point before taking local tables. Declaration validation and prepared
    /// statement checking enter independently; collected tables and symbols remain their read authority.
    pub(super) fn enter(checker: &mut TypeChecker) -> Self {
        let visibility = checker.check_environment().visibility_point();
        let mut enclosing = BodyLocal::take(checker);
        enclosing.install_shared_inputs(checker);
        enclosing.mutation.enter_statement_frame(&mut checker.mut_params);
        #[cfg(test)]
        checker.body_enclosing_observations.push(enclosing.clone());
        Self {
            enclosing,
            symbol_start: checker.symbols.all_symbols().len(),
            output_start: checker.body_checks.len(),
            order: BodyDiagnosticOrder {
                mutation: checker.check_work.mutation_arguments,
                dictionary: checker.check_work.dictionary_lookups,
            },
            diagnostic_start: checker.errors.len(),
            warning_start: checker.warnings.len(),
            child_start: checker.diagnostic_owner_ranges.len(),
            visibility,
        }
    }

    /// Return the owned body result and restore its enclosing writer, preserving source diagnostic order. Pending
    /// decisions are recorded, not resolved, and the environment cannot advance during this activation.
    pub(super) fn finish(mut self, checker: &mut TypeChecker) -> BodyLocal {
        debug_assert_eq!(self.visibility, checker.check_environment().visibility_point());
        let mut local = BodyLocal::take(checker);
        local.nested = checker
            .symbols
            .nested_declaration_identities_from(self.symbol_start)
            .filter(|(span, _)| {
                !checker.body_checks[self.output_start..]
                    .iter()
                    .any(|child| child.local().nested.contains_key(span))
            })
            .map(|(span, identity)| (span, identity.clone()))
            .collect();
        local.order = self.order;
        let children = &checker.diagnostic_owner_ranges[self.child_start..];
        local.diagnostic_slots = (self.diagnostic_start..checker.errors.len())
            .filter(|index| !children.iter().any(|(errors, _)| errors.contains(index)))
            .collect();
        local.warning_slots = (self.warning_start..checker.warnings.len())
            .filter(|index| !children.iter().any(|(_, warnings)| warnings.contains(index)))
            .collect();
        checker.diagnostic_owner_ranges.push((
            self.diagnostic_start..checker.errors.len(),
            self.warning_start..checker.warnings.len(),
        ));
        local.mutation.return_statement_frame(&mut self.enclosing.mutation);
        local.restore_shared_inputs(&mut self.enclosing);
        self.enclosing.install(checker);
        #[cfg(test)]
        let _ = checker.body_enclosing_observations.pop();
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
            owner: Span::default(),
            kind: LocalOwnerKind::Body,
            order: BodyDiagnosticOrder::default(),
            nested: BTreeMap::new(),
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
            diagnostic_slots: Vec::new(),
            warning_slots: Vec::new(),
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

    /// Append one checked body's facts exactly once. Relocate only the local open-value indices; every map value and
    /// deferred decision otherwise reaches the original settlement owners unchanged. Diagnostics are not facts here:
    /// they already occupy their traversal positions in the writer and are published by settlement.
    fn append(mut self, checker: &mut TypeChecker) {
        checker
            .type_info
            .declarations
            .nested_declaration_identities
            .extend(self.nested);
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
    /// Coordinate a completed function declaration and its independent statement producer. Defaults see generic
    /// binders but no callable parameters; statements consume the exact prepared frame without rechecking it.
    pub(super) fn check_function(&mut self, function: &FunctionDecl, span: Span) {
        let index = self.body_checks.len();
        let prepared =
            self.check_declaration_owner(span, |checker| checker.prepare_function_declaration(function, span));
        self.check_prepared_function(function, span, prepared);
        self.complete_function_declaration(function, span, index);
        #[cfg(test)]
        self.observe_body_local(span);
    }

    /// Run a declaration's actual semantic rules in the same scoped writer used for callable bodies. Constants keep
    /// their const-evaluation owner, statics retain registry initialization rules, and defaults retain declaration
    /// visibility. Reserve the publication position before nested bodies so traversal order remains source order.
    pub(super) fn check_declaration_owner<T>(&mut self, span: Span, check: impl FnOnce(&mut Self) -> T) -> T {
        #[cfg(test)]
        if self.legacy_body_checking {
            return check(self);
        }
        self.check_work.declaration_local_checks += 1;
        let index = self.body_checks.len();
        let scope = BodyCheckScope::enter(self);
        #[cfg(test)]
        self.active_declaration_checks.push(span);
        let prepared = check(self);
        #[cfg(test)]
        let _ = self.active_declaration_checks.pop();
        let mut local = scope.finish(self);
        local.owner = span;
        local.kind = LocalOwnerKind::Declaration;
        self.body_checks.insert(index, BodyCheckInput::Local(local));
        prepared
    }

    /// Append post-body declaration work to its original output without checking the declaration again. The writer
    /// resumes only that owner's facts; child bodies have already returned and retain their own output ownership.
    pub(super) fn continue_declaration_owner(&mut self, index: usize, check: impl FnOnce(&mut Self)) {
        #[cfg(test)]
        if self.legacy_body_checking {
            check(self);
            return;
        }

        // ---- Retain the declaration's prior output ----
        let BodyCheckInput::Local(previous) = &mut self.body_checks[index] else {
            unreachable!("a declaration continuation requires its fresh local producer output");
        };
        let mut previous = std::mem::take(previous);
        let owner = previous.owner;
        let order = previous.order;
        let nested = std::mem::take(&mut previous.nested);
        let mut diagnostics = std::mem::take(&mut previous.diagnostic_slots);
        let mut warnings = std::mem::take(&mut previous.warning_slots);

        // ---- Resume post-body declaration work ----
        let scope = BodyCheckScope::enter(self);
        previous.append(self);
        #[cfg(test)]
        self.active_declaration_checks.push(owner);
        check(self);
        #[cfg(test)]
        let _ = self.active_declaration_checks.pop();

        // ---- Replace the invocation-owned output ----
        let mut completed = scope.finish(self);
        completed.owner = owner;
        completed.kind = LocalOwnerKind::Declaration;
        completed.order = order;
        completed.nested.extend(nested);
        diagnostics.append(&mut completed.diagnostic_slots);
        warnings.append(&mut completed.warning_slots);
        completed.diagnostic_slots = diagnostics;
        completed.warning_slots = warnings;
        self.body_checks[index] = BodyCheckInput::Local(completed);
    }

    /// Check one concrete callable's statements with prepared parameters, receiver, bounds and return context.
    pub(super) fn check_callable_body(
        &mut self,
        body: &[Spanned<Statement>],
        is_async: bool,
        span: Span,
        context: YieldContext,
    ) {
        #[cfg(test)]
        if !self.legacy_body_checking {
            assert!(
                self.active_declaration_checks.is_empty(),
                "statement checking must consume completed declaration inputs"
            );
        }
        self.check_work.body_local_checks += 1;
        #[cfg(test)]
        if self.legacy_body_checking {
            self.check_callable_body_in_frame(body, is_async, context);
            return;
        }
        let scope = BodyCheckScope::enter(self);
        self.check_callable_body_in_frame(body, is_async, context);
        let mut local = scope.finish(self);
        local.owner = span;
        self.body_checks.push(BodyCheckInput::Local(local));
    }

    /// Check one computed property's statements in its own writer, preserving its original synchronous block rules.
    /// Property checking does not acquire callable-only iterator or resource finalization semantics at this boundary.
    pub(super) fn check_property_body(&mut self, body: &[Spanned<Statement>], span: Span) {
        #[cfg(test)]
        if !self.legacy_body_checking {
            assert!(
                self.active_declaration_checks.is_empty(),
                "property checking must consume completed declaration inputs"
            );
        }
        self.check_work.body_local_checks += 1;
        self.check_work.property_bodies += 1;
        #[cfg(test)]
        if self.legacy_body_checking {
            self.check_property_body_in_frame(body);
            return;
        }
        let scope = BodyCheckScope::enter(self);
        self.check_property_body_in_frame(body);
        let mut local = scope.finish(self);
        local.owner = span;
        self.body_checks.push(BodyCheckInput::Local(local));
    }
    /// Apply the property's existing synchronous statement semantics under its prepared receiver and return scope.
    /// Callable-only flow finalization stays outside this property boundary, matching the original checker.
    fn check_property_body_in_frame(&mut self, body: &[Spanned<Statement>]) {
        let previous_async = std::mem::replace(&mut self.in_async_body, false);
        self.check_statement_block(body);
        self.in_async_body = previous_async;
    }
}

#[cfg(test)]
mod tests;

/// A completed local output and its separately ordered settlement diagnostics. Publication consumes this value only
/// after the local checker and the effect producer have finished; it never enters `check_callable_body` again.
/// Body IR combines these local facts with separate declaration and BodyFacts inputs. Later module publication
/// projections, including native-newtype and empty-list facts, are not recreated by this view.
///
/// Every diagnostic the owner produced is stored as an [`AnchoredDiagnostic`]: local refusals and advisories, the
/// final mutation and dictionary decisions, and open-value refusals. Their ranges are structural anchors into the
/// checked syntax, and the current revision's [`DiagnosticPositions`] binds them when they are published.
#[derive(Debug, Clone)]
pub(crate) struct CheckedBodyView {
    local: Arc<BodyLocal>,
    effect_reads: super::mut_arguments::MutEffectReads,
    diagnostics: Vec<AnchoredDiagnostic>,
    warnings: Vec<AnchoredDiagnostic>,
    mutation: Vec<(usize, AnchoredDiagnostic)>,
    dictionary: Vec<(usize, AnchoredDiagnostic)>,
    open: Vec<AnchoredDiagnostic>,
}

impl BodyLocal {
    /// Finalize one complete lexical owner using only its recorded uses, destinations and final callee effects.
    /// The temporary writer exposes the existing unifier, not a second inference implementation or a local check.
    /// The owner's diagnostics are captured as anchored values, and the writer publishes their renderings.
    pub(super) fn finalize(
        mut self,
        checker: &mut TypeChecker,
        effects: &super::mut_arguments::MutEffects,
        positions: &mut DiagnosticPositions<'_>,
    ) -> CheckedBodyView {
        // ---- Complete lexical destinations ----
        let owner = self.owner;
        let kind = self.kind;
        let order = self.order;
        let nested = std::mem::take(&mut self.nested);
        let diagnostic_slots = std::mem::take(&mut self.diagnostic_slots);
        let warning_slots = std::mem::take(&mut self.warning_slots);
        let mut enclosing = Self::take(checker);
        self.install(checker);
        enclosing.install_shared_inputs(checker);
        let start = checker.errors.len();
        checker.settle_body_open_values();
        let open = checker.errors.split_off(start);
        let mut local = Self::take(checker);
        local.restore_shared_inputs(&mut enclosing);
        enclosing.install(checker);
        local.owner = owner;
        local.kind = kind;
        local.order = order;
        local.nested = nested;

        // ---- Stored local diagnostics, published from their values ----
        let diagnostics = diagnostic_slots
            .into_iter()
            .map(|slot| publish_stored(&mut checker.errors[slot], positions))
            .collect();
        let warnings = warning_slots
            .into_iter()
            .map(|slot| publish_stored(&mut checker.warnings[slot], positions))
            .collect();
        let open = open.into_iter().map(|error| positions.store(error)).collect();

        // ---- Recorded caller and dictionary decisions ----
        let mut mutation = Vec::new();
        local.mutation.finalize_arguments(effects, &mut mutation);
        let mutation = store_decisions(mutation, positions);
        let dictionary = local
            .dictionary
            .iter()
            .filter_map(|(order, span, value)| {
                (!local.expressions.read_only_dict_lookups.contains(span)).then(|| {
                    (
                        *order,
                        positions.store(crate::diagnostics::errors::kept_dict_lookup_value_cannot_be_copied(
                            value, *span,
                        )),
                    )
                })
            })
            .collect();
        if local.kind == LocalOwnerKind::Declaration {
            checker.check_work.declaration_finalizations += 1;
        } else {
            checker.check_work.body_finalizations += 1;
        }
        CheckedBodyView {
            effect_reads: local.mutation.effect_reads(effects),
            local: Arc::new(local),
            diagnostics,
            warnings,
            mutation,
            dictionary,
            open,
        }
    }
}

/// Capture one writer entry as its stored value and replace the entry with that value's rendering, so the published
/// diagnostic is the stored one bound to the current revision.
fn publish_stored(slot: &mut CompileError, positions: &mut DiagnosticPositions<'_>) -> AnchoredDiagnostic {
    let value = positions.store(slot.clone());
    *slot = positions.publish(&value);
    value
}

/// Store ordered deferred decisions as anchored values, keeping their traversal ordinals.
fn store_decisions(
    decisions: Vec<(usize, CompileError)>,
    positions: &mut DiagnosticPositions<'_>,
) -> Vec<(usize, AnchoredDiagnostic)> {
    decisions
        .into_iter()
        .map(|(order, error)| (order, positions.store(error)))
        .collect()
}

impl TypeChecker {
    /// Close effects from local projections once, finalize each lexical owner, and join its final facts. Recorded
    /// traversal ordinals restore legacy mutation and dictionary diagnostic order independently of completion order.
    /// `program` is the checked syntax; it binds stored diagnostics to this revision and is never checked again.
    pub(super) fn settle_body_checks(&mut self, program: &Program) {
        let mut positions = DiagnosticPositions::new(program, self.symbols.declaration_origin());
        // ---- Final callee effects ----
        let locals = std::mem::take(&mut self.body_checks);
        let mut inputs = self.mut_params.clone();
        for local in &locals {
            inputs.append_body_local(local.local().mutation.clone());
        }
        let effects = inputs.close_effects();
        #[cfg(test)]
        {
            let original = std::mem::replace(&mut self.mut_params, inputs);
            assert_eq!(effects.changed, self.legacy_mut_param_change_closure());
            self.mut_params = original;
        }
        self.check_work.mutation_settlements += 1;
        self.check_work.dictionary_settlements += 1;
        self.check_work.mutation_components += effects.components;
        self.check_work.mutation_member_visits += effects.members;
        let mut mutation = Vec::new();

        // ---- Declaration-expression owners ----
        let mut module = self.mut_params.take_body_local();
        module.finalize_arguments(&effects, &mut mutation);
        module.clear_pending_arguments();
        self.mut_params.install_body_local(module);
        let mut dictionary = std::mem::take(&mut self.pending_uncopyable_dict_lookups)
            .into_iter()
            .filter_map(|(order, span, value)| {
                (!self.type_info.is_read_only_dict_lookup(span)).then(|| {
                    (
                        order,
                        crate::diagnostics::errors::kept_dict_lookup_value_cannot_be_copied(&value, span),
                    )
                })
            })
            .collect::<Vec<_>>();

        // ---- Completed body publication ----
        for local in locals {
            let order = local.order();
            let checked = local.finalize(self, &effects, &mut positions);
            append_ordered_decisions(
                &mut mutation,
                &checked.mutation,
                (checked.local.order.mutation, order.mutation),
                &mut positions,
            );
            append_ordered_decisions(
                &mut dictionary,
                &checked.dictionary,
                (checked.local.order.dictionary, order.dictionary),
                &mut positions,
            );
            for value in &checked.open {
                self.body_open_diagnostics.push(positions.publish(value));
            }
            // Local diagnostics already occupy their original traversal positions.
            let mut published = checked.local.as_ref().clone();
            published.mutation.clear_pending_arguments();
            published.dictionary.clear();
            published.append(self);
            if checked.local.kind == LocalOwnerKind::Declaration {
                self.checked_declarations.push(checked);
            } else {
                self.checked_bodies.push(checked);
            }
        }
        render_decisions(mutation, &mut self.errors, true);
        render_decisions(dictionary, &mut self.errors, false);
        let counts = positions.capture_counts();
        self.check_work.stored_diagnostics += counts.stored;
        self.check_work.node_relative_anchors += counts.within;
        self.check_work.detached_anchors += counts.detached;
    }
}

/// Render deferred refusals in their original stage/traversal order. Mutation arguments retain their legacy
/// span/message deduplication; dictionary refusals retain each recorded lookup and the catalog's exact wording.
fn render_decisions(mut decisions: Vec<(usize, CompileError)>, errors: &mut Vec<CompileError>, deduplicate: bool) {
    decisions.sort_by_key(|(order, _)| *order);
    for (_, error) in decisions {
        if !deduplicate
            || !errors
                .iter()
                .any(|existing| existing.span == error.span && existing.message == error.message)
        {
            errors.push(error);
        }
    }
}

/// Prepared local work or a previous final value whose local-check inputs the caller has already validated.
/// This adapter owns no cache or scheduler; the engine remains responsible for input applicability and positions.
#[derive(Debug, Clone)]
pub(super) enum BodyCheckInput {
    Local(BodyLocal),
    Reuse {
        checked: Arc<CheckedBodyView>,
        order: BodyDiagnosticOrder,
    },
}

impl BodyCheckInput {
    /// Read the recorded local effect contribution without running checking or finalization.
    fn local(&self) -> &BodyLocal {
        match self {
            Self::Local(local) => local,
            Self::Reuse { checked, .. } => &checked.local,
        }
    }

    /// Reuse an equal completed value or re-finalize only the callee projections this caller subscribed to.
    fn finalize(
        self,
        checker: &mut TypeChecker,
        effects: &super::mut_arguments::MutEffects,
        positions: &mut DiagnosticPositions<'_>,
    ) -> Arc<CheckedBodyView> {
        match self {
            Self::Local(local) => Arc::new(local.finalize(checker, effects, positions)),
            Self::Reuse { checked, .. } => {
                let reads = checked.local.mutation.effect_reads(effects);
                if reads == checked.effect_reads {
                    if checked.local.kind == LocalOwnerKind::Declaration {
                        checker.check_work.declaration_view_reuses += 1;
                    } else {
                        checker.check_work.body_view_reuses += 1;
                    }
                    return checked;
                }
                let mut mutation = Vec::new();
                checked.local.mutation.finalize_arguments(effects, &mut mutation);
                if checked.local.kind == LocalOwnerKind::Declaration {
                    checker.check_work.declaration_finalizations += 1;
                } else {
                    checker.check_work.body_finalizations += 1;
                }
                Arc::new(CheckedBodyView {
                    local: Arc::clone(&checked.local),
                    effect_reads: reads,
                    diagnostics: checked.diagnostics.clone(),
                    warnings: checked.warnings.clone(),
                    mutation: store_decisions(mutation, positions),
                    dictionary: checked.dictionary.clone(),
                    open: checked.open.clone(),
                })
            }
        }
    }
}

impl TypeChecker {
    /// Accept an unchanged declaration/default result whose declaration inputs the caller has already validated.
    /// Its stored diagnostics are bound to the current revision by `positions`. Reusing a statement body alone never
    /// supplies its declaration's checked expressions.
    pub(super) fn reuse_checked_declaration(
        &mut self,
        checked: Arc<CheckedBodyView>,
        positions: &mut DiagnosticPositions<'_>,
    ) -> Result<(), AnchorFault> {
        debug_assert_eq!(checked.local.kind, LocalOwnerKind::Declaration);
        self.reuse_checked_local(checked, positions)
    }

    /// Accept a previous body only after its source, declaration/default/name inputs have been proved applicable by
    /// the caller. A stored diagnostic that does not bind to the current revision refuses the reuse before anything
    /// is published. This is a bootstrap adapter, never an implicit cache lookup by body name.
    pub(super) fn reuse_checked_body(
        &mut self,
        checked: Arc<CheckedBodyView>,
        positions: &mut DiagnosticPositions<'_>,
    ) -> Result<(), AnchorFault> {
        debug_assert_eq!(checked.local.kind, LocalOwnerKind::Body);
        self.reuse_checked_local(checked, positions)
    }

    /// Bind one validated prior output to this activation's traversal order without replaying semantic checks.
    fn reuse_checked_local(
        &mut self,
        checked: Arc<CheckedBodyView>,
        positions: &mut DiagnosticPositions<'_>,
    ) -> Result<(), AnchorFault> {
        let errors = checked
            .diagnostics
            .iter()
            .map(|value| positions.render(value))
            .collect::<Result<Vec<_>, _>>()?;
        let warnings = checked
            .warnings
            .iter()
            .map(|value| positions.render(value))
            .collect::<Result<Vec<_>, _>>()?;
        self.errors.extend(errors);
        self.warnings.extend(warnings);
        let order = BodyDiagnosticOrder {
            mutation: self.check_work.mutation_arguments,
            dictionary: self.check_work.dictionary_lookups,
        };
        self.check_work.mutation_arguments += checked.local.mutation.argument_count();
        self.check_work.dictionary_lookups += checked.local.dictionary.len();
        self.body_checks.push(BodyCheckInput::Reuse { checked, order });
        Ok(())
    }
}

/// Invocation-local traversal binding, separated from the reusable body's semantic decisions.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct BodyDiagnosticOrder {
    mutation: usize,
    dictionary: usize,
}

impl BodyCheckInput {
    /// Read the current traversal binding without changing the reusable checked value.
    fn order(&self) -> BodyDiagnosticOrder {
        match self {
            Self::Local(local) => local.order,
            Self::Reuse { order, .. } => *order,
        }
    }
}

/// Rebind a body's stored decision ordinals from their recorded start to this invocation's start, and bind each
/// stored value to the current revision. Construction guarantees that each stored ordinal is at least its recorded
/// start; source coordinates and producer completion order are irrelevant.
fn append_ordered_decisions(
    target: &mut Vec<(usize, CompileError)>,
    source: &[(usize, AnchoredDiagnostic)],
    (old, current): (usize, usize),
    positions: &mut DiagnosticPositions<'_>,
) {
    target.extend(
        source
            .iter()
            .map(|(order, value)| (current + (order - old), positions.publish(value))),
    );
}

impl TypeChecker {
    /// Read a callable's settled statement result by its revision-local declaration span. A missing result means
    /// this activation has not settled that body or the declaration has no body; this accessor never checks it.
    pub(crate) fn checked_body(&self, declaration: Span) -> Option<&CheckedBodyView> {
        self.checked_bodies
            .iter()
            .find(|body| body.local.owner == declaration)
            .map(AsRef::as_ref)
    }

    /// Read settled declaration/default/initializer facts independently of statement facts. The span belongs to this
    /// checked source revision; callers must use the record adapter's identity and position binding across edits.
    pub(crate) fn checked_declaration(&self, declaration: Span) -> Option<&CheckedBodyView> {
        self.checked_declarations
            .iter()
            .find(|body| body.local.owner == declaration)
            .map(AsRef::as_ref)
    }
}

impl CheckedBodyView {
    /// Revision-local declaration span selecting this immutable local result, never a persistent semantic key.
    pub(crate) fn owner(&self) -> Span {
        self.local.owner
    }

    /// Settled local expression, ownership, destination and control-flow facts. Shared declaration inputs remain with
    /// their own producer and are not recreated or copied into this view.
    pub(crate) fn expressions(&self) -> &ExpressionArtifacts {
        &self.local.expressions
    }

    /// Local canonical references and write-target identities used by Body IR construction.
    pub(crate) fn references(&self) -> &ReferenceArtifacts {
        &self.local.references
    }

    /// Local checked calls, substitutions, argument carriers and constructor decisions.
    pub(crate) fn calls(&self) -> &CallArtifacts {
        &self.local.calls
    }

    /// Local checked iteration, indexing and operator protocol facts.
    pub(crate) fn protocols(&self) -> &ProtocolArtifacts {
        &self.local.protocols
    }

    /// Local Rust-call and coercion facts; shared Rust declaration inputs stay with their declaration views.
    pub(crate) fn rust(&self) -> &RustInteropArtifacts {
        &self.local.rust
    }

    /// Local trait-dispatch and result-carrier decisions, excluding shared trait declarations.
    pub(crate) fn traits(&self) -> &TraitArtifacts {
        &self.local.traits
    }

    /// Local verified C-boundary uses; declared native binding facts remain separate inputs.
    pub(crate) fn c_abi(&self) -> &CAbiInteropArtifacts {
        &self.local.c_abi
    }

    /// Read a checked sum-iterator item carrier owned by this body, without consulting another body or a module map.
    pub(crate) fn iterator_sum_item(&self, expression: Span) -> Option<&ResolvedType> {
        self.local.iterator_sum_items.get(&expression)
    }

    /// Read a nested declaration's canonical identity from this owner's checked lexical facts.
    pub(crate) fn nested_declaration_identity(
        &self,
        declaration: Span,
    ) -> Option<&incan_semantics_core::CanonicalSymbolId> {
        self.local.nested.get(&declaration)
    }

    /// Whether local checking or deferred settlement refused this owner. Successful checking of another owner does
    /// not turn a refused result into usable Body IR input. A false result does not establish whole-module admission;
    /// declaration and module refusals remain independent inputs.
    pub(crate) fn is_refused(&self) -> bool {
        !self.diagnostics.is_empty()
            || !self.mutation.is_empty()
            || !self.dictionary.is_empty()
            || !self.open.is_empty()
    }
}
