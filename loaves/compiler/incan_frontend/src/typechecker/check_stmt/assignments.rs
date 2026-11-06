//! Assignment statements and the bindings they declare, refine or write.

use super::*;

impl TypeChecker {
    /// Validate an assignment statement, then remember a local bound directly to a module static (see
    /// [`Self::note_static_alias_binding`]).
    ///
    /// A discard checks its value without introducing a symbol or retaining binding facts. A new binding of a
    /// capturing callable takes the callable's own type, and a reassignment of one, or to one, is refused (see
    /// `capturing_callables.rs`).
    pub(super) fn check_assignment(&mut self, assign: &AssignmentStmt, span: Span) {
        if assign.name == "_" {
            self.check_discard_assignment(assign, span);
            return;
        }
        self.check_assignment_binding(assign, span);
        self.refuse_owned_native_tuple_field(assign);
        self.note_static_alias_binding(assign);
        let declares = self
            .lookup_symbol(&assign.name)
            .is_some_and(|symbol| symbol.span == assign.name_span);
        if declares {
            self.note_capturing_binding(&assign.value, assign.name_span, span);
        } else {
            self.refuse_capturing_reassignment(Some(&assign.name), &assign.value, assign.name_span);
        }
    }

    /// Check a discarded value and its optional annotation without making `_` a readable or writable binding.
    /// Each discard owns its type independently; open Rust generics must resolve here because no later read can
    /// constrain them. Retain the value's checked carrier for Body IR, but no local ownership or alias facts.
    pub(super) fn check_discard_assignment(&mut self, assign: &AssignmentStmt, span: Span) {
        let annotated_ty = assign
            .ty
            .as_ref()
            .map(|annotation| self.resolve_type_checked(annotation));
        let value_ty = self.check_expr_with_expected(&assign.value, annotated_ty.as_ref());
        let ty = self.new_assignment_binding_type(assign, annotated_ty, value_ty);
        self.record_assignment_binding_type(span, ty);
        self.reject_discarded_open_rust_generic_call(assign.value.span);
    }

