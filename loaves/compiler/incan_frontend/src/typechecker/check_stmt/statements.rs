//! Statement dispatch, compound assignments, trait-typed annotations, open native call settlement and tuple statements.

use super::*;

impl TypeChecker {
    // ========================================================================
    // Statements
    // ========================================================================

    /// Check the value of a compound assignment `x op= y` against the type `var_ty` of the local or static it writes.
    ///
    /// Numeric operands are checked as `x = x op y`: the operator's result type from the operator result table, then
    /// the assignment rule. So `s *= s` on an `f32` binding keeps `f32`, and `n += 1` and `n |= 1` on an `i32` binding
    /// combine two `i32` values, and all are accepted, while `x /= 2` on an `int` binding stays refused because its
    /// `float` result is not assignable to `int` (#1812). A user operator receiver resolves through its in-place or
    /// binary hook, and any other value must be assignable to the binding.
    pub(super) fn check_compound_assignment_value(
        &mut self,
        compound: &CompoundAssignmentStmt,
        var_ty: &ResolvedType,
        value_ty: &ResolvedType,
        stmt_span: Span,
    ) {
        let binop = compound.op.binary_op();
        if let (Some(lhs), Some(rhs)) = (numeric_ty_from_resolved(var_ty), numeric_ty_from_resolved(value_ty)) {
            let result_ty = if numeric_op_from_ast(&binop).is_some() {
                let target = Spanned::new(Expr::Ident(compound.name.clone()), compound.name_span);
                self.numeric_arithmetic_result_type(
                    (&target, var_ty),
                    binop,
                    (&compound.value, value_ty),
                    compound.value.span,
                )
            } else if matches!(
                binop,
                BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor | BinaryOp::Shl | BinaryOp::Shr
            ) && matches!((lhs, rhs), (NumericTy::Int, NumericTy::Int))
            {
                let target = Spanned::new(Expr::Ident(compound.name.clone()), compound.name_span);
                self.integer_bitwise_result_type(
                    (&target, var_ty),
                    binop,
                    (&compound.value, value_ty),
                    compound.value.span,
                )
            } else {
                self.errors.push(errors::type_mismatch(
                    "supported compound operator operands",
                    &format!("{var_ty} {binop} {value_ty}"),
                    compound.value.span,
                ));
                return;
            };
            if !self.types_compatible(&result_ty, var_ty) {
                self.errors.push(errors::type_mismatch(
                    &var_ty.to_string(),
                    &result_ty.to_string(),
                    compound.value.span,
                ));
            }
        } else if self.is_user_operator_receiver(var_ty) {
            match self.resolve_compound_assignment_operator(var_ty, compound.op, &compound.value, value_ty, stmt_span) {
                Some(result_ty) if !self.types_compatible(&result_ty, var_ty) => {
                    self.errors.push(errors::type_mismatch(
                        &var_ty.to_string(),
                        &result_ty.to_string(),
                        compound.value.span,
                    ));
                }
                Some(_) => {}
                None => self.errors.push(errors::missing_method(
                    &var_ty.to_string(),
                    compound_assignment_fallback_dunder(compound.op),
                    stmt_span,
                )),
            }
        } else if !self.types_compatible(value_ty, var_ty) {
            self.errors.push(errors::type_mismatch(
                &var_ty.to_string(),
                &value_ty.to_string(),
                compound.value.span,
            ));
        }
    }

    /// Return whether a local annotation names a trait surface that does not yet have a local value representation.
    ///
    /// Callable parameters and returns have dedicated trait-bound lowering paths, but local bindings do not preserve a
    /// hidden concrete adopter yet. Rejecting this shape in the typechecker prevents accepted Incan from reaching Rust
    /// codegen as a bare trait local type.
    pub(super) fn is_trait_typed_local_annotation(&self, ty: &ResolvedType) -> bool {
        match ty {
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => {
                self.lookup_semantic_trait_info(name).is_some()
            }
            _ => false,
        }
    }

