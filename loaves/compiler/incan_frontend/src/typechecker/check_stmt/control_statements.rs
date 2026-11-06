//! Control-flow, loop, assertion and surface statements, and iterator element types.

use super::*;

impl TypeChecker {
    /// Check an acknowledgment body in its surrounding ordinary statement scope.
    pub(super) fn check_unsafe_stmt(&mut self, unsafe_stmt: &UnsafeStmt) {
        self.unsafe_depth += 1;
        self.check_statement_block(&unsafe_stmt.body);
        self.unsafe_depth = self.unsafe_depth.saturating_sub(1);
    }

    /// Validate an `if` statement and apply branch-local narrowing where supported.
    pub(super) fn check_if_stmt(&mut self, if_stmt: &IfStmt) {
        let mut false_refinements = Vec::new();

        if let Some(refinement) = self.check_condition_body(&if_stmt.condition, &if_stmt.then_body, &false_refinements)
        {
            Self::upsert_branch_refinement(&mut false_refinements, refinement);
        }

        for (elif_cond, elif_body) in &if_stmt.elif_branches {
            if let Some(refinement) = self.check_expr_condition_body(elif_cond, elif_body, &false_refinements) {
                Self::upsert_branch_refinement(&mut false_refinements, refinement);
            }
        }

        if let Some(else_body) = &if_stmt.else_body {
            self.symbols.enter_scope(ScopeKind::Block);
            self.apply_branch_refinements(&false_refinements);
            self.check_statement_block(else_body);
            self.symbols.exit_scope();
        }
    }

    /// Type-check a statement-form `while` at `span`, including ordinary truthiness and pattern-driven `while let`
    /// conditions.
    pub(super) fn check_while_stmt(&mut self, while_stmt: &WhileStmt, span: Span) {
        match &while_stmt.condition {
            // ---- Context: ordinary boolean `while` condition ----
            Condition::Expr(expr) => {
                let cond_ty = self.check_expr(expr);
                self.validate_truthiness_condition(&cond_ty, expr.span);

                self.symbols.enter_scope(ScopeKind::Block);
                self.push_loop_context(LoopContextKind::Statement, None, span);
                self.check_statement_block(&while_stmt.body);
                let _ = self.pop_loop_context();
                self.symbols.exit_scope();
            }
            // ---- Context: pattern-driven `while let` loop ----
            Condition::Let { pattern, value } => {
                let value_ty = self.check_expr(value);
                self.note_loan_free_scrutinee(&value_ty, value.span);
                let view_param = self.pattern_view_param(value);
                self.symbols.enter_scope(ScopeKind::Block);
                self.check_pattern(pattern, &value_ty);
                let views = self.enter_pattern_views(&pattern.node, view_param);
                self.push_loop_context(LoopContextKind::Statement, None, span);
                self.check_statement_block(&while_stmt.body);
                let _ = self.pop_loop_context();
                self.exit_pattern_views(views);
                self.symbols.exit_scope();
                self.note_dict_lookup_let(value, pattern, &while_stmt.body);
            }
        }
    }

    /// Type-check a statement-form `loop:` body; the statement starts at `span`.
    ///
    /// Statement loops share the same loop context stack as `for` / `while`, but they do not accept `break <value>`
    /// because no surrounding expression consumes a result.
    pub(super) fn check_loop_stmt(&mut self, loop_stmt: &LoopStmt, span: Span) {
        self.symbols.enter_scope(ScopeKind::Block);
        self.push_loop_context(LoopContextKind::Statement, None, span);
        self.check_statement_block(&loop_stmt.body);
        let _ = self.pop_loop_context();
        self.symbols.exit_scope();
    }

