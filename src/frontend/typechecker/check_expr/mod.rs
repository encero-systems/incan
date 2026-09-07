//! Check expressions and resolve their types.
//!
//! This module owns the expression-checking entrypoint (`check_expr`) and delegates to themed
//! submodules for maintainability. Expression checking is error-accumulating: on invalid input it
//! returns [`ResolvedType::Unknown`] so later checks can continue.
//!
//! ## See also
//! - [`super::TypeChecker`]: the main type checker entrypoint.

use crate::frontend::ast::*;
use crate::frontend::diagnostics::{CompileError, errors};
use crate::frontend::symbols::{FieldInfo, FunctionInfo, ResolvedType, SymbolKind, VariableInfo};
use crate::frontend::typechecker::helpers::{is_frozen_bytes, is_frozen_str};
use incan_core::lang::keywords;
use incan_core::numeric_values::{IntegerBounds, integer_bounds};
use incan_semantics_core::SurfaceExprTypeCheck;
use std::collections::HashMap;

use super::TypeChecker;

mod access;
mod basics;
mod calls;
mod collections;
mod comps;
mod control_flow;
mod match_;
mod ops;

impl TypeChecker {
    /// Type-check a local partial expression and return its projected callable type.
    fn check_partial_expr(&mut self, partial: &PartialExpr, span: Span) -> ResolvedType {
        if !partial.type_args.is_empty() {
            self.errors
                .push(errors::explicit_call_site_type_args_not_supported(span));
        }
        let target_ty = self.check_expr(&partial.target);
        let ResolvedType::Function(params, ret) = target_ty else {
            self.errors.push(CompileError::type_error(
                "Partial expression target must be a callable value".to_string(),
                partial.target.span,
            ));
            for arg in &partial.args {
                self.check_expr(&arg.value);
            }
            return ResolvedType::Unknown;
        };

        // Calling a stored callable is supported, but constructing another partial from one is not yet representable:
        // the existing callable signature does not retain enough provenance to distinguish a preset captured by the
        // target from one captured by this new partial. Reject this at the source boundary instead of accepting a
        // program that legacy lowering or Body IR must later refuse.
        if let Expr::Ident(name) = &partial.target.node
            && self
                .lookup_symbol(name)
                .is_some_and(|symbol| matches!(&symbol.kind, SymbolKind::Variable(_)))
        {
            self.errors.push(CompileError::type_error(
                "Partial application of a locally stored callable is not supported".to_string(),
                partial.target.span,
            ));
            for arg in &partial.args {
                self.check_expr(&arg.value);
            }
            return ResolvedType::Unknown;
        }

        let Some(projected) = self.project_partial_params("<local partial>", "<callable>", params, &partial.args, span)
        else {
            for arg in &partial.args {
                self.check_expr(&arg.value);
            }
            return ResolvedType::Unknown;
        };

        for arg in &partial.args {
            let expected = projected
                .iter()
                .find(|param| param.name() == Some(arg.name.as_str()))
                .map(|param| &param.ty);
            let actual = self.check_expr_with_expected(&arg.value, expected);
            if let Some(param) = projected.iter().find(|param| param.name() == Some(arg.name.as_str()))
                && !self.types_compatible(&actual, &param.ty)
            {
                self.errors.push(errors::type_mismatch(
                    &param.ty.to_string(),
                    &actual.to_string(),
                    arg.value.span,
                ));
            }
        }

        // A local partial retains its complete callable signature. Presets become defaulted, name-overrideable
        // slots; `is_partial_preset` preserves the separate positional rule that starts at the residual arguments.
        ResolvedType::Function(Self::local_partial_params(projected, &partial.args), ret)
    }