    /// Return the first trait type written inside another type, as a type argument or a tuple item (`Serialize` in
    /// `Option[Serialize]`), never the annotation itself.
    ///
    /// A trait stands for the types that adopt it only where the whole annotation is the trait: a parameter lowers to
    /// a hidden type parameter bounded by it and a return to one hidden adopting type. Inside another type it has no
    /// value representation, and Rust would name a bare trait there (E0782, #1866). A reference or a
    /// type token wraps the annotation without nesting it. A function type is not searched: its parameters are how a
    /// decorator names the trait receiver of the method it decorates (`(Service) -> int`).
    pub(in crate::typechecker) fn trait_type_nested_in(&self, ty: &ResolvedType) -> Option<ResolvedType> {
        self.find_trait_type(ty, false)
    }

    /// Search `ty` for a trait type, reporting the type itself only when it is `nested` inside the annotation.
    pub(super) fn find_trait_type(&self, ty: &ResolvedType, nested: bool) -> Option<ResolvedType> {
        match ty {
            ResolvedType::Named(name) | ResolvedType::Generic(name, _)
                if nested && self.lookup_semantic_trait_info(name).is_some() =>
            {
                Some(ty.clone())
            }
            ResolvedType::Generic(_, items) | ResolvedType::Tuple(items) => {
                items.iter().find_map(|item| self.find_trait_type(item, true))
            }
            ResolvedType::FrozenList(inner) | ResolvedType::FrozenSet(inner) => self.find_trait_type(inner, true),
            ResolvedType::FrozenDict(key, value) => self
                .find_trait_type(key, true)
                .or_else(|| self.find_trait_type(value, true)),
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => self.find_trait_type(inner, nested),
            ResolvedType::TypeToken(inner) => self.find_trait_type(inner, false),
            _ => None,
        }
    }

    /// Refuse a callable parameter, return or local annotation that writes a trait inside another type (#1866).
    pub(in crate::typechecker) fn refuse_trait_type_nested_in_annotation(&mut self, ty: &ResolvedType, span: Span) {
        if let Some(trait_type) = self.trait_type_nested_in(ty) {
            self.errors.push(errors::trait_type_nested_in_annotation_unsupported(
                &trait_type.to_string(),
                &ty.to_string(),
                span,
            ));
        }
    }

