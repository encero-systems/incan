//! Lowering an expression into an operand or a place, and materializing one into the other.

use super::primitives::*;
use super::refusals::*;
use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Lower one expression into an [`bir::Operand`], dispatching on its AST kind and, where evaluation has side
    /// effects or must be flattened (calls, binary/unary ops, aggregates), pushing supporting statements into `out`
    /// first. Expression kinds outside v0's covered subset fall through to [`Self::unsupported_operand`] rather than
    /// panicking (see this module's module-level docs for the exact covered/uncovered split).
    pub(super) fn lower_expr_to_operand(
        &mut self,
        expr: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let span = hir_span(expr.span);
        match &expr.node {
            ast::Expr::Ident(name) => {
                let ty = self.resolve_ty(expr.span);
                let Some(place) = self.place_for_name(name, expr.span, &ty) else {
                    return self.unsupported_operand(
                        format!("resolved reference `{name}` has no Body IR value representation"),
                        scope,
                        span,
                        out,
                    );
                };
                let (fact, last_use) = self.ownership_fact_for_place(&place, &ty);
                bir::Operand::place(place, fact, last_use)
            }
            ast::Expr::SelfExpr => {
                // Resolved exactly like `Ident("self")` — see `BodyBuilder::declare_receiver_local`, which binds
                // the receiver under the name "self" so this shares `place_for_name`'s canonical lookup path. A
                // top-level function body can never actually contain `SelfExpr` (the parser only accepts it inside
                // a method), so this arm's unproven-reference fallback to an `External` local is purely defensive.
                let ty = self.resolve_ty(expr.span);
                let Some(place) = self.place_for_name("self", expr.span, &ty) else {
                    return self.unsupported_operand(
                        "resolved receiver has no Body IR local".to_string(),
                        scope,
                        span,
                        out,
                    );
                };
                let (fact, last_use) = self.ownership_fact_for_place(&place, &ty);
                bir::Operand::place(place, fact, last_use)
            }
            ast::Expr::Literal(lit) => bir::Operand::Constant(lower_checked_literal(lit, &self.resolve_ty(expr.span))),
            ast::Expr::Paren(inner) => self.lower_expr_to_operand(inner, scope, out),
            ast::Expr::Field(base, name) => {
                if let Some(target) = self.local_fieldless_enum_variant_target(base, name, expr.span) {
                    return self.push_assign_temp(
                        bir::Rvalue::FieldlessEnumVariant(target),
                        self.resolve_ty(expr.span),
                        scope,
                        span,
                        out,
                    );
                }
                if let Some(target) = self.local_value_enum_variant_target(base, name, expr.span) {
                    return self.push_assign_temp(
                        bir::Rvalue::ValueEnumVariant(target),
                        self.resolve_ty(expr.span),
                        scope,
                        span,
                        out,
                    );
                }
                let mut place = self.lower_expr_to_place(base, scope, out);
                place.projection.push(bir::PlaceElem::field(
                    name.clone(),
                    self.type_info.resolved_identity(expr.span).cloned(),
                ));
                let ty = self.resolve_ty(expr.span);
                let (fact, last_use) = self.ownership_fact_for_place(&place, &ty);
                bir::Operand::place(place, fact, last_use)
            }
            ast::Expr::Index(base, index) => {
                let index_operand = self.lower_expr_to_operand(index, scope, out);
                let mut place = self.lower_expr_to_place(base, scope, out);
                place.projection.push(bir::PlaceElem::Index(Box::new(index_operand)));
                let ty = self.resolve_ty(expr.span);
                let (fact, last_use) = self.ownership_fact_for_place(&place, &ty);
                bir::Operand::place(place, fact, last_use)
            }
            ast::Expr::Slice(base, slice) => self.lower_slice(base, slice, expr.span, scope, out),
            ast::Expr::Unary(ast::UnaryOp::Neg, inner) => {
                let ty = self.resolve_ty(expr.span);
                if let ast::Expr::Literal(literal) = &inner.node
                    && let Some(constant) = lower_checked_negative_literal(literal, &ty)
                {
                    bir::Operand::Constant(constant)
                } else {
                    let operand = self.lower_expr_to_operand(inner, scope, out);
                    self.push_assign_temp(bir::Rvalue::UnaryOp(bir::UnOp::Neg, operand), ty, scope, span, out)
                }
            }
            ast::Expr::Unary(op, inner) => {
                let un_op = lower_unary_op(*op);
                let operand = self.lower_expr_to_operand(inner, scope, out);
                let ty = self.resolve_ty(expr.span);
                self.push_assign_temp(bir::Rvalue::UnaryOp(un_op, operand), ty, scope, span, out)
            }
            ast::Expr::Binary(lhs, op, rhs) => self.lower_binary(lhs, *op, rhs, expr.span, scope, out),
            ast::Expr::Call(callee, type_args, args) => self.lower_call(callee, type_args, args, expr.span, scope, out),
            ast::Expr::MethodCall(recv, name, type_args, args) => {
                self.lower_method_call(recv, name, type_args, args, expr.span, scope, out)
            }
            ast::Expr::Tuple(items) => self.lower_aggregate(bir::AggregateKind::Tuple, items, expr.span, scope, out),
            ast::Expr::List(entries) => self.lower_list_literal(entries, expr.span, scope, out),
            ast::Expr::Dict(entries) => self.lower_dict(entries, expr.span, scope, out),
            ast::Expr::Set(items) => self.lower_aggregate(bir::AggregateKind::Set, items, expr.span, scope, out),
            ast::Expr::Constructor(name, args) => self.lower_constructor(name, args, expr.span, scope, out),
            ast::Expr::ListComp(comp) => self.lower_list_comp(comp, expr.span, scope, out),
            ast::Expr::DictComp(comp) => self.lower_dict_comp(comp, expr.span, scope, out),
            ast::Expr::Generator(generator) => self.lower_generator_expr(generator, expr.span, scope, out),
            ast::Expr::If(if_expr) => self.lower_if_expr(if_expr, scope, expr.span, out),
            ast::Expr::Loop(loop_expr) => self.lower_loop_expr(loop_expr, scope, expr.span, out),
            ast::Expr::Try(inner) => self.lower_try(inner, expr.span, scope, out),
            ast::Expr::FString(parts) => self.lower_fstring(parts, expr.span, scope, out),
            ast::Expr::Closure(params, body) => self.lower_closure(params, body, expr.span, scope, out),
            ast::Expr::Partial(partial) => self.lower_partial(partial, expr.span, scope, out),
            ast::Expr::Match(subject, arms) => self.lower_match(subject, arms, expr.span, scope, out),
            ast::Expr::Surface(surface) => self.lower_surface_expr(surface, expr.span, scope, out),
            ast::Expr::Range { start, end, inclusive } => {
                self.lower_range_value(start, end, *inclusive, expr.span, scope, out)
            }
            other => self.unsupported_operand(unsupported_expr_label(other), scope, span, out),
        }
    }

    /// Lower an expression that is being used as a place base (the target of `.field`/`[index]` projection or a
    /// bare name), synthesizing a temporary to hold the value when the expression is not itself place-shaped.
    pub(super) fn lower_expr_to_place(
        &mut self,
        expr: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Place {
        match &expr.node {
            ast::Expr::Ident(name) => {
                let ty = self.resolve_ty(expr.span);
                self.place_for_name(name, expr.span, &ty).unwrap_or_else(|| {
                    let operand = self.unsupported_operand(
                        format!("resolved reference `{name}` has no Body IR place representation"),
                        scope,
                        hir_span(expr.span),
                        out,
                    );
                    self.materialize_operand_to_place(operand, ty, scope, hir_span(expr.span), out)
                })
            }
            ast::Expr::SelfExpr => {
                let ty = self.resolve_ty(expr.span);
                self.place_for_name("self", expr.span, &ty).unwrap_or_else(|| {
                    let operand = self.unsupported_operand(
                        "resolved receiver has no Body IR local".to_string(),
                        scope,
                        hir_span(expr.span),
                        out,
                    );
                    self.materialize_operand_to_place(operand, ty, scope, hir_span(expr.span), out)
                })
            }
            ast::Expr::Field(base, name) => {
                let mut place = self.lower_expr_to_place(base, scope, out);
                place.projection.push(bir::PlaceElem::field(
                    name.clone(),
                    self.type_info.resolved_identity(expr.span).cloned(),
                ));
                place
            }
            ast::Expr::Index(base, index) => {
                let index_operand = self.lower_expr_to_operand(index, scope, out);
                let mut place = self.lower_expr_to_place(base, scope, out);
                place.projection.push(bir::PlaceElem::Index(Box::new(index_operand)));
                place
            }
            ast::Expr::Paren(inner) => self.lower_expr_to_place(inner, scope, out),
            _ => {
                let ty = self.resolve_ty(expr.span);
                let operand = self.lower_expr_to_operand(expr, scope, out);
                self.materialize_operand_to_place(operand, ty, scope, hir_span(expr.span), out)
            }
        }
    }

    /// Ensure `operand` is place-shaped, materializing a fresh temporary holding it first if it is a bare constant.
    /// Used wherever a value that has already been lowered to an [`bir::Operand`] needs a [`bir::Place`] to project
    /// further into -- [`Self::lower_expr_to_place`]'s own non-place-shaped fallback, plus tuple-element
    /// extraction for [`Self::lower_tuple_unpack`]/[`Self::lower_tuple_assign`].
    pub(super) fn materialize_operand_to_place(
        &mut self,
        operand: bir::Operand,
        ty: IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Place {
        match operand {
            bir::Operand::Place(place_operand) => place_operand.place,
            constant @ bir::Operand::Constant(_) => {
                let temp = self.new_temp(ty, scope, span);
                out.push(bir::Statement {
                    kind: bir::StatementKind::Assign {
                        place: bir::Place::from_local(temp),
                        rvalue: bir::Rvalue::Use(constant),
                    },
                    span,
                });
                bir::Place::from_local(temp)
            }
        }
    }
}