    /// Resolve a field by canonical name or alias, returning the canonical name and FieldInfo.
    ///
    /// - `allow_alias`: whether alias lookup is allowed (models only).
    /// - `allow_numeric_alias`: whether numeric spellings like `"1"` may match aliases.
    fn resolve_field_info<'a>(
        &self,
        fields: &'a HashMap<String, FieldInfo>,
        field_name: &str,
        allow_alias: bool,
        allow_numeric_alias: bool,
    ) -> Option<(String, &'a FieldInfo)> {
        if let Some(info) = fields.get(field_name) {
            return Some((field_name.to_string(), info));
        }
        if !allow_alias {
            return None;
        }
        if !allow_numeric_alias && field_name.parse::<usize>().is_ok() {
            return None;
        }
        fields
            .iter()
            .find(|(_, info)| info.alias.as_deref() == Some(field_name))
            .map(|(name, info)| (name.clone(), info))
    }

    /// Return whether one checked field is private outside its declaring nominal type.
    fn private_field_is_inaccessible(&self, type_name: &str, field: &FieldInfo) -> bool {
        let owner = field.owner.as_deref().unwrap_or(type_name);
        field.is_type_private && self.current_method_owner.as_deref() != Some(owner)
    }

    /// Resolve an expression receiver like `math` to an imported module binding path.
    ///
    /// This is intentionally module-kind driven (`SymbolKind::Module`) instead of name-driven so member access does not
    /// require per-module hardcoded registries.
    fn imported_module_for_expr(&self, expr: &Spanned<Expr>) -> Option<(String, Vec<String>)> {
        let Expr::Ident(name) = &expr.node else {
            return None;
        };
        let sym = self.lookup_symbol(name)?;
        let SymbolKind::Module(info) = &sym.kind else {
            return None;
        };
        // Rust imports keep their dedicated metadata path and Python modules remain dynamic.
        if info.path.first().is_some_and(|seg| seg == "rust") || info.is_python {
            return None;
        }
        Some((name.clone(), info.path.clone()))
    }

    /// Resolve a function member from a stdlib or public-package module binding.
    fn resolve_imported_module_function_member(&mut self, module_path: &[String], member: &str) -> Option<SymbolKind> {
        self.resolve_imported_module_function_member_with_source(module_path, member)
            .map(|(kind, _)| kind)
    }

    /// Resolve an imported callable together with the authored module path used by lowering and codegraph facts.
    fn resolve_imported_module_function_member_with_source(
        &mut self,
        module_path: &[String],
        member: &str,
    ) -> Option<(SymbolKind, Vec<String>)> {
        if let Some(kind) = self.stdlib_cache.lookup_function_symbol(module_path, member) {
            return Some((kind, module_path.to_vec()));
        }
        if module_path.len() >= 2 && module_path.first().is_some_and(|seg| seg == "pub") {
            let (kind, source_module_path) =
                self.lookup_pub_library_module_symbol_member(&module_path[1], &module_path[2..], member)?;
            return matches!(kind, SymbolKind::Function(_) | SymbolKind::FunctionOverloads(_))
                .then_some((kind, source_module_path));
        }
        let import_path = ImportPath::simple(module_path.to_vec());
        let kind = self.dependency_member_symbol_for_path(&import_path, member)?;
        if !matches!(kind, SymbolKind::Function(_) | SymbolKind::FunctionOverloads(_)) {
            return None;
        }
        let identity = self.dependency_member_identity(&import_path, member)?;
        let incan_semantics_core::SymbolOrigin::Module(source_module_path) = identity.origin else {
            return None;
        };
        Some((kind, source_module_path))
    }

    /// Resolve a constant reached through an imported standard-library or checked public-package module.
    fn resolve_imported_module_constant_member(
        &mut self,
        module_path: &[String],
        member: &str,
    ) -> Option<(VariableInfo, Option<incan_semantics_core::CanonicalSymbolId>)> {
        if let Some(info) = self.stdlib_cache.lookup_constant(module_path, member) {
            let identity = self.stdlib_cache.lookup_identity(module_path, member);
            return Some((info, identity));
        }
        if module_path.len() >= 2 && module_path.first().is_some_and(|seg| seg == "pub") {
            let resolved = self
                .resolve_pub_library_module_symbol_member(&module_path[1], &module_path[2..], member)
                .ok()
                .flatten()?;
            return match resolved.kind {
                SymbolKind::Variable(info) => Some((info, resolved.canonical)),
                SymbolKind::Static(info) => Some((
                    VariableInfo {
                        ty: info.ty,
                        is_mutable: false,
                        is_used: info.is_used,
                    },
                    resolved.canonical,
                )),
                _ => None,
            };
        }
        let kind = self.dependency_member_symbol_for_path(&ImportPath::simple(module_path.to_vec()), member)?;
        let identity = self.dependency_member_identity(&ImportPath::simple(module_path.to_vec()), member);
        match kind {
            SymbolKind::Variable(info) => Some((info, identity)),
            SymbolKind::Static(info) => Some((
                VariableInfo {
                    ty: info.ty,
                    is_mutable: false,
                    is_used: info.is_used,
                },
                identity,
            )),
            _ => None,
        }
    }

    /// Convert function symbol information into a resolved function type.
    fn function_info_to_resolved_function_type(info: &FunctionInfo) -> ResolvedType {
        ResolvedType::Function(info.params.clone(), Box::new(info.return_type.clone()))
    }
    // ========================================================================
    // Expressions
    // ========================================================================

    /// Validate an expression and return its resolved type.
    ///
    /// Dispatches to specialized helpers (`check_call`, `check_binary`, `check_match`, etc.)
    /// and accumulates errors. Returns [`ResolvedType::Unknown`] when the expression is
    /// invalid so checking can continue.
    pub(crate) fn check_expr(&mut self, expr: &Spanned<Expr>) -> ResolvedType {
        let ty = match &expr.node {
            Expr::Ident(name) => self.check_ident(name, expr.span),
            Expr::Literal(lit) => self.check_literal(lit),
            Expr::SelfExpr => self.check_self(expr.span),
            Expr::Binary(left, op, right) => self.check_binary(left, *op, right, expr.span),
            Expr::Unary(op, operand) => self.check_unary(*op, operand, expr.span),
            Expr::Call(callee, type_args, args) => self.check_call(callee, type_args, args, expr.span),
            Expr::Index(base, index) => self.check_index(base, index, expr.span),
            Expr::Slice(base, slice) => self.check_slice(base, slice, expr.span),
            Expr::Field(base, field) => self.check_field(base, field, expr.span),
            Expr::MethodCall(base, method, type_args, args) => {
                self.check_method_call(base, method, type_args, args, expr.span)
            }
            Expr::Partial(partial) => self.check_partial_expr(partial, expr.span),
            Expr::Surface(surface_expr) => self.check_surface_expr(surface_expr, expr.span),
            Expr::Try(inner) => self.check_try(inner, expr.span),
            Expr::Match(subject, arms) => self.check_match(subject, arms, expr.span),
            Expr::If(if_expr) => self.check_if_expr(if_expr, expr.span),
            Expr::Loop(loop_expr) => self.check_loop_expr(loop_expr, None, expr.span),
            Expr::Generator(generator) => self.check_generator_expr(generator, expr.span),
            Expr::ListComp(comp) => self.check_list_comp(comp, expr.span),
            Expr::DictComp(comp) => self.check_dict_comp(comp, expr.span),
            Expr::Closure(params, body) => self.check_closure(params, body, expr.span),
            Expr::Tuple(elems) => self.check_tuple(elems),
            Expr::List(elems) => self.check_list(elems),
            Expr::Dict(entries) => self.check_dict(entries),
            Expr::Set(elems) => self.check_set(elems),
            Expr::Paren(inner) => self.check_expr(inner),
            Expr::Constructor(name, args) => self.check_constructor(name, args, expr.span),
            Expr::FString(parts) => {
                for part in parts {
                    if let FStringPart::Expr { expr, .. } = part {
                        self.check_expr(expr);
                    }
                }
                ResolvedType::Str
            }
            Expr::Yield(inner) => {
                let context = self.current_yield_context.clone();
                match context {
                    super::YieldContext::Disallowed => {
                        let yield_ty = inner
                            .as_ref()
                            .map(|inner| self.check_expr(inner))
                            .unwrap_or(ResolvedType::Unit);
                        self.errors.push(errors::yield_outside_generator(expr.span));
                        yield_ty
                    }
                    super::YieldContext::Fixture => inner
                        .as_ref()
                        .map(|inner| self.check_expr(inner))
                        .unwrap_or(ResolvedType::Unit),
                    super::YieldContext::Generator { element_ty } => {
                        if let Some(inner) = inner {
                            let yield_ty = self.check_expr_with_expected(inner, Some(&element_ty));
                            if !self.types_compatible(&yield_ty, &element_ty) {
                                self.errors.push(errors::type_mismatch(
                                    &element_ty.to_string(),
                                    &yield_ty.to_string(),
                                    inner.span,
                                ));
                            }
                            yield_ty
                        } else {
                            self.errors.push(errors::generator_yield_requires_value(expr.span));
                            ResolvedType::Unknown
                        }
                    }
                }
            }
            Expr::Range {
                start,
                end,
                inclusive: _,
            } => self.check_range_expr(start, end),
            Expr::VocabBlock(block) => {
                self.errors.push(CompileError::type_error(
                    format!(
                        "Vocab expression declaration `{}` reached typechecking before desugaring",
                        block.keyword
                    ),
                    expr.span,
                ));
                ResolvedType::Unknown
            }
            Expr::Embedded(fragment) => self.check_embedded_fragment_expr(fragment),
        };

        // Record for downstream stages (lowering/codegen).
        self.record_expr_type(expr.span, ty.clone());
        ty
    }

    /// Type-check an expression used as a type-owned receiver, such as `Type.method()` or `Enum.Variant`.
    pub(crate) fn check_type_receiver_expr(&mut self, expr: &Spanned<Expr>) -> ResolvedType {
        self.type_receiver_spans.push((expr.span.start, expr.span.end));
        let ty = self.check_expr(expr);
        self.type_receiver_spans.pop();
        ty
    }

    /// Return whether a span identifies a type receiver.
    pub(crate) fn is_type_receiver_span(&self, span: Span) -> bool {
        self.type_receiver_spans
            .iter()
            .rev()
            .any(|&(start, end)| start == span.start && end == span.end)
    }

    /// Return whether a span identifies a type name being checked against an explicit `Type[T]` destination.
    pub(crate) fn is_type_token_value_span(&self, span: Span) -> bool {
        self.type_token_value_spans
            .iter()
            .rev()
            .any(|&(start, end)| start == span.start && end == span.end)
    }

    /// Type-check an expression with an expected destination type when one is already known.
    ///
    /// This is intentionally narrow: only expression forms that benefit from contextual typing without broad inference
    /// changes should use the hint.
    pub(crate) fn check_expr_with_expected(
        &mut self,
        expr: &Spanned<Expr>,
        expected: Option<&ResolvedType>,
    ) -> ResolvedType {
        let ty = match (&expr.node, expected) {
            (_, Some(ResolvedType::TypeVar(_))) => return self.check_expr(expr),
            (Expr::Paren(inner), Some(expected_ty)) => self.check_expr_with_expected(inner, Some(expected_ty)),
            (Expr::Ident(_), Some(ResolvedType::TypeToken(_))) => {
                self.type_token_value_spans.push((expr.span.start, expr.span.end));
                let ty = self.check_expr(expr);
                self.type_token_value_spans.pop();
                ty
            }
            (Expr::Literal(Literal::Int(_)), Some(expected_ty))
                if super::numeric_type_id_for_compat(expected_ty).is_some() =>
            {
                self.check_int_literal_with_expected(expr, expected_ty)
            }
            (Expr::Unary(UnaryOp::Neg, inner), Some(expected_ty))
                if super::numeric_type_id_for_compat(expected_ty).is_some()
                    && matches!(inner.node, Expr::Literal(Literal::Int(_))) =>
            {
                self.check_int_literal_with_expected(expr, expected_ty)
            }
            (Expr::Literal(Literal::Float(_)), Some(expected_ty))
                if matches!(
                    expected_ty,
                    ResolvedType::Numeric(
                        incan_core::lang::types::numerics::NumericTypeId::F32
                            | incan_core::lang::types::numerics::NumericTypeId::F64
                    )
                ) =>
            {
                self.check_float_literal_with_expected(expr, expected_ty)
            }
            (Expr::Literal(Literal::Decimal(_)), Some(expected_ty)) if is_decimal_type(expected_ty) => {
                self.validate_decimal_literal_with_expected(expr, expected_ty);
                expected_ty.clone()
            }
            (Expr::Literal(Literal::String(_)), Some(expected_ty)) if is_frozen_str(expected_ty) => expected_ty.clone(),
            (Expr::Literal(Literal::Bytes(_)), Some(expected_ty)) if is_frozen_bytes(expected_ty) => {
                expected_ty.clone()
            }
            (Expr::Binary(left, op, right), Some(expected_ty)) => {
                self.check_binary_with_expected(left, *op, right, expr.span, Some(expected_ty))
            }
            (Expr::Unary(UnaryOp::Neg, operand), Some(expected_ty))
                if matches!(
                    expected_ty,
                    ResolvedType::Numeric(
                        incan_core::lang::types::numerics::NumericTypeId::F32
                            | incan_core::lang::types::numerics::NumericTypeId::F64
                    )
                ) && matches!(operand.node, Expr::Literal(Literal::Float(_))) =>
            {
                self.check_expr_with_expected(operand, Some(expected_ty));
                expected_ty.clone()
            }
            (Expr::Unary(op, operand), Some(expected_ty)) => {
                self.check_unary_with_expected(*op, operand, expr.span, Some(expected_ty))
            }
            (Expr::Try(inner), Some(expected_ty)) => self.check_try_with_expected(inner, expr.span, Some(expected_ty)),
            (Expr::Call(callee, type_args, args), Some(expected_ty)) => {
                self.check_call_with_expected(callee, type_args, args, expr.span, Some(expected_ty))
            }
            (Expr::MethodCall(base, method, type_args, args), Some(expected_ty)) => {
                self.check_method_call_with_expected(base, method, type_args, args, expr.span, Some(expected_ty))
            }
            (Expr::Tuple(items), Some(ResolvedType::Unit)) if items.is_empty() => ResolvedType::Unit,
            (Expr::Closure(params, body), Some(ResolvedType::Function(expected_params, expected_ret))) => {
                self.check_closure_with_expected(params, body, expected_params, expected_ret, expr.span)
            }
            (Expr::List(elems), expected_ty) => self.check_list_with_expected(elems, expected_ty),
            (Expr::Dict(entries), expected_ty) => self.check_dict_with_expected(entries, expected_ty),
            (Expr::Loop(loop_expr), expected_ty) => self.check_loop_expr(loop_expr, expected_ty, expr.span),
            _ => return self.check_expr(expr),
        };

        self.record_expr_type(expr.span, ty.clone());
        ty
    }

    /// Typecheck an integer literal in a known numeric target context.
    fn check_int_literal_with_expected(&mut self, expr: &Spanned<Expr>, expected_ty: &ResolvedType) -> ResolvedType {
        let Some(target) = super::numeric_type_id_for_compat(expected_ty) else {
            return self.check_expr(expr);
        };
        if matches!(target, incan_core::lang::types::numerics::NumericTypeId::U128) {
            if unsigned_int_literal_magnitude(expr).is_some() {
                return expected_ty.clone();
            }
            self.errors.push(CompileError::type_error(
                format!(
                    "Integer literal does not fit in {expected_ty}; valid range is 0..={}",
                    u128::MAX
                ),
                expr.span,
            ));
            return expected_ty.clone();
        }
        if matches!(
            target,
            incan_core::lang::types::numerics::NumericTypeId::F32
                | incan_core::lang::types::numerics::NumericTypeId::F64
        ) {
            let finite = match signed_int_literal_value(expr) {
                Some(value) => match target {
                    incan_core::lang::types::numerics::NumericTypeId::F32 => (value as f32).is_finite(),
                    incan_core::lang::types::numerics::NumericTypeId::F64 => (value as f64).is_finite(),
                    _ => false,
                },
                None => unsigned_int_literal_magnitude(expr).is_some_and(|value| match target {
                    incan_core::lang::types::numerics::NumericTypeId::F32 => (value as f32).is_finite(),
                    incan_core::lang::types::numerics::NumericTypeId::F64 => (value as f64).is_finite(),
                    _ => false,
                }),
            };
            if !finite {
                self.errors.push(CompileError::type_error(
                    format!("Integer literal does not fit in {expected_ty}"),
                    expr.span,
                ));
            }
            return expected_ty.clone();
        }
        let Some(value) = signed_int_literal_value(expr) else {
            return self.check_expr(expr);
        };
        match integer_bounds(target) {
            Some(IntegerBounds::Signed { minimum, maximum }) if value < minimum || value > maximum => {
                self.errors.push(CompileError::type_error(
                    format!(
                        "Integer literal {value} does not fit in {expected_ty}; valid range is {minimum}..={maximum}"
                    ),
                    expr.span,
                ));
            }
            Some(IntegerBounds::Unsigned { maximum }) if value < 0 || (value as u128) > maximum => {
                self.errors.push(CompileError::type_error(
                    format!("Integer literal {value} does not fit in {expected_ty}; valid range is 0..={maximum}"),
                    expr.span,
                ));
            }
            _ => {}
        }
        expected_ty.clone()
    }

    /// Typecheck a binary-float literal in a known `f32` or `f64` target context.
    fn check_float_literal_with_expected(&mut self, expr: &Spanned<Expr>, expected_ty: &ResolvedType) -> ResolvedType {
        let Expr::Literal(Literal::Float(value)) = &expr.node else {
            return self.check_expr(expr);
        };
        let fits = match expected_ty {
            ResolvedType::Numeric(incan_core::lang::types::numerics::NumericTypeId::F32) => {
                value.value.is_finite() && value.value.abs() <= f64::from(f32::MAX)
            }
            ResolvedType::Numeric(incan_core::lang::types::numerics::NumericTypeId::F64) => value.value.is_finite(),
            _ => true,
        };
        if !fits {
            self.errors.push(CompileError::type_error(
                format!("Float literal {} does not fit in {expected_ty}", value.repr),
                expr.span,
            ));
        }
        expected_ty.clone()
    }

    /// Reject an out-of-domain float literal nested beneath an exact-float arithmetic destination.
    ///
    /// Binary operands are otherwise checked independently, so the enclosing destination does not reach a literal
    /// such as `1e9999` in `1e9999 + 0.0`. This validation deliberately records no inferred type and does not make
    /// ordinary `float` finite-only; it only preserves the exact destination's literal-domain check and literal span.
    pub(in crate::frontend::typechecker::check_expr) fn validate_exact_float_literals_in_arithmetic(
        &mut self,
        expr: &Spanned<Expr>,
        expected_ty: &ResolvedType,
    ) {
        match &expr.node {
            Expr::Literal(Literal::Float(value)) => {
                let fits = match expected_ty {
                    ResolvedType::Numeric(incan_core::lang::types::numerics::NumericTypeId::F32) => {
                        value.value.is_finite() && value.value.abs() <= f64::from(f32::MAX)
                    }
                    ResolvedType::Numeric(incan_core::lang::types::numerics::NumericTypeId::F64) => {
                        value.value.is_finite()
                    }
                    _ => return,
                };
                if !fits {
                    self.errors.push(CompileError::type_error(
                        format!("Float literal {} does not fit in {expected_ty}", value.repr),
                        expr.span,
                    ));
                }
            }
            Expr::Unary(UnaryOp::Neg, inner) | Expr::Paren(inner) => {
                self.validate_exact_float_literals_in_arithmetic(inner, expected_ty);
            }
            Expr::Binary(
                left,
                BinaryOp::Add
                | BinaryOp::Sub
                | BinaryOp::Mul
                | BinaryOp::Div
                | BinaryOp::FloorDiv
                | BinaryOp::Mod
                | BinaryOp::Pow,
                right,
            ) => {
                self.validate_exact_float_literals_in_arithmetic(left, expected_ty);
                self.validate_exact_float_literals_in_arithmetic(right, expected_ty);
            }
            _ => {}
        }
    }

    /// Validate a decimal literal against a known decimal precision and scale.
    fn validate_decimal_literal_with_expected(&mut self, expr: &Spanned<Expr>, expected_ty: &ResolvedType) {
        let Expr::Literal(Literal::Decimal(value)) = &expr.node else {
            return;
        };
        let Some((precision, scale)) = decimal_precision_scale(expected_ty) else {
            return;
        };
        let Some((integer_digits, fractional_digits, total_digits)) = decimal_literal_digit_counts(value.body.as_str())
        else {
            self.errors.push(CompileError::type_error(
                format!("Decimal literal {} is not a plain decimal literal", value.repr),
                expr.span,
            ));
            return;
        };
        let max_integer_digits = precision - scale;
        if integer_digits > max_integer_digits {
            self.errors.push(CompileError::type_error(
                format!(
                    "Decimal literal {} has {integer_digits} integer digit(s), but {expected_ty} allows at most {max_integer_digits}"
                    , value.repr
                ),
                expr.span,
            ));
        }
        if fractional_digits > scale {
            self.errors.push(CompileError::type_error(
                format!(
                    "Decimal literal {} has {fractional_digits} fractional digit(s), but {expected_ty} allows at most {scale}"
                    , value.repr
                ),
                expr.span,
            ));
        }
        if total_digits > precision {
            self.errors.push(CompileError::type_error(
                format!(
                    "Decimal literal {} has {total_digits} total digit(s), but {expected_ty} allows at most {precision}",
                    value.repr
                ),
                expr.span,
            ));
        }
    }

    /// Typecheck a surface expression via the semantics registry.
    fn check_surface_expr(&mut self, expr: &SurfaceExpr, span: Span) -> ResolvedType {
        use crate::semantics_registry::semantics_registry;

        let Some(action) = semantics_registry().typecheck_surface_expr_action(&expr.key) else {
            // No pack claimed this surface expression — report as unknown.
            let label = match &expr.key {
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
            return ResolvedType::Unknown;
        };

        match (action, &expr.payload) {
            (SurfaceExprTypeCheck::AwaitCheck, SurfaceExprPayload::PrefixUnary(inner)) => self.check_await(inner, span),
            (SurfaceExprTypeCheck::RaceForCheck, SurfaceExprPayload::RaceFor(race)) => self.check_race_for(race, span),
            _ => ResolvedType::Unknown,
        }
    }

    /// Typecheck a descriptor-gated embedded-fragment expression (RFC 081, `#1023`).
    ///
    /// Unlike [`TypeChecker::check_surface_expr`], this node is never eliminated by the pre-typecheck vocab
    /// desugar pass (`src/frontend/vocab_desugar_pass/rewrite.rs`) — it must reach `check_expr` as itself, because
    /// its [`EmbeddedNode::Hole`] sub-expressions are genuine Incan expressions that need real types, exactly as
    /// if they appeared in ordinary expression position. The surrounding structural content (tags, selectors,
    /// declarations, regex/type shapes, ...) is DSL-owned syntax with no ordinary Incan type of its own — its
    /// runtime meaning is supplied by the owning DSL's desugarer or lowering hook (RFC 081 §Semantics), which is
    /// downstream of `#1023`. The fragment as a whole therefore resolves to `ResolvedType::Unknown`, deliberately
    /// reusing the existing "no further ordinary-Incan meaning to check here" sentinel rather than introducing a
    /// new `ResolvedType` variant that every exhaustive match over `ResolvedType` across the compiler would need
    /// to handle for a type with no ordinary-Incan operations anyway.
    fn check_embedded_fragment_expr(&mut self, fragment: &EmbeddedFragmentExpr) -> ResolvedType {
        for node in &fragment.nodes {
            self.check_embedded_fragment_node_holes(node);
        }
        ResolvedType::Unknown
    }

    /// Recursively typecheck every expression hole nested inside one embedded-fragment node.
    ///
    /// Structural node kinds with no possible nested hole (`Text`, `EntityRef`, `Comment`, `Value`, `Regex`,
    /// `TypeShape`) are no-ops here; `Hole` is the one leaf that reaches ordinary `check_expr`, and container
    /// kinds (`Element`, `StyleRule`, `Declaration`) recurse into their children/attrs/selectors/declarations.
    fn check_embedded_fragment_node_holes(&mut self, node: &Spanned<EmbeddedNode>) {
        match &node.node {
            EmbeddedNode::Text(_)
            | EmbeddedNode::EntityRef(_)
            | EmbeddedNode::Comment(_)
            | EmbeddedNode::Value(_)
            | EmbeddedNode::Regex { .. }
            | EmbeddedNode::TypeShape(_) => {}
            EmbeddedNode::Hole(expr) => {
                self.check_expr(expr);
            }
            EmbeddedNode::Element(element) => {
                for attr in &element.attrs {
                    if let Some(value) = &attr.value {
                        self.check_embedded_fragment_node_holes(value);
                    }
                }
                for child in &element.children {
                    self.check_embedded_fragment_node_holes(child);
                }
            }
            EmbeddedNode::StyleRule(rule) => {
                for selector in &rule.selectors {
                    self.check_embedded_fragment_node_holes(selector);
                }
                for declaration in &rule.declarations {
                    self.check_embedded_fragment_node_holes(declaration);
                }
            }
            EmbeddedNode::Declaration(declaration) => {
                for value in &declaration.value {
                    self.check_embedded_fragment_node_holes(value);
                }
            }
        }
    }
}

