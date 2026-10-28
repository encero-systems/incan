//! Lowering for list and dict comprehensions and generator expressions, and the clause/terminal machinery they share.

use super::args::*;
use super::control_flow::intersect_range_layouts;
use super::free_vars::*;
use super::primitives::collection_member_types;
use super::reads::*;
use super::*;
use crate::typechecker::ProtocolIterationInfo;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Lower a scalar-literal range comprehension through its retained exact cardinality when available. Other
    /// list comprehensions build an empty list and push each accepted element through the desugared clause loop.
    /// Only v0's single mirrored `(pattern, iter, filter)` clause is lowered -- `comp.clauses` is intentionally
    /// not consulted, since the typechecker (`check_list_comp` in
    /// `loaves/compiler/incan_frontend/src/typechecker/check_expr/comps.rs`) does not read it; a list comprehension
    /// with more than one `for` clause is not actually type-checked or emitted as multi-clause today; treating
    /// `comp.clauses` as authoritative here would silently lower a shape nothing else in the pipeline validates.
    pub(super) fn lower_list_comp(
        &mut self,
        comp: &ast::ListComp,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let ty = self.refine_task_storage(
            &self.resolve_ty(span),
            &IncanType::Generic {
                base: "List".into(),
                args: vec![self.retained_expression_type(&comp.expr)],
            },
        );
        // An `Ok(...)` or `Err(...)` element is built with the element type the checker settled for the
        // comprehension, which fixes the side the constructor leaves open.
        if let Some([element_ty]) = collection_member_types(&ty, CollectionTypeId::List) {
            self.settle_open_result_value(&comp.expr, element_ty);
        }
        if let Some(filled) = self.lower_uniform_range_list(comp, &ty, span, scope, out) {
            return filled;
        }
        let list_local = self.new_temp(ty.clone(), scope, hir_span_value);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(list_local),
                rvalue: bir::Rvalue::Aggregate(bir::AggregateKind::List, Vec::new()),
            },
            span: hir_span_value,
        });
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);

        let clauses = single_comprehension_clauses(&comp.pattern, &comp.iter, comp.filter.as_ref());
        let terminal = ComprehensionTerminal::ListPush {
            list_local,
            element: &comp.expr,
        };
        self.lower_scoped_comprehension_clauses(&clauses, &terminal, scope, hir_span_value, out);
        self.temp_operand(list_local, &self.locals[list_local.index()].ty)
    }

    /// Retain a scalar literal comprehension's exact cardinality instead of discarding it into repeated pushes.
    /// Literal evaluation has no effects or ownership transfers. Filters, computed elements and non-unit ranges
    /// keep ordinary iteration; bounds still evaluate once through the canonical range boundary.
    fn lower_uniform_range_list(
        &mut self,
        comp: &ast::ListComp,
        ty: &IncanType,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        if comp.filter.is_some()
            || !matches!(
                &comp.expr.node,
                ast::Expr::Literal(ast::Literal::Bool(_) | ast::Literal::Int(_) | ast::Literal::Float(_))
            )
        {
            return None;
        }
        let count = match self.zero_based_unit_range_count(&comp.iter, scope, out) {
            Ok(Some(count)) => count,
            Ok(None) => return None,
            Err(reason) => {
                return Some(self.unsupported_operand(reason, scope, hir_span(span), out));
            }
        };
        let value = self.lower_expr_to_operand(&comp.expr, scope, out);
        let helper = bir::HelperOp::RangeListFill;
        let span = hir_span(span);
        self.record_runtime_requirement(AbiV0RuntimeRequirement::RuntimeHelper(helper.as_str().into()));
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
        self.record_runtime_requirement(AbiV0RuntimeRequirement::PanicStrategy);
        self.panic_facts.push(bir::PanicFact {
            span,
            reason: bir::PanicReason::HelperMayPanic(helper),
        });
        Some(self.push_call_temp(
            bir::Callee::Helper(helper),
            fixed_elements(vec![value, count]),
            ty.clone(),
            scope,
            span,
            true,
            out,
        ))
    }

    /// Lower a dict comprehension `{key: value for pattern in iter if filter}` the same way
    /// [`Self::lower_list_comp`] lowers a list comprehension, but growing an `AggregateKind::Dict` temporary via a
    /// compiler-synthesized `insert` call. See [`Self::lower_list_comp`]'s docs for why only the single mirrored
    /// clause is lowered, not `comp.clauses`.
    pub(super) fn lower_dict_comp(
        &mut self,
        comp: &ast::DictComp,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let ty = self.resolve_ty(span);
        let dict_local = self.new_temp(ty.clone(), scope, hir_span_value);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(dict_local),
                rvalue: bir::Rvalue::Dict(Vec::new()),
            },
            span: hir_span_value,
        });
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);

        let clauses = single_comprehension_clauses(&comp.pattern, &comp.iter, comp.filter.as_ref());
        let terminal = ComprehensionTerminal::DictInsert {
            dict_local,
            key: &comp.key,
            value: &comp.value,
        };
        self.lower_scoped_comprehension_clauses(&clauses, &terminal, scope, hir_span_value, out);
        self.temp_operand(dict_local, &ty)
    }

    /// Lower a generator expression into a distinct, deferred [`bir::Rvalue::Generator`].
    ///
    /// The first `for` source is evaluated exactly once at construction, matching the established legacy
    /// iterator-adapter emitter. Its value and every other needed outer lexical value are then captured into fresh
    /// generator-local bindings. Clause polling, later `for` sources, filters, and element evaluation lower only
    /// into the generator body, so the enclosing body neither materializes the sequence nor runs a deferred effect.
    ///
    /// Generator clause patterns use the same checked item-binding and projection helpers as statement loops.
    /// The generator's lexical bindings and deferred range-layout facts do not escape into the enclosing body.
    pub(super) fn lower_generator_expr(
        &mut self,
        generator: &ast::GeneratorExpr,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let Some((first_clause, remaining_clauses)) = generator.clauses.split_first() else {
            return self.unsupported_operand(
                "generator expression without a for clause".to_string(),
                scope,
                hir_span_value,
                out,
            );
        };
        let ast::ComprehensionClause::For {
            pattern: first_pattern,
            iter: first_iter,
        } = first_clause
        else {
            return self.unsupported_operand(
                "generator expression whose first clause is not a for clause".to_string(),
                scope,
                hir_span_value,
                out,
            );
        };
        // The first source is the legacy adapter chain's eager boundary. Lowering it before creating the rvalue
        // preserves source-visible construction timing; all remaining expression lowering below writes only into
        // `generator_stmts` and therefore happens at poll time.
        let first_protocol = self.type_info.protocol_iteration(first_iter.span).cloned();
        let first_is_fallible = first_protocol
            .as_ref()
            .is_some_and(|protocol| protocol.fallible_error_type.is_some());
        let effective_first_iter: &ast::Spanned<ast::Expr> = match (&first_iter.node, first_is_fallible) {
            (ast::Expr::Try(inner), true) => inner,
            _ => first_iter,
        };
        let source = self.lower_expr_to_operand(effective_first_iter, scope, out);

        let generator_scope = self.new_scope(Some(scope), hir_span_value);
        let source_local = self.new_temp(
            self.resolve_ty(effective_first_iter.span),
            generator_scope,
            hir_span_value,
        );
        self.locals[source_local.index()].origin = bir::LocalOrigin::Captured;

        // Capture every lexical value used after the first source once, at construction. The body cannot read the
        // enclosing place directly after this point, and restoring the full binding map below prevents generator
        // clause/capture names from leaking into the following enclosing statement.
        let enclosing_bindings = self.bindings.clone();
        let enclosing_identity_bindings = self.identity_bindings.clone();
        // Only the first source is evaluated at construction. The rest is deferred until polling, so range-layout
        // facts it creates belong to the generator frame rather than the enclosing straight-line body.
        let saved_materialized_range_locals = self.materialized_range_locals.clone();
        let (captured_operands, capture_locals) =
            self.lower_generator_captures(generator, generator_scope, hir_span_value);
        let generator_stmts = self.lower_generator_body(
            generator,
            first_pattern,
            remaining_clauses,
            first_protocol.as_ref(),
            effective_first_iter.span,
            source_local,
            generator_scope,
            hir_span_value,
        );
        self.bindings = enclosing_bindings;
        self.identity_bindings = enclosing_identity_bindings;
        self.materialized_range_locals = saved_materialized_range_locals;

        // `Generator::new` owns a boxed iterator in the legacy runtime, even when every captured source value is
        // Copy-shaped. Record that allocation fact directly rather than relying on incidental temporary locals.
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
        let ty = self.resolve_ty(span);
        work::nested_body(self.module_identity, span, "generator");
        self.push_assign_temp(
            bir::Rvalue::Generator {
                source,
                captured_operands,
                body: Box::new(bir::GeneratorBody {
                    source_local,
                    capture_locals,
                    stmts: generator_stmts,
                }),
            },
            ty,
            scope,
            hir_span_value,
            out,
        )
    }

    /// Capture each enclosing local used after the eager source once, retaining its checked ownership fact and
    /// identity. Names without frame locals remain available for deferred canonical global resolution.
    fn lower_generator_captures(
        &mut self,
        generator: &ast::GeneratorExpr,
        generator_scope: bir::ScopeId,
        span: HirSourceSpan,
    ) -> (Vec<bir::Operand>, Vec<bir::LocalId>) {
        let free_names = free_vars_in_generator_deferred_body(generator);
        let mut captured_operands = Vec::with_capacity(free_names.len());
        let mut capture_locals = Vec::with_capacity(free_names.len());
        for name in &free_names {
            let Some(&outer_local) = self.bindings.get(name) else {
                // Names without an enclosing frame local are not captures. The deferred body resolves them again
                // from compiler-recorded identity: module storage becomes a canonical global place, while a proven
                // identity Body IR cannot represent becomes an explicit unsupported operand.
                continue;
            };
            let outer_ty = self.locals[outer_local.index()].ty.clone();
            let outer_place = bir::Place::from_local(outer_local);
            let (fact, last_use) = self.ownership_fact_for_place(&outer_place, &outer_ty);
            captured_operands.push(bir::Operand::place(outer_place, fact, last_use));

            let total_reads = count_reads_in_generator_deferred_body(name, generator);
            let capture_local =
                self.declare_new_local_with_reads(name.clone(), outer_ty, generator_scope, span, total_reads);
            self.locals[capture_local.index()].origin = bir::LocalOrigin::Captured;
            if let Some(identity) = self.locals[outer_local.index()].identity.clone() {
                self.locals[capture_local.index()].identity = Some(identity.clone());
                self.identity_bindings.insert(identity, capture_local);
            }
            capture_locals.push(capture_local);
        }
        (captured_operands, capture_locals)
    }

    /// Build only deferred polling statements, binding the first item before lowering the remaining clauses.
    /// Iterator construction stays inside the generator body; its source operand has already been captured.
    fn lower_generator_body(
        &mut self,
        generator: &ast::GeneratorExpr,
        first_pattern: &ast::Spanned<ast::Pattern>,
        remaining_clauses: &[ast::ComprehensionClause],
        first_protocol: Option<&ProtocolIterationInfo>,
        source_span: ast::Span,
        source_local: bir::LocalId,
        generator_scope: bir::ScopeId,
        hir_span_value: HirSourceSpan,
    ) -> Vec<bir::Statement> {
        let first_loop_scope = self.new_scope(Some(generator_scope), hir_span_value);
        // The first clause binds through the same helpers a statement `for` uses, so a destructuring generator
        // clause produces the same facts as the equivalent loop (#1161). The item type comes from the pattern's own
        // span, which the typechecker records for comprehension clauses as it does for a statement `for`.
        let first_item_ty = self.resolve_ty(first_pattern.span);
        let first_local = self.declare_for_item_local(
            first_pattern,
            &first_item_ty,
            first_loop_scope,
            hir_span(first_pattern.span),
            &|name| {
                count_reads_in_expr(name, &generator.expr.node)
                    + count_reads_in_comprehension_clauses(name, remaining_clauses)
            },
        );

        let mut generator_stmts = Vec::new();
        let iterator_local = self.lower_generator_iterator(
            first_protocol,
            source_span,
            source_local,
            generator_scope,
            hir_span_value,
            &mut generator_stmts,
        );

        self.loop_break_targets.push(None);
        self.loop_scopes.push(first_loop_scope);
        self.loop_continue_actions.push(Vec::new());
        let mut first_loop_stmts = vec![bir::Statement {
            kind: bir::StatementKind::IterNext {
                destination: bir::Place::from_local(first_local),
                iterator: bir::Operand::place(
                    bir::Place::from_local(iterator_local),
                    bir::OwnershipFact::MutBorrow,
                    false,
                ),
                protocol: match first_protocol {
                    Some(protocol) => bir::IterProtocol::UserDefined {
                        next_method: protocol.next_method.clone(),
                        fallible: protocol.fallible_error_type.is_some(),
                    },
                    None => bir::IterProtocol::Builtin,
                },
            },
            span: hir_span_value,
        }];
        // Project the pattern's fields straight after `IterNext` has written the item, the same ordering
        // `lower_for` uses, so every nested binding reads through a projection of a value that already exists.
        self.bind_for_pattern(
            first_pattern,
            &first_item_ty,
            first_local,
            first_loop_scope,
            &|name| {
                count_reads_in_expr(name, &generator.expr.node)
                    + count_reads_in_comprehension_clauses(name, remaining_clauses)
            },
            &mut first_loop_stmts,
        );
        let terminal = ComprehensionTerminal::GeneratorYield {
            element: &generator.expr,
        };
        self.lower_comprehension_clauses(
            remaining_clauses,
            &terminal,
            first_loop_scope,
            hir_span_value,
            &mut first_loop_stmts,
        );
        self.insert_scope_drops(&mut first_loop_stmts, first_loop_scope);
        self.loop_break_targets.pop();
        self.loop_scopes.pop();
        self.loop_continue_actions.pop();
        generator_stmts.push(bir::Statement {
            kind: bir::StatementKind::Loop {
                body: bir::Block {
                    scope: first_loop_scope,
                    stmts: first_loop_stmts,
                },
            },
            span: hir_span_value,
        });
        generator_stmts
    }

    /// Initialize the generator-local iterator through the checked protocol or builtin source borrow. This emits
    /// into the deferred statement list and must not evaluate the source again at construction.
    fn lower_generator_iterator(
        &mut self,
        protocol: Option<&ProtocolIterationInfo>,
        source_span: ast::Span,
        source_local: bir::LocalId,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::LocalId {
        let iterator_ty = match protocol {
            Some(protocol) => semantic_type_from_resolved(&protocol.iterator_type),
            None => self.resolve_ty(source_span),
        };
        let iterator_local = self.new_temp(iterator_ty, scope, span);
        match protocol {
            Some(protocol) => out.push(bir::Statement {
                kind: bir::StatementKind::Call {
                    destination: Some(bir::Place::from_local(iterator_local)),
                    callee: bir::Callee::Method(bir::MethodTarget::synthesized(protocol.iter_method.clone())),
                    args: fixed_elements(vec![bir::Operand::place(
                        bir::Place::from_local(source_local),
                        bir::OwnershipFact::Borrow,
                        false,
                    )]),
                    may_panic: false,
                },
                span,
            }),
            None => out.push(bir::Statement {
                kind: bir::StatementKind::Assign {
                    place: bir::Place::from_local(iterator_local),
                    rvalue: bir::Rvalue::Use(bir::Operand::place(
                        bir::Place::from_local(source_local),
                        bir::OwnershipFact::Borrow,
                        false,
                    )),
                },
                span,
            }),
        }
        iterator_local
    }

    /// Lower a comprehension/generator clause chain with bindings that are lexical to that expression. The clause
    /// lowering itself declares each `for` pattern binding through [`Self::declare_new_local_with_reads`] so normal
    /// operand lowering can resolve it. Those bindings must disappear when the expression ends, however: unlike a
    /// statement `for`, a comprehension's `x` in `[x for x in values]` cannot shadow an enclosing `x` in the next
    /// enclosing statement. Preserve the outer lookup map while retaining the locals and ownership facts the nested
    /// lowering legitimately recorded in the Body IR.
    pub(super) fn lower_scoped_comprehension_clauses(
        &mut self,
        clauses: &[ast::ComprehensionClause],
        terminal: &ComprehensionTerminal<'_>,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let enclosing_bindings = self.bindings.clone();
        let range_layouts_before = self.materialized_range_locals.clone();
        self.lower_comprehension_clauses(clauses, terminal, scope, span, out);
        self.bindings = enclosing_bindings;
        // A comprehension clause may execute zero times, so a source-local range layout constructed only inside
        // its body cannot become a fact about the following enclosing statement.
        self.materialized_range_locals =
            intersect_range_layouts(vec![range_layouts_before, self.materialized_range_locals.clone()]);
    }

    /// Recursively desugar a comprehension/generator clause chain into nested `Loop`/`If` statements, terminating
    /// in `terminal`'s compiler-synthesized collection-growth call once every clause has been satisfied for one
    /// binding combination. `For` clauses reuse [`Self::lower_general_iteration`] (the same builtin-vs-protocol
    /// iteration primitive [`Self::lower_for`] uses), so comprehensions never duplicate that split. A non-binding
    /// `For` clause pattern lowers to `Unsupported`, matching [`Self::lower_for`]'s own restriction (destructuring
    /// patterns need `match`-shaped compilation, out of scope here).
    /// A filtered continuation retains owners declared by preceding clauses when the filter skips that continuation.
    pub(super) fn lower_comprehension_clauses(
        &mut self,
        clauses: &[ast::ComprehensionClause],
        terminal: &ComprehensionTerminal<'_>,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let Some((head, tail)) = clauses.split_first() else {
            self.lower_comprehension_terminal(terminal, scope, out);
            return;
        };
        match head {
            ast::ComprehensionClause::If(cond) => {
                let cond_operand = self.lower_expr_to_operand(cond, scope, out);
                let then_scope = self.new_scope(Some(scope), span);
                let mut then_stmts = Vec::new();
                self.conditional_scopes.push(then_scope);
                self.lower_comprehension_clauses(tail, terminal, then_scope, span, &mut then_stmts);
                self.conditional_scopes.pop();
                out.push(bir::Statement {
                    kind: bir::StatementKind::If {
                        cond: cond_operand,
                        then_block: bir::Block {
                            scope: then_scope,
                            stmts: then_stmts,
                        },
                        else_block: None,
                    },
                    span,
                });
            }
            ast::ComprehensionClause::For { pattern, iter } => {
                // A destructuring clause binds exactly as the equivalent statement `for` does, through the same two
                // helpers (#1125): one declares the item local, the other projects the pattern's fields out of it.
                // Two spellings of the same iteration producing different bindings is the drift #1161 closes.
                //
                // The item type comes from the pattern's own span, which the typechecker now records for
                // comprehension clauses exactly as it does for a statement `for`.
                let item_ty = self.for_item_type(pattern, &self.retained_expression_type(iter));
                let loop_scope = self.new_scope(Some(scope), span);
                // One counter, used by both helpers: a clause name is read by the remaining clauses and by the
                // terminal, and the two helpers must agree on that or a binding's last-use lands in the wrong place.
                let reads =
                    move |name: &str| terminal.count_reads(name) + count_reads_in_comprehension_clauses(name, tail);
                let item_local =
                    self.declare_for_item_local(pattern, &item_ty, loop_scope, hir_span(pattern.span), &reads);
                let bind_ty = item_ty.clone();
                self.lower_general_iteration(
                    iter,
                    item_local,
                    scope,
                    loop_scope,
                    span,
                    out,
                    move |builder, loop_scope, body_stmts| {
                        builder.bind_for_pattern(pattern, &bind_ty, item_local, loop_scope, &reads, body_stmts);
                        builder.lower_comprehension_clauses(tail, terminal, loop_scope, span, body_stmts);
                        builder.insert_scope_drops(body_stmts, loop_scope);
                    },
                );
            }
        }
    }

    /// Lower the innermost action of one accepted comprehension/generator binding combination: evaluate the
    /// element (or key/value) expression(s) and push a compiler-synthesized `push`/`insert`
    /// [`bir::Callee::Method`] call growing the target collection. The receiver is read as
    /// [`bir::OwnershipFact::MutBorrow`] since the call mutates the collection in place -- the first real producer
    /// of that fact in this module (every other place read so far has been `Copy`/`Move`/`Clone`/`Borrow`).
    pub(super) fn lower_comprehension_terminal(
        &mut self,
        terminal: &ComprehensionTerminal<'_>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) {
        match terminal {
            ComprehensionTerminal::ListPush { list_local, element } => {
                let element_operand = self.lower_expr_to_operand(element, scope, out);
                // The comprehension's own checked binder is now installed. Preserve that operand's concrete task
                // storage, never an outer same-spelled binding observed before this lexical scope was entered.
                if let Some(actual) = self.retained_operand_type(&element_operand) {
                    let actual_list = IncanType::Generic {
                        base: "List".into(),
                        args: vec![actual],
                    };
                    self.locals[list_local.index()].ty =
                        self.refine_task_storage(&self.locals[list_local.index()].ty, &actual_list);
                }
                let span = hir_span(element.span);
                out.push(bir::Statement {
                    kind: bir::StatementKind::Call {
                        destination: None,
                        callee: bir::Callee::Method(bir::MethodTarget {
                            native_trait_delegation: None,
                            canonical: Some(crate::symbols::canonical_builtin_member_identity("List", "append")),
                            ..bir::MethodTarget::synthesized("push")
                        }),
                        args: fixed_elements(vec![
                            bir::Operand::place(
                                bir::Place::from_local(*list_local),
                                bir::OwnershipFact::MutBorrow,
                                false,
                            ),
                            element_operand,
                        ]),
                        may_panic: false,
                    },
                    span,
                });
            }
            ComprehensionTerminal::DictInsert { dict_local, key, value } => {
                let key_operand = self.lower_expr_to_operand(key, scope, out);
                let key_operand = self.retain_evaluated_value_read(key, key_operand, scope, out);
                let pending_start = self.pending_argument_reads.len();
                self.retain_pending_operand_read(&key_operand);
                let value_operand = self.lower_expr_to_operand(value, scope, out);
                self.pending_argument_reads.truncate(pending_start);
                let span = hir_span(value.span);
                out.push(bir::Statement {
                    kind: bir::StatementKind::Call {
                        destination: None,
                        callee: bir::Callee::Method(bir::MethodTarget {
                            native_trait_delegation: None,
                            canonical: Some(crate::symbols::canonical_builtin_member_identity("Dict", "insert")),
                            ..bir::MethodTarget::synthesized("insert")
                        }),
                        args: fixed_elements(vec![
                            bir::Operand::place(
                                bir::Place::from_local(*dict_local),
                                bir::OwnershipFact::MutBorrow,
                                false,
                            ),
                            key_operand,
                            value_operand,
                        ]),
                        may_panic: false,
                    },
                    span,
                });
            }
            ComprehensionTerminal::GeneratorYield { element } => {
                let value = self.lower_expr_to_operand(element, scope, out);
                out.push(bir::Statement {
                    kind: bir::StatementKind::Yield { value },
                    span: hir_span(element.span),
                });
            }
        }
    }
}

