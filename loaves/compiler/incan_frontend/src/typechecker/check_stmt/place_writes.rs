//! Writes through places: read-only bindings, closure captures, `self`, and field and index assignments.

use super::*;

impl TypeChecker {
    /// Spell a place expression rooted at the method receiver the way the source reads it (`self.width`,
    /// `self.items[...]`), or return `None` when the expression does not read through `self`.
    ///
    /// Only place steps are followed: field reads and index reads, through parentheses. A call in the chain
    /// (`self.items.clone()`) yields a fresh value, so nothing past it touches the receiver and the walk stops with
    /// `None`. An index is rendered as `[...]` because the message names the place, not the element.
    pub(in crate::typechecker) fn self_rooted_place(expr: &Spanned<Expr>) -> Option<String> {
        match &expr.node {
            Expr::SelfExpr => Some("self".to_string()),
            Expr::Paren(inner) => Self::self_rooted_place(inner),
            Expr::Field(base, field) => Self::self_rooted_place(base).map(|place| format!("{place}.{field}")),
            Expr::Index(base, _) => Self::self_rooted_place(base).map(|place| format!("{place}[...]")),
            _ => None,
        }
    }

    /// Remember the declaration span of a binding declared without `mut`, whose place a field or element write may not
    /// go through (see [`Self::refuse_write_through_read_only_binding`]).
    pub(in crate::typechecker) fn record_read_only_binding(&mut self, is_mutable: bool, declaration_span: Span) {
        if !is_mutable {
            self.read_only_binding_spans.insert(declaration_span);
        }
    }

    /// Return the name a written place is rooted at, `items` for `items[0].count`, or `None` for a place rooted at
    /// anything else, such as `self` or a call result.
    pub(super) fn written_place_root(expr: &Spanned<Expr>) -> Option<&str> {
        match &expr.node {
            Expr::Ident(name) => Some(name),
            Expr::Paren(inner) | Expr::Field(inner, _) | Expr::Index(inner, _) => Self::written_place_root(inner),
            _ => None,
        }
    }

    /// Refuse a change at `span`, inside a closure, through the local `symbol_id` (named `name`) that the closure
    /// captures from its enclosing callable (#1561). Returns whether it was refused.
    ///
    /// A closure reads each outer local it names as the value the local holds when the closure is constructed, so a
    /// change the closure makes through one would change the closure's own copy, which the local never sees. A
    /// caller-visible `mut` parameter, and a view into one, are refused as held by the closure instead (see
    /// `note_param_change`).
    pub(in crate::typechecker) fn refuse_change_through_closure_capture(
        &mut self,
        symbol_id: SymbolId,
        name: &str,
        span: Span,
    ) -> bool {
        if !self.local_is_captured_by_closure(symbol_id) || self.reaches_caller_visible_param(name) {
            return false;
        }
        let error = errors::change_through_closure_capture(name, span);
        let already_reported = self
            .errors
            .iter()
            .any(|existing| existing.span == error.span && existing.message == error.message);
        if !already_reported {
            self.errors.push(error);
        }
        true
    }

    /// Return whether the local `symbol_id` resolves to is held by a callable enclosing the closure being checked, so
    /// that the closure captures it.
    pub(in crate::typechecker) fn local_is_captured_by_closure(&self, symbol_id: SymbolId) -> bool {
        !self.checks_standard_library_source()
            && self.symbols.get(symbol_id).is_some_and(|symbol| {
                matches!(symbol.kind, SymbolKind::Variable(_)) && self.symbols.read_crosses_callable_scope(symbol.scope)
            })
    }

