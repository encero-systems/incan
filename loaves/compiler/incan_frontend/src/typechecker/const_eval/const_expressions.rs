//! Const declaration checking, numeric expectations, and const names, expressions, identifiers and tuples.

use super::*;
use crate::typechecker::ResolvedNamespaceMemberKind;

impl TypeChecker {
    /// Resolve a `const` declaration's written annotation to the frozen type the const will carry, rejecting the one
    /// family of annotations that cannot be honored.
    ///
    /// `str`/`bytes` and the frozen wrappers freeze silently: `const X: str = ...` carries `FrozenStr`, which reads
    /// wherever `str` is expected, so the written annotation holds at every use site. A mutable container (`list`,
    /// `dict`, `set`) does not: its frozen wrapper is a different type at every use site, so accepting the annotation
    /// would only move the contradiction to the first call that passes the const along (#1488). That annotation is
    /// rejected here, at its own span, naming the frozen spelling to write instead; the const still resolves as its
    /// frozen type afterwards so the declaration site stays the only place the mismatch is reported.
    pub(super) fn resolve_const_annotation(&mut self, konst: &ConstDecl, ann: &Spanned<Type>) -> ResolvedType {
        let resolved = self.resolve_type_checked(ann);
        let rejected = is_mutable_collection_const_annotation(&resolved);
        let frozen = freeze_const_type(resolved);
        if rejected {
            self.errors.push(errors::const_mutable_collection_annotation(
                &konst.name,
                &ann.node.to_string(),
                &frozen.to_string(),
                ann.span,
            ));
        }
        frozen
    }

    /// Evaluate, type-check, classify, and publish one `const` declaration for later compiler stages.
    pub fn check_and_resolve_const(&mut self, konst: &ConstDecl, decl_span: Span) {
        // ---- Annotation first: its diagnostic lands whether or not the initializer evaluates ----
        let annotation = konst.ty.as_ref().map(|ann| self.resolve_const_annotation(konst, ann));

        // Evaluate (with cycle detection) and update the symbol table entry.
        let mut stack = Vec::new();
        let Some(mut result) = self.eval_const_by_name(&konst.name, &mut stack) else {
            return;
        };

        // Publish classification for downstream stages.
        self.type_info
            .consts
            .const_kinds
            .insert(konst.name.clone(), result.kind);
        if let Some(val) = result.value.clone() {
            self.type_info.consts.const_values.insert(konst.name.clone(), val);
        }

        // If an annotation exists, require compatibility.
        if let Some(expected) = annotation {
            match self.const_numeric_value_checked_against_numeric_expected(&result, &expected, konst.value.span) {
                Some(true) => result.ty = expected,
                Some(false) => {}
                None if self.types_compatible(&result.ty, &expected) => result.ty = expected,
                None => {
                    self.errors.push(errors::type_mismatch(
                        &expected.to_string(),
                        &result.ty.to_string(),
                        konst.value.span,
                    ));
                }
            }
        } else if matches!(result.ty, ResolvedType::Unknown) {
            self.errors
                .push(errors::const_missing_type_annotation(&konst.name, decl_span));
        }

        // A value the evaluator does not fold to a scalar (a list of model values, for one) is lowered as computed
        // storage, which reads the checked types and identities of the initializer's own expressions.
        if result.value.is_none() && !matches!(result.ty, ResolvedType::Unknown) {
            let expected = result.ty.clone();
            self.check_expr_with_expected(&konst.value, Some(&expected));
        }

        // Record the root initializer type so lowering/codegen can use it.
        self.record_expr_type(konst.value.span, result.ty.clone());
        self.const_eval_cache.insert(konst.name.clone(), result.clone());

        // Update the symbol table type (so later expressions see the refined type).
        if let Some(var_info) = self.lookup_local_variable_info_mut(&konst.name) {
            var_info.ty = result.ty.clone();
        }
    }

