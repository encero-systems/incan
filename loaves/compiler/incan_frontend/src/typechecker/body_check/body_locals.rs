//! Taking, installing, finalizing and ordering a body's local check state.

use super::*;

impl BodyLocal {
    /// Detach only body-owned artifact families and pending decisions; declaration, imported metadata and signature
    /// inputs are moved back into the active writer by the scope before any semantic operation runs. The exhaustive
    /// root inventory requires every added artifact to choose local, declaration or final-settlement ownership.
    pub(super) fn take(checker: &mut TypeChecker) -> Self {
        let TypeCheckInfo {
            body_ir_baseline_inputs: _,
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
    pub(super) fn install(self, checker: &mut TypeChecker) {
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
    pub(super) fn install_shared_inputs(&mut self, checker: &mut TypeChecker) {
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
    pub(super) fn restore_shared_inputs(&mut self, enclosing: &mut Self) {
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
    pub(super) fn append(self, checker: &mut TypeChecker) {
        checker
            .type_info
            .declarations
            .nested_declaration_identities
            .extend(self.nested);
        append_local_expressions(&mut checker.type_info.expressions, self.expressions);
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

impl BodyLocal {
    /// Finalize one complete lexical owner using only its recorded uses, destinations and final callee effects.
    /// The temporary writer exposes the existing unifier, not a second inference implementation or a local check.
    /// The owner's diagnostics are captured as anchored values, and the writer publishes their renderings.
    pub(in crate::typechecker) fn finalize(
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
pub(super) fn publish_stored(slot: &mut CompileError, positions: &mut DiagnosticPositions<'_>) -> AnchoredDiagnostic {
    let value = positions.store(slot.clone());
    *slot = positions.publish(&value);
    value
}

/// Store ordered deferred decisions as anchored values, keeping their traversal ordinals.
pub(super) fn store_decisions(
    decisions: Vec<(usize, CompileError)>,
    positions: &mut DiagnosticPositions<'_>,
) -> Vec<(usize, AnchoredDiagnostic)> {
    decisions
        .into_iter()
        .map(|(order, error)| (order, positions.store(error)))
        .collect()
}

impl BodyCheckInput {
    /// Read the recorded local effect contribution without running checking or finalization.
    pub(super) fn local(&self) -> &BodyLocal {
        match self {
            Self::Local(local) => local,
            Self::Reuse { checked, .. } => &checked.local,
        }
    }

    /// Reuse an equal completed value or re-finalize only the callee projections this caller subscribed to.
    pub(super) fn finalize(
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

impl BodyCheckInput {
    /// Read the current traversal binding without changing the reusable checked value.
    pub(super) fn order(&self) -> BodyDiagnosticOrder {
        match self {
            Self::Local(local) => local.order,
            Self::Reuse { order, .. } => *order,
        }
    }
}