    /// Refuse a field or element write through a binding declared without `mut` (#1561).
    ///
    /// A binding is immutable unless declared `mut`, and that covers what it holds: a local declared by `let` or a
    /// first plain assignment, or a parameter not marked `mut`, gives no write access to its fields and elements. A
    /// local bound directly to a module static is an alias of the static's storage and writes through to it, and a
    /// loop or pattern binding follows the place it binds, so neither is refused here. A change through a local a
    /// closure captures is refused (see [`Self::refuse_change_through_closure_capture`]). `object` is the place whose
    /// field or element is written.
    pub(in crate::typechecker) fn refuse_write_through_read_only_binding(
        &mut self,
        object: &Spanned<Expr>,
        span: Span,
    ) {
        let Some(root) = Self::written_place_root(object) else {
            return;
        };
        let Some(symbol_id) = self.symbols.lookup(root) else {
            return;
        };
        if self.refuse_change_through_closure_capture(symbol_id, root, span) {
            return;
        }
        if self.static_alias_bindings.contains_key(&symbol_id) {
            return;
        }
        let Some(symbol) = self.symbols.get(symbol_id) else {
            return;
        };
        if !matches!(symbol.kind, SymbolKind::Variable(_)) || !self.read_only_binding_spans.contains(&symbol.span) {
            return;
        }
        self.errors.push(errors::mutation_without_mut(root, span));
    }

    /// Refuse a write to `place`, a place rooted at `self`, when the enclosing method takes a plain `self` (#1723).
    ///
    /// The receiver spelling is what the generated code honors: a plain `self` method has no way to write, so the
    /// assignment or changing call would only fail later, in the build. Outside a plain-`self` method body nothing
    /// is reported; a `mut self` method writes freely, and a function has no receiver. `mutation` says whether the
    /// body assigns to the place or calls a method that changes it, which is the difference between the two
    /// message shapes.
    pub(in crate::typechecker) fn reject_write_through_immutable_self(
        &mut self,
        place: &str,
        mutation: SelfMutation<'_>,
        span: Span,
    ) {
        let Some(method) = self.current_immutable_self_method.as_deref() else {
            return;
        };
        let error = errors::self_mutation_requires_mut_self(method, place, mutation, span);
        self.errors.push(error);
    }

    /// Validate assignment to an object field, including generic-owner field substitution. Mutable references expose
    /// their referent's fields; encountering a shared reference still refuses the write.
    pub(super) fn check_field_assignment(&mut self, field_assign: &FieldAssignmentStmt, span: Span) {
        // Check the object expression
        let obj_ty = self.check_expr(&field_assign.object);
        let mut obj_ty = &obj_ty;
        while let ResolvedType::RefMut(inner) = obj_ty {
            obj_ty = inner;
        }
        self.refuse_write_through_read_only_binding(&field_assign.object, field_assign.target_span);
        self.note_mut_param_write(&field_assign.object);
        self.refuse_mut_param_held_by(&field_assign.value);
        let field = &field_assign.field;
        if let Some(place) = Self::self_rooted_place(&field_assign.object) {
            self.reject_write_through_immutable_self(
                &format!("{place}.{field}"),
                SelfMutation::Assignment,
                field_assign.target_span,
            );
        }

        // Tuples are immutable - disallow field assignment on tuples
        if matches!(obj_ty, ResolvedType::Tuple(_)) {
            self.errors.push(errors::tuple_field_assignment(span));
            return;
        }

        // Verify field exists on object and value type matches field type
        match &obj_ty {
            ResolvedType::SelfType => {
                if let Some(expected_ty) = self.trait_required_field_type(field, field_assign.target_span) {
                    self.record_expr_type(field_assign.target_span, expected_ty.clone());
                    let value_ty = self.check_expr_with_expected(&field_assign.value, Some(&expected_ty));
                    if !self.types_compatible(&value_ty, &expected_ty) {
                        self.errors.push(errors::field_type_mismatch(
                            field,
                            &expected_ty.to_string(),
                            &value_ty.to_string(),
                            field_assign.value.span,
                        ));
                    }
                    self.record_value_destination_if_compatible(field_assign.value.span, &value_ty, &expected_ty);
                }
            }
            ResolvedType::Named(type_name) => {
                match self.resolve_nominal_field_type(type_name, None, field, field_assign.target_span) {
                    Some(expected_ty) => {
                        self.record_expr_type(field_assign.target_span, expected_ty.clone());
                        let value_ty = self.check_expr_with_expected(&field_assign.value, Some(&expected_ty));
                        if !self.types_compatible(&value_ty, &expected_ty) {
                            self.errors.push(errors::field_type_mismatch(
                                field,
                                &expected_ty.to_string(),
                                &value_ty.to_string(),
                                field_assign.value.span,
                            ));
                        }
                        self.record_value_destination_if_compatible(field_assign.value.span, &value_ty, &expected_ty);
                    }
                    None => {
                        self.errors.push(errors::missing_field(type_name, field, span));
                    }
                }
            }
            ResolvedType::Generic(type_name, type_args) => {
                match self.resolve_nominal_field_type(
                    type_name,
                    Some(type_args.as_slice()),
                    field,
                    field_assign.target_span,
                ) {
                    Some(expected_ty) => {
                        self.record_expr_type(field_assign.target_span, expected_ty.clone());
                        let value_ty = self.check_expr_with_expected(&field_assign.value, Some(&expected_ty));
                        if !self.types_compatible(&value_ty, &expected_ty) {
                            self.errors.push(errors::field_type_mismatch(
                                field,
                                &expected_ty.to_string(),
                                &value_ty.to_string(),
                                field_assign.value.span,
                            ));
                        }
                        self.record_value_destination_if_compatible(field_assign.value.span, &value_ty, &expected_ty);
                    }
                    None => {
                        self.errors.push(errors::missing_field(type_name, field, span));
                    }
                }
            }
            // A value typed by a Rust path (`mut plan = empty_plan()` where the helper returns
            // `rust::substrait::proto::Plan`) assigns a field through the same metadata that answers a read of it,
            // and the expected field type drives the value's boundary coercions exactly as a constructor argument
            // would.
            ResolvedType::RustPath(path) => match self.rust_path_field_type(path, field, field_assign.target_span) {
                Some(expected_ty) => {
                    self.record_expr_type(field_assign.target_span, expected_ty.clone());
                    let value_ty = self.check_expr_with_expected(&field_assign.value, Some(&expected_ty));
                    if !self.types_compatible(&value_ty, &expected_ty) {
                        self.errors.push(errors::field_type_mismatch(
                            field,
                            &expected_ty.to_string(),
                            &value_ty.to_string(),
                            field_assign.value.span,
                        ));
                    }
                    self.record_value_destination_if_compatible(field_assign.value.span, &value_ty, &expected_ty);
                }
                None => {
                    self.errors
                        .push(errors::missing_field(&obj_ty.to_string(), field, span));
                }
            },
            ResolvedType::Unknown => {
                // Don't report additional errors on unknown types
            }
            _ => {
                // Cannot assign fields to primitive types
                self.errors
                    .push(errors::missing_field(&obj_ty.to_string(), field, span));
            }
        }
        self.note_iteration_field_replacement(&field_assign.object, field_assign.target_span);
    }