    /// Validate a numeric const literal against an exact-width annotation without widening its declared type.
    pub(super) fn const_numeric_value_checked_against_numeric_expected(
        &mut self,
        result: &ConstEvalResult,
        expected: &ResolvedType,
        span: Span,
    ) -> Option<bool> {
        let target = super::numeric_type_id_for_compat(expected)?;
        if let Some(value) = result.value.as_ref().and_then(const_int) {
            if matches!(target, NumericTypeId::F32 | NumericTypeId::F64) {
                return Some(true);
            }
            match integer_bounds(target)? {
                IntegerBounds::Signed { minimum, maximum }
                    if i128::from(value) < minimum || i128::from(value) > maximum =>
                {
                    self.errors.push(CompileError::type_error(
                        format!(
                            "Integer literal {value} does not fit in {expected}; valid range is {minimum}..={maximum}"
                        ),
                        span,
                    ));
                    return Some(false);
                }
                IntegerBounds::Unsigned { maximum } if value < 0 || value as u128 > maximum => {
                    self.errors.push(CompileError::type_error(
                        format!("Integer literal {value} does not fit in {expected}; valid range is 0..={maximum}"),
                        span,
                    ));
                    return Some(false);
                }
                _ => {}
            }
            return Some(true);
        }

        let value = result.value.as_ref().and_then(const_float)?;
        let fits = match expected {
            ResolvedType::Numeric(NumericTypeId::F32) => value.is_finite() && value.abs() <= f64::from(f32::MAX),
            _ => return None,
        };
        if !fits {
            self.errors.push(CompileError::type_error(
                format!("Constant value {value} does not fit in {expected}"),
                span,
            ));
        }
        Some(fits)
    }

    /// Evaluate the named module-level const, memoizing the result and detecting dependency cycles.
    ///
    /// `stack` is the chain of consts currently being evaluated: a const whose initializer references another const
    /// recurses through here, and a name found `InProgress` is a cycle reported at that const's declaration span. A
    /// const that fails to evaluate returns `None` after its diagnostic has been pushed, so a dependent const's own
    /// evaluation stops rather than reporting the same failure again. The written annotation supplies only the
    /// frozen expected type here; whether it may be written at all is `check_and_resolve_const`'s question.
    pub(super) fn eval_const_by_name(&mut self, name: &str, stack: &mut Vec<String>) -> Option<ConstEvalResult> {
        if let Some(res) = self.const_eval_cache.get(name).cloned() {
            return Some(res);
        }

        let state = self
            .const_eval_state
            .get(name)
            .copied()
            .unwrap_or(ConstEvalState::NotStarted);
        match state {
            ConstEvalState::Done => return self.const_eval_cache.get(name).cloned(),
            ConstEvalState::InProgress => {
                // Cycle: stack + name
                let mut cycle = stack.clone();
                cycle.push(name.to_string());
                let cycle_str = cycle.join(" -> ");
                let span = self.const_decls.get(name).map(|(_, s)| *s).unwrap_or_default();
                self.errors.push(errors::const_dependency_cycle(&cycle_str, span));
                return None;
            }
            ConstEvalState::NotStarted => {}
        }

        let Some((decl, decl_span)) = self.const_decls.get(name).cloned() else {
            self.errors.push(errors::unknown_symbol(name, Span::default()));
            return None;
        };

        self.const_eval_state
            .insert(name.to_string(), ConstEvalState::InProgress);
        stack.push(name.to_string());

        // The evaluator only needs the frozen expectation; the declaration check (`check_and_resolve_const`) is
        // the one place an unhonorable annotation is reported.
        let expected = decl.ty.as_ref().map(|t| self.resolve_type_checked(t));
        let expected = expected.map(freeze_const_type);
        let result = self.eval_const_expr(&decl.value, expected.as_ref(), stack, decl_span);

        stack.pop();
        self.const_eval_state.insert(name.to_string(), ConstEvalState::Done);

        if let Some(res) = &result {
            self.const_eval_cache.insert(name.to_string(), res.clone());
        }

        result
    }