    /// Type-check a statement-form `for` at `span`, binding the loop pattern from builtin collections or RFC 068
    /// iteration hooks.
    pub(super) fn check_for_stmt(&mut self, for_stmt: &ForStmt, span: Span) {
        let elem_ty = match &for_stmt.iter.node {
            Expr::Try(inner) => {
                let iter_ty = self.check_expr(inner);
                if let Some(unwrapped_iter_ty) = iter_ty.result_ok_type().cloned() {
                    if self.probe_fallible_iteration_protocol(&unwrapped_iter_ty, for_stmt.iter.span) {
                        self.errors
                            .push(errors::result_wrapped_fallible_iterator_requires_local(
                                for_stmt.iter.span,
                            ));
                        ResolvedType::Unknown
                    } else {
                        // Preserve the established `for item in result_of_iterable?` spelling. Here `?` unwraps the
                        // iterable before ordinary `Option`-based iteration; it is not the fallible-poll protocol.
                        let unwrapped_iter_ty = self.validate_try_result_type(&iter_ty, for_stmt.iter.span);
                        self.record_expr_type(for_stmt.iter.span, unwrapped_iter_ty.clone());
                        self.retain_body_ir_result_value(&for_stmt.iter, &unwrapped_iter_ty);
                        self.infer_iterator_element_type_from_expr(&for_stmt.iter, &unwrapped_iter_ty)
                    }
                } else {
                    self.infer_fallible_iterator_element_type_from_expr(inner, &iter_ty, for_stmt.iter.span)
                }
            }
            _ => {
                let iter_ty = self.check_expr(&for_stmt.iter);
                let iter_ty = self.settle_open_constructor_sides_in_place(&for_stmt.iter, iter_ty);
                self.infer_iterator_element_type_from_expr(&for_stmt.iter, &iter_ty)
            }
        };
        let owns_items = self.plan_for_item_taking(for_stmt, &elem_ty);

        // The iterated place is resolved before the loop's bindings shadow it (`for items in items:`).
        let previous_borrows = if owns_items {
            self.iteration_borrows.len()
        } else {
            self.enter_iteration_borrow(&for_stmt.iter)
        };
        let loop_view_param = self.loop_view_param(&for_stmt.iter);
        let derived_views = if loop_view_param.is_none() {
            self.read_only_derived_loop_views(&for_stmt.iter, &for_stmt.pattern.node)
        } else {
            Vec::new()
        };
        self.symbols.enter_scope(ScopeKind::Block);
        // Record the resolved element type at the pattern's own span. Body IR's `lower_for` already reads the loop
        // pattern's type back through `TypeCheckInfo::expr_type`, and every binding the pattern introduces -- one
        // for a plain binding, or one per tuple element -- takes its type from it (#1125). Without this the loop
        // bindings would carry `Unknown` even though the element type is fully resolved right here.
        self.record_expr_type(for_stmt.pattern.span, elem_ty.clone());
        self.define_for_pattern_bindings(&for_stmt.pattern, &elem_ty);
        self.retain_body_ir_iteration_binding(&for_stmt.pattern, &for_stmt.iter);
        self.remember_for_pattern_bindings(&for_stmt.pattern.node, owns_items);
        self.push_loop_context(LoopContextKind::Statement, None, span);
        let loop_body = self.enter_for_loop_body(&for_stmt.pattern.node, loop_view_param, derived_views);

        self.check_statement_block(&for_stmt.body);
        self.iteration_borrows.truncate(previous_borrows);
        self.exit_for_loop_body(loop_body);
        let _ = self.pop_loop_context();
        self.symbols.exit_scope();
    }

    /// Resolve the element types a statement destructure should bind, reporting the right diagnostic when the
    /// value cannot supply them.
    ///
    /// Returns exactly `arity` types so callers can bind positionally without re-checking length. The `Unknown`
    /// padding it returns on the error paths is recovery state for the *rest* of the check, not a claim that the
    /// value had that shape — the diagnostic has already been recorded by then.
    pub(super) fn destructured_element_types(
        &mut self,
        value_ty: &ResolvedType,
        arity: usize,
        span: Span,
    ) -> Vec<ResolvedType> {
        match classify_tuple_shape(value_ty) {
            TupleShape::Tuple(types) => {
                if types.len() != arity {
                    self.errors
                        .push(errors::tuple_unpack_count_mismatch(arity, types.len(), span));
                    return vec![ResolvedType::Unknown; arity];
                }
                types
            }
            TupleShape::RustTuple(rust_arity) => {
                if rust_arity != arity {
                    self.errors
                        .push(errors::tuple_unpack_count_mismatch(arity, rust_arity, span));
                }
                vec![ResolvedType::Unknown; arity]
            }
            TupleShape::OpaqueRust => {
                self.errors.push(errors::tuple_unpack_rust_shape_unverified(
                    arity,
                    &value_ty.to_string(),
                    span,
                ));
                vec![ResolvedType::Unknown; arity]
            }
            TupleShape::NotTuple => {
                self.errors.push(errors::tuple_unpack_expects_tuple_value(
                    arity,
                    &value_ty.to_string(),
                    span,
                ));
                vec![ResolvedType::Unknown; arity]
            }
            TupleShape::Recovery => vec![ResolvedType::Unknown; arity],
        }
    }

