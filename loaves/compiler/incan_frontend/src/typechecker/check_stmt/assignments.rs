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
        self.note_native_result_local(assign);
        self.note_held_borrowed_result(&assign.value);
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
        let annotated_ty = assign.ty.as_ref().map(|annotation| {
            let canonical = self.canonical_borrow_annotation(annotation, false);
            self.resolve_type_checked(canonical.as_ref().unwrap_or(annotation))
        });
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
        // A local borrow takes the spelling its callable's parameters take: explicit where the signature needs the
        // proof.
        let explicit_borrows = self
            .current_body_identity()
            .is_some_and(|owner| self.type_info.declarations.ownership_contracts.contains_key(owner));
        let annotated_ty = assign.ty.as_ref().map(|ty_ann| {
            let annotation = self.canonical_borrow_annotation(ty_ann, explicit_borrows);
            let resolved = self.resolve_type_checked_for_input(annotation.as_ref().unwrap_or(ty_ann), defer_local);
            self.canonical_borrow_type(resolved, explicit_borrows)
        });
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
            self.record_write_target_identity(target_span, &assign.name);
            self.note_iteration_binding_replacement(&assign.name, target_span);
        }
        let reassignment_ty = (!introduces_binding)
            .then(|| {
                self.lookup_variable_info_in_scope_chain(&assign.name)
                    .map(|var_info| var_info.ty.clone())
            })
            .flatten();
        let value_ty = if let Some(var_ty) = reassignment_ty.as_ref() {
            // A write through an exclusive borrow expects its referent (RFC 164).
            let expected = match var_ty {
                ResolvedType::RefMut(referent) => referent.as_ref(),
                other => other,
            };
            self.check_expr_with_expected(&assign.value, Some(expected))
        } else {
            self.check_new_binding_value(assign, annotated_ty.as_ref())
        };
        // A local borrow lends its value for the borrow's life (RFC 164); it does not hold a `mut` parameter's value.
        if !matches!(annotated_ty, Some(ResolvedType::Ref(_) | ResolvedType::RefMut(_))) {
            self.refuse_mut_param_held_by(&assign.value);
        }

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
            // RFC 164: assigning a value to an exclusive borrow writes through it, as for `BorrowedMut[T]`; only a
            // reference value rebinds the name.
            let through = written_through_referent(&var_ty, &value_ty);

            if !is_mutable {
                self.errors
                    .push(errors::mutation_without_mut(&assign.name, target_span));
            } else if through.is_none() {
                self.refuse_caller_visible_mut_param_rebinding(&assign.name, target_span);
            }
            self.note_local_reassignment(&assign.name);
            let value_view = crate::typechecker::helpers::borrowed_value_type(value_ty.clone());
            let compatible = match through {
                Some(referent) => self.types_compatible(&value_view, referent),
                None => self.types_compatible(&value_ty, &var_ty),
            };
            if !compatible {
                let error = self.reassignment_mismatch(&assign.name, &var_ty, &value_ty, assign.value.span);
                self.errors.push(error);
            }
            self.consumed_iterator_bindings.remove(&assign.name);
            self.transferred_c_resource_bindings.remove(&assign.name);
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

        // A local borrow takes a reference to the place rather than copying it, so it is not a place copy.
        let local_borrow = matches!(annotated_ty, Some(ResolvedType::Ref(_) | ResolvedType::RefMut(_)));
        if explicit_mode.is_none() && !local_borrow {
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

        // A native local borrow, of a value or of another borrow, keeps its loan for the conflict pass.
        let local_borrow_of_value =
            explicit_mode.is_none() && matches!(annotated_ty, Some(ResolvedType::Ref(_) | ResolvedType::RefMut(_)));
        let ty = self.new_assignment_binding_type(assign, annotated_ty, value_ty);
        let ty = self.retain_qualified_assignment_binding_owner(assign, ty);
        self.declare_assignment_binding(assign, ty.clone(), is_mutable, introduces_binding, span);
        self.retain_qualified_assignment_storage(assign, span);
        self.record_explicit_local_mode(assign, explicit_mode, is_mutable);
        if local_borrow_of_value {
            self.record_native_local_borrow(assign, ty, is_mutable);
        }
    }

    /// RFC 164: a local borrow of a value outside the whole-body proof keeps its loan for the conflict pass, which
    /// reports an edit of its owner while the borrow, or a closure holding it, is still used.
    fn record_native_local_borrow(&mut self, assign: &AssignmentStmt, binding: ResolvedType, writable: bool) {
        use incan_lang::lang::types::ownership::OwnershipMode;
        let mode = match &binding {
            ResolvedType::Ref(_) => OwnershipMode::Borrowed,
            ResolvedType::RefMut(_) => OwnershipMode::BorrowedMut,
            _ => return,
        };
        let (Some(annotation), Some(identity), Some(owner)) = (
            assign.ty.as_ref(),
            self.type_info
                .resolved_write_identity(assign.name_span, &assign.name)
                .filter(|identity| identity.declaration_span == assign.name_span)
                .cloned(),
            self.current_body_identity().cloned(),
        ) else {
            return;
        };
        self.native_local_borrows.insert(
            identity,
            super::ownership_contracts::LocalMode {
                owner,
                mode,
                value_type: binding,
                writable,
                annotation: annotation.span,
            },
        );
        self.note_native_borrow_body();
    }

    /// Check the value a new binding receives against its annotation. While an unannotated binding's value is checked,
    /// the binding's name is kept, so an undetermined construction's hint can spell the annotation that fixes it
    /// (#2762).
    fn check_new_binding_value(
        &mut self,
        assign: &AssignmentStmt,
        annotated_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        if annotated_ty.is_some() {
            return self.check_expr_with_expected(&assign.value, annotated_ty);
        }
        let enclosing = self
            .unannotated_binding_value
            .replace((assign.value.span, assign.name.clone()));
        let value_ty = self.check_expr_with_expected(&assign.value, None);
        self.unannotated_binding_value = enclosing;
        value_ty
    }

    /// Preserve a module-qualified annotation's selected nominal owner after source compatibility is checked.
    ///
    /// The checker already uses a private module-qualified nominal spelling to distinguish equal-named declarations
    /// in unions. Reuse that spelling for an explicitly qualified local so later field and method selection cannot
    /// fall back to another imported declaration's short name. The corresponding carrier binding lets Body IR place
    /// the same canonical owner. Generic arguments remain exactly as checked; only the nominal head is replaced.
    fn retain_qualified_assignment_binding_owner(
        &mut self,
        assign: &AssignmentStmt,
        binding: ResolvedType,
    ) -> ResolvedType {
        let Some(identity) = self.qualified_assignment_identity(assign) else {
            return binding;
        };
        let module_path = match &identity.origin {
            incan_semantics_core::SymbolOrigin::Module(path) => path.clone(),
            incan_semantics_core::SymbolOrigin::Package { module_path, .. } => module_path.clone(),
            _ => return binding,
        };
        let Some(carrier) = incan_semantics_core::body_ir::nominal_carrier_key(&identity) else {
            return binding;
        };
        let qualified = module_qualified_nominal_name(&module_path, &identity.declaration_name);
        self.type_info
            .declarations
            .named_type_identities
            .insert(carrier, identity.clone());
        self.type_info
            .declarations
            .named_type_identities
            .insert(qualified.clone(), identity);
        match binding {
            ResolvedType::Named(_) => ResolvedType::Named(qualified),
            ResolvedType::Generic(_, arguments) => ResolvedType::Generic(qualified, arguments),
            other => other,
        }
    }

    /// Project a checker-qualified local binding to its canonical executable carrier for Body IR storage.
    fn retain_qualified_assignment_storage(&mut self, assign: &AssignmentStmt, span: Span) {
        let Some(identity) = self.qualified_assignment_identity(assign) else {
            return;
        };
        let Some(carrier) = incan_semantics_core::body_ir::nominal_carrier_key(&identity) else {
            return;
        };
        let Some(binding) = self.type_info.expressions.assignment_binding_types.get_mut(&span) else {
            return;
        };
        match binding {
            ResolvedType::Named(_) => *binding = ResolvedType::Named(carrier.clone()),
            ResolvedType::Generic(_, arguments) => {
                *binding = ResolvedType::Generic(carrier.clone(), arguments.clone());
            }
            _ => return,
        }
        self.type_info
            .declarations
            .named_type_identities
            .insert(carrier, identity);
    }

    /// Return the canonical nominal selected for a dotted assignment annotation.
    fn qualified_assignment_identity(
        &self,
        assign: &AssignmentStmt,
    ) -> Option<incan_semantics_core::CanonicalSymbolId> {
        let annotation = assign.ty.as_ref()?;
        let segments = match &annotation.node {
            Type::Dotted(segments) | Type::DottedGeneric(segments, _) => segments,
            _ => return None,
        };
        self.type_info
            .qualified_type_reference(&segments.join("."))
            .map(|reference| reference.identity.clone())
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
            // RFC 164: a `mut` binding never receives an implicit clone of a borrowed value.
            if assign.binding == BindingKind::Mutable
                && !matches!(ann_ty, ResolvedType::Ref(_) | ResolvedType::RefMut(_))
                && self.borrowed_source_value_materializes(&value_ty)
                && self.types_compatible(&value_ty, &ann_ty)
            {
                self.errors.push(errors::borrowed_value_needs_explicit_clone(
                    &value_ty.to_string(),
                    &format!("The mut binding '{}'", assign.name),
                    assign.value.span,
                ));
            }
            // RFC 164: a local borrow binds a reference to its value, shared or exclusive, as a parameter does.
            let borrows = match (&ann_ty, &value_ty) {
                (_, ResolvedType::Ref(_) | ResolvedType::RefMut(_)) => false,
                (ResolvedType::Ref(referent) | ResolvedType::RefMut(referent), _) => {
                    self.types_compatible(&value_ty, referent)
                }
                _ => false,
            };
            if borrows && matches!(ann_ty, ResolvedType::RefMut(_)) {
                self.refuse_write_through_read_only_binding(&assign.value, assign.value.span);
                self.note_mut_param_exclusive_use(&assign.value);
            }
            // Check value matches annotation
            if !borrows
                && !self.types_compatible(&value_ty, &ann_ty)
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
                    let error = self.reassignment_mismatch(name, &declared_ty, &value_ty, value_span);
                    self.errors.push(error);
                }
                self.consumed_iterator_bindings.remove(name);
                self.transferred_c_resource_bindings.remove(name);
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

    /// The refusal of a reassignment whose value does not fit the binding. RFC 164: a `mut` binding of a borrowed
    /// value stays a borrow, so owning a new value needs an explicit clone where it is first bound.
    fn reassignment_mismatch(
        &self,
        name: &str,
        declared: &ResolvedType,
        value: &ResolvedType,
        span: Span,
    ) -> crate::diagnostics::CompileError {
        match declared {
            ResolvedType::Ref(inner) if self.types_compatible(value, inner) => {
                errors::borrowed_value_needs_explicit_clone(
                    &declared.to_string(),
                    &format!("The mut binding '{name}'"),
                    span,
                )
            }
            _ => errors::assignment_type_mismatch(name, &declared.to_string(), &value.to_string(), span),
        }
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

/// The referent an assignment writes through: a value assigned to an exclusive borrow (`&mut T`, `BorrowedMut[T]`)
/// replaces what it borrows (RFC 164). Assigning another reference rebinds the name instead.
pub(in crate::typechecker) fn written_through_referent<'a>(
    target: &'a ResolvedType,
    value: &ResolvedType,
) -> Option<&'a ResolvedType> {
    match (target, value) {
        (_, ResolvedType::Ref(_) | ResolvedType::RefMut(_)) => None,
        (ResolvedType::RefMut(referent), _) => Some(referent.as_ref()),
        _ => None,
    }
}
