//! Check comprehensions and closures.
//!
//! This module implements comprehensions, generator expressions, and closure expressions, introducing local bindings
//! and type-checking the generated element/value expressions in a nested scope.

use crate::frontend::ast::*;
use crate::frontend::diagnostics::errors;
use crate::frontend::symbols::*;
use crate::frontend::typechecker::helpers::{dict_ty, generator_ty, list_ty};

use super::TypeChecker;

impl TypeChecker {
    /// Return whether contextual callable output still contains a method-level type variable to infer.
    fn closure_output_needs_inference(ty: &ResolvedType) -> bool {
        match ty {
            ResolvedType::TypeVar(_) => true,
            ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => {
                args.iter().any(Self::closure_output_needs_inference)
            }
            ResolvedType::Function(params, output) => {
                params
                    .iter()
                    .any(|param| Self::closure_output_needs_inference(&param.ty))
                    || Self::closure_output_needs_inference(output)
            }
            ResolvedType::FrozenList(inner)
            | ResolvedType::FrozenSet(inner)
            | ResolvedType::TypeToken(inner)
            | ResolvedType::Ref(inner)
            | ResolvedType::RefMut(inner) => Self::closure_output_needs_inference(inner),
            ResolvedType::FrozenDict(key, value) => {
                Self::closure_output_needs_inference(key) || Self::closure_output_needs_inference(value)
            }
            ResolvedType::Never
            | ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Numeric(_)
            | ResolvedType::Bool
            | ResolvedType::Str
            | ResolvedType::Bytes
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
            | ResolvedType::Unit
            | ResolvedType::Named(_)
            | ResolvedType::SelfType
            | ResolvedType::RustPath(_)
            | ResolvedType::CallSiteInfer
            | ResolvedType::Unknown => false,
        }
    }

    /// Type-check a generator expression and return `Generator[T]`.
    pub(in crate::frontend::typechecker::check_expr) fn check_generator_expr(
        &mut self,
        generator: &GeneratorExpr,
        _span: Span,
    ) -> ResolvedType {
        self.symbols.enter_scope(ScopeKind::Block);

        for clause in &generator.clauses {
            match clause {
                ComprehensionClause::For { pattern, iter } => {
                    let iter_ty = self.check_expr(iter);
                    let elem_ty = self.infer_iterator_element_type_from_expr(iter, &iter_ty);
                    self.define_for_pattern_bindings(pattern, &elem_ty);
                }
                ComprehensionClause::If(condition) => {
                    let cond_ty = self.check_expr(condition);
                    self.validate_truthiness_condition(&cond_ty, condition.span);
                }
            }
        }

        let result_elem_ty = self.check_expr(&generator.expr);
        self.symbols.exit_scope();

        generator_ty(result_elem_ty)
    }

    /// Type-check a list comprehension and return `List[T]`.
    pub(in crate::frontend::typechecker::check_expr) fn check_list_comp(
        &mut self,
        comp: &ListComp,
        _span: Span,
    ) -> ResolvedType {
        let iter_ty = self.check_expr(&comp.iter);
        let elem_ty = self.infer_iterator_element_type_from_expr(&comp.iter, &iter_ty);

        self.symbols.enter_scope(ScopeKind::Block);
        self.define_for_pattern_bindings(&comp.pattern, &elem_ty);

        if let Some(filter) = &comp.filter {
            self.check_expr(filter);
        }

        let result_elem_ty = self.check_expr(&comp.expr);
        self.symbols.exit_scope();

        list_ty(result_elem_ty)
    }

    /// Type-check a dict comprehension and return `Dict[K, V]`.
    pub(in crate::frontend::typechecker::check_expr) fn check_dict_comp(
        &mut self,
        comp: &DictComp,
        _span: Span,
    ) -> ResolvedType {
        let iter_ty = self.check_expr(&comp.iter);
        let elem_ty = self.infer_iterator_element_type_from_expr(&comp.iter, &iter_ty);

        self.symbols.enter_scope(ScopeKind::Block);
        self.define_for_pattern_bindings(&comp.pattern, &elem_ty);

        if let Some(filter) = &comp.filter {
            self.check_expr(filter);
        }

        let key_ty = self.check_expr(&comp.key);
        let val_ty = self.check_expr(&comp.value);
        self.symbols.exit_scope();

        dict_ty(key_ty, val_ty)
    }

    /// Type-check a closure expression and return a function type.
    pub(in crate::frontend::typechecker::check_expr) fn check_closure(
        &mut self,
        params: &[Spanned<Param>],
        body: &Spanned<Expr>,
        _: Span,
    ) -> ResolvedType {
        self.symbols.enter_scope(ScopeKind::Function);

        let prev_in_async_body = self.in_async_body;
        self.in_async_body = false;
        let prev_return_error_type = self.current_return_error_type.take();

        let param_types: Vec<_> = params
            .iter()
            .map(|p| {
                let ty = self.resolve_type_checked(&p.node.ty);
                self.symbols.define(Symbol {
                    name: p.node.name.clone(),
                    kind: SymbolKind::Variable(VariableInfo {
                        ty: ty.clone(),
                        is_mutable: false,
                        is_used: false,
                    }),
                    span: p.span,
                    scope: 0,
                });
                CallableParam::named(p.node.name.clone(), ty, p.node.kind)
            })
            .collect();

        let return_ty = self.check_expr(body);
        self.current_return_error_type = prev_return_error_type;
        self.in_async_body = prev_in_async_body;
        self.symbols.exit_scope();

        ResolvedType::Function(param_types, Box::new(return_ty))
    }

    /// Type-check a closure expression against an expected function shape.
    pub(in crate::frontend::typechecker::check_expr) fn check_closure_with_expected(
        &mut self,
        params: &[Spanned<Param>],
        body: &Spanned<Expr>,
        expected_params: &[CallableParam],
        expected_ret: &ResolvedType,
        span: Span,
    ) -> ResolvedType {
        if params.len() != expected_params.len() {
            self.errors.push(errors::builtin_arity(
                "closure",
                expected_params.len(),
                params.len(),
                span,
            ));
            return ResolvedType::Unknown;
        }

        self.symbols.enter_scope(ScopeKind::Function);

        let prev_in_async_body = self.in_async_body;
        self.in_async_body = false;
        let prev_return_error_type = self.current_return_error_type.take();

        let param_types: Vec<_> = params
            .iter()
            .zip(expected_params.iter())
            .map(|(param, expected)| {
                let ty = expected.ty.clone();
                self.symbols.define(Symbol {
                    name: param.node.name.clone(),
                    kind: SymbolKind::Variable(VariableInfo {
                        ty: ty.clone(),
                        is_mutable: false,
                        is_used: false,
                    }),
                    span: param.span,
                    scope: 0,
                });
                CallableParam::named(param.node.name.clone(), ty, param.node.kind)
            })
            .collect();

        let return_ty = self.check_expr_with_expected(body, Some(expected_ret));
        if !matches!(return_ty, ResolvedType::Unknown) && !self.types_compatible(&return_ty, expected_ret) {
            self.errors.push(errors::type_mismatch(
                &expected_ret.to_string(),
                &return_ty.to_string(),
                body.span,
            ));
        }

        self.current_return_error_type = prev_return_error_type;
        self.in_async_body = prev_in_async_body;
        self.symbols.exit_scope();

        let resolved_return = if Self::closure_output_needs_inference(expected_ret) {
            return_ty
        } else {
            expected_ret.clone()
        };
        ResolvedType::Function(param_types, Box::new(resolved_return))
    }
}
