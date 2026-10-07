//! Lowering an expression into an operand or a place, and materializing one into the other.

use super::primitives::*;
use super::refusals::*;
use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Retain the checker's constant tuple index, including negative literals, before flattening ordinary expressions.
    /// Non-tuple indexing keeps its existing evaluation and ownership behavior.
    fn lower_checked_index_operand(
        &mut self,
        base: &ast::Spanned<ast::Expr>,
        index: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        if tuple_type_elements(&self.resolve_ty(base.span)).is_some()
            && let Some(value) = crate::typechecker::TypeChecker::constant_tuple_index(index)
        {
            return bir::Operand::Constant(bir::Constant::Int(value));
        }
        self.lower_expr_to_operand(index, scope, out)
    }

    /// Classify source-written tuple projections from the checked base type, preserving nominal field authority.
    ///
    /// Numeric spelling alone proves nothing: only an in-bounds element of the existing checked tuple shape is
    /// structural. Other fields retain their checked canonical identity, including an explicit unresolved value.
    fn lower_checked_field_projection(
        &self,
        base: &ast::Spanned<ast::Expr>,
        name: &str,
        span: ast::Span,
    ) -> bir::PlaceElem {
        let ty = self.resolve_ty(base.span);
        if name == "0"
            && let IncanType::Named(owner) = &ty
            && let Some(declaration) = self.local_nominal_declarations.values().find(|declaration| {
                declaration.name == *owner && declaration.canonical.kind == SemanticSourceTargetKind::Newtype
            })
        {
            return bir::PlaceElem::field(name, Some(declaration.canonical.clone()));
        }
        if tuple_type_elements(&ty)
            .is_some_and(|elements| name.parse::<usize>().is_ok_and(|index| index < elements.len()))
        {
            bir::PlaceElem::structural_field(name)
        } else {
            bir::PlaceElem::field(name, self.type_info.resolved_identity(span).cloned())
        }
    }

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
                // A function named as a value (`apply(square, 7)`) is a partial with no presets: a capture-free closure
                // that forwards every argument to it, as a local partial is lowered.
                if !self.bindings.contains_key(name)
                    && self
                        .type_info
                        .resolved_identity(expr.span)
                        .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Function)
                {
                    let partial = ast::PartialExpr {
                        target: Box::new(expr.clone()),
                        type_args: Vec::new(),
                        args: Vec::new(),
                    };
                    return self.lower_partial(&partial, expr.span, scope, out);
                }
                let Some(place) = self.place_for_name(name, expr.span, &ty) else {
                    return self.unsupported_operand(
                        format!("resolved reference `{name}` has no Body IR value representation"),
                        scope,
                        span,
                        out,
                    );
                };
                // Retain the checker's evaluated scalar or text for a source-local constant. Identity must prove the
                // module and declaration kind before the name-keyed const-evaluation table is consulted;
                // imported globals and same-spelled locals keep their existing place representation.
                if let Some(global) = place.global()
                    && global.identity.kind == SemanticSourceTargetKind::Const
                    && incan_semantics_core::canonical_module_identity(&global.identity).as_deref()
                        == Some(self.module_identity)
                    && let Some(value) = self.type_info.const_value(&global.identity.declaration_name)
                {
                    use crate::typechecker::ConstValue;
                    let constant = match value {
                        ConstValue::Int(number) if ty == IncanType::Primitive(IncanPrimitiveType::Int) => {
                            bir::Constant::Int(*number)
                        }
                        ConstValue::Int(number) if ty == IncanType::Primitive(IncanPrimitiveType::Float) => {
                            // Preserve the evaluated signed decimal value; the admitted float carrier parses it once.
                            bir::Constant::Float(number.to_string())
                        }
                        ConstValue::Float(number) if ty == IncanType::Primitive(IncanPrimitiveType::Float) => {
                            bir::Constant::Float(number.to_string())
                        }
                        ConstValue::Bool(flag) => bir::Constant::Bool(*flag),
                        ConstValue::FrozenStr(text) => bir::Constant::Str(text.clone()),
                        _ => {
                            let (fact, last_use) = self.ownership_fact_for_place(&place, &ty);
                            return bir::Operand::place(place, fact, last_use);
                        }
                    };
                    return bir::Operand::Constant(constant);
                }
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
                if let Some(target) = self.checked_enum_variant_target(base, name, expr.span) {
                    return self.push_assign_temp(
                        bir::Rvalue::Aggregate(bir::AggregateKind::EnumVariant(Box::new(target)), Vec::new()),
                        self.resolve_ty(expr.span),
                        scope,
                        span,
                        out,
                    );
                }
                if let Some(place) = self.module_member_place(base, expr.span) {
                    let ty = self.resolve_ty(expr.span);
                    let (fact, last_use) = self.ownership_fact_for_place(&place, &ty);
                    return bir::Operand::place(place, fact, last_use);
                }
                let mut place = self.lower_expr_to_place(base, scope, out);
                place
                    .projection
                    .push(self.lower_checked_field_projection(base, name, expr.span));
                let ty = self.resolve_ty(expr.span);
                let (fact, last_use) = self.ownership_fact_for_place(&place, &ty);
                bir::Operand::place(place, fact, last_use)
            }
            ast::Expr::Index(base, index) => {
                let index_operand = self.lower_checked_index_operand(base, index, scope, out);
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
                // `Color.Red` used where a place is needed (a method receiver, say) is a variant value, not a field of
                // a value named `Color`: materialize it like any other non-place operand.
                if self
                    .local_fieldless_enum_variant_target(base, name, expr.span)
                    .is_some()
                    || self.local_value_enum_variant_target(base, name, expr.span).is_some()
                    || self.checked_enum_variant_target(base, name, expr.span).is_some()
                {
                    let ty = self.resolve_ty(expr.span);
                    let operand = self.lower_expr_to_operand(expr, scope, out);
                    return self.materialize_operand_to_place(operand, ty, scope, hir_span(expr.span), out);
                }
                if let Some(place) = self.module_member_place(base, expr.span) {
                    return place;
                }
                let mut place = self.lower_expr_to_place(base, scope, out);
                place
                    .projection
                    .push(self.lower_checked_field_projection(base, name, expr.span));
                place
            }
            ast::Expr::Index(base, index) => {
                let index_operand = self.lower_checked_index_operand(base, index, scope, out);
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