/// Return whether a resolved type is one of the parameterized decimal families.
fn is_decimal_type(ty: &ResolvedType) -> bool {
    match ty {
        ResolvedType::Generic(name, args) => {
            incan_core::lang::types::numerics::decimal_constructor_from_str(name.as_str()).is_some() && args.len() == 2
        }
        _ => false,
    }
}

/// Extract precision and scale from a checked resolved decimal type.
fn decimal_precision_scale(ty: &ResolvedType) -> Option<(usize, usize)> {
    match ty {
        ResolvedType::Generic(name, args)
            if incan_core::lang::types::numerics::decimal_constructor_from_str(name.as_str()).is_some()
                && args.len() == 2 =>
        {
            let precision = decimal_type_arg_usize(&args[0])?;
            let scale = decimal_type_arg_usize(&args[1])?;
            Some((precision, scale))
        }
        _ => None,
    }
}

/// Parse one resolved decimal type argument into a host integer for validation.
fn decimal_type_arg_usize(ty: &ResolvedType) -> Option<usize> {
    match ty {
        ResolvedType::TypeVar(value) => value.parse().ok(),
        _ => None,
    }
}

/// Count integer, fractional, and total digits in a plain decimal literal body.
fn decimal_literal_digit_counts(body: &str) -> Option<(usize, usize, usize)> {
    if body.contains('e') || body.contains('E') {
        return None;
    }
    let (integer, fractional) = body.split_once('.').unwrap_or((body, ""));
    let integer_digits = integer.chars().filter(|ch| ch.is_ascii_digit()).count();
    let fractional_digits = fractional.chars().filter(|ch| ch.is_ascii_digit()).count();
    Some((integer_digits, fractional_digits, integer_digits + fractional_digits))
}

/// Return the signed value represented by an integer literal or unary-negative integer literal.
fn signed_int_literal_value(expr: &Spanned<Expr>) -> Option<i128> {
    match &expr.node {
        Expr::Literal(Literal::Int(value)) => i128::try_from(value.magnitude).ok(),
        Expr::Unary(UnaryOp::Neg, inner) => match &inner.node {
            Expr::Literal(Literal::Int(value)) if value.magnitude == (1_u128 << 127) => Some(i128::MIN),
            Expr::Literal(Literal::Int(value)) => i128::try_from(value.magnitude).ok().map(|value| -value),
            _ => None,
        },
        _ => None,
    }
}

/// Return the unsigned magnitude for a non-negative integer literal.
fn unsigned_int_literal_magnitude(expr: &Spanned<Expr>) -> Option<u128> {
    match &expr.node {
        Expr::Literal(Literal::Int(value)) => Some(value.magnitude),
        _ => None,
    }
}
