//! Returns, `isinstance` and `None` branch narrowing, and C ABI condition facts.

use super::*;

impl TypeChecker {
    /// Check a return statement against the active function context, including an authenticated native usize result.
    pub(super) fn check_return(&mut self, expr: Option<&Spanned<Expr>>, span: Span) {
        if matches!(self.current_yield_context, super::YieldContext::Generator { .. }) {
            if let Some(expr) = expr {
                self.check_expr(expr);
                self.errors.push(errors::generator_return_value_not_supported(span));
            }
            return;
        }

        self.record_returned_value(expr);
        let return_ty = if let Some(e) = expr {
            let expected_return_ty = self.symbols.current_return_type().cloned();
            self.check_expr_with_expected(e, expected_return_ty.as_ref())
        } else {
            ResolvedType::Unit
        };

        if let Some(expected) = self.symbols.current_return_type().cloned()
            && !self.types_compatible(&return_ty, &expected)
            && !self.record_native_usize_return(expr, &return_ty, &expected)
        {
            self.errors.push(errors::return_type_mismatch(
                &expected.to_string(),
                &return_ty.to_string(),
                span,
            ));
        }
        if let (Some(e), Some(expected)) = (expr, self.symbols.current_return_type().cloned()) {
            self.allow_body_ir_empty_list_return(e, &expected);
            self.record_value_destination_if_compatible(e.span, &return_ty, &expected);
            if matches!(expected, ResolvedType::Function(..)) {
                self.check_capturing_return(e);
            }
        }
    }

    /// Return the canonical declaration identity for a nominal type name, when resolution proved one.
    pub(super) fn canonical_isinstance_target_identity(
        &self,
        name: &str,
    ) -> Option<incan_semantics_core::CanonicalSymbolId> {
        let imported = self.type_info.resolved_import_identity(name).cloned();
        let local = self.lookup_symbol(name).and_then(|symbol| {
            let target = self.source_target_for_symbol(name, &symbol.kind)?;
            let kind = incan_semantics_core::SemanticSourceTargetKind::from_kind_str(&target.kind);
            Some(incan_semantics_core::CanonicalSymbolId::module_declaration(
                target.module_path,
                target.name,
                kind,
                symbol.span,
            ))
        });
        imported.or(local)
    }

    /// Resolve the type argument used by a narrowing expression.
    pub(in crate::typechecker) fn resolve_isinstance_target(
        &mut self,
        expr: &Spanned<Expr>,
    ) -> Option<super::type_info::IsInstanceTargetInfo> {
        let name = match &expr.node {
            Expr::Ident(name) => name,
            Expr::Paren(inner) => {
                let mut target = self.resolve_isinstance_target(inner)?;
                target.span = expr.span;
                return Some(target);
            }
            _ => return None,
        };
        let source_type = Spanned::new(Type::Simple(name.clone()), expr.span);
        let ty = self.resolve_type_checked(&source_type);
        if ty == ResolvedType::Unknown {
            return None;
        }
        let canonical = self.canonical_isinstance_target_identity(name).or_else(|| match &ty {
            ResolvedType::Named(resolved_name) => self.canonical_isinstance_target_identity(resolved_name),
            _ => None,
        });
        Some(super::type_info::IsInstanceTargetInfo {
            ty,
            canonical,
            span: expr.span,
        })
    }

    /// Return whether two union member candidates are equivalent for narrowing.
    ///
    /// A `str` and a `FrozenStr` are one semantic string: `isinstance(value, str)` selects a `FrozenStr` member, the
    /// same way lowering binds that member for the branch, so the branch checks the value as the member it holds
    /// rather than as the whole union (#1748).
    pub(super) fn union_member_matches(&self, member: &ResolvedType, target: &ResolvedType) -> bool {
        let is_text = |ty: &ResolvedType| matches!(ty, ResolvedType::Str | ResolvedType::FrozenStr);
        (is_text(member) && is_text(target))
            || (self.types_compatible(member, target) && self.types_compatible(target, member))
    }

