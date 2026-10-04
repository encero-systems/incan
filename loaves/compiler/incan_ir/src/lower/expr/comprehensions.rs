//! Comprehension and generator-expression lowering.

use super::super::super::TypedExpr;
use super::super::super::expr::{IrExprKind, IrGeneratorClause, VarAccess};
use super::super::super::types::IrType;
use super::super::AstLowering;
use super::super::errors::LoweringError;
use super::frozen_reads::owned_frozen_iteration_source;
use incan_frontend::ast;
use incan_lang::lang::types::collections::{self, CollectionTypeId};

impl AstLowering {
    /// Lower a generator expression `(expr for ... if ...)`.
    pub(in crate::lower) fn lower_generator_expr(
        &mut self,
        generator: &ast::GeneratorExpr,
    ) -> Result<(IrExprKind, IrType), LoweringError> {
        let mut clauses = Vec::with_capacity(generator.clauses.len());
        self.non_linear_context_depth += 1;
        for clause in &generator.clauses {
            match clause {
                ast::ComprehensionClause::For { pattern, iter } => {
                    clauses.push(IrGeneratorClause::For {
                        pattern: self.lower_pattern(&pattern.node),
                        iterable: Box::new(owned_frozen_iteration_source(self.lower_expr_spanned(iter)?)),
                    });
                }
                ast::ComprehensionClause::If(condition) => {
                    clauses.push(IrGeneratorClause::If(self.lower_expr_spanned(condition)?));
                }
            }
        }
        let element = self.lower_expr_spanned(&generator.expr);
        self.non_linear_context_depth -= 1;
        let element = element?;
        let element_ty = element.ty.clone();

        Ok((
            IrExprKind::Generator {
                element: Box::new(element),
                clauses,
            },
            IrType::NamedGeneric(
                collections::as_str(CollectionTypeId::Generator).to_string(),
                vec![element_ty],
            ),
        ))
    }

    /// Lower a list comprehension `[expr for var in iter if cond]`.
    ///
    /// An `Ok(...)` or `Err(...)` element is built with the element type the checker settled for the comprehension
    /// (#1561).
    pub(in crate::lower) fn lower_list_comp(
        &mut self,
        comp: &ast::ListComp,
        span: ast::Span,
    ) -> Result<(IrExprKind, IrType), LoweringError> {
        let mut iter_expr = self.lower_expr_spanned(&comp.iter)?;
        self.apply_checked_comprehension_source_consumption(comp.iter.span, &mut iter_expr);
        let iter_expr = owned_frozen_iteration_source(iter_expr);
        let pattern = self.lower_pattern(&comp.pattern.node);

        // Build the filter predicate if present
        self.non_linear_context_depth += 1;
        let filter_tokens_result: Result<Option<Box<TypedExpr>>, LoweringError> = if let Some(filter) = &comp.filter {
            Ok(Some(Box::new(self.lower_expr_spanned(filter)?)))
        } else {
            Ok(None)
        };

        // Build the map expression
        let map_expr_result = self.lower_expr_spanned(&comp.expr);
        self.non_linear_context_depth -= 1;
        let filter_tokens = filter_tokens_result?;
        let mut map_expr = map_expr_result?;
        self.pin_settled_comprehension_result_constructor(span, &mut map_expr);

        // Determine element type from map expression
        let elem_ty = map_expr.ty.clone();

        Ok((
            IrExprKind::ListComp {
                element: Box::new(map_expr),
                pattern: Box::new(pattern),
                iterable: Box::new(iter_expr),
                filter: filter_tokens,
            },
            IrType::List(Box::new(elem_ty)),
        ))
    }

    /// Lower a dict comprehension `{key: value for var in iter if cond}`.
    pub(in crate::lower) fn lower_dict_comp(
        &mut self,
        comp: &ast::DictComp,
    ) -> Result<(IrExprKind, IrType), LoweringError> {
        let mut iter_expr = self.lower_expr_spanned(&comp.iter)?;
        self.apply_checked_comprehension_source_consumption(comp.iter.span, &mut iter_expr);
        let iter_expr = owned_frozen_iteration_source(iter_expr);
        let pattern = self.lower_pattern(&comp.pattern.node);

        self.non_linear_context_depth += 1;
        let filter_tokens_result: Result<Option<Box<TypedExpr>>, LoweringError> = if let Some(filter) = &comp.filter {
            Ok(Some(Box::new(self.lower_expr_spanned(filter)?)))
        } else {
            Ok(None)
        };

        let key_expr_result = self.lower_expr_spanned(&comp.key);
        let value_expr_result = self.lower_expr_spanned(&comp.value);
        self.non_linear_context_depth -= 1;
        let filter_tokens = filter_tokens_result?;
        let key_expr = key_expr_result?;
        let value_expr = value_expr_result?;

        let key_ty = key_expr.ty.clone();
        let value_ty = value_expr.ty.clone();

        Ok((
            IrExprKind::DictComp {
                key: Box::new(key_expr),
                value: Box::new(value_expr),
                pattern: Box::new(pattern),
                iterable: Box::new(iter_expr),
                filter: filter_tokens,
            },
            IrType::Dict(Box::new(key_ty), Box::new(value_ty)),
        ))
    }

    /// Apply the frontend's source-consumption fact to a direct comprehension iterable (#1983).
    ///
    /// The checker has already used this exact decision to require `Clone` when the source stays live. Overriding the
    /// general identifier access here prevents lowering from independently re-running its last-use heuristic and
    /// makes the emitter's owned-versus-borrowed item plan agree with the diagnostic by construction.
    fn apply_checked_comprehension_source_consumption(&self, span: ast::Span, iterable: &mut TypedExpr) {
        let Some(consumed) = self
            .type_info
            .as_ref()
            .and_then(|info| info.comprehension_source_is_consumed(span))
        else {
            return;
        };
        if let IrExprKind::Var { access, .. } = &mut iterable.kind {
            *access = if consumed { VarAccess::Move } else { VarAccess::Read };
        }
    }
}
