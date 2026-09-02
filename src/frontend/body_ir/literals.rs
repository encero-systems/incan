//! Lowering for literal and aggregate forms: lists, dicts, f-strings, slices, `?`, constructors, surface exprs.

use super::args::*;
use super::primitives::*;
use super::refusals::*;
use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Lower a list literal, including spread entries, into a [`bir::AggregateKind::List`] aggregate.
    ///
    /// Elements are lowered in written source order, so a spread source's evaluation is interleaved with the fixed
    /// elements around it exactly as written. A spread contributes one [`bir::ArgumentElement::Spread`] whose
    /// length is a runtime fact; surrounding fixed elements keep their positions relative to it.
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
                    elements.push(bir::ArgumentElement::One(self.lower_expr_to_operand(item, scope, out)));
                }
                ast::ListEntry::Spread(source) => {
                    elements.push(self.lower_spread_element(source, bir::SpreadKind::Sequence, scope, out));
                }
            }
        }
        let ty = self.resolve_ty(span);
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
    /// always allocates; tuples do not).
    pub(super) fn lower_aggregate(
        &mut self,
        kind: bir::AggregateKind,
        items: &[ast::Spanned<ast::Expr>],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let operands: Vec<bir::Operand> = items
            .iter()
            .map(|item| self.lower_expr_to_operand(item, scope, out))
            .collect();
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

    /// Lower `start..end` / `start..=end` used as a **value** into a [`bir::AggregateKind::Range`] aggregate.
    ///
    /// A range is a value, not only a loop header: `r = 0..10` typechecks, so Body IR has to be able to hold one
    /// wherever an operand goes. The four operands are laid down in [`bir::AggregateKind::RANGE_FIELDS`] order --
    /// the two bounds first, in written source order because both are arbitrary expressions whose evaluation is
    /// observable, then the step and the inclusivity flag, neither of which the surface can spell as an
    /// expression. See that variant's own docs for why a range is an aggregate rather than a constant form or a
    /// helper-constructed value, and for why inclusivity rides as an operand instead of as a static property of
    /// the kind.
    ///
    /// No runtime requirement is recorded: four scalars side by side allocate nothing and call nothing, which is
    /// [`bir::AggregateKind::Tuple`]'s treatment rather than [`Self::lower_list_literal`]'s.
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
        let start_operand = self.lower_expr_to_operand(start, scope, out);
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
                    let value_operand = self.lower_expr_to_operand(value, scope, out);
                    lowered.push(bir::DictEntry::Pair(key_operand, value_operand));
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
    /// f-string interpolations exactly like any other expression use. Mirrors the existing Rust-emission backend's
    /// dedicated `Format` node (`src/backend/ir/lower/expr/mod.rs`) rather than desugaring into a helper call --
    /// see [`bir::Rvalue::Format`]'s own docs for why this needed its own `Rvalue` shape.
    ///
    /// Building the formatted string always allocates and always needs the `fstring` runtime helper
    /// (`incan_stdlib::strings::fstring`, the function the existing Rust-emission backend's `Format` node itself
    /// compiles down to -- see `src/backend/ir/emit/expressions/format.rs`), so both requirements are recorded
    /// unconditionally here, the same way [`Self::lower_binary_from_operands`] records requirements for its own
    /// compiler-owned string helpers.
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
                    let style = match format {
                        ast::FStringFormat::Display => bir::FormatStyle::Display,
                        ast::FStringFormat::Debug => bir::FormatStyle::Debug,
                    };
                    bir::FormatPart::Expr { operand, style }
                }
            })
            .collect();
        self.record_runtime_requirement(AbiV0RuntimeRequirement::RuntimeHelper("fstring".to_string()));
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
        let ty = self.resolve_ty(span);
        self.push_assign_temp(bir::Rvalue::Format(ir_parts), ty, scope, hir_span_value, out)
    }

    /// Lower `base[start:end:step]` (each component independently optional) into a value read through a
    /// [`bir::PlaceElem::Slice`] projection, mirroring how `Expr::Index` builds an `[index]`-projected place read
    /// in [`Self::lower_expr_to_operand`] (including that same arm's index-before-base evaluation order, extended
    /// here to start-then-end-then-step-then-base).
    pub(super) fn lower_slice(
        &mut self,
        base: &ast::Spanned<ast::Expr>,
        slice: &ast::SliceExpr,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let start = slice
            .start
            .as_ref()
            .map(|e| Box::new(self.lower_expr_to_operand(e, scope, out)));
        let end = slice
            .end
            .as_ref()
            .map(|e| Box::new(self.lower_expr_to_operand(e, scope, out)));
        let step = slice
            .step
            .as_ref()
            .map(|e| Box::new(self.lower_expr_to_operand(e, scope, out)));
        let mut place = self.lower_expr_to_place(base, scope, out);
        place.projection.push(bir::PlaceElem::Slice { start, end, step });
        let ty = self.resolve_ty(span);
        let (fact, last_use) = self.ownership_fact_for_place(&place, &ty);
        bir::Operand::place(place, fact, last_use)
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
        let hir_span_value = hir_span(outer_span);
        let operand_result_type = self.resolve_ty(inner.span);
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
        let operand = self.lower_expr_to_operand(inner, scope, out);
        let ty = self.resolve_ty(outer_span);
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
    /// No stage of the current pipeline produces this AST variant: `P(x=1, y=2)` parses as an
    /// `ast::Expr::Call` whose callee is a bare identifier, and `lower_call` recognises the construction from the
    /// typechecker's recorded field binding. The arm is kept because the variant is still part of the AST contract,
    /// and it delegates rather than duplicating the lowering so a future producer cannot reach a second, divergent
    /// construction path.
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
    /// (`check_expr/mod.rs`) and the existing Rust-emission backend (`backend/ir/lower/expr/mod.rs`) both dispatch
    /// on the key/payload pair for exactly this reason.
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