    /// Validate assignment statements, including declarations, reassignments, and local annotation compatibility.
    ///
    /// This is the frontend boundary for rejecting unsupported local type annotations before lowering. In particular,
    /// trait-typed locals must not proceed to codegen because Rust has no valid bare trait type for `let` annotations.
    pub(super) fn check_assignment_binding(&mut self, assign: &AssignmentStmt, span: Span) {
        let target_span = assign.name_span;
        let new_local = Self::binding_introduces_name(assign.binding)
            || (self.lookup_variable_info_in_scope_chain(&assign.name).is_none()
                && self.lookup_static_info(&assign.name).is_none());
        let defer_local = new_local
            && self.current_body_identity().is_some_and(|owner| {
                matches!(
                    owner.kind,
                    incan_semantics_core::SemanticSourceTargetKind::Function
                        | incan_semantics_core::SemanticSourceTargetKind::Method
                )
            });
        let annotated_ty = assign
            .ty
            .as_ref()
            .map(|ty_ann| self.resolve_type_checked_for_input(ty_ann, defer_local));
        let explicit_mode = assign
            .ty
            .as_ref()
            .zip(annotated_ty.as_ref())
            .and_then(|(annotation, ty)| self.explicit_root_contract(&annotation.node, ty));
        let annotated_ty = explicit_mode
            .as_ref()
            .map(|mode| mode.underlying.clone())
            .or(annotated_ty);
        // `let` and `mut` are declaration forms: they introduce a binding that may deliberately shadow an active
        // outer one, so they never resolve against the scope chain. Only a bare `x = value` asks "does this name
        // already exist somewhere out there", and the two halves have to move together — walking outward without
        // this branch would turn every in-block `let` into a reassignment of the outer binding (#1072).
        let introduces_binding = Self::binding_introduces_name(assign.binding);
        if !introduces_binding {
            self.refuse_iterated_place_replacement(
                &Spanned::new(Expr::Ident(assign.name.clone()), target_span),
                None,
                target_span,
            );
            self.record_write_target_identity(target_span, &assign.name);
        }
        let reassignment_ty = (!introduces_binding)
            .then(|| {
                self.lookup_variable_info_in_scope_chain(&assign.name)
                    .map(|var_info| var_info.ty.clone())
            })
            .flatten();
        let value_ty = if let Some(var_ty) = reassignment_ty.as_ref() {
            self.check_expr_with_expected(&assign.value, Some(var_ty))
        } else {
            self.check_expr_with_expected(&assign.value, annotated_ty.as_ref())
        };
        self.refuse_mut_param_held_by(&assign.value);

        // A `const` is registered as a module-scope variable, so the scope-chain walk below finds it. Answer the
        // more specific question first: reassigning a const is not a mutability mistake to be fixed with `mut`, it
        // is a request for a `static`, and the generic "variable is immutable" error would send the reader the
        // wrong way. A `let`/`mut` declaration that shadows a const is a new binding and never reaches here.
        if !introduces_binding && self.active_binding_is_const(&assign.name) {
            self.errors
                .push(errors::const_reassignment_suggests_static(&assign.name, target_span));
            return;
        }

        // Check if it's a re-assignment
        if let Some(var_info) = (!introduces_binding)
            .then(|| self.lookup_variable_info_in_scope_chain(&assign.name))
            .flatten()
        {
            let is_mutable = var_info.is_mutable;
            let var_ty = var_info.ty.clone();

            if !is_mutable {
                self.errors
                    .push(errors::mutation_without_mut(&assign.name, target_span));
            } else {
                self.refuse_caller_visible_mut_param_rebinding(&assign.name, target_span);
            }
            self.note_local_reassignment(&assign.name);
            if !self.types_compatible(&value_ty, &var_ty) {
                self.errors.push(errors::assignment_type_mismatch(
                    &assign.name,
                    &var_ty.to_string(),
                    &value_ty.to_string(),
                    assign.value.span,
                ));
            }
            self.consumed_iterator_bindings.remove(&assign.name);
            self.transferred_c_resource_bindings.remove(&assign.name);
            self.note_list_reassignment(&assign.name, span);
            self.mark_open_rust_generic_binding_read(&assign.name, assign.name_span);
            return;
        }

        if let Some(static_info) = self.lookup_static_info(&assign.name) {
            if static_info.is_imported {
                self.errors.push(errors::imported_static_reassignment_not_allowed(
                    &assign.name,
                    target_span,
                ));
                return;
            }
            let static_ty = static_info.ty.clone();
            let value_ty = self.check_expr_with_expected(&assign.value, Some(&static_ty));
            if !self.types_compatible(&value_ty, &static_ty) {
                self.errors.push(errors::type_mismatch(
                    &static_ty.to_string(),
                    &value_ty.to_string(),
                    assign.value.span,
                ));
            }
            return;
        }

        if explicit_mode.is_none() {
            self.warn_place_binding_migration(assign);
        }

        // New binding
        let is_mutable = matches!(assign.binding, BindingKind::Mutable)
            && explicit_mode
                .as_ref()
                .is_none_or(|mode| mode.mode != incan_lang::lang::types::ownership::OwnershipMode::Borrowed);

        if is_mutable {
            self.mutable_bindings.insert(assign.name.clone());
        }

        let ty = self.new_assignment_binding_type(assign, annotated_ty, value_ty);
        self.declare_assignment_binding(assign, ty, is_mutable, introduces_binding, span);
        self.record_explicit_local_mode(assign, explicit_mode, is_mutable);
    }

    /// Preserve the requested mode on this exact newly declared local and callable. Capturing it before lookup is
    /// changed prevents a binding named like a prelude mode from rewriting its own or another body's contract.
    pub(super) fn record_explicit_local_mode(
        &mut self,
        assign: &AssignmentStmt,
        explicit_mode: Option<crate::ownership_metadata::AccessMode<ResolvedType>>,
        is_mutable: bool,
    ) {
        if let Some(mode) = explicit_mode
            && let Some(annotation) = &assign.ty
            && let Some(identity) = self.type_info.resolved_write_identity(assign.name_span, &assign.name)
            && identity.declaration_span == assign.name_span
            && let Some(owner) = self.current_body_identity().cloned()
        {
            self.type_info.protocols.explicit_local_modes.insert(
                identity.clone(),
                super::ownership_contracts::LocalMode {
                    owner,
                    mode: mode.mode,
                    value_type: mode.underlying,
                    writable: is_mutable,
                    annotation: annotation.span,
                },
            );
        }
    }

    /// Settle a new local's declared or inferred carrier after its value has checked. Unsupported trait annotations
    /// still report their source error while retaining the value type; inferred constructor sides keep their facts.
    pub(super) fn new_assignment_binding_type(
        &mut self,
        assign: &AssignmentStmt,
        annotated_ty: Option<ResolvedType>,
        value_ty: ResolvedType,
    ) -> ResolvedType {
        if let Some(ty_ann) = &assign.ty {
            let ann_ty = annotated_ty.unwrap_or_else(|| self.resolve_type_checked(ty_ann));
            let trait_typed_local = self.is_trait_typed_local_annotation(&ann_ty);
            if trait_typed_local {
                self.errors.push(errors::trait_typed_local_annotation_unsupported(
                    &ann_ty.to_string(),
                    ty_ann.span,
                ));
            } else {
                self.refuse_trait_type_nested_in_annotation(&ann_ty, ty_ann.span);
            }
            // Check value matches annotation
            if !self.types_compatible(&value_ty, &ann_ty)
                && !self.record_validated_newtype_coercion_if_possible(&value_ty, &ann_ty, assign.value.span)
            {
                self.errors.push(errors::assignment_type_mismatch(
                    &assign.name,
                    &ann_ty.to_string(),
                    &value_ty.to_string(),
                    assign.value.span,
                ));
            }
            if trait_typed_local { value_ty } else { ann_ty }
        } else {
            let settled = self.settle_open_constructor_side(&assign.value, value_ty.clone());
            if settled != value_ty {
                self.record_settled_branch_values(&assign.value, &settled);
            }
            settled
        }
    }

