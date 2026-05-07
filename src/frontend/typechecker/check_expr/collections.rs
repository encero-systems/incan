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

    /// Extract key/value types from an expected `Dict[K, V]` destination, if one is already known.
    fn dict_expected_entry_types(expected: Option<&ResolvedType>) -> (Option<ResolvedType>, Option<ResolvedType>) {
        match expected {
            Some(ResolvedType::Generic(name, args))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Dict) && args.len() == 2 =>
            {
                (Some(args[0].clone()), Some(args[1].clone()))
            }
            _ => (None, None),
        }
    }

    /// Extract the element type from a statically known list spread operand.
    fn list_spread_element_type(ty: &ResolvedType) -> Option<ResolvedType> {
        match ty {
            ResolvedType::Generic(name, args)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::List) && args.len() == 1 =>
            {
                Some(args[0].clone())
            }
            _ => None,
        }
    }

    /// Extract key/value types from a statically known dict spread operand.
    fn dict_spread_entry_types(ty: &ResolvedType) -> Option<(ResolvedType, ResolvedType)> {
        match ty {
            ResolvedType::Generic(name, args)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Dict) && args.len() == 2 =>
            {
                Some((args[0].clone(), args[1].clone()))
            }
            _ => None,
        }
    }

    /// Merge one observed collection member type into the literal's candidate member type.
    fn merge_collection_member_type(&mut self, member_ty: &mut ResolvedType, value_ty: ResolvedType, span: Span) {
        if matches!(member_ty, ResolvedType::Unknown) {
            *member_ty = value_ty;
            return;
        }

        if self.types_compatible(&value_ty, member_ty) {
            return;
        }

        if self.types_compatible(member_ty, &value_ty) {
            *member_ty = value_ty;
            return;
        }

        self.errors.push(errors::type_mismatch(
            &member_ty.to_string(),
            &value_ty.to_string(),
            span,
        ));
    }

    /// Type-check a list literal with an optional destination-type hint.
    ///
    /// When a surrounding context already expects `List[T]`, empty lists adopt `T` directly and non-empty lists
    /// validate each element against that hinted type. Without a hint, the first element still seeds the candidate
    /// element type, but later elements must remain compatible instead of being ignored.
    pub(in crate::frontend::typechecker::check_expr) fn check_list_with_expected(
        &mut self,
        elems: &[ListEntry],
        expected: Option<&ResolvedType>,
    ) -> ResolvedType {
        let hinted_elem_ty = Self::list_expected_element_type(expected);
        let mut elem_ty = hinted_elem_ty.clone().unwrap_or(ResolvedType::Unknown);

        for elem in elems {
            match elem {
                ListEntry::Element(value) => {
                    let value_ty = self.check_expr_with_expected(value, hinted_elem_ty.as_ref());
                    self.merge_collection_member_type(&mut elem_ty, value_ty, value.span);
                }
                ListEntry::Spread(value) => {
                    let expected_spread = hinted_elem_ty.clone().map(list_ty);
                    let spread_ty = self.check_expr_with_expected(value, expected_spread.as_ref());
                    if let ResolvedType::Tuple(item_types) = spread_ty {
                        for item_ty in item_types {
                            self.merge_collection_member_type(&mut elem_ty, item_ty, value.span);
                        }
                    } else if let ResolvedType::Generic(name, item_types) = &spread_ty
                        && collection_type_id(name.as_str()) == Some(CollectionTypeId::Tuple)
                    {
                        for item_ty in item_types {
                            self.merge_collection_member_type(&mut elem_ty, item_ty.clone(), value.span);
                        }
                    } else if let Some(value_ty) = Self::list_spread_element_type(&spread_ty) {
                        self.merge_collection_member_type(&mut elem_ty, value_ty, value.span);
                    } else {
                        self.errors.push(errors::type_mismatch(
                            "List[_] or tuple[...]",
                            &spread_ty.to_string(),
                            value.span,
                        ));
                    }
                }
            }
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
    pub(in crate::frontend::typechecker::check_expr) fn check_list(&mut self, elems: &[ListEntry]) -> ResolvedType {
        self.check_list_with_expected(elems, None)
    }

    /// Type-check a dict literal.
    pub(in crate::frontend::typechecker::check_expr) fn check_dict(&mut self, entries: &[DictEntry]) -> ResolvedType {
        self.check_dict_with_expected(entries, None)
    }

    /// Type-check a dict literal with an optional destination-type hint.
    pub(in crate::frontend::typechecker::check_expr) fn check_dict_with_expected(
        &mut self,
        entries: &[DictEntry],
        expected: Option<&ResolvedType>,
    ) -> ResolvedType {
        let (hinted_key_ty, hinted_value_ty) = Self::dict_expected_entry_types(expected);
        let mut key_ty = hinted_key_ty.clone().unwrap_or(ResolvedType::Unknown);
        let mut val_ty = hinted_value_ty.clone().unwrap_or(ResolvedType::Unknown);

        for entry in entries {
            match entry {
                DictEntry::Pair(key, value) => {
                    let observed_key_ty = self.check_expr_with_expected(key, hinted_key_ty.as_ref());
                    let observed_value_ty = self.check_expr_with_expected(value, hinted_value_ty.as_ref());
                    self.merge_collection_member_type(&mut key_ty, observed_key_ty, key.span);
                    self.merge_collection_member_type(&mut val_ty, observed_value_ty, value.span);
                }
                DictEntry::Spread(value) => {
                    let expected_spread = match (hinted_key_ty.clone(), hinted_value_ty.clone()) {
                        (Some(key), Some(value)) => Some(dict_ty(key, value)),
                        _ => None,
                    };
                    let spread_ty = self.check_expr_with_expected(value, expected_spread.as_ref());
                    let Some((observed_key_ty, observed_value_ty)) = Self::dict_spread_entry_types(&spread_ty) else {
                        self.errors
                            .push(errors::type_mismatch("Dict[_, _]", &spread_ty.to_string(), value.span));
                        continue;
                    };
                    self.merge_collection_member_type(&mut key_ty, observed_key_ty, value.span);
                    self.merge_collection_member_type(&mut val_ty, observed_value_ty, value.span);
                }
            }
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
