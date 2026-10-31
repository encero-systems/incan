//! Lowering for literal and aggregate forms: lists, dicts, f-strings, slices, `?`, constructors, surface exprs.
//! By-value scalar and text entries are captured at their evaluation point before subsequent entries can mutate global
//! storage. List spreads capture their source contents before subsequent entries can mutate them; ordinary collection
//! handles retain their borrowing rules.

use super::args::*;
use super::primitives::*;
use super::refusals::*;
use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Lower a list literal, including spread entries, into a [`bir::AggregateKind::List`] aggregate.
    ///
    /// Elements are lowered in written source order, so a spread source's evaluation is interleaved with the fixed
    /// elements around it exactly as written. Each source is captured into an owned temporary using its checked
    /// ownership fact before later entries run. A spread contributes one [`bir::ArgumentElement::Spread`] whose
    /// length is a runtime fact; surrounding fixed elements keep their positions relative to it.
    /// Metadata-selected native element adjustments execute before a contextually typed native list is assembled.
    pub(super) fn lower_list_literal(
        &mut self,
        entries: &[ast::ListEntry],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let mut elements = Vec::with_capacity(entries.len());
        for entry in entries {
            match entry {
                ast::ListEntry::Element(item) => {
                    let value = match self.lower_adjusted_argument(item, scope, out) {
                        Some(Ok(value)) => value,
                        Some(Err(reason)) => self.unsupported_operand(reason, scope, hir_span(item.span), out),
                        None => self.lower_expr_to_operand(item, scope, out),
                    };
                    let value = self.retain_evaluated_value_read(item, value, scope, out);
                    elements.push(bir::ArgumentElement::One(value));
                }
                ast::ListEntry::Spread(source) => {
                    let operand = self.lower_expr_to_operand(source, scope, out);
                    let ty = self.resolve_ty(source.span);
                    let operand =
                        self.push_assign_temp(bir::Rvalue::Use(operand), ty, scope, hir_span(source.span), out);
                    elements.push(bir::ArgumentElement::Spread(bir::SpreadElement {
                        source: operand,
                        kind: bir::SpreadKind::Sequence,
                    }));
                }
            }
        }
        let ty = self.retained_list_type(&self.resolve_ty(span), &elements);
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
        self.push_assign_temp(
            bir::Rvalue::Aggregate(bir::AggregateKind::List, elements),
            ty,
            scope,
            hir_span_value,
            out,
        )
    }

    /// Lower one spread source into a [`bir::ArgumentElement::Spread`].
    ///
    /// The source is read through the ordinary ownership path, so a spliced source carries the same
    /// [`bir::OwnershipFact`]/last-use discipline as any other read. That fact is recorded on the spread itself
    /// rather than inferred from the surrounding aggregate or call, because a spliced source is consumed
    /// differently from a single element: its contents are distributed into the surrounding list.
    pub(super) fn lower_spread_element(
        &mut self,
        source: &ast::Spanned<ast::Expr>,
        kind: bir::SpreadKind,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::ArgumentElement {
        let operand = self.lower_expr_to_operand(source, scope, out);
        bir::ArgumentElement::Spread(bir::SpreadElement { source: operand, kind })
    }

    /// Lower a tuple or set literal to a [`bir::Rvalue::Aggregate`], recording an
    /// [`AbiV0RuntimeRequirement::Allocator`] requirement for lists and sets specifically (list/set construction
    /// always allocates; tuples do not). Deferred owner reads stay reserved across later nested entries, as they do
    /// across nested call arguments, so a later entry cannot move storage an earlier entry still reads.
    pub(super) fn lower_aggregate(
        &mut self,
        kind: bir::AggregateKind,
        items: &[ast::Spanned<ast::Expr>],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let pending_start = self.pending_argument_reads.len();
        let mut operands = Vec::with_capacity(items.len());
        for item in items {
            let value = self.lower_expr_to_operand(item, scope, out);
            let value = self.retain_evaluated_value_read(item, value, scope, out);
            self.retain_pending_operand_read(&value);
            operands.push(value);
        }
        self.pending_argument_reads.truncate(pending_start);
        let ty = self.resolve_ty(span);
        if matches!(kind, bir::AggregateKind::List | bir::AggregateKind::Set) {
            self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
        }
        self.push_assign_temp(
            bir::Rvalue::Aggregate(kind, fixed_elements(operands)),
            ty,
            scope,
            hir_span_value,
            out,
        )
    }

    /// Retain a canonical set conversion as a set aggregate splicing one checked iterable operand.
    ///
    /// The constructor identity has already been checked by the caller. A sequence spread expresses the element
    /// conversion without inventing a named function target; the source keeps its ordinary ownership fact. A frozen
    /// or generator source retains its proven element even when legacy leaves the constructor result unresolved.
    pub(super) fn lower_set_constructor_source(
        &mut self,
        source: &ast::Spanned<ast::Expr>,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let element = self.lower_spread_element(source, bir::SpreadKind::Sequence, scope, out);
        let checked = self.resolve_ty(span);
        let ty = self.retain_set_constructor_type(checked, &element, span);
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
        self.push_assign_temp(
            bir::Rvalue::Aggregate(bir::AggregateKind::Set, vec![element]),
            ty,
            scope,
            hir_span(span),
            out,
        )
    }

    /// Retain a canonical `list(source)` conversion as the loop it is: an empty list that receives each item a `for`
    /// loop over `source` yields, in order.
    ///
    /// The checker types the constructor's element from exactly that iteration rule, so lists, text, frozen
    /// collections, dictionaries (their keys) and generators all convert through the one iteration primitive a `for`
    /// statement and a comprehension use, rather than through per-source helpers. Each item moves into the list.
    pub(super) fn lower_list_constructor_source(
        &mut self,
        source: &ast::Spanned<ast::Expr>,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let ty = self.resolve_ty(span);
        let Some([element_ty]) = collection_member_types(&ty, CollectionTypeId::List) else {
            return self.unsupported_operand(
                "list constructor without a checked element type".to_string(),
                scope,
                hir_span_value,
                out,
            );
        };
        let element_ty = element_ty.clone();
        let list_local = self.new_temp(ty.clone(), scope, hir_span_value);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(list_local),
                rvalue: bir::Rvalue::Aggregate(bir::AggregateKind::List, Vec::new()),
            },
            span: hir_span_value,
        });
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
        let loop_scope = self.new_scope(Some(scope), hir_span_value);
        let item_local = self.new_temp(element_ty.clone(), loop_scope, hir_span_value);
        self.lower_general_iteration(
            source,
            item_local,
            scope,
            loop_scope,
            hir_span_value,
            out,
            move |builder, loop_scope, body_stmts| {
                let item = builder.temp_operand(item_local, &element_ty);
                body_stmts.push(bir::Statement {
                    kind: bir::StatementKind::Call {
                        destination: None,
                        callee: bir::Callee::Method(bir::MethodTarget {
                            native_trait_delegation: None,
                            canonical: Some(crate::symbols::canonical_builtin_member_identity("List", "append")),
                            ..bir::MethodTarget::synthesized("push")
                        }),
                        args: fixed_elements(vec![
                            bir::Operand::place(
                                bir::Place::from_local(list_local),
                                bir::OwnershipFact::MutBorrow,
                                false,
                            ),
                            item,
                        ]),
                        may_panic: false,
                    },
                    span: hir_span_value,
                });
                builder.insert_scope_drops(body_stmts, loop_scope);
            },
        );
        self.temp_operand(list_local, &ty)
    }

    /// Retain a canonical `dict(source)` conversion of a dictionary as a dictionary literal holding one `**source`
    /// spread, so the copy takes every entry of the source in its order, exactly as `{**source}` does.
    ///
    /// Other sources, such as a list of pairs, have no retained representation yet and stay refused by name.
    pub(super) fn lower_dict_constructor_source(
        &mut self,
        source: &ast::Spanned<ast::Expr>,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        if collection_member_types(&self.resolve_ty(source.span), CollectionTypeId::Dict).is_none() {
            return self.unsupported_operand(
                "dict constructor from a non-dict source".to_string(),
                scope,
                hir_span_value,
                out,
            );
        }
        let bir::ArgumentElement::Spread(spread) =
            self.lower_spread_element(source, bir::SpreadKind::Mapping, scope, out)
        else {
            return self.unsupported_operand("dict constructor source".to_string(), scope, hir_span_value, out);
        };
        let ty = self.resolve_ty(span);
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
        self.push_assign_temp(
            bir::Rvalue::Dict(vec![bir::DictEntry::Spread(spread)]),
            ty,
            scope,
            hir_span_value,
            out,
        )
    }

    /// Lower `start..end` / `start..=end` used as a **value** into a [`bir::AggregateKind::Range`] aggregate.
    ///
    /// Source ranges retain their four operands in [`bir::AggregateKind::RANGE_FIELDS`] order: bounds evaluated once in
    /// written order, then unit step and inclusivity. A checker-selected native argument instead uses its retained
    /// canonical constructor and ordered slots; this boundary performs no native lookup or inference.
    ///
    /// The source aggregate records no runtime requirement because four scalars allocate nothing and call nothing, as
    /// with [`bir::AggregateKind::Tuple`].
    pub(super) fn lower_range_value(
        &mut self,
        start: &ast::Spanned<ast::Expr>,
        end: &ast::Spanned<ast::Expr>,
        inclusive: bool,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        if let Some(owner) = self.type_info.body_ir_native_constructor_identity(span) {
            let name = owner.declaration_name.clone();
            let arguments = [
                ast::CallArg::Positional(start.clone()),
                ast::CallArg::Positional(end.clone()),
            ];
            return self.lower_nominal_construction(&name, &arguments, span, span, scope, out);
        }
        let start_operand = self.lower_expr_to_operand(start, scope, out);
        let start_operand = self.retain_evaluated_value_read(start, start_operand, scope, out);
        let end_operand = self.lower_expr_to_operand(end, scope, out);
        let elements = fixed_elements(vec![
            start_operand,
            end_operand,
            bir::Operand::Constant(bir::Constant::Int(RANGE_UNIT_STEP)),
            bir::Operand::Constant(bir::Constant::Bool(inclusive)),
        ]);
        let ty = self.resolve_ty(span);
        self.push_assign_temp(
            bir::Rvalue::Aggregate(bir::AggregateKind::Range, elements),
            ty,
            scope,
            hir_span_value,
            out,
        )
    }

    /// Lower a dict literal `{k: v, ...}` to a [`bir::Rvalue::Dict`], one entry per source entry, in written order.
    ///
    /// Keys and values are lowered in written order, key before value, because both are arbitrary expressions
    /// whose evaluation order is source-observable. A `**source` spread contributes one
    /// [`bir::DictEntry::Spread`] in written position; entries take effect in order and a later entry overwrites an
    /// earlier one with the same key, which is what makes `{**base, "x": 1}` well defined.
    pub(super) fn lower_dict(
        &mut self,
        entries: &[ast::DictEntry],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let mut lowered = Vec::with_capacity(entries.len());
        for entry in entries {
            match entry {
                ast::DictEntry::Pair(key, value) => {
                    let key_operand = self.lower_expr_to_operand(key, scope, out);
                    let key_operand = self.retain_evaluated_value_read(key, key_operand, scope, out);
                    let value_operand = self.lower_expr_to_operand(value, scope, out);
                    let value_operand = self.retain_evaluated_value_read(value, value_operand, scope, out);
                    lowered.push(bir::DictEntry::Pair(key_operand, Box::new(value_operand)));
                }
                ast::DictEntry::Spread(source) => {
                    // Reuse the shared spread lowering so the two construction sites cannot drift.
                    let bir::ArgumentElement::Spread(spread) =
                        self.lower_spread_element(source, bir::SpreadKind::Mapping, scope, out)
                    else {
                        return self.unsupported_operand("dict spread entry".to_string(), scope, hir_span_value, out);
                    };
                    lowered.push(bir::DictEntry::Spread(spread));
                }
            }
        }
        let ty = self.resolve_ty(span);
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
        self.push_assign_temp(bir::Rvalue::Dict(lowered), ty, scope, hir_span_value, out)
    }

    /// Lower an f-string `f"...{expr}...{expr!r}..."` to a [`bir::Rvalue::Format`]. Literal text chunks are
    /// carried through verbatim; each embedded expression is lowered through the same
    /// [`Self::lower_expr_to_operand`] path as any other read, so ownership facts and last-use tracking apply to
    /// f-string interpolations exactly like any other expression use. See [`bir::Rvalue::Format`]'s docs for why
    /// formatting has its own `Rvalue` shape rather than desugaring into a helper call. A displayed value
    /// whose checked type is a type parameter keeps that fact as [`bir::FormatStyle::TypeParameterDisplay`] before
    /// specialization replaces its type with a concrete carrier, because the generic display contract can differ from
    /// that carrier's direct adapter.
    ///
    /// Record allocator and formatting runtime requirements unconditionally: constructing the formatted string
    /// allocates and invokes the formatting operation.
    pub(super) fn lower_fstring(
        &mut self,
        parts: &[ast::FStringPart],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let ir_parts: Vec<bir::FormatPart> = parts
            .iter()
            .map(|part| match part {
                ast::FStringPart::Literal(s) => bir::FormatPart::Literal(s.clone()),
                ast::FStringPart::Expr { expr, format } => {
                    let operand = self.lower_expr_to_operand(expr, scope, out);
                    let operand = self.retain_evaluated_value_read(expr, operand, scope, out);
                    let operand = if matches!(format, ast::FStringFormat::Display) {
                        self.lower_selected_display_operand(expr.span, operand, scope, out)
                    } else {
                        operand
                    };
                    let style = match format {
                        ast::FStringFormat::Display if matches!(self.resolve_ty(expr.span), IncanType::TypeVar(_)) => {
                            bir::FormatStyle::TypeParameterDisplay
                        }
                        ast::FStringFormat::Display => bir::FormatStyle::Display,
                        ast::FStringFormat::Debug => bir::FormatStyle::Debug,
                        ast::FStringFormat::Unsupported(_) => bir::FormatStyle::Display,
                    };
                    bir::FormatPart::Expr {
                        operand: Box::new(operand),
                        style,
                    }
                }
            })
            .collect();
        self.record_runtime_requirement(AbiV0RuntimeRequirement::RuntimeHelper("fstring".to_string()));
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
        let ty = self.resolve_ty(span);
        self.push_assign_temp(bir::Rvalue::Format(ir_parts), ty, scope, hir_span_value, out)
    }

    /// Lower `base[start:end:step]` through its checked native call or a builtin [`bir::PlaceElem::Slice`] projection.
    /// Native targets and dereference steps come only from the checker. Both routes retain independently optional
    /// parts and the builtin start-then-end-then-step-then-base evaluation order, extending `Expr::Index`'s
    /// index-before-base order in [`Self::lower_expr_to_operand`].
    pub(super) fn lower_slice(
        &mut self,
        base: &ast::Spanned<ast::Expr>,
        slice: &ast::SliceExpr,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        if let Some(call) = self.type_info.rust.checked_calls.get(&span).cloned() {
            return self.lower_checked_slice(base, slice, &call, span, scope, out);
        }
        let start = slice.start.as_ref().map(|e| {
            let value = self.lower_expr_to_operand(e, scope, out);
            Box::new(self.retain_evaluated_value_read(e, value, scope, out))
        });
        let end = slice.end.as_ref().map(|e| {
            let value = self.lower_expr_to_operand(e, scope, out);
            Box::new(self.retain_evaluated_value_read(e, value, scope, out))
        });
        let step = slice.step.as_ref().map(|e| {
            let value = self.lower_expr_to_operand(e, scope, out);
            Box::new(self.retain_evaluated_value_read(e, value, scope, out))
        });
        let mut place = self.lower_expr_to_place(base, scope, out);
        place.projection.push(bir::PlaceElem::Slice { start, end, step });
        let ty = self.resolve_ty(span);
        let (fact, last_use) = self.ownership_fact_for_place(&place, &ty);
        bir::Operand::place(place, fact, last_use)
    }

    /// Materialize a checked native slice as its already selected call. Parts retain builtin start/end/step-before-base
    /// evaluation order and typed absent endpoints; the borrowed receiver follows only checked adjustment calls.
    fn lower_checked_slice(
        &mut self,
        base: &ast::Spanned<ast::Expr>,
        slice: &ast::SliceExpr,
        call: &crate::typechecker::RustCheckedCall,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let option_int = IncanType::Generic {
            base: "Option".into(),
            args: vec![IncanType::Primitive(IncanPrimitiveType::Int)],
        };
        let mut parts = Vec::new();
        for part in [&slice.start, &slice.end, &slice.step] {
            let value = match part {
                Some(expression) => {
                    let payload = self.lower_expr_to_operand(expression, scope, out);
                    let payload = self.retain_evaluated_value_read(expression, payload, scope, out);
                    bir::Rvalue::Aggregate(bir::AggregateKind::OptionSome, fixed_elements(vec![payload]))
                }
                None => bir::Rvalue::Use(bir::Operand::Constant(bir::Constant::None)),
            };
            parts.push(self.push_assign_temp(value, option_int.clone(), scope, hir_span(span), out));
        }
        let mut receiver = match self.lower_native_adjustment_value(base, &call.receiver_adjustments, scope, out) {
            Ok(value) => value,
            Err(reason) => return self.unsupported_operand(reason, scope, hir_span(span), out),
        };
        if let bir::Operand::Place(read) = &mut receiver {
            self.retain_argument_borrow(read, false);
        }
        let mut arguments = vec![bir::ArgumentElement::One(receiver)];
        arguments.extend(parts.into_iter().map(bir::ArgumentElement::One));
        self.emit_checked_rust_call(
            call,
            arguments,
            bir::ArgumentBinding::resolved_positional(4),
            span,
            scope,
            out,
        )
        .unwrap_or_else(|| {
            self.unsupported_operand(
                "native slice without checked storage".to_string(),
                scope,
                hir_span(span),
                out,
            )
        })
    }

    /// Lower `expr?` (`ast::Expr::Try`) into a single [`bir::StatementKind::TryPropagate`] primitive rather than
    /// decomposing it into explicit `is_err`/`unwrap`-shaped calls -- see that variant's own docs for the full
    /// rationale (it mirrors the same #653-criterion-3 compiler-owned-primitive treatment as
    /// [`bir::Callee::Helper`], standing in for what the existing Rust-emission backend defers entirely to Rust's
    /// native `?` operator).
    pub(super) fn lower_try(
        &mut self,
        inner: &ast::Spanned<ast::Expr>,
        outer_span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let operand_result_type = self.resolve_ty(inner.span);
        let operand = self.lower_expr_to_operand(inner, scope, out);
        self.emit_try(operand, operand_result_type, outer_span, scope, out)
    }

    /// Propagate one evaluated Result using the enclosing declaration's checked error contract. Awaited async
    /// calls enter here after suspension, so their future storage never crosses the Result boundary.
    pub(super) fn emit_try(
        &mut self,
        operand: bir::Operand,
        operand_result_type: IncanType,
        outer_span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(outer_span);
        let error_routing = match (
            result_error_type(&operand_result_type),
            result_error_type(&self.owner_return_type),
        ) {
            (Some(source_error_type), Some(destination_error_type)) if source_error_type == destination_error_type => {
                bir::TryErrorRouting::SameType {
                    error_type: source_error_type.clone(),
                }
            }
            (Some(source_error_type), Some(destination_error_type)) => bir::TryErrorRouting::ConversionRequired {
                source_error_type: source_error_type.clone(),
                destination_error_type: destination_error_type.clone(),
            },
            _ => bir::TryErrorRouting::Unresolved,
        };
        // The checked Result already fixes the success type, including when the outer expression has no separate type
        // entry.
        let ty = match &operand_result_type {
            IncanType::Generic { base, args } if base == "Result" && args.len() == 2 => args[0].clone(),
            _ => self.resolve_ty(outer_span),
        };
        let destination = self.new_temp(ty.clone(), scope, hir_span_value);
        out.push(bir::Statement {
            kind: bir::StatementKind::TryPropagate {
                destination: bir::Place::from_local(destination),
                operand,
                error_routing,
            },
            span: hir_span_value,
        });
        self.temp_operand(destination, &ty)
    }

    /// Lower an `ast::Expr::Constructor` node by delegating to [`Self::lower_nominal_construction`].
    ///
    /// No stage of the current pipeline produces this AST variant: `P(x=1, y=2)` parses as an `ast::Expr::Call` whose
    /// callee is a bare identifier, and `lower_call` recognizes the construction from the typechecker's recorded field
    /// binding. The arm is kept because the variant is still part of the AST contract, and it delegates rather than
    /// duplicating the lowering so a future producer cannot reach a second, divergent construction path.
    pub(super) fn lower_constructor(
        &mut self,
        name: &str,
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        self.lower_nominal_construction(name, args, span, span, scope, out)
    }

    // ---- Async surface (#1164) ----

    /// Lower an `ast::Expr::Surface` node, accepting only the async pair this issue owns.
    ///
    /// Dispatch is on the surface **key**, not the payload shape. `SurfaceExprPayload::PrefixUnary` is generic over
    /// any prefix soft keyword and `await` merely happens to be the only one registered today, so matching the
    /// payload alone would silently accept a future prefix keyword as an await. The typechecker
    /// (`check_expr/mod.rs`) dispatches on the key/payload pair for exactly this reason.
    ///
    /// Every other payload -- the scoped-DSL surface nodes -- keeps its existing named refusal. Those reach this
    /// module only when a caller skips the desugar pass the legacy pipeline runs first, and they belong to the Body
    /// IR input-contract issue, not to this one.
    pub(super) fn lower_surface_expr(
        &mut self,
        surface: &ast::SurfaceExpr,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        match (&surface.key, &surface.payload) {
            (SurfaceFeatureKey::SoftKeyword(KeywordId::Await), ast::SurfaceExprPayload::PrefixUnary(awaited)) => {
                self.lower_await(awaited, span, scope, out)
            }
            (
                SurfaceFeatureKey::ScopedDslSurface {
                    dependency_key,
                    descriptor_key,
                },
                ast::SurfaceExprPayload::RaceFor(race),
            ) if dependency_key == "std.async" && descriptor_key == "race_for" => {
                self.lower_race_for(race, span, scope, out)
            }
            (_, payload) => self.unsupported_operand(surface_expr_label(payload), scope, hir_span(span), out),
        }
    }
}