    /// Return the type available in the true branch of an `isinstance` check.
    pub(super) fn narrowed_type_for_isinstance(
        &self,
        current_ty: &ResolvedType,
        target_ty: &ResolvedType,
    ) -> Option<ResolvedType> {
        let member_target = self.union_member_target_spelling(target_ty.clone());
        if let Some(members) = current_ty.union_members() {
            return members
                .iter()
                .find(|member| self.union_member_matches(member, &member_target))
                .map(|member| Self::narrowed_union_member_type(member, target_ty));
        }

        if let Some(inner) = current_ty.option_inner_type() {
            if let Some(members) = inner.union_members() {
                return members
                    .iter()
                    .find(|member| self.union_member_matches(member, &member_target))
                    .map(|member| Self::narrowed_union_member_type(member, target_ty));
            }
            if self.union_member_matches(inner, target_ty) {
                return Some(inner.clone());
            }
        }

        None
    }

    /// Return the union-minus-target type after a failed `isinstance` check.
    pub(super) fn union_minus_type(&self, members: &[ResolvedType], target_ty: &ResolvedType) -> Option<ResolvedType> {
        let member_target = self.union_member_target_spelling(target_ty.clone());
        let remaining: Vec<_> = members
            .iter()
            .filter(|member| !self.union_member_matches(member, &member_target))
            .cloned()
            .collect();
        if remaining.len() == members.len() {
            None
        } else {
            Some(Self::localize_union_member(union_ty(remaining)))
        }
    }

    /// Return the else-branch type for an `isinstance` check.
    pub(super) fn else_type_for_isinstance(
        &self,
        current_ty: &ResolvedType,
        target_ty: &ResolvedType,
    ) -> Option<ResolvedType> {
        if let Some(members) = current_ty.union_members() {
            return self.union_minus_type(members, target_ty);
        }

        if let Some(inner) = current_ty.option_inner_type() {
            if let Some(members) = inner.union_members() {
                return self.union_minus_type(members, target_ty).map(option_ty);
            }
            if self.union_member_matches(inner, target_ty) {
                return Some(ResolvedType::Unit);
            }
        }

        None
    }

    /// Return whether an expression is the source-level `None` value.
    pub(super) fn is_none_expr(expr: &Spanned<Expr>) -> bool {
        matches!(&expr.node, Expr::Literal(Literal::None))
            || matches!(&expr.node, Expr::Ident(name) if name == constructors::as_str(ConstructorId::None))
    }

    /// Determine branch-local narrowing introduced by a boolean condition.
    pub(super) fn condition_branch_narrowing(&self, expr: &Spanned<Expr>) -> Option<BranchNarrowing> {
        if let Some(narrowing) = self.isinstance_branch_narrowing(expr) {
            return Some(narrowing);
        }
        self.none_check_branch_narrowing(expr)
    }

    /// Return the value arguments of either source spelling for one checked builtin call.
    pub(super) fn checked_isinstance_condition_args(expr: &Spanned<Expr>) -> Option<&[CallArg]> {
        match &expr.node {
            Expr::Call(_, _, args) | Expr::MethodCall(_, _, _, args) => Some(args),
            _ => None,
        }
    }

    /// Determine branch-local narrowing introduced by `isinstance`.
    pub(super) fn isinstance_branch_narrowing(&self, expr: &Spanned<Expr>) -> Option<BranchNarrowing> {
        let args = Self::checked_isinstance_condition_args(expr)?;
        if args.len() != 2 {
            return None;
        }
        let value_expr = match &args[0] {
            CallArg::Positional(expr) => expr,
            _ => return None,
        };
        let Expr::Ident(var_name) = &value_expr.node else {
            return None;
        };

        let target_ty = &self.type_info.isinstance_target(expr.span)?.ty;
        let var_info = self.lookup_variable_info(var_name)?;
        let true_ty = self.narrowed_type_for_isinstance(&var_info.ty, target_ty)?;
        let false_ty = self.else_type_for_isinstance(&var_info.ty, target_ty);

        Some(BranchNarrowing {
            name: var_name.clone(),
            true_ty,
            false_ty,
            is_mutable: var_info.is_mutable,
            span: value_expr.span,
        })
    }