    /// Validate a `break` statement against the innermost active loop context.
    ///
    /// For expression-form `loop:` bodies this records the break value type so the loop result can be resolved after
    /// the body finishes checking. For statement loops it rejects `break <value>` while still type-checking the
    /// provided expression to surface any nested errors. The exit is recorded, since it can skip the statements after
    /// it in the loop it leaves.
    pub(super) fn check_break_stmt(&mut self, value: Option<&Spanned<Expr>>, span: Span) {
        self.note_loop_exit(span);
        let Some((loop_kind, expected_break_ty)) = self
            .loop_stack
            .last()
            .map(|ctx| (ctx.kind, ctx.expected_break_ty.clone()))
        else {
            if let Some(value) = value {
                self.check_expr(value);
            }
            self.errors.push(errors::break_outside_loop(span));
            return;
        };

        let break_ty = match (loop_kind, value) {
            (LoopContextKind::Statement, Some(value)) => {
                let value_ty = self.check_expr(value);
                self.errors
                    .push(errors::break_value_requires_loop_expression(value.span));
                Some((value_ty, value.span))
            }
            (LoopContextKind::Statement, None) => None,
            (LoopContextKind::Expression, Some(value)) => {
                let value_ty = if let Some(expected) = expected_break_ty.as_ref() {
                    self.check_expr_with_expected(value, Some(expected))
                } else {
                    self.check_expr(value)
                };
                Some((value_ty, value.span))
            }
            (LoopContextKind::Expression, None) => Some((ResolvedType::Unit, span)),
        };

        if let Some(break_ty) = break_ty
            && let Some(loop_ctx) = self.current_loop_context_mut()
        {
            loop_ctx.break_types.push(break_ty);
        }
    }

    /// Validate that `continue` appears inside some active loop context, and record the exit, which can skip the
    /// statements after it in that loop.
    pub(super) fn check_continue_stmt(&mut self, span: Span) {
        self.note_loop_exit(span);
        if self.loop_stack.is_empty() {
            self.errors.push(errors::continue_outside_loop(span));
        }
    }