    /// Return the key and value types a list or dict index assignment writes through: `int` and the element type of a
    /// `list[T]`, the key and value types of a `dict[K, V]`.
    pub(super) fn index_assignment_slot_types(obj_ty: &ResolvedType) -> Option<(ResolvedType, ResolvedType)> {
        let ResolvedType::Generic(name, args) = obj_ty else {
            return None;
        };
        match (collection_type_id(name.as_str()), args.as_slice()) {
            (Some(CollectionTypeId::List), [element]) => Some((ResolvedType::Int, element.clone())),
            (Some(CollectionTypeId::Dict), [key, value]) => Some((key.clone(), value.clone())),
            _ => None,
        }
    }

    /// Validate list/dict index assignment or RFC 028 `__setitem__` dispatch for user-defined receivers.
    ///
    /// The index and the value of a list or dict index assignment are checked against the slot they write, so an
    /// integer literal written to an element of a `list[float]` takes the float type, as it does in a declaration
    /// (#1854), and a value written to an `Option` slot is recorded for lowering to wrap (#1858).
    pub(super) fn check_index_assignment(&mut self, index_assign: &IndexAssignmentStmt, span: Span) {
        // Check the object expression (should be a collection)
        let obj_ty = self.check_expr(&index_assign.object);
        self.note_mut_param_write(&index_assign.object);
        self.refuse_mut_param_held_by(&index_assign.value);
        if let Some(place) = Self::self_rooted_place(&index_assign.object) {
            self.reject_write_through_immutable_self(&format!("{place}[...]"), SelfMutation::Assignment, span);
        }
        let slot_types = Self::index_assignment_slot_types(&obj_ty);
        if slot_types.is_some() {
            self.refuse_write_through_read_only_binding(&index_assign.object, span);
        }
        // Check the index expression
        let index_ty = self.check_expr_with_expected(&index_assign.index, slot_types.as_ref().map(|(key, _)| key));
        // Check the value expression
        let value_ty = self.check_expr_with_expected(&index_assign.value, slot_types.as_ref().map(|(_, value)| value));
        if let Some((_, value_slot_ty)) = &slot_types {
            self.record_value_destination_if_compatible(index_assign.value.span, &value_ty, value_slot_ty);
        }

        // Verify object is indexable and types match
        if self.refuse_unassignable_element_receiver(&obj_ty, span) {
            return;
        }
        match &obj_ty {
            ResolvedType::Generic(name, args) if collection_type_id(name.as_str()) == Some(CollectionTypeId::List) => {
                // List[T] - index must be int, value must be T
                if !matches!(index_ty, ResolvedType::Int) {
                    self.errors.push(errors::index_type_mismatch(
                        "int",
                        &index_ty.to_string(),
                        index_assign.index.span,
                    ));
                }
                if let Some(elem_ty) = args.first()
                    && !self.types_compatible(&value_ty, elem_ty)
                {
                    self.errors.push(errors::index_value_type_mismatch(
                        &elem_ty.to_string(),
                        &value_ty.to_string(),
                        index_assign.value.span,
                    ));
                }
            }
            ResolvedType::Generic(name, args) if collection_type_id(name.as_str()) == Some(CollectionTypeId::Dict) => {
                // Dict[K, V] - index must be K, value must be V
                if let Some(key_ty) = args.first()
                    && !self.types_compatible(&index_ty, key_ty)
                {
                    self.errors.push(errors::index_type_mismatch(
                        &key_ty.to_string(),
                        &index_ty.to_string(),
                        index_assign.index.span,
                    ));
                }
                if let Some(val_ty) = args.get(1)
                    && !self.types_compatible(&value_ty, val_ty)
                {
                    self.errors.push(errors::index_value_type_mismatch(
                        &val_ty.to_string(),
                        &value_ty.to_string(),
                        index_assign.value.span,
                    ));
                }
            }
            ResolvedType::Unknown => {
                // Don't report additional errors on unknown types
            }
            // Every other receiver left by the refusal above is a type that defines its own element write.
            ty => {
                if self
                    .resolve_index_set_dunder(ty, &index_assign.index, &index_ty, &index_assign.value, &value_ty, span)
                    .is_none()
                {
                    self.errors
                        .push(errors::missing_method(&ty.to_string(), "__setitem__", span));
                }
            }
        }
    }