    /// Publish a newly checked local binding before retaining foreign and callable carrier facts. Annotation-free
    /// bindings keep their open Rust generic watch; explicit shadows preserve the declaration's source identity.
    pub(super) fn declare_assignment_binding(
        &mut self,
        assign: &AssignmentStmt,
        ty: ResolvedType,
        is_mutable: bool,
        introduces_binding: bool,
        span: Span,
    ) {
        let target_span = assign.name_span;
        self.record_assignment_binding_type(span, ty.clone());
        self.validate_protected_builtin_binding(&assign.name, span);
        let symbol = Symbol {
            name: assign.name.clone(),
            kind: SymbolKind::Variable(VariableInfo {
                ty,
                is_mutable,
                is_used: false,
            }),
            span: target_span,
            scope: 0,
        };
        if introduces_binding {
            self.symbols.define_explicit_shadow(symbol);
        } else {
            self.symbols.define(symbol);
        }
        self.record_read_only_binding(is_mutable, target_span);
        self.record_write_target_identity(target_span, &assign.name);
        self.retain_rust_value_binding(&assign.name, &assign.value);
        self.retain_body_ir_assignment_value(assign);
        self.note_function_value_binding(target_span, &assign.value);
        self.bind_c_abi_output_slot_assignment(&assign.name, assign.value.span);
        self.bind_c_abi_span_assignment(&assign.name, assign.value.span);
        self.bind_c_abi_raw_result_assignment(&assign.name, assign.value.span);
        self.bind_scoped_c_string_view_assignment(target_span, assign.value.span);
        if assign.ty.is_none() {
            // An annotation is itself what fixes the arguments; only an unannotated binding is watched (#1720).
            self.bind_open_rust_generic_assignment(assign, span);
        }
        self.consumed_iterator_bindings.remove(&assign.name);
        self.transferred_c_resource_bindings.remove(&assign.name);
    }

    /// Require retained ownership evidence for a foreign tuple field bound by value. Explicit references keep
    /// the existing loan representation; unsupported owned bindings refuse before Body IR requests Clone.
    pub(super) fn refuse_owned_native_tuple_field(&mut self, assign: &AssignmentStmt) {
        if !self.is_tuple_field_read(&assign.value) {
            return;
        }
        let Some(target) = self.lookup_variable_info_in_scope_chain(&assign.name) else {
            return;
        };
        if matches!(target.ty, ResolvedType::Ref(_) | ResolvedType::RefMut(_)) {
            return;
        }
        let Some(value) = self.type_info.expr_type(assign.value.span) else {
            return;
        };
        if self.native_field_clone_candidate(value) == Some(false)
            && !matches!(self.derive_support(value, DeriveId::Clone), DeriveSupport::Supported)
        {
            self.errors.push(errors::native_tuple_field_copy_not_retained(
                &assign.name,
                &value.to_string(),
                assign.value.span,
            ));
        }
    }

    /// Recognize checked tuple projections through either field spelling or literal indexing and transparent
    /// parentheses. The base's checked storage distinguishes a tuple read from a computed property or user indexer.
    pub(super) fn is_tuple_field_read(&self, value: &Spanned<Expr>) -> bool {
        match &value.node {
            Expr::Paren(inner) => self.is_tuple_field_read(inner),
            Expr::Field(base, _) | Expr::Index(base, _) => self
                .type_info
                .expr_type(base.span)
                .is_some_and(|ty| self.has_tuple_storage(ty)),
            _ => false,
        }
    }

    /// Normalize the checker's tuple annotation and reference forms through the same collection registry as ordinary
    /// tuple access. Alias expansion remains in the accepting checker, never in a downstream ownership consumer.
    pub(super) fn has_tuple_storage(&self, ty: &ResolvedType) -> bool {
        match self.expand_type_aliases(ty.clone()) {
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => self.has_tuple_storage(&inner),
            ResolvedType::Tuple(_) => true,
            ResolvedType::Generic(name, _) => collection_type_id(&name) == Some(CollectionTypeId::Tuple),
            _ => false,
        }
    }