    /// Determine branch-local narrowing introduced by `x is None` or `x is not None`.
    pub(super) fn none_check_branch_narrowing(&self, expr: &Spanned<Expr>) -> Option<BranchNarrowing> {
        let Expr::Binary(value_expr, op @ (BinaryOp::Is | BinaryOp::IsNot), right_expr) = &expr.node else {
            return None;
        };
        if !Self::is_none_expr(right_expr) {
            return None;
        }
        let Expr::Ident(var_name) = &value_expr.node else {
            return None;
        };
        let var_info = self.lookup_variable_info(var_name)?;
        let inner = var_info.ty.option_inner_type()?.clone();
        let (true_ty, false_ty) = if matches!(op, BinaryOp::IsNot) {
            (inner, ResolvedType::Unit)
        } else {
            (ResolvedType::Unit, inner)
        };

        Some(BranchNarrowing {
            name: var_name.clone(),
            true_ty,
            false_ty: Some(false_ty),
            is_mutable: var_info.is_mutable,
            span: value_expr.span,
        })
    }

    /// Shadow a binding inside a branch with its narrowed type.
    pub(super) fn define_narrowed_binding(&mut self, name: String, ty: ResolvedType, is_mutable: bool, span: Span) {
        self.symbols.define_refined_binding(Symbol {
            name: name.clone(),
            kind: SymbolKind::Variable(VariableInfo {
                ty,
                is_mutable,
                is_used: false,
            }),
            span,
            scope: 0,
        });
        if is_mutable {
            self.mutable_bindings.insert(name);
        }
    }

    /// Convert a condition narrowing result into the refinement available after the condition is false.
    pub(super) fn branch_false_refinement(narrowing: BranchNarrowing) -> Option<BranchRefinement> {
        narrowing.false_ty.map(|ty| BranchRefinement {
            name: narrowing.name,
            ty,
            is_mutable: narrowing.is_mutable,
            span: narrowing.span,
        })
    }

    /// Shadow all currently-known branch refinements in the active scope.
    pub(super) fn apply_branch_refinements(&mut self, refinements: &[BranchRefinement]) {
        for refinement in refinements {
            self.define_narrowed_binding(
                refinement.name.clone(),
                refinement.ty.clone(),
                refinement.is_mutable,
                refinement.span,
            );
        }
    }

    /// Insert or replace the accumulated false-branch refinement for one binding.
    pub(super) fn upsert_branch_refinement(refinements: &mut Vec<BranchRefinement>, refinement: BranchRefinement) {
        if let Some(existing) = refinements.iter_mut().find(|existing| existing.name == refinement.name) {
            *existing = refinement;
        } else {
            refinements.push(refinement);
        }
    }