    /// Refuse a write to an element of a value of type `obj_ty` when no element of such a value is assignable, and
    /// return whether the write was refused.
    ///
    /// A list, a dict, a type that defines its own element write and an unknown type are left to the caller. A tuple in
    /// either spelling and a string are immutable, and any other type, such as a frozen collection, is not indexable
    /// for a write. `x[i] = value` and an element target of `a, x[i] = value` share this rule, so a tuple
    /// assignment never writes an element that a single assignment refuses (#1561).
    pub(super) fn refuse_unassignable_element_receiver(&mut self, obj_ty: &ResolvedType, span: Span) -> bool {
        let refusal = match obj_ty {
            ResolvedType::Generic(name, _) => match collection_type_id(name.as_str()) {
                Some(CollectionTypeId::List | CollectionTypeId::Dict) => None,
                // A written `tuple[A, B]` is immutable like a tuple literal's type below.
                Some(CollectionTypeId::Tuple) => Some(errors::tuple_field_assignment(span)),
                _ if self.is_user_operator_receiver(obj_ty) => None,
                _ => Some(errors::not_indexable(&obj_ty.to_string(), span)),
            },
            ResolvedType::Tuple(_) => Some(errors::tuple_field_assignment(span)),
            ResolvedType::Str => Some(errors::string_index_assignment_not_allowed(span)),
            ResolvedType::Unknown => None,
            ty if self.is_user_operator_receiver(ty) => None,
            _ => Some(errors::not_indexable(&obj_ty.to_string(), span)),
        };
        let refused = refusal.is_some();
        self.errors.extend(refusal);
        refused
    }
}
