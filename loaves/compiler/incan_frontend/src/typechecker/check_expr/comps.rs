//! Check comprehensions and closures.
//!
//! This module implements comprehensions, generator expressions, and closure expressions, introducing local bindings
//! and type-checking the generated element/value expressions in a nested scope.

use crate::ast::*;
use crate::diagnostics::errors::{self, HashedCollectionRole};
use crate::symbols::*;
use crate::typechecker::helpers::{dict_ty, generator_ty, list_ty};

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
    pub(in crate::typechecker::check_expr) fn check_generator_expr(
        &mut self,
        generator: &GeneratorExpr,
        _span: Span,
    ) -> ResolvedType {
        self.symbols.enter_scope(ScopeKind::Block);
        let mut previous_views = None;

        for clause in &generator.clauses {
            match clause {
                ComprehensionClause::For { pattern, iter } => {
                    let iter_ty = self.check_expr(iter);
                    let elem_ty = self.infer_iterator_element_type_from_expr(iter, &iter_ty);
                    // The iterated place is resolved before the clause's bindings shadow it.
                    let item_views = self.generator_item_views(iter, &pattern.node);
                    // Record the element type at the pattern's own span, exactly as `check_for_stmt` does for a
                    // statement `for` (#1125). Body IR reads a clause pattern's type back through
                    // `TypeCheckInfo::expr_type`, so without this a destructuring clause binds names typed
                    // `Unknown` even though the element type is fully resolved right here -- and the same
                    // source destructured by a statement `for` would bind them concretely (#1161).
                    self.record_expr_type(pattern.span, elem_ty.clone());
                    self.define_for_pattern_bindings(pattern, &elem_ty);
                    let entered = self.enter_item_views(item_views);
                    previous_views.get_or_insert(entered);
                }
                ComprehensionClause::If(condition) => {
                    let cond_ty = self.check_expr(condition);
                    self.validate_truthiness_condition(&cond_ty, condition.span);
                }
            }
        }

        let result_elem_ty = self.check_expr(&generator.expr);
        if let Some(previous) = previous_views {
            self.exit_pattern_views(previous);
        }
        self.symbols.exit_scope();

        generator_ty(result_elem_ty)
    }

    /// Type-check a list comprehension and return `List[T]`.
    pub(in crate::typechecker::check_expr) fn check_list_comp(&mut self, comp: &ListComp, _span: Span) -> ResolvedType {
        let iter_ty = self.check_expr(&comp.iter);
        let elem_ty = self.infer_iterator_element_type_from_expr(&comp.iter, &iter_ty);
        // The iterated place is resolved before the clause's bindings shadow it.
        let item_views = self.read_only_comprehension_item_views(&comp.iter, &comp.pattern.node);

        self.symbols.enter_scope(ScopeKind::Block);
        // See `check_generator_expr` for why the element type is recorded at the pattern's span.
        self.record_expr_type(comp.pattern.span, elem_ty.clone());
        self.define_for_pattern_bindings(&comp.pattern, &elem_ty);
        let previous_views = self.enter_item_views(item_views);

        if let Some(filter) = &comp.filter {
            self.check_expr(filter);
        }

        let result_elem_ty = self.check_expr(&comp.expr);
        // A side an `Ok(...)` or `Err(...)` element leaves open is built with a type all the same (#1561).
        let result_elem_ty = self.settle_open_constructor_side(&comp.expr, result_elem_ty);
        self.exit_pattern_views(previous_views);
        self.symbols.exit_scope();

        list_ty(result_elem_ty)
    }

    /// Type-check a dict comprehension and return `Dict[K, V]`, refusing a key type without `Eq` and `Hash` (#1758).
    pub(in crate::typechecker::check_expr) fn check_dict_comp(&mut self, comp: &DictComp, _span: Span) -> ResolvedType {
        let iter_ty = self.check_expr(&comp.iter);
        let elem_ty = self.infer_iterator_element_type_from_expr(&comp.iter, &iter_ty);
        // A dict comprehension reads the items it changes in place, as a list comprehension does (#1561).
        let item_views = self.read_only_comprehension_item_views(&comp.iter, &comp.pattern.node);

        self.symbols.enter_scope(ScopeKind::Block);
        // See `check_generator_expr` for why the element type is recorded at the pattern's span.
        self.record_expr_type(comp.pattern.span, elem_ty.clone());
        self.define_for_pattern_bindings(&comp.pattern, &elem_ty);
        let previous_views = self.enter_item_views(item_views);

        if let Some(filter) = &comp.filter {
            self.check_expr(filter);
        }

        let key_ty = self.check_expr(&comp.key);
        let val_ty = self.check_expr(&comp.value);
        self.exit_pattern_views(previous_views);
        self.symbols.exit_scope();
        self.refuse_unhashable_collection_member(HashedCollectionRole::DictKey, &key_ty, comp.key.span);

        dict_ty(key_ty, val_ty)
    }

    /// Type-check a closure expression and return a function type.
    pub(in crate::typechecker::check_expr) fn check_closure(
        &mut self,
        params: &[Spanned<Param>],
        body: &Spanned<Expr>,
        span: Span,
    ) -> ResolvedType {
        self.note_closure_captures(params, body, span);
        self.symbols.enter_scope(ScopeKind::Function);

        let prev_in_async_body = self.in_async_body;
        self.in_async_body = false;
        let prev_return_error_type = self.current_return_error_type.take();

        let param_types: Vec<_> = params
            .iter()
            .map(|p| {
                let ty = self.resolve_type_checked(&p.node.ty);
                self.validate_protected_builtin_binding(&p.node.name, p.span);
                self.symbols.define_with_target_kind(
                    Symbol {
                        name: p.node.name.clone(),
                        kind: SymbolKind::Variable(VariableInfo {
                            ty: ty.clone(),
                            is_mutable: false,
                            is_used: false,
                        }),
                        span: p.span,
                        scope: 0,
                    },
                    incan_semantics_core::SemanticSourceTargetKind::Parameter,
                );
                self.record_write_target_identity(p.span, &p.node.name);
                CallableParam::named(p.node.name.clone(), ty, p.node.kind)
            })
            .collect();

        self.enter_mut_param_closure();
        let return_ty = self.check_expr(body);
        self.exit_mut_param_closure();
        // A side the body's `Ok(...)` or `Err(...)` leaves open is built with a type all the same (#1561).
        let return_ty = self.settle_open_closure_result_side(body, return_ty, None);
        self.current_return_error_type = prev_return_error_type;
        self.in_async_body = prev_in_async_body;
        self.symbols.exit_scope();

        ResolvedType::Function(param_types, Box::new(return_ty))
    }

    /// Type-check a closure expression against an expected function shape.
    pub(in crate::typechecker::check_expr) fn check_closure_with_expected(
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

        self.note_closure_captures(params, body, span);
        self.symbols.enter_scope(ScopeKind::Function);

        let prev_in_async_body = self.in_async_body;
        self.in_async_body = false;
        let prev_return_error_type = self.current_return_error_type.take();

        let param_types: Vec<_> = params
            .iter()
            .zip(expected_params.iter())
            .map(|(param, expected)| {
                let ty = expected.ty.clone();
                self.validate_protected_builtin_binding(&param.node.name, param.span);
                self.symbols.define_with_target_kind(
                    Symbol {
                        name: param.node.name.clone(),
                        kind: SymbolKind::Variable(VariableInfo {
                            ty: ty.clone(),
                            is_mutable: false,
                            is_used: false,
                        }),
                        span: param.span,
                        scope: 0,
                    },
                    incan_semantics_core::SemanticSourceTargetKind::Parameter,
                );
                self.record_write_target_identity(param.span, &param.node.name);
                // The closure takes each argument the way the expected shape passes it, `mut` marker included.
                CallableParam::named(param.node.name.clone(), ty, param.node.kind).with_mut(expected.is_mut)
            })
            .collect();

        self.enter_mut_param_closure();
        let return_ty = self.check_expr_with_expected(body, Some(expected_ret));
        self.exit_mut_param_closure();
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

        // An open expected result (a `map` callback's) is the body's type, as a result type left to inference is.
        let resolved_return =
            if Self::closure_output_needs_inference(expected_ret) || matches!(expected_ret, ResolvedType::Unknown) {
                return_ty
            } else {
                expected_ret.clone()
            };
        // A side the body's `Ok(...)` or `Err(...)` leaves open and the expected type does not fix, such as the error
        // side of an `or_else` callback's `Ok(...)`, is built with a type all the same (#1561).
        let resolved_return = self.settle_open_closure_result_side(body, resolved_return, Some(expected_ret));
        ResolvedType::Function(param_types, Box::new(resolved_return))
    }
}