    /// Define loop-scope bindings introduced by a `for` header pattern.
    ///
    /// The parser currently admits only bindings, `_`, and tuple bindings, but the exhaustive match keeps hand-built
    /// ASTs from silently reaching lowering with unsupported pattern forms.
    pub(in crate::typechecker) fn define_for_pattern_bindings(
        &mut self,
        pattern: &Spanned<Pattern>,
        ty: &ResolvedType,
    ) {
        match &pattern.node {
            Pattern::Binding(name) => {
                self.validate_protected_builtin_binding(name, pattern.span);
                self.symbols.define(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Variable(VariableInfo {
                        ty: ty.clone(),
                        is_mutable: false,
                        is_used: false,
                    }),
                    span: pattern.span,
                    scope: 0,
                });
                self.record_write_target_identity(pattern.span, name);
            }
            Pattern::Wildcard => {}
            Pattern::Tuple(items) => {
                // The shape question is answered by `classify_tuple_shape`, shared with the statement-level
                // destructuring arms (#1132). Only the diagnostic differs: a loop names the *iteration item* type,
                // because that is what the reader has to change.
                let element_types = match classify_tuple_shape(ty) {
                    TupleShape::Tuple(types) => {
                        if types.len() != items.len() {
                            self.errors.push(errors::tuple_unpack_count_mismatch(
                                items.len(),
                                types.len(),
                                pattern.span,
                            ));
                            vec![ResolvedType::Unknown; items.len()]
                        } else {
                            types
                        }
                    }
                    TupleShape::RustTuple(rust_arity) => {
                        if rust_arity != items.len() {
                            self.errors.push(errors::tuple_unpack_count_mismatch(
                                items.len(),
                                rust_arity,
                                pattern.span,
                            ));
                        }
                        vec![ResolvedType::Unknown; items.len()]
                    }
                    TupleShape::OpaqueRust => {
                        self.errors.push(errors::for_pattern_rust_shape_unverified(
                            items.len(),
                            &ty.to_string(),
                            pattern.span,
                        ));
                        vec![ResolvedType::Unknown; items.len()]
                    }
                    TupleShape::NotTuple => {
                        // A tuple pattern can only destructure a tuple. Binding `Unknown` per name and staying
                        // silent used to hide `for a, b in items` over a `list[int]` completely, and Body IR would
                        // then project `.0`/`.1` out of an `int` (#1125).
                        self.errors.push(errors::for_pattern_expects_tuple_item(
                            items.len(),
                            &ty.to_string(),
                            pattern.span,
                        ));
                        vec![ResolvedType::Unknown; items.len()]
                    }
                    TupleShape::Recovery => vec![ResolvedType::Unknown; items.len()],
                };

                for (i, item) in items.iter().enumerate() {
                    let item_ty = element_types.get(i).cloned().unwrap_or(ResolvedType::Unknown);
                    self.define_for_pattern_bindings(item, &item_ty);
                }
            }
            Pattern::Constructor(_, _) | Pattern::Literal(_) | Pattern::Group(_) | Pattern::Or(_) => {
                self.errors.push(errors::expected_token_message(
                    "Expected identifier, wildcard, or tuple binding in for-loop pattern",
                    &format!("{:?}", pattern.node),
                    pattern.span,
                ));
            }
        }
    }

    /// Validate a condition and its true branch body.
    pub(super) fn check_condition_body(
        &mut self,
        condition: &Condition,
        body: &[Spanned<Statement>],
        incoming_refinements: &[BranchRefinement],
    ) -> Option<BranchRefinement> {
        match condition {
            Condition::Expr(expr) => self.check_expr_condition_body(expr, body, incoming_refinements),
            Condition::Let { pattern, value } => {
                let value_ty = self.check_expr(value);
                self.note_loan_free_scrutinee(&value_ty, value.span);
                let view_param = self.pattern_view_param(value);
                self.symbols.enter_scope(ScopeKind::Block);
                self.apply_branch_refinements(incoming_refinements);
                self.check_pattern(pattern, &value_ty);
                let views = self.enter_pattern_views(&pattern.node, view_param);
                self.check_statement_block(body);
                self.exit_pattern_views(views);
                self.symbols.exit_scope();
                self.note_dict_lookup_let(value, pattern, body);
                None
            }
        }
    }

    /// Check an assert statement.
    pub(super) fn check_assert_stmt(&mut self, assert_stmt: &AssertStmt) {
        match &assert_stmt.kind {
            AssertKind::Condition(condition) => {
                let cond_ty = self.check_expr(condition);
                let is_compatible = self.types_compatible(&cond_ty, &ResolvedType::Bool);
                ensure_bool_condition(&cond_ty, condition.span, is_compatible, &mut self.errors);
            }
            AssertKind::IsPattern { value, pattern } => self.check_assert_is_pattern(value, pattern),
            AssertKind::Raises { call, error_type } => {
                self.check_expr(call);
                if let Type::Simple(name) = &error_type.node
                    && (runtime_errors::from_str(name).is_some() || name == "AssertionError")
                {
                    // Known runtime error vocabulary.
                } else {
                    self.errors
                        .push(errors::unknown_symbol(&error_type.node.to_string(), error_type.span));
                }
            }
        }

        if let Some(message) = &assert_stmt.message {
            let msg_ty = self.check_expr(message);
            if !self.types_compatible(&msg_ty, &ResolvedType::Str) {
                self.errors.push(errors::type_mismatch(
                    &ResolvedType::Str.to_string(),
                    &msg_ty.to_string(),
                    message.span,
                ));
            }
        }
    }

    /// Validate the restricted RFC 018 `assert value is Some/None/Ok/Err` pattern subset.
    pub(super) fn check_assert_is_pattern(&mut self, scrutinee: &Spanned<Expr>, pattern: &Spanned<Pattern>) {
        let scrutinee_ty = self.check_expr(scrutinee);
        let Some(pattern) = Self::assert_is_pattern_from_pattern(pattern) else {
            self.errors.push(errors::expected_token_message(
                "Expected assert `is` pattern Some(name), Some(_), None, Ok(name), Ok(_), Err(name), or Err(_)",
                &format!("{:?}", pattern.node),
                pattern.span,
            ));
            return;
        };

        let expected = match pattern.kind {
            AssertIsPatternKind::Some | AssertIsPatternKind::None => "Option[_]",
            AssertIsPatternKind::Ok | AssertIsPatternKind::Err => "Result[_, _]",
        };
        let compatible = match pattern.kind {
            AssertIsPatternKind::Some | AssertIsPatternKind::None => scrutinee_ty.is_option(),
            AssertIsPatternKind::Ok | AssertIsPatternKind::Err => scrutinee_ty.is_result(),
        };
        if !compatible && !matches!(scrutinee_ty, ResolvedType::Unknown) {
            self.errors.push(errors::type_mismatch(
                expected,
                &scrutinee_ty.to_string(),
                scrutinee.span,
            ));
            return;
        }

        if compatible {
            let constructor = match pattern.kind {
                AssertIsPatternKind::Some => ConstructorId::Some,
                AssertIsPatternKind::None => ConstructorId::None,
                AssertIsPatternKind::Ok => ConstructorId::Ok,
                AssertIsPatternKind::Err => ConstructorId::Err,
            };
            self.record_pattern_lexical_identity(constructors::as_str(constructor), pattern.constructor_span);
        }

        if let Some((name, span)) = pattern.binding {
            let ty = match pattern.kind {
                AssertIsPatternKind::Some => scrutinee_ty
                    .option_inner_type()
                    .cloned()
                    .unwrap_or(ResolvedType::Unknown),
                AssertIsPatternKind::Ok => scrutinee_ty.result_ok_type().cloned().unwrap_or(ResolvedType::Unknown),
                AssertIsPatternKind::Err => scrutinee_ty.result_err_type().cloned().unwrap_or(ResolvedType::Unknown),
                AssertIsPatternKind::None => ResolvedType::Unit,
            };
            // The assertion subset defines its one binding here rather than through `check_pattern`, so it records
            // the same per-pattern-node type fact that walk records (#1245): lowering reads one fact for a pattern
            // binding whichever construct owns it.
            self.record_expr_type(span, ty.clone());
            self.validate_protected_builtin_binding(&name, span);
            self.symbols.define(Symbol {
                name: name.clone(),
                kind: SymbolKind::Variable(VariableInfo {
                    ty,
                    is_mutable: false,
                    is_used: false,
                }),
                span,
                scope: 0,
            });
            self.record_write_target_identity(span, &name);
        }
    }

    /// Build an assertion pattern from a parsed pattern.
    pub(super) fn assert_is_pattern_from_pattern(pattern: &Spanned<Pattern>) -> Option<AssertIsPattern> {
        match &pattern.node {
            Pattern::Constructor(name, args)
                if name.node == constructors::as_str(ConstructorId::None) && args.is_empty() =>
            {
                Some(AssertIsPattern {
                    kind: AssertIsPatternKind::None,
                    constructor_span: name.span,
                    binding: None,
                })
            }
            Pattern::Constructor(name, args) => {
                let kind = match name.node.as_str() {
                    n if n == constructors::as_str(ConstructorId::Some) => AssertIsPatternKind::Some,
                    n if n == constructors::as_str(ConstructorId::Ok) => AssertIsPatternKind::Ok,
                    n if n == constructors::as_str(ConstructorId::Err) => AssertIsPatternKind::Err,
                    _ => return None,
                };
                let [PatternArg::Positional(arg)] = args.as_slice() else {
                    return None;
                };
                let binding = match &arg.node {
                    Pattern::Wildcard => None,
                    Pattern::Binding(name) => Some((name.clone(), arg.span)),
                    _ => return None,
                };
                Some(AssertIsPattern {
                    kind,
                    constructor_span: name.span,
                    binding,
                })
            }
            _ => None,
        }
    }

    /// Typecheck a surface statement via the semantics registry.
    pub(super) fn check_surface_stmt(&mut self, stmt: &SurfaceStmt, span: Span) {
        use crate::semantics_registry::semantics_registry;

        let Some(action) = semantics_registry().typecheck_surface_stmt_action(&stmt.key) else {
            // No pack claimed this surface statement — report as unknown.
            let label = match &stmt.key {
                incan_semantics_core::SurfaceFeatureKey::SoftKeyword(id) => keywords::as_str(*id).to_string(),
                incan_semantics_core::SurfaceFeatureKey::Decorator(_) => "decorator-surface-feature".to_string(),
                incan_semantics_core::SurfaceFeatureKey::ScopedDslSurface {
                    dependency_key,
                    descriptor_key,
                } => {
                    format!("{dependency_key}:{descriptor_key}")
                }
            };
            self.errors.push(errors::unknown_symbol(&label, span));
            return;
        };

        match (action, &stmt.payload) {
            (SurfaceStmtTypeCheck::AssertCheck, SurfaceStmtPayload::KeywordArgs(args)) => {
                if let Some(condition) = args.first() {
                    let assert_stmt = AssertStmt {
                        kind: AssertKind::Condition(condition.clone()),
                        message: args.get(1).cloned(),
                    };
                    self.check_assert_stmt(&assert_stmt);
                }
            }
        }
    }

    /// Infer the item type for built-in iterable surfaces.
    ///
    /// This captures language-level iteration semantics, such as strings yielding one-character `str` values, bytes
    /// yielding `int` values, and a type parameter bounded by `Iterable[T]` yielding `T`, before the backend chooses
    /// the Rust iterator adapter that implements them.
    pub fn infer_iterator_element_type(&self, iter_ty: &ResolvedType) -> ResolvedType {
        if let Some(type_param) = self.active_type_param_name(iter_ty)
            && let Some(item) = self
                .innermost_type_param_bounds(type_param)
                .into_iter()
                .flatten()
                .find_map(|bound| {
                    let trait_id = builtin_traits::from_qualified_str(&bound.name)
                        .or_else(|| builtin_traits::from_str(Self::type_bound_source_name(bound)))?;
                    if matches!(trait_id, TraitId::Iterable | TraitId::Iterator | TraitId::IntoIterator) {
                        bound.type_args.first().cloned()
                    } else {
                        None
                    }
                })
        {
            return item;
        }
        match iter_ty {
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => self.infer_iterator_element_type(inner),
            ResolvedType::FrozenList(elem) | ResolvedType::FrozenSet(elem) => elem.as_ref().clone(),
            ResolvedType::FrozenDict(key, _) => key.as_ref().clone(),
            ResolvedType::Generic(name, args) => {
                if let Some(elem) = iter_ty.iterator_item_type() {
                    return elem.clone();
                }
                // A range is not a collection, so it has no `collection_type_id`, but iterating one still yields its
                // element type. The inline `for i in 0..10` header never reaches here because `check_for_stmt`
                // resolves that expression directly; only a range *bound to a local* arrives as a `Range[T]` value.
                // Without this arm that binding iterates with an unknown item type, which stays invisible until
                // something downstream needs the type -- `acc + i` refusing to lower, for instance.
                if name == incan_lang::lang::surface::types::RANGE_TYPE_NAME && !args.is_empty() {
                    return args[0].clone();
                }
                match collection_type_id(name.as_str()) {
                    Some(CollectionTypeId::List) | Some(CollectionTypeId::Set) if !args.is_empty() => args[0].clone(),
                    Some(CollectionTypeId::Dict) if args.len() >= 2 => {
                        // Iterating dict gives keys
                        args[0].clone()
                    }
                    Some(CollectionTypeId::Tuple) if !args.is_empty() => {
                        // For tuple iteration, return first element type (simplified)
                        args[0].clone()
                    }
                    Some(CollectionTypeId::Generator) if !args.is_empty() => args[0].clone(),
                    _ => ResolvedType::Unknown,
                }
            }
            ResolvedType::Str | ResolvedType::FrozenStr => ResolvedType::Str,
            ResolvedType::Bytes | ResolvedType::FrozenBytes => ResolvedType::Int,
            _ => ResolvedType::Unknown,
        }
    }

    /// Infer a loop item type from an iterable expression, falling back to structural `__iter__` / `__next__` hooks.
    ///
    /// Every `for` statement and comprehension clause iterates through here, so this is also where iterating a
    /// generator that a caller-visible `mut` parameter holds is recorded as a change to the parameter. Builtin
    /// iterator values retain builtin polling. Native items come from the selected `Iterator::Item` or
    /// `IntoIterator::Item` projection; a collection's owned iteration need not implement Iterator itself. Structural
    /// hooks are selected only by the custom protocol resolver.
    pub fn infer_iterator_element_type_from_expr(
        &mut self,
        iter_expr: &Spanned<Expr>,
        iter_ty: &ResolvedType,
    ) -> ResolvedType {
        self.note_mut_param_iteration(iter_expr, iter_ty);
        if iter_ty.iterator_item_type().is_some()
            && let Expr::Ident(name) = &iter_expr.node
        {
            self.consumed_iterator_bindings.insert(name.clone(), iter_expr.span);
        }
        let elem_ty = self.infer_iterator_element_type(iter_ty);
        // A source protocol default can return its opaque Iterator contract while retaining a concrete source
        // adopter. Keep its checked hooks rather than routing that value through builtin collection polling.
        if iter_ty.iterator_item_type().is_some()
            && self.type_info.resolved_method_call(iter_expr.span).is_some_and(|call| {
                let super::ResolvedMethodDispatch::Trait { trait_name, .. } = &call.dispatch;
                matches!(iter_ty, ResolvedType::Generic(name, _) if name == trait_name
                || builtin_traits::from_qualified_str(name).is_some_and(|id| {
                    builtin_traits::from_qualified_str(trait_name) == Some(id)
                }))
            })
        {
            return self
                .retain_owned_source_iterator_protocol(iter_ty, iter_expr.span)
                .unwrap_or(ResolvedType::Unknown);
        }
        if !matches!(elem_ty, ResolvedType::Unknown) || matches!(iter_ty, ResolvedType::Unknown) {
            return elem_ty;
        }
        if let Some(item) = self.retain_rust_iteration_hooks(iter_expr, iter_ty) {
            if let Expr::Ident(name) = &iter_expr.node {
                self.consumed_iterator_bindings.insert(name.clone(), iter_expr.span);
            }
            return item;
        }
        if let Some(item) = self.rust_associated_type(iter_ty, "std::iter::Iterator", "Item") {
            return item;
        }
        if let Some(item) = self.rust_associated_type(iter_ty, "std::iter::IntoIterator", "Item") {
            return item;
        }
        if self.is_user_operator_receiver(iter_ty) {
            return self
                .resolve_iteration_protocol(iter_ty, iter_expr.span)
                .unwrap_or(ResolvedType::Unknown);
        }
        elem_ty
    }

    /// Infer a loop item from a `for item in iterable?` header.
    ///
    /// The trailing `?` belongs to the loop protocol rather than to the iterable expression: it asks lowering to
    /// propagate errors returned from each `__next__` poll. Ordinary `expr?` remains the existing Result expression.
    pub(super) fn infer_fallible_iterator_element_type_from_expr(
        &mut self,
        iter_expr: &Spanned<Expr>,
        iter_ty: &ResolvedType,
        loop_span: Span,
    ) -> ResolvedType {
        let has_source_trait_dispatch = self
            .type_info
            .resolved_method_call(iter_expr.span)
            .is_some_and(|call| matches!(call.dispatch, crate::typechecker::ResolvedMethodDispatch::Trait { .. }));
        if self.is_user_operator_receiver(iter_ty) || has_source_trait_dispatch {
            return self
                .resolve_fallible_iteration_protocol(iter_ty, iter_expr.span, loop_span)
                .unwrap_or(ResolvedType::Unknown);
        }

        self.errors.push(errors::type_mismatch(
            "fallible iterator with __iter__() and __next__() -> Result[Option[_], _]",
            &iter_ty.to_string(),
            iter_expr.span,
        ));
        ResolvedType::Unknown
    }
}