    /// Validate a statement and its subexpressions.
    ///
    /// Handles assignments (including mutability checks), control flow (`if`, `while`, `for`), returns, and expression
    /// statements. A statement-position `match` has a discarded unit destination, which lets an arm's `None` denote
    /// unit while the match checker still requires every arm to share one type. Other expressions delegate directly
    /// to [`check_expr`](Self::check_expr).
    pub fn check_statement(&mut self, stmt: &Spanned<Statement>) {
        match &stmt.node {
            Statement::Assignment(assign) => self.check_assignment(assign, stmt.span),
            Statement::FieldAssignment(field_assign) => {
                self.check_field_assignment(field_assign, stmt.span);
                self.refuse_capturing_reassignment(None, &field_assign.value, field_assign.value.span);
            }
            Statement::IndexAssignment(index_assign) => {
                self.check_index_assignment(index_assign, stmt.span);
                self.refuse_capturing_reassignment(None, &index_assign.value, index_assign.value.span);
            }
            Statement::Return(expr) => self.check_return(expr.as_ref(), stmt.span),
            Statement::If(if_stmt) => self.check_if_stmt(if_stmt),
            Statement::Loop(loop_stmt) => self.check_loop_stmt(loop_stmt, stmt.span),
            Statement::While(while_stmt) => self.check_while_stmt(while_stmt, stmt.span),
            Statement::For(for_stmt) => self.check_for_stmt(for_stmt, stmt.span),
            Statement::Unsafe(unsafe_stmt) => self.check_unsafe_stmt(unsafe_stmt),
            Statement::VocabBlock(vocab_block) if vocab_block.not_checked => {}
            Statement::VocabBlock(vocab_block) => {
                self.errors.push(crate::diagnostics::CompileError::new(
                    format!(
                        "raw vocab block `{}` reached typechecker before desugaring",
                        vocab_block.keyword
                    ),
                    stmt.span,
                ));
            }
            Statement::Assert(assert_stmt) => self.check_assert_stmt(assert_stmt),
            Statement::Surface(surface_stmt) => self.check_surface_stmt(surface_stmt, stmt.span),
            Statement::Expr(expr) => {
                if matches!(expr.node, Expr::Match(..)) {
                    self.check_expr_with_expected(expr, Some(&ResolvedType::Unit));
                } else {
                    self.check_expr(expr);
                }
                self.reject_discarded_open_rust_generic_call(expr.span);
            }
            Statement::VocabExpressionItem(_item) => {
                self.errors.push(crate::diagnostics::CompileError::new(
                    "raw vocab expression-list item reached typechecker before desugaring".to_string(),
                    stmt.span,
                ));
            }
            Statement::Pass => {}
            Statement::Break(value) => {
                if let Some(value) = value {
                    self.refuse_mut_param_held_by(value);
                }
                self.check_break_stmt(value.as_ref(), stmt.span)
            }
            Statement::Continue => self.check_continue_stmt(stmt.span),
            Statement::CompoundAssignment(compound) => {
                self.record_write_target_identity(compound.name_span, &compound.name);
                // Check that the variable exists and is mutable (search all scopes)
                let var_info_opt = self
                    .symbols
                    .lookup(&compound.name)
                    .and_then(|id| self.symbols.get(id))
                    .and_then(|sym| {
                        if let SymbolKind::Variable(var_info) = &sym.kind {
                            Some((var_info.is_mutable, var_info.ty.clone()))
                        } else {
                            None
                        }
                    });

                if let Some((is_mutable, var_ty)) = var_info_opt {
                    if !is_mutable {
                        self.errors
                            .push(errors::mutation_without_mut(&compound.name, stmt.span));
                    } else {
                        self.refuse_caller_visible_mut_param_rebinding(&compound.name, stmt.span);
                    }
                    // Type check the value expression
                    let value_ty = self.check_expr(&compound.value);
                    self.refuse_capturing_reassignment(Some(&compound.name), &compound.value, compound.name_span);

                    self.check_compound_assignment_value(compound, &var_ty, &value_ty, stmt.span);
                } else if let Some(static_info) = self.lookup_static_info(&compound.name).cloned() {
                    if static_info.is_imported {
                        self.errors.push(errors::imported_static_reassignment_not_allowed(
                            &compound.name,
                            stmt.span,
                        ));
                        return;
                    }
                    let value_ty = self.check_expr(&compound.value);
                    let var_ty = static_info.ty;

                    self.check_compound_assignment_value(compound, &var_ty, &value_ty, stmt.span);
                } else if self.const_decls.contains_key(&compound.name) {
                    self.errors
                        .push(errors::const_reassignment_suggests_static(&compound.name, stmt.span));
                } else {
                    self.errors.push(errors::unknown_symbol(&compound.name, stmt.span));
                }
            }
            Statement::TupleUnpack(unpack) => self.check_tuple_unpack_statement(unpack, stmt.span),
            Statement::TupleAssign(assign) => self.check_tuple_assignment_statement(assign, stmt.span),
            Statement::ChainedAssignment(ca) => {
                // The value is refused before the chain's targets are bound, so a target cannot shadow the parameter.
                self.refuse_mut_param_held_by(&ca.value);
                self.check_chained_assignment(ca, stmt.span);
                self.refuse_capturing_reassignment(None, &ca.value, ca.value.span);
            }
        }
        self.reject_unbound_c_abi_span_constructors();
        self.reject_unbound_scoped_c_string_views(&stmt.node);
        self.settle_open_rust_generic_calls();
    }

    /// End the statement's native call selection facts. A call nested somewhere other than a direct binding or a
    /// bare statement is closed by its context (an argument's declared type, a typed return) when its callee was
    /// retained; one without a callee is refused (#1720, #2481). A watched binding this statement read first takes
    /// the type its use fixed, or is refused.
    pub(super) fn settle_open_rust_generic_calls(&mut self) {
        let mut nested = std::mem::take(&mut self.open_rust_generic_calls)
            .into_iter()
            .collect::<Vec<_>>();
        nested.sort_by_key(|(span, _)| *span);
        for (span, call) in nested {
            self.refuse_unselected_open_rust_generic_call(&call, span);
        }
        self.tied_rust_trait_callees.clear();
        self.settle_read_open_rust_generic_bindings(self.statement_blocks.current);
    }

