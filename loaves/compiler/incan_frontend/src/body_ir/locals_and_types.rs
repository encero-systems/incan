//! Body builder construction, scopes, checked types, open type settlement, and local declarations.

use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Start a fresh builder for one function body, with no locals, scopes, or accumulated facts yet.
    pub(super) fn new(lowering_facts: &BodyIrLoweringFacts<'type_info, 'source>, owner_return_type: IncanType) -> Self {
        Self {
            native_enum_payload_bindings: HashSet::new(),
            published_methods: lowering_facts.published_methods,
            model_constants: lowering_facts.model_constants,
            local_enum_declarations: lowering_facts.local_enum_declarations,
            published_callable_returns: lowering_facts.published_callable_returns,
            published_callable_defaults: lowering_facts.published_callable_defaults,
            published_newtype_returns: lowering_facts.published_newtype_returns,
            published_storage_types: lowering_facts.published_storage_types,
            type_info: lowering_facts.type_info,
            input_body: Ok(None),
            input_transfer_subjects: std::collections::BTreeSet::new(),
            input_paths_checked: false,
            input_owned_initializers: Vec::new(),
            input_local_bindings: Vec::new(),
            returned_view: None,
            retained_result_source: None,
            carried_view_places: HashMap::new(),
            awaited_view_places: HashMap::new(),
            carried_view_locals: HashMap::new(),
            aggregate_view_locals: HashSet::new(),
            aggregate_view_temporary_dependencies: HashMap::new(),
            retained_statement_temporaries: HashSet::new(),
            input_if_scopes: Vec::new(),
            input_loops: Vec::new(),
            runtime_type_conflicts: lowering_facts.runtime_type_conflicts,
            retain_nominal_carriers: lowering_facts.retain_nominal_carriers,
            function_default_sources: lowering_facts.function_default_sources,
            local_function_declarations: lowering_facts.local_function_declarations,
            local_nongeneric_functions: lowering_facts.local_nongeneric_functions,
            local_receiver_methods: lowering_facts.local_receiver_methods,
            local_trait_partials: lowering_facts.local_trait_partials,
            local_nominal_declarations: lowering_facts.local_nominal_declarations,
            local_fieldless_enum_declarations: lowering_facts.local_fieldless_enum_declarations,
            local_value_enum_declarations: lowering_facts.local_value_enum_declarations,
            published_constants: lowering_facts.published_constants,
            module_identity: lowering_facts.module_identity,
            registry_package: lowering_facts.registry_package,
            provider_operations: lowering_facts.provider_operations,
            owner_return_type,
            type_parameters: Vec::new(),
            newtype_implementation: None,
            locals: Vec::new(),
            scopes: Vec::new(),
            bindings: HashMap::new(),
            identity_bindings: HashMap::new(),
            input_aliases: HashMap::new(),
            static_aliases: HashMap::new(),
            static_binding_locals: HashSet::new(),
            external_locals: HashMap::new(),
            remaining_reads: HashMap::new(),
            pending_argument_reads: Vec::new(),
            loop_scopes: Vec::new(),
            conditional_scopes: Vec::new(),
            terminal_paths: Vec::new(),
            replacing_local: None,
            function_pointer_locals: Vec::new(),
            closure_holding_locals: Vec::new(),
            moved_out: HashSet::new(),
            borrowed_parameters: HashSet::new(),
            owned_parameters: HashSet::new(),
            materialized_range_locals: HashSet::new(),
            loop_break_targets: Vec::new(),
            loop_continue_actions: Vec::new(),
            runtime_requirements: Vec::new(),
            panic_facts: Vec::new(),
            type_overrides: HashMap::new(),
            contextual_type_identities: std::collections::BTreeMap::new(),
            shared_method_chain: false,
            in_place_pattern: false,
            settled_types: HashMap::new(),
            coercions_in_progress: HashSet::new(),
            next_local: 0,
            next_scope: 0,
        }
    }

    // ---- Scopes and locals ----

    /// Restore source-name bindings after a nested scope, in the order their previous values were saved.
    ///
    /// Pattern and closure locals remain in the local inventory; only their source-name visibility ends here.
    pub(super) fn restore_local_bindings(&mut self, saved: Vec<(String, Option<bir::LocalId>)>) {
        for (name, previous) in saved {
            match previous {
                Some(local) => {
                    self.bindings.insert(name, local);
                }
                None => {
                    self.bindings.remove(&name);
                }
            }
        }
    }

    /// Allocate a fresh lexical scope with the given `parent`, recording it in `scopes` for later span lookup.
    pub(super) fn new_scope(&mut self, parent: Option<bir::ScopeId>, span: HirSourceSpan) -> bir::ScopeId {
        let id = bir::ScopeId(self.next_scope);
        self.next_scope += 1;
        self.scopes.push(bir::ScopeInfo { id, parent, span });
        id
    }

    /// Look up the source span recorded for `scope`, or a zero-width span if the id is unknown (defensive default;
    /// every scope this builder hands out is always recorded in `scopes` first).
    pub(super) fn scope_span(&self, scope: bir::ScopeId) -> HirSourceSpan {
        self.scopes
            .iter()
            .find(|info| info.id == scope)
            .map(|info| info.span)
            .unwrap_or(HirSourceSpan::new(0, 0))
    }

    /// Retain the checker's concrete executable constructor/call type when an open caller context masks it; otherwise
    /// use the contextual expression type. Canonical Result reads keep their retained channels; missing facts remain
    /// [`IncanType::Unknown`], never guessed from source names.
    /// Standard-library Self calls use their additive declared-result fact before the legacy expression type, so
    /// enclosing operators can validate the same type that the call itself retains.
    /// Canonical local Result reads retain closed constructor channels without replacing fixed contextual channels.
    /// While one evaluation of a chained value written per target is lowered, that target's types for the value's
    /// nodes come first (see [`Self::type_overrides`]), then the settled type of a `Result` constructor whose open
    /// side its destination fixes (see [`Self::settled_types`]).
    /// A checked lexical read whose contextual type remains caller-open takes its active local's retained type by
    /// full canonical binding identity. This carries a concrete matched payload through the arm without changing
    /// legacy expression facts or resolving a variable by spelling.
    pub(super) fn resolve_ty(&self, span: ast::Span) -> IncanType {
        if let Some(ty) = checked_bridge::constructor_storage(span, self.type_info) {
            return ty;
        }
        if let Some(ty) = self.type_overrides.get(&span) {
            return ty.clone();
        }
        if let Some(ty) = self.settled_types.get(&span) {
            return ty.clone();
        }
        let native = self.type_info.body_ir_newtypes.expression_type(span, self.type_info);
        let checked = native
            .or_else(|| self.type_info.expressions.body_ir_expr_types.get(&span))
            .or_else(|| self.type_info.calls.body_ir_constructor_types.get(&span))
            .or_else(|| self.type_info.calls.body_ir_call_result_types.get(&span))
            .or_else(|| self.type_info.calls.inferred_constructor_types.get(&span))
            .or_else(|| self.type_info.calls.inferred_call_result_types.get(&span))
            .or_else(|| self.type_info.traits.self_method_result_types.get(&span))
            .or_else(|| self.type_info.expr_type(span))
            .or_else(|| self.type_info.calls.body_ir_native_argument_types.get(&span))
            .filter(|ty| native.is_some() || !self.type_info.body_ir_newtypes.controls_type(ty))
            .or_else(|| self.type_info.expr_type(span))
            .map(|ty| self.checked_type(ty))
            .unwrap_or(IncanType::Unknown);
        if let Some(identity) = self.type_info.resolved_identity(span)
            && let Some(local) = self
                .bindings
                .values()
                .filter(|id| self.native_enum_payload_bindings.contains(id))
                .filter_map(|id| self.locals.get(id.index()))
                .find(|local| local.identity.as_ref() == Some(identity))
        {
            return local.ty.clone();
        }
        let checked = self.retained_result_read_type(span, checked);
        if self.leaves_type_open(&checked)
            && let Some(identity) = self.type_info.resolved_identity(span)
            && let Some(local) = self
                .bindings
                .values()
                .filter_map(|id| self.locals.get(id.index()))
                .find(|local| local.identity.as_ref() == Some(identity) && !self.leaves_type_open(&local.ty))
        {
            return local.ty.clone();
        }
        checked
    }

    /// Whether `ty` leaves a part unresolved in this body: an unknown or still-inferred type, or a type parameter the
    /// body does not declare, which checking left uninstantiated where a call's result was used.
    pub(super) fn leaves_type_open(&self, ty: &IncanType) -> bool {
        match ty {
            IncanType::Unknown | IncanType::Infer => true,
            // An imported native type has a qualified owner. The SDK's unresolved bare T is a placeholder,
            // and must yield to the closed checked carrier that contains it rather than masquerade as a type.
            IncanType::RustInteropPath(name) if name == "T" => !self.type_parameters.contains(name),
            IncanType::TypeVar(name) => !self.type_parameters.contains(name),
            IncanType::Generic { args, .. } | IncanType::Tuple(args) => {
                args.iter().any(|arg| self.leaves_type_open(arg))
            }
            IncanType::Ref(inner) | IncanType::RefMut(inner) | IncanType::NativeArray { element: inner, .. } => {
                self.leaves_type_open(inner)
            }
            IncanType::Function { params, return_type } => {
                params.iter().any(|param| self.leaves_type_open(&param.ty)) || self.leaves_type_open(return_type)
            }
            _ => false,
        }
    }

    /// Retain, for an `Ok(...)`, `Err(...)` or authenticated `Some(...)` value and each list, set, dict or tuple
    /// literal holding one, the type its destination settled for the side the constructor leaves open (#1561), and
    /// the destination's union for a side the constructor fixes as one of that union's members.
    ///
    /// The checker settles such a side where the value flows: a generic call's argument takes the parameter type the
    /// call's other arguments or the enclosing function's `Result` side fix, a closure's result takes the closure's
    /// settled return type, and a list comprehension's element the comprehension's element type. It records the
    /// settled type on the destination but keeps the open side on the constructor's own span. Body IR builds the
    /// constructor with the settled type instead, through [`Self::settled_types`]. An open destination, a value of
    /// another shape, and a part whose shape differs from the destination's settle nothing.
    pub(super) fn settle_open_result_value(&mut self, value: &ast::Spanned<ast::Expr>, destination: &IncanType) {
        if self.leaves_type_open(destination) {
            return;
        }
        if self.settle_enum_result_value(value, destination) {
            return;
        }
        match &value.node {
            ast::Expr::Paren(inner) => self.settle_open_result_value(inner, destination),
            ast::Expr::Call(callee, type_args, args)
                if type_args.is_empty()
                    && args.len() == 1
                    && (self.intrinsic_result_constructor(callee, value.span).is_some()
                        || (self.intrinsic_option_constructor(callee)
                            && destination.collection_arguments(CollectionTypeId::Option).is_some())) =>
            {
                self.settle_open_value_type(value.span, destination);
            }
            ast::Expr::List(entries) => {
                let Some([member]) = destination.collection_arguments(CollectionTypeId::List) else {
                    return;
                };
                for entry in entries {
                    if let ast::ListEntry::Element(element) = entry {
                        self.settle_open_result_value(element, member);
                    }
                }
                self.settle_open_value_type(value.span, destination);
            }
            ast::Expr::Set(elements) => {
                let Some([member]) = destination.collection_arguments(CollectionTypeId::Set) else {
                    return;
                };
                for element in elements {
                    self.settle_open_result_value(element, member);
                }
                self.settle_open_value_type(value.span, destination);
            }
            ast::Expr::Dict(entries) => {
                let Some([key_ty, value_ty]) = destination.collection_arguments(CollectionTypeId::Dict) else {
                    return;
                };
                for entry in entries {
                    if let ast::DictEntry::Pair(key, member) = entry {
                        self.settle_open_result_value(key, key_ty);
                        self.settle_open_result_value(member, value_ty);
                    }
                }
                self.settle_open_value_type(value.span, destination);
            }
            ast::Expr::Tuple(items) => {
                let IncanType::Tuple(item_types) = destination else {
                    return;
                };
                if items.len() != item_types.len() {
                    return;
                }
                for (item, item_ty) in items.iter().zip(item_types) {
                    self.settle_open_result_value(item, item_ty);
                }
                self.settle_open_value_type(value.span, destination);
            }
            _ => {}
        }
    }

    /// Retain `destination`'s parts for the parts the type recorded at `span` leaves open, and its unions for the
    /// members `span` fixes in their place, when the result is closed (see [`Self::settle_open_result_value`] and
    /// [`Self::fill_open_type_parts`]). A span with no recorded type, or one the destination changes nothing in, keeps
    /// what it has.
    pub(super) fn settle_open_value_type(&mut self, span: ast::Span, destination: &IncanType) {
        let recorded = self.resolve_ty(span);
        if matches!(recorded, IncanType::Unknown) {
            return;
        }
        let settled = self.fill_open_type_parts(&recorded, destination);
        if settled != recorded && !self.leaves_type_open(&settled) {
            self.settled_types.insert(span, settled);
        }
    }

    /// The canonical identities of a constructed `Result`'s named leaves when its sides were settled at a destination
    /// (see [`Self::settle_open_result_value`]): the checker's identity at a path it recorded, otherwise the module
    /// binding the leaf names. A leaf with neither stays absent, so the construction refuses its payload identity
    /// rather than guessing one.
    pub(crate) fn settled_result_identities(
        &self,
        result_ty: &IncanType,
        recorded: &std::collections::BTreeMap<Vec<usize>, CanonicalSymbolId>,
    ) -> std::collections::BTreeMap<Vec<usize>, CanonicalSymbolId> {
        let mut leaves = Vec::new();
        if let IncanType::Generic { args, .. } = result_ty {
            for (index, side) in args.iter().enumerate() {
                named_leaf_paths(side, &mut vec![index], &mut leaves);
            }
        }
        leaves
            .into_iter()
            .filter_map(|(path, leaf)| {
                let identity = match leaf {
                    RetainedTypeLeaf::Named(name) => recorded
                        .get(&path)
                        .or_else(|| parameter_types::retained_nominal_identity(&name, self.type_info)),
                    RetainedTypeLeaf::NativeBuiltin(builtin) => recorded.get(&path).or_else(|| {
                        self.type_info
                            .body_ir_runtime_types
                            .get(builtin.binding_name())
                            .filter(|identity| {
                                incan_semantics_core::RuntimeBuiltinType::from_identity(identity).as_ref()
                                    == Some(&builtin)
                            })
                    }),
                };
                identity.cloned().map(|identity| (path, identity))
            })
            .collect()
    }

    /// Fill each part of `recorded` this body leaves open with the matching part of `settled`, keeping every part
    /// `recorded` fixes and every part whose shape differs from `settled`'s.
    ///
    /// A fixed part that is one member of the union `settled` declares in its place takes that union: legacy builds
    /// `Ok("fine")` for a `Result[int | str, str]` destination as the union's `str` member.
    pub(super) fn fill_open_type_parts(&self, recorded: &IncanType, settled: &IncanType) -> IncanType {
        match (recorded, settled) {
            (
                IncanType::NativeArray { element, length },
                IncanType::NativeArray {
                    element: settled_element,
                    length: settled_length,
                },
            ) if length == settled_length => IncanType::NativeArray {
                element: Box::new(self.fill_open_type_parts(element, settled_element)),
                length: *length,
            },
            (IncanType::Unknown | IncanType::Infer, _) => settled.clone(),
            (IncanType::TypeVar(name), _) if !self.type_parameters.contains(name) => settled.clone(),
            (_, _)
                if recorded != settled
                    && settled
                        .union_members()
                        .is_some_and(|members| members.contains(recorded)) =>
            {
                settled.clone()
            }
            (
                IncanType::Generic { base, args },
                IncanType::Generic {
                    base: settled_base,
                    args: settled_args,
                },
            ) if base == settled_base && args.len() == settled_args.len() => IncanType::Generic {
                base: base.clone(),
                args: args
                    .iter()
                    .zip(settled_args)
                    .map(|(arg, settled_arg)| self.fill_open_type_parts(arg, settled_arg))
                    .collect(),
            },
            (IncanType::Tuple(items), IncanType::Tuple(settled_items)) if items.len() == settled_items.len() => {
                IncanType::Tuple(
                    items
                        .iter()
                        .zip(settled_items)
                        .map(|(item, settled_item)| self.fill_open_type_parts(item, settled_item))
                        .collect(),
                )
            }
            _ => recorded.clone(),
        }
    }

    /// Declare a new user-facing local (parameter or source binding), seeding its last-use countdown from the
    /// number of `Ident` reads of `name` found in `remaining` (the declaring block's statement suffix, or a loop
    /// body for per-iteration bindings). Defaults to [`bir::LocalOrigin::UserBinding`]; callers that declare a
    /// parameter overwrite the origin afterward.
    pub(super) fn declare_new_local(
        &mut self,
        name: String,
        ty: IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        remaining: &[ast::Spanned<ast::Statement>],
    ) -> bir::LocalId {
        let total_reads = count_reads_in_stmts(&name, remaining);
        self.declare_new_local_with_reads(name, ty, scope, span, total_reads)
    }

    /// Declare a new user-facing local with an already-computed last-use countdown, for declaration sites whose
    /// "remaining reads" context is not a plain statement suffix -- currently only comprehension/generator `for`
    /// clause bindings (see `Self::lower_comprehension_clauses`), whose remaining context is a tail of
    /// [`ast::ComprehensionClause`]s plus a terminal element/key/value expression, not [`ast::Statement`]s.
    /// [`Self::declare_new_local`] is a thin wrapper over this that seeds `total_reads` from a statement suffix via
    /// [`count_reads_in_stmts`].
    pub(super) fn declare_new_local_with_reads(
        &mut self,
        name: String,
        ty: IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        total_reads: usize,
    ) -> bir::LocalId {
        let source_span = span;
        let identity = self.type_info.resolved_write_identity(source_span, &name).cloned();
        let id = bir::LocalId(self.next_local);
        self.next_local += 1;
        self.locals.push(bir::LocalDecl {
            id,
            name: Some(name.clone()),
            identity: identity.clone(),
            ty,
            origin: bir::LocalOrigin::UserBinding,
            scope,
            span,
        });
        self.bindings.insert(name, id);
        if let Some(identity) = identity {
            self.identity_bindings.insert(identity, id);
        }
        self.remaining_reads.insert(id, ReadCountdown::new(total_reads));
        id
    }

    /// Declare a method's `self`/`mut self` receiver as a [`bir::LocalOrigin::Receiver`] local, bound under the
    /// name `"self"` in [`Self::bindings`] exactly like an ordinary local so [`Self::place_for_name`] resolves
    /// `self` reads without a separate lookup path.
    ///
    /// Unlike [`Self::declare_new_local`], no last-use countdown is seeded: a receiver is always a Rust-level
    /// reference (`&self`/`&mut self`), so nothing about it can be "used up" the way an owned local's remaining
    /// reads can — see the receiver carve-out in [`Self::ownership_fact_for_place`], which decides the ownership
    /// fact for every `self` read before that countdown would ever be consulted. Computed getters use their separate
    /// Body IR-only receiver provenance when the legacy write table has no implicit receiver entry.
    pub(super) fn declare_receiver_local(
        &mut self,
        ty: IncanType,
        mutable: bool,
        scope: bir::ScopeId,
        span: HirSourceSpan,
    ) -> bir::LocalId {
        let source_span = span;
        let identity = self
            .type_info
            .resolved_write_identity(source_span, "self")
            .cloned()
            .or_else(|| {
                self.type_info
                    .declarations
                    .body_ir_property_bindings
                    .get(&source_span)
                    .and_then(|binding| binding.receiver_identity.clone())
            });
        let id = bir::LocalId(self.next_local);
        self.next_local += 1;
        self.locals.push(bir::LocalDecl {
            id,
            name: Some("self".to_string()),
            identity: identity.clone(),
            ty,
            origin: bir::LocalOrigin::Receiver { mutable },
            scope,
            span,
        });
        self.bindings.insert("self".to_string(), id);
        if let Some(identity) = identity {
            self.identity_bindings.insert(identity, id);
        }
        id
    }

    /// Allocate a compiler-introduced temporary. Flattened expression temporaries are consumed once and omitted from
    /// last-use tracking and scope-exit drop insertion. A normalized construct that reuses a value must explicitly seed
    /// its read count and consume its final owner, as chained assignment does; loop state carries explicit poll and
    /// cursor reads instead. See [`Self::temp_operand`] and [`Self::insert_scope_drops`].
    pub(super) fn new_temp(&mut self, ty: IncanType, scope: bir::ScopeId, span: HirSourceSpan) -> bir::LocalId {
        let id = bir::LocalId(self.next_local);
        self.next_local += 1;
        self.locals.push(bir::LocalDecl {
            id,
            name: None,
            identity: None,
            ty,
            origin: bir::LocalOrigin::Temporary,
            scope,
            span,
        });
        id
    }
}