    /// Evaluate a const expression into the limited compile-time value domain used by declarations and attributes.
    pub(super) fn eval_const_expr(
        &mut self,
        expr: &Spanned<Expr>,
        expected: Option<&ResolvedType>,
        stack: &mut Vec<String>,
        decl_span: Span,
    ) -> Option<ConstEvalResult> {
        match &expr.node {
            Expr::Literal(lit) => Some(self.eval_const_literal(lit, expected, expr.span, decl_span)),
            Expr::Ident(name) => self.eval_const_ident(name, expected, stack, expr.span),
            Expr::Field(_, _) => self.eval_const_qualified_name(expr, expected),
            Expr::Tuple(items) => self.eval_const_tuple(items, stack, decl_span),
            Expr::Unary(op, inner) => self.eval_const_unary(op, inner, expected, stack, decl_span, expr.span),
            Expr::Binary(left, op, right) => self.eval_const_binary(left, op, right, stack, decl_span, expr.span),
            Expr::List(items) => self.eval_const_list(items, expected, stack, decl_span, expr.span),
            Expr::Set(items) => self.eval_const_set(items, expected, stack, decl_span, expr.span),
            Expr::Dict(pairs) => self.eval_const_dict(pairs, expected, stack, decl_span, expr.span),

            Expr::Index(base, index) => self.eval_const_index(base, index, stack, decl_span, expr.span),
            Expr::Slice(base, slice) => self.eval_const_slice(base, slice, stack, decl_span, expr.span),
            Expr::Call(callee, type_args, args) if type_args.is_empty() => {
                self.eval_const_call(callee, args, expected, stack, decl_span, expr.span)
            }
            Expr::Constructor(name, args) if self.is_const_model_constructor_name(name) => {
                self.eval_const_model_constructor(name, args, expected, stack, decl_span, expr.span)
            }

            // Disallowed constructs for RFC 008 phase 1.
            Expr::Call(_, _, _)
            | Expr::MethodCall(_, _, _, _)
            | Expr::Partial(_)
            | Expr::Generator(_)
            | Expr::ListComp(_)
            | Expr::DictComp(_)
            | Expr::Match(_, _)
            | Expr::If(_)
            | Expr::Loop(_)
            | Expr::Closure(_, _)
            | Expr::Yield(_)
            | Expr::Range { .. }
            | Expr::Surface(_)
            | Expr::VocabBlock(_)
            | Expr::Embedded(_)
            | Expr::Try(_)
            | Expr::Paren(_)
            | Expr::Constructor(_, _)
            | Expr::FString(_) => {
                self.errors.push(errors::const_expression_not_allowed(expr.span));
                None
            }
            Expr::SelfExpr => {
                self.errors.push(errors::const_self_not_allowed(expr.span));
                None
            }
        }
    }

    /// Resolve a module-qualified const through the shared namespace authority.
    fn eval_const_qualified_name(
        &mut self,
        expr: &Spanned<Expr>,
        expected: Option<&ResolvedType>,
    ) -> Option<ConstEvalResult> {
        let segments = crate::typechecker::collect::dotted_path_segments(&expr.node)?;
        let resolved = match self.resolve_qualified_name(&segments) {
            Ok(resolved) => resolved,
            Err(failure) => {
                self.report_namespace_resolution_failure(failure, expr.span);
                return None;
            }
        };
        let identity = resolved.canonical?;
        if identity.kind != incan_semantics_core::SemanticSourceTargetKind::Const {
            self.errors
                .push(errors::const_non_const_name(&segments.join("."), expr.span));
            return None;
        }
        let ty = match resolved.kind {
            ResolvedNamespaceMemberKind::Symbol(SymbolKind::Variable(info)) => info.ty,
            ResolvedNamespaceMemberKind::Symbol(SymbolKind::Static(info)) => info.ty,
            _ => {
                self.errors
                    .push(errors::const_non_const_name(&segments.join("."), expr.span));
                return None;
            }
        };
        self.type_info.record_resolved_identity(expr.span, identity);
        Some(ConstEvalResult {
            ty: expected.cloned().unwrap_or(ty),
            kind: ConstKind::RustNative,
            value: None,
        })
    }

    /// Resolve an Incan constant or retain an imported Rust constant as an opaque native value.
    ///
    /// An enclosing annotation supplies an opaque value's type when available; rustc remains responsible for its
    /// actual value and type compatibility. Incan declarations keep the existing dependency-cycle stack.
    pub(super) fn eval_const_ident(
        &mut self,
        name: &str,
        expected: Option<&ResolvedType>,
        stack: &mut Vec<String>,
        span: Span,
    ) -> Option<ConstEvalResult> {
        if self.const_decls.contains_key(name) {
            return self.eval_const_by_name(name, stack);
        }
        if self.is_rust_namespace_binding(name) {
            return Some(ConstEvalResult {
                ty: expected.cloned().unwrap_or(ResolvedType::Unknown),
                kind: ConstKind::RustNative,
                value: None,
            });
        }
        self.errors.push(errors::const_non_const_name(name, span));
        None
    }

    /// Validate tuple members in source order and retain a frozen classification if any member requires it.
    pub(super) fn eval_const_tuple(
        &mut self,
        items: &[Spanned<Expr>],
        stack: &mut Vec<String>,
        decl_span: Span,
    ) -> Option<ConstEvalResult> {
        let mut types = Vec::with_capacity(items.len());
        let mut kind = ConstKind::RustNative;
        for item in items {
            let result = self.eval_const_expr(item, None, stack, decl_span)?;
            types.push(result.ty);
            if result.kind == ConstKind::Frozen {
                kind = ConstKind::Frozen;
            }
        }
        Some(ConstEvalResult {
            ty: ResolvedType::Tuple(types),
            kind,
            value: None,
        })
    }
}