    /// Return the slot handles made readable by one explicit raw-result outcome comparison.
    pub(super) fn c_abi_output_slots_available_for_condition(&self, expr: &Spanned<Expr>) -> HashSet<String> {
        let Expr::Binary(left, BinaryOp::Eq, right) = &expr.node else {
            return HashSet::new();
        };
        let Some((status, outcome)) =
            [(&**left, &**right), (&**right, &**left)]
                .into_iter()
                .find(|(status, outcome)| {
                    let Expr::Ident(result_name) = &status.node else {
                        return false;
                    };
                    self.c_abi_raw_call_result_for_local(result_name).is_some()
                        && Self::c_abi_qualified_expr_name(outcome).is_some()
                })
        else {
            return HashSet::new();
        };
        let (Expr::Ident(result_name), Some(outcome_name)) = (&status.node, Self::c_abi_qualified_expr_name(outcome))
        else {
            return HashSet::new();
        };
        let Some(raw_result) = self.c_abi_raw_call_result_for_local(result_name) else {
            return HashSet::new();
        };
        let Some(binding) = self.type_info.c_abi.bindings.get(&raw_result.binding) else {
            return HashSet::new();
        };
        let Some(symbol) = binding.symbols.iter().find(|symbol| symbol.name == raw_result.symbol) else {
            return HashSet::new();
        };
        // Binding declarations store outcome names relative to their binding (`Status.OK`), while ordinary source
        // compares the emitted class-qualified enum value (`Fixture.Status.OK`). Preserve that declaration-local
        // spelling rather than leaking a second outcome representation into the binding descriptor.
        let binding_prefix = format!("{}.", raw_result.binding);
        let outcome_name = outcome_name.strip_prefix(&binding_prefix).unwrap_or(&outcome_name);
        let Some(declared) = symbol.outcomes.iter().find(|declared| declared.result == outcome_name) else {
            return HashSet::new();
        };

        symbol
            .parameters
            .iter()
            .filter_map(|parameter| {
                let CBindingType::Output { mode, .. } = &parameter.ty else {
                    return None;
                };
                let slot_identity = raw_result.slots_by_parameter.get(&parameter.name)?;
                let readable = match mode {
                    COutputMode::Out => declared.initializes.contains(&parameter.name),
                    COutputMode::InOut => !declared.invalidates.contains(&parameter.name),
                };
                readable.then(|| slot_identity.clone())
            })
            .collect()
    }

    /// Resolve one raw C call outcome relation through the same lexical binding currently selected for a local name.
    pub(super) fn c_abi_raw_call_result_for_local(&self, name: &str) -> Option<&super::CAbiRawCallResult> {
        let symbol_span = self
            .symbols
            .lookup(name)
            .and_then(|symbol_id| self.symbols.get(symbol_id))
            .map(|symbol| symbol.span)?;
        self.c_abi_raw_call_results
            .iter()
            .rev()
            .find(|result| result.local_name.as_deref() == Some(name) && result.local_symbol_span == Some(symbol_span))
    }

    /// Render one ordinary source enum/namespace reference into the descriptor outcome spelling.
    pub(super) fn c_abi_qualified_expr_name(expr: &Spanned<Expr>) -> Option<String> {
        match &expr.node {
            Expr::Ident(name) => Some(name.clone()),
            Expr::Field(base, member) => Some(format!("{}.{}", Self::c_abi_qualified_expr_name(base)?, member)),
            Expr::Paren(inner) => Self::c_abi_qualified_expr_name(inner),
            _ => None,
        }
    }

    /// Check one expression-conditioned branch under incoming false-branch refinements.
    pub(super) fn check_expr_condition_body(
        &mut self,
        expr: &Spanned<Expr>,
        body: &[Spanned<Statement>],
        incoming_refinements: &[BranchRefinement],
    ) -> Option<BranchRefinement> {
        let available_before = self.available_c_abi_output_slots.clone();
        self.symbols.enter_scope(ScopeKind::Block);
        self.apply_branch_refinements(incoming_refinements);

        let cond_ty = self.check_expr(expr);
        self.validate_truthiness_condition(&cond_ty, expr.span);
        self.available_c_abi_output_slots
            .extend(self.c_abi_output_slots_available_for_condition(expr));
        let true_narrowing = self.condition_branch_narrowing(expr);
        let false_refinement = true_narrowing.as_ref().cloned().and_then(Self::branch_false_refinement);

        if let Some(narrowing) = true_narrowing {
            self.define_narrowed_binding(narrowing.name, narrowing.true_ty, narrowing.is_mutable, narrowing.span);
        }
        self.check_statement_block(body);
        self.symbols.exit_scope();
        self.available_c_abi_output_slots = available_before;

        false_refinement
    }
}