    /// Whether this spelling deliberately declares a new source binding instead of resolving an active one.
    pub(super) const fn binding_introduces_name(binding: BindingKind) -> bool {
        matches!(binding, BindingKind::Let | BindingKind::Mutable)
    }

    /// Whether the binding currently selected by lexical lookup is the collected module `const`.
    ///
    /// `const_decls` is keyed by spelling for constant evaluation, so consulting it alone mistakes an inner `mut NAME`
    /// for the outer `const NAME`. Pairing it with the active symbol's declaration span preserves normal lexical
    /// shadowing while retaining the dedicated const-reassignment diagnostic for the const itself.
    pub(super) fn active_binding_is_const(&self, name: &str) -> bool {
        let Some((_, const_span)) = self.const_decls.get(name) else {
            return false;
        };
        self.symbols
            .lookup(name)
            .and_then(|symbol_id| self.symbols.get(symbol_id))
            .is_some_and(|symbol| symbol.span == *const_span)
    }

    /// Check or introduce one target from a tuple-unpack or chained source assignment.
    ///
    /// This is intentionally the unannotated half of [`Self::check_assignment`]: all three spellings share the
    /// binding-form decision, but only a single assignment owns a local type annotation and C-ABI constructor facts.
    pub(super) fn check_unannotated_assignment_target(
        &mut self,
        name: &str,
        binding: BindingKind,
        value_ty: ResolvedType,
        target_span: Span,
        value_span: Span,
    ) {
        if !Self::binding_introduces_name(binding) {
            self.record_write_target_identity(target_span, name);
            if self.active_binding_is_const(name) {
                self.errors
                    .push(errors::const_reassignment_suggests_static(name, target_span));
                return;
            }
            if let Some(var_info) = self.lookup_variable_info_in_scope_chain(name) {
                let is_mutable = var_info.is_mutable;
                let declared_ty = var_info.ty.clone();
                if !is_mutable {
                    self.errors.push(errors::mutation_without_mut(name, target_span));
                } else {
                    self.refuse_caller_visible_mut_param_rebinding(name, target_span);
                }
                self.note_local_reassignment(name);
                if !self.types_compatible(&value_ty, &declared_ty) {
                    self.errors.push(errors::assignment_type_mismatch(
                        name,
                        &declared_ty.to_string(),
                        &value_ty.to_string(),
                        value_span,
                    ));
                }
                self.consumed_iterator_bindings.remove(name);
                self.transferred_c_resource_bindings.remove(name);
                self.note_list_reassignment(name, target_span);
                self.mark_open_rust_generic_binding_read(name, target_span);
                return;
            }
            if let Some(static_info) = self.lookup_static_info(name) {
                if static_info.is_imported {
                    self.errors
                        .push(errors::imported_static_reassignment_not_allowed(name, target_span));
                    return;
                }
                let static_ty = static_info.ty.clone();
                if !self.types_compatible(&value_ty, &static_ty) {
                    self.errors.push(errors::type_mismatch(
                        &static_ty.to_string(),
                        &value_ty.to_string(),
                        value_span,
                    ));
                }
                return;
            }
        }

        let is_mutable = matches!(binding, BindingKind::Mutable);
        self.validate_protected_builtin_binding(name, target_span);
        let symbol = Symbol {
            name: name.to_string(),
            kind: SymbolKind::Variable(VariableInfo {
                ty: value_ty,
                is_mutable,
                is_used: false,
            }),
            span: target_span,
            scope: 0,
        };
        if Self::binding_introduces_name(binding) {
            self.symbols.define_explicit_shadow(symbol);
        } else {
            self.symbols.define(symbol);
        }
        self.record_read_only_binding(is_mutable, target_span);
        self.record_write_target_identity(target_span, name);
        if is_mutable {
            self.mutable_bindings.insert(name.to_string());
        }
        self.consumed_iterator_bindings.remove(name);
        self.transferred_c_resource_bindings.remove(name);
    }

    /// Preserve the binding selected for a write at its exact authored target span.
    pub(in crate::typechecker) fn record_write_target_identity(&mut self, span: Span, name: &str) {
        let resolved = self.symbols.lookup(name).and_then(|symbol_id| {
            let identity = self.symbols.identity_of(symbol_id)?.clone();
            let ty = match &self.symbols.get(symbol_id)?.kind {
                SymbolKind::Variable(info) => info.ty.clone(),
                SymbolKind::Static(info) => info.ty.clone(),
                _ => return None,
            };
            Some((identity, ty))
        });
        if let Some((identity, ty)) = resolved {
            self.type_info.record_resolved_write_identity(span, name, identity, ty);
        }
    }
}