    /// Refuse the watched bindings first read in statement block `block` for their open parameters: a first use that
    /// fixes them stops the watch at once, so a binding still watched here was read by a use that fixed nothing
    /// (#2481).
    pub(super) fn settle_read_open_rust_generic_bindings(&mut self, block: usize) {
        if !self
            .open_rust_generic_bindings
            .iter()
            .any(|binding| binding.read_in == Some(block))
        {
            return;
        }
        let (read, still_watched): (Vec<_>, Vec<_>) = std::mem::take(&mut self.open_rust_generic_bindings)
            .into_iter()
            .partition(|binding| binding.read_in == Some(block));
        self.open_rust_generic_bindings = still_watched;
        for binding in read {
            self.refuse_unselected_open_rust_generic_call(&binding.call, binding.span);
        }
    }

    /// Check a tuple unpack before introducing its names. Plain names reassign the nearest active binding while
    /// `let` and `mut` introduce lexical bindings; open Rust tuple elements retain their individual target facts.
    pub(super) fn check_tuple_unpack_statement(&mut self, unpack: &TupleUnpackStmt, span: Span) {
        let value_ty = self.check_expr(&unpack.value);
        self.refuse_mut_param_held_by(&unpack.value);
        let element_types = self.destructured_element_types(&value_ty, unpack.names.len(), span);
        for (i, name) in unpack.names.iter().enumerate() {
            let ty = element_types.get(i).cloned().unwrap_or(ResolvedType::Unknown);
            let target_span = unpack.name_spans.get(i).copied().unwrap_or(span);
            self.check_unannotated_assignment_target(name, unpack.binding, ty, target_span, unpack.value.span);
            self.bind_open_tuple_element(unpack.value.span, i, target_span, name);
        }
    }

    /// Check tuple assignment targets in source order after checking the source value. Name targets use ordinary
    /// binding/reassignment rules; projections retain receiver mutability, read-only and caller-write checks.
    pub(super) fn check_tuple_assignment_statement(&mut self, assign: &TupleAssignStmt, span: Span) {
        let value_ty = self.check_expr(&assign.value);
        self.refuse_mut_param_held_by(&assign.value);
        let element_types = self.destructured_element_types(&value_ty, assign.targets.len(), span);
        for (i, target) in assign.targets.iter().enumerate() {
            let expected_ty = element_types.get(i).cloned().unwrap_or(ResolvedType::Unknown);
            // A bound name is reassigned; an unbound name is declared with the element's type. Caller-visible
            // mutable parameters still change in place rather than rebinding (#1773).
            if let Expr::Ident(name) = &target.node {
                self.check_unannotated_assignment_target(
                    name,
                    BindingKind::Inferred,
                    expected_ty,
                    target.span,
                    assign.value.span,
                );
                self.bind_open_tuple_element(assign.value.span, i, target.span, name);
                continue;
            }
            let target_ty = self.check_expr(target);
            match &target.node {
                Expr::Index(object, _) | Expr::Field(object, _) => {
                    // Projection writes follow the same ownership checks as single field/element assignments.
                    if matches!(target.node, Expr::Index(_, _))
                        && let Some(object_ty) = self.type_info.expr_type(object.span).cloned()
                    {
                        self.refuse_unassignable_element_receiver(&object_ty, target.span);
                    }
                    if let Some(place) = Self::self_rooted_place(target) {
                        self.reject_write_through_immutable_self(&place, SelfMutation::Assignment, target.span);
                    }
                    let writes_builtin_slot = matches!(target.node, Expr::Field(_, _))
                        || self
                            .type_info
                            .expr_type(object.span)
                            .is_some_and(|ty| Self::index_assignment_slot_types(ty).is_some());
                    if writes_builtin_slot {
                        self.refuse_write_through_read_only_binding(object, target.span);
                    }
                    self.note_mut_param_write(target);
                }
                _ => self.errors.push(errors::invalid_tuple_assignment_target(target.span)),
            }
            if !self.types_compatible(&expected_ty, &target_ty) {
                self.errors.push(errors::type_mismatch(
                    &target_ty.to_string(),
                    &expected_ty.to_string(),
                    target.span,
                ));
            }
        }
    }
}
