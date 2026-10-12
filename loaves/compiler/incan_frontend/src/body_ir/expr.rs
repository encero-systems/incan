//! Lowering an expression into an operand or a place, and materializing one into the other.

use super::primitives::*;
use super::refusals::*;
use super::*;
use incan_semantics_core::SymbolNamespace;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Keep a source call's producer-owned closed return carrier at inferred bindings and projected reads.
    /// A bare call's selected identity belongs to its callee span, while a module-qualified call records it on the
    /// whole call. Function references do not enter this boundary and keep their callable types.
    pub(super) fn published_expression_type(&self, expr: &ast::Spanned<ast::Expr>) -> Option<IncanType> {
        let span = match &expr.node {
            ast::Expr::Call(callee, _, _) => callee.span,
            ast::Expr::MethodCall(_, _, _, _) => expr.span,
            _ => return None,
        };
        let identity = self.type_info.resolved_identity(span)?;
        self.published_callable_returns.get(identity).cloned()
    }

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

    /// Classify tuple projections and retain the declaration name selected for a checked nominal field alias.
    ///
    /// Numeric spelling alone proves nothing: only an in-bounds element of the existing checked tuple shape is
    /// structural. Checked member fields use the canonical declaration's name and identity together, so an alias
    /// cannot become a second storage slot. A field lacking an expression identity can use the exact stored-field
    /// identity of its checker-proven owner layout; a source spelling alone cannot select an owner. Other unresolved
    /// fields preserve their source spelling and remain unproven.
    /// A newtype's wrapped-value slot uses the method's retained concrete receiver when the expression fact keeps Self
    /// open; an open trait receiver cannot authorize that physical projection.
    pub(super) fn lower_checked_field_projection(
        &self,
        base: &ast::Spanned<ast::Expr>,
        name: &str,
        span: ast::Span,
    ) -> bir::PlaceElem {
        let ty = self
            .published_expression_type(base)
            .unwrap_or_else(|| self.resolve_ty(base.span));
        // The checker keeps a Self-typed parameter open in expression facts even when this method's retained receiver
        // is concrete. Use that receiver fact for physical wrapped-value projection; trait defaults keep Self open.
        let ty = if ty == IncanType::SelfType {
            self.bindings
                .get("self")
                .and_then(|local| self.locals.get(local.index()))
                .map(|local| local.ty.clone())
                .unwrap_or(ty)
        } else {
            ty
        };
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
            if let Some(identity) = self.retained_native_field_identity(&ty, name) {
                return bir::PlaceElem::field(name, Some(identity));
            }
            if let Some(identity) = self.type_info.resolved_identity(span)
                && identity.namespace == SymbolNamespace::Member
                && identity.kind == SemanticSourceTargetKind::Field
            {
                return bir::PlaceElem::field(identity.declaration_name.clone(), Some(identity.clone()));
            }
            let identity = self
                .type_info
                .resolved_identity(span)
                .cloned()
                .or_else(|| self.retained_field_identity(&ty, name));
            bir::PlaceElem::field(name, identity)
        }
    }

    /// Compiler-provided metadata results have a checked native owner without requiring a lexical import. Select a
    /// field only from that exact retained runtime layout; any active source owner with the same declaration name
    /// prevents this path. Source-module stdlib identities normalize to the package identity the bridge retains.
    fn retained_native_field_identity(&self, ty: &IncanType, name: &str) -> Option<CanonicalSymbolId> {
        let IncanType::Named(owner_name) = ty else {
            return None;
        };
        let declaration = self.local_nominal_declarations.values().find(|declaration| {
            declaration.name == *owner_name
                && !declaration.native_item.is_empty()
                && bir::native_nominal_item(&declaration.canonical).as_deref() == Some(declaration.native_item.as_str())
        })?;
        for owner in self
            .type_info
            .declarations
            .named_type_identities
            .values()
            .filter(|owner| owner.declaration_name == *owner_name)
        {
            let owner = super::library_nominals::sdk_identity(owner).unwrap_or_else(|| owner.clone());
            if owner != declaration.canonical {
                return None;
            }
        }
        let slot = declaration.fields.iter().position(|field| field == name)?;
        declaration.field_identities.get(slot).cloned()
    }

    /// Read a stored slot from a checker-proven owner and its published layout when checking supplied no field target.
    /// This preserves user Json storage through the legacy wrapper shortcut without resolving aliases or properties
    /// from syntax. Only an exact declared field spelling qualifies; unresolved owners keep refusing downstream.
    fn retained_field_identity(&self, ty: &IncanType, name: &str) -> Option<CanonicalSymbolId> {
        let owner_name = match ty {
            IncanType::Named(owner) | IncanType::Generic { base: owner, .. } => owner,
            _ => return None,
        };
        let owner = self.type_info.declarations.named_type_identities.get(owner_name)?;
        let declaration = self.local_nominal_declarations.get(&owner.render_compact())?;
        let slot = declaration.fields.iter().position(|field| field == name)?;
        declaration.field_identities.get(slot).cloned()
    }

    /// Return the checked value a `const` read at `ty` stands for, so the read is that constant rather than a global.
    ///
    /// A constant a context module published keeps its retained value, whether it is read by a bare imported name or
    /// through a module binding (`helpers.LABEL`). A constant this module declares takes the checker's evaluated value
    /// once its identity proves the module and declaration kind, because the evaluation table is keyed by name. Any
    /// other global, and a value the read's checked type does not admit, keeps its place.
    fn checked_constant(&self, place: &bir::Place, ty: &IncanType) -> Option<bir::Constant> {
        let global = place.global()?;
        if global.identity.kind != SemanticSourceTargetKind::Const {
            return None;
        }
        let float = IncanType::Primitive(IncanPrimitiveType::Float);
        if let Some(value) = self.published_constants.get(&global.identity) {
            return Some(match value {
                bir::Constant::Int(number) if *ty == float => bir::Constant::Float(number.to_string()),
                value => value.clone(),
            });
        }
        if incan_semantics_core::canonical_module_identity(&global.identity).as_deref() != Some(self.module_identity) {
            return None;
        }
        use crate::typechecker::ConstValue;
        match self.type_info.const_value(&global.identity.declaration_name)? {
            ConstValue::Int(number) if *ty == IncanType::Primitive(IncanPrimitiveType::Int) => {
                Some(bir::Constant::Int(*number))
            }
            // Preserve the evaluated signed decimal value; the admitted float carrier parses it once.
            ConstValue::Int(number) if *ty == float => Some(bir::Constant::Float(number.to_string())),
            ConstValue::Float(number) if *ty == float => Some(bir::Constant::Float(number.to_string())),
            ConstValue::Bool(flag) => Some(bir::Constant::Bool(*flag)),
            ConstValue::FrozenStr(text) => Some(bir::Constant::FrozenStr(text.clone())),
            ConstValue::FrozenBytes(bytes) if *ty == IncanType::Primitive(IncanPrimitiveType::FrozenBytes) => {
                Some(bir::Constant::Bytes(bytes.clone()))
            }
            _ => None,
        }
    }

    /// Keep a frozen byte const's checked carrier in a typed temporary. The byte literal vocabulary alone does not
    /// distinguish mutable bytes from static storage, so flattening this read to a bare constant would lose that fact.
    fn checked_constant_operand(
        &mut self,
        constant: bir::Constant,
        ty: IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        if ty != IncanType::Primitive(IncanPrimitiveType::FrozenBytes) {
            return bir::Operand::Constant(constant);
        }
        let local = self.new_temp(ty, scope, span);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(local),
                rvalue: bir::Rvalue::Use(bir::Operand::Constant(constant)),
            },
            span,
        });
        bir::Operand::place(bir::Place::from_local(local), bir::OwnershipFact::Copy, true)
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
        // Checking can convert this value into a newtype where it is used. A conversion that only wraps the value in a
        // retained plain newtype is that newtype's construction; one that validates or constrains it has no Body IR
        // form yet, so it refuses rather than handing a consumer the unconverted underlying value.
        if let Some(coercion) = self.type_info.validated_newtype_coercion(expr.span)
            && !self.coercions_in_progress.contains(&(expr.span.start, expr.span.end))
        {
            return self.lower_newtype_coercion(expr, coercion, scope, out);
        }
        if let Some(lowered) = self.lower_operator_hook_expr(expr, scope, out) {
            return lowered;
        }
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
                    let value = self.lower_partial(&partial, expr.span, scope, out);
                    self.retain_function_item(out);
                    return value;
                }
                let Some(place) = self.place_for_name(name, expr.span, &ty) else {
                    return self.unsupported_operand(
                        format!("resolved reference `{name}` has no Body IR value representation"),
                        scope,
                        span,
                        out,
                    );
                };
                if let Some(constant) = self.checked_constant(&place, &ty) {
                    return self.checked_constant_operand(constant, ty, scope, span, out);
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
            ast::Expr::Field(base, name) if self.is_property_read(expr.span) => {
                self.lower_property_read(base, name, expr.span, scope, out)
            }
            ast::Expr::Field(base, name) => {
                if let Some(value) = self.lower_property_read(base, expr.span, scope, out) {
                    return value;
                }
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
                    if let Some(constant) = self.checked_constant(&place, &ty) {
                        return self.checked_constant_operand(constant, ty, scope, span, out);
                    }
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
        let ty = self.resolve_ty(expr.span);
        if (ty == IncanType::Primitive(IncanPrimitiveType::FrozenBytes)
            || matches!(&ty, IncanType::Generic { base, .. } if matches!(base.as_str(), "FrozenList" | "FrozenSet" | "FrozenDict")))
            && matches!(expr.node, ast::Expr::Ident(_) | ast::Expr::Field(..))
        {
            // Const receivers need their retained value, just like argument reads; a global const has no mutable cell.
            let operand = self.lower_expr_to_operand(expr, scope, out);
            return self.materialize_operand_to_place(operand, ty, scope, hir_span(expr.span), out);
        }
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
                if let Some(value) = self.lower_property_read(base, expr.span, scope, out) {
                    let ty = self.resolve_ty(expr.span);
                    return self.materialize_operand_to_place(value, ty, scope, hir_span(expr.span), out);
                }
                // `Color.Red` used where a place is needed (a method receiver, say) is a variant value, not a field of
                // a value named `Color`, and a computed property read is a getter call: materialize either like any
                // other non-place operand.
                if self
                    .local_fieldless_enum_variant_target(base, name, expr.span)
                    .is_some()
                    || self.local_value_enum_variant_target(base, name, expr.span).is_some()
                    || self.checked_enum_variant_target(base, name, expr.span).is_some()
                    || self.is_property_read(expr.span)
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
            // A `__getitem__` hook returns a value; it is not a projection of its receiver.
            ast::Expr::Index(base, index) if self.type_info.resolved_operator_call(expr.span).is_none() => {
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
