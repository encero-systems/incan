//! List and dict comprehension lowering.

use super::super::super::TypedExpr;
use super::super::super::expr::IrExprKind;
use super::super::super::types::IrType;
use super::super::AstLowering;
use super::super::errors::LoweringError;
use crate::frontend::ast;

impl AstLowering {
    /// Lower a list comprehension `[expr for var in iter if cond]`.
    pub(in crate::backend::ir::lower) fn lower_list_comp(
        &mut self,
        comp: &ast::ListComp,
    ) -> Result<(IrExprKind, IrType), LoweringError> {
        let iter_expr = self.lower_expr(&comp.iter.node)?;
        let var_name = comp.var.clone();

        // Build the filter predicate if present
        self.non_linear_context_depth += 1;
        let filter_tokens_result: Result<Option<Box<TypedExpr>>, LoweringError> = if let Some(filter) = &comp.filter {
            Ok(Some(Box::new(self.lower_expr(&filter.node)?)))
        } else {
            Ok(None)
        };

        // Build the map expression
        let map_expr_result = self.lower_expr(&comp.expr.node);
        self.non_linear_context_depth -= 1;
        let filter_tokens = filter_tokens_result?;
        let map_expr = map_expr_result?;

        // Determine element type from map expression
        let elem_ty = map_expr.ty.clone();

        Ok((
            IrExprKind::ListComp {
                element: Box::new(map_expr),
                variable: var_name,
                iterable: Box::new(iter_expr),
                filter: filter_tokens,
            },
            IrType::List(Box::new(elem_ty)),
        ))
    }

    /// Lower a dict comprehension `{key: value for var in iter if cond}`.
    pub(in crate::backend::ir::lower) fn lower_dict_comp(
        &mut self,
        comp: &ast::DictComp,
    ) -> Result<(IrExprKind, IrType), LoweringError> {
        let iter_expr = self.lower_expr(&comp.iter.node)?;
        let var_name = comp.var.clone();

        self.non_linear_context_depth += 1;
        let filter_tokens_result: Result<Option<Box<TypedExpr>>, LoweringError> = if let Some(filter) = &comp.filter {
            Ok(Some(Box::new(self.lower_expr(&filter.node)?)))
        } else {
            Ok(None)
        };

        let key_expr_result = self.lower_expr(&comp.key.node);
        let value_expr_result = self.lower_expr(&comp.value.node);
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
                variable: var_name,
                iterable: Box::new(iter_expr),
                filter: filter_tokens,
            },
            IrType::Dict(Box::new(key_ty), Box::new(value_ty)),
        ))
    }
}
