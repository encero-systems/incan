//! Check collection literals (tuple, list, dict, and set).
//!
//! These helpers validate collection literal expressions and compute container element types using the current
//! checker's compatibility rules.

use crate::frontend::ast::*;
use crate::frontend::diagnostics::errors;
use crate::frontend::symbols::ResolvedType;
use crate::frontend::typechecker::helpers::{collection_type_id, dict_ty, list_ty, set_ty};
use incan_core::lang::types::collections::CollectionTypeId;

use super::TypeChecker;

impl TypeChecker {
    /// Extract the element type from an expected `List[T]` destination, if one is already known.
    fn list_expected_element_type(expected: Option<&ResolvedType>) -> Option<ResolvedType> {
        match expected {
            Some(ResolvedType::Generic(name, args))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::List) && args.len() == 1 =>
            {
                Some(args[0].clone())
            }
            _ => None,
        }
    }

    /// Type-check a list literal with an optional destination-type hint.
    ///
    /// When a surrounding context already expects `List[T]`, empty lists adopt `T` directly and non-empty lists
    /// validate each element against that hinted type. Without a hint, the first element still seeds the candidate
    /// element type, but later elements must remain compatible instead of being ignored.
    pub(in crate::frontend::typechecker::check_expr) fn check_list_with_expected(
        &mut self,
        elems: &[Spanned<Expr>],
        expected: Option<&ResolvedType>,
    ) -> ResolvedType {
        let hinted_elem_ty = Self::list_expected_element_type(expected);
        let mut elem_ty = hinted_elem_ty.clone().unwrap_or(ResolvedType::Unknown);

        for (index, elem) in elems.iter().enumerate() {
            let value_ty = self.check_expr_with_expected(elem, hinted_elem_ty.as_ref());

            if index == 0 && hinted_elem_ty.is_none() {
                elem_ty = value_ty.clone();
                continue;
            }

            if self.types_compatible(&value_ty, &elem_ty) {
                continue;
            }

            if self.types_compatible(&elem_ty, &value_ty) {
                elem_ty = value_ty;
                continue;
            }

            self.errors.push(errors::type_mismatch(
                &elem_ty.to_string(),
                &value_ty.to_string(),
                elem.span,
            ));
        }

        list_ty(elem_ty)
    }

    /// Type-check a tuple literal.
    pub(in crate::frontend::typechecker::check_expr) fn check_tuple(
        &mut self,
        elems: &[Spanned<Expr>],
    ) -> ResolvedType {
        let elem_types: Vec<_> = elems.iter().map(|e| self.check_expr(e)).collect();
        ResolvedType::Tuple(elem_types)
    }

    /// Type-check a list literal.
    pub(in crate::frontend::typechecker::check_expr) fn check_list(&mut self, elems: &[Spanned<Expr>]) -> ResolvedType {
        self.check_list_with_expected(elems, None)
    }

    /// Type-check a dict literal.
    pub(in crate::frontend::typechecker::check_expr) fn check_dict(
        &mut self,
        entries: &[(Spanned<Expr>, Spanned<Expr>)],
    ) -> ResolvedType {
        let (key_ty, val_ty) = if let Some((k, v)) = entries.first() {
            (self.check_expr(k), self.check_expr(v))
        } else {
            (ResolvedType::Unknown, ResolvedType::Unknown)
        };

        for (k, v) in entries.iter().skip(1) {
            self.check_expr(k);
            self.check_expr(v);
        }

        dict_ty(key_ty, val_ty)
    }

    /// Type-check a set literal.
    pub(in crate::frontend::typechecker::check_expr) fn check_set(&mut self, elems: &[Spanned<Expr>]) -> ResolvedType {
        let elem_ty = if let Some(first) = elems.first() {
            self.check_expr(first)
        } else {
            ResolvedType::Unknown
        };

        for elem in elems.iter().skip(1) {
            self.check_expr(elem);
        }

        set_ty(elem_ty)
    }
}