/// The innermost action a list/dict-comprehension clause chain performs once every clause accepts one binding
/// combination -- what [`BodyBuilder::lower_comprehension_terminal`] lowers. It distinguishes a list's
/// single-element push from a dict's key/value insert while sharing the same clause-chain desugar.
pub(super) enum ComprehensionTerminal<'a> {
    /// Push `element`'s value into the list at `list_local`.
    ListPush {
        list_local: bir::LocalId,
        element: &'a ast::Spanned<ast::Expr>,
    },
    /// Insert `key`/`value` into the dict at `dict_local`.
    DictInsert {
        dict_local: bir::LocalId,
        key: &'a ast::Spanned<ast::Expr>,
        value: &'a ast::Spanned<ast::Expr>,
    },
    /// Suspend the surrounding generator body with `element` for one accepted binding combination.
    GeneratorYield { element: &'a ast::Spanned<ast::Expr> },
}
impl ComprehensionTerminal<'_> {
    /// Count `name` occurrences in this terminal's own expression(s), for seeding a comprehension `for`-clause
    /// binding's last-use countdown (see [`BodyBuilder::declare_new_local_with_reads`]'s doc for why comprehension
    /// bindings cannot reuse the statement-suffix-based [`count_reads_in_stmts`]).
    fn count_reads(&self, name: &str) -> usize {
        match self {
            Self::ListPush { element, .. } => count_reads_in_expr(name, &element.node),
            Self::DictInsert { key, value, .. } => {
                count_reads_in_expr(name, &key.node) + count_reads_in_expr(name, &value.node)
            }
            Self::GeneratorYield { element } => count_reads_in_expr(name, &element.node),
        }
    }
}
/// Build the single mirrored `(pattern, iter, filter)` clause list a list/dict comprehension carries, as an owned
/// `Vec<ast::ComprehensionClause>` so [`BodyBuilder::lower_comprehension_clauses`] can share its
/// `&[ast::ComprehensionClause]`-based recursion with generator expressions' real multi-clause `generator.clauses`
/// without a second clause-walking implementation. See [`BodyBuilder::lower_list_comp`]'s docs for why only this
/// single mirrored clause is used, not the comprehension's own (unread-elsewhere) `clauses` field.
pub(super) fn single_comprehension_clauses(
    pattern: &ast::Spanned<ast::Pattern>,
    iter: &ast::Spanned<ast::Expr>,
    filter: Option<&ast::Spanned<ast::Expr>>,
) -> Vec<ast::ComprehensionClause> {
    let mut clauses = vec![ast::ComprehensionClause::For {
        pattern: pattern.clone(),
        iter: iter.clone(),
    }];
    if let Some(filter) = filter {
        clauses.push(ast::ComprehensionClause::If(filter.clone()));
    }
    clauses
}
