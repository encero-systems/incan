//! Comprehension and generator-expression lowering.

use super::super::super::TypedExpr;
use super::super::super::expr::{BuiltinFn, IrExprKind, IrGeneratorClause};
use super::super::super::types::IrType;
use super::super::AstLowering;
use super::super::errors::LoweringError;
use incan_frontend::ast;
use incan_lang::lang::types::collections::{self, CollectionTypeId};

impl AstLowering {
    /// Hand a comprehension over a frozen collection of Incan text the owned `list[str]` it iterates.
    ///
    /// A `const` `FrozenList[str]` or `FrozenSet[str]` stores its text as `'static` slices, while the comprehension
    /// binds each item as the owned `str` the checker typed it as. Iterating the source's `list(...)` conversion, the
    /// conversion `list(source)` performs anywhere, yields exactly those owned items (#1757). Every other source is
    /// returned unchanged.
    fn owned_text_comprehension_source(source: TypedExpr) -> TypedExpr {
        let is_frozen_text = match &source.ty {
            IrType::NamedGeneric(name, items) => {
                matches!(
                    collections::from_str(name),
                    Some(CollectionTypeId::FrozenList | CollectionTypeId::FrozenSet)
                ) && matches!(items.first(), Some(IrType::String | IrType::StaticStr | IrType::StrRef))
            }
            _ => false,
        };
        if !is_frozen_text {
            return source;
        }
        let span = source.span;
        TypedExpr::new(
            IrExprKind::BuiltinCall {
                func: BuiltinFn::CollectionConstructor(CollectionTypeId::List),
                args: vec![source],
            },
            IrType::List(Box::new(IrType::String)),
        )
        .with_span(span)
    }

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
                        iterable: Box::new(Self::owned_text_comprehension_source(self.lower_expr_spanned(iter)?)),
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
    pub(in crate::lower) fn lower_list_comp(
        &mut self,
        comp: &ast::ListComp,
    ) -> Result<(IrExprKind, IrType), LoweringError> {
        let iter_expr = Self::owned_text_comprehension_source(self.lower_expr_spanned(&comp.iter)?);
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
        let map_expr = map_expr_result?;

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
        let iter_expr = Self::owned_text_comprehension_source(self.lower_expr_spanned(&comp.iter)?);
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
}
