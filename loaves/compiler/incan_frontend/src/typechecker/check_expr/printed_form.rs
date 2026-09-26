//! One display rule for `print`/`println` arguments, `str(value)` and f-string `{value}` parts (#1748).
//!
//! The three positions display a value alike. Scalars and `str` display their own text, a type that defines `__str__`
//! displays what it returns, and a tuple, list, dict, set, `Option` or `Result` displays its structure (lowering hands
//! `print` and `str` the same rendering an f-string part lowers to). A value with none of those has no printed form, so
//! the checker refuses it in every position with `INCAN-T0103` instead of leaving the build to fail on it.

use incan_lang::lang::magic_methods::{self, MagicMethodId};
use incan_lang::lang::traits::{self as core_traits, TraitId};
use incan_lang::lang::types::collections::CollectionTypeId;

use super::TypeChecker;
use crate::ast::{Expr, Spanned};
use crate::diagnostics::errors::{self, DisplayPosition, UnprintableValue};
use crate::symbols::{ResolvedType, TypeInfo};
use crate::typechecker::helpers::collection_type_id;

/// How deep a class's `extends` chain is followed when looking for an inherited `__str__`.
const MAX_EXTENDS_DEPTH: usize = 64;

impl TypeChecker {
    /// Refuse one displayed operand whose checked type has no printed form.
    ///
    /// `position` is where the source displays it and words the message; `operand` names the value in the message
    /// when the source spells it as a plain name. Every value with a printed form, and every type the checker cannot
    /// classify (a type parameter, a Rust type, an unknown), is left alone.
    pub(in crate::typechecker::check_expr) fn check_display_operand(
        &mut self,
        position: DisplayPosition<'_>,
        operand: &Spanned<Expr>,
        operand_ty: &ResolvedType,
    ) {
        let operand_ty = self.expand_type_aliases(operand_ty.clone());
        let Some(value) = self.unprintable_value(&operand_ty) else {
            return;
        };
        let name = match &operand.node {
            Expr::Ident(name) => Some(name.as_str()),
            _ => None,
        };
        let error = errors::value_has_no_printed_form(position, name, value, operand.span);
        self.errors.push(error);
    }

    /// Classify a value type with no printed form, or return `None` when every display position renders it.
    ///
    /// A union value, a generator, a function, `bytes`, and a model or class whose type defines no `__str__` have none.
    /// A model or class that adopts `Error` is left to the display rule for errors, and one that adopts `Display`
    /// supplies `__str__` by that trait's contract.
    fn unprintable_value<'ty>(&self, ty: &'ty ResolvedType) -> Option<UnprintableValue<'ty>> {
        match ty {
            _ if ty.is_union() => Some(UnprintableValue::Union),
            ResolvedType::Generic(name, _)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Generator) =>
            {
                Some(UnprintableValue::Generator)
            }
            ResolvedType::Function(..) => Some(UnprintableValue::Function),
            ResolvedType::Bytes => Some(UnprintableValue::Bytes),
            ResolvedType::Named(type_name) | ResolvedType::Generic(type_name, _)
                if self.nominal_has_no_printed_form(type_name) =>
            {
                Some(UnprintableValue::Nominal {
                    type_name: type_name.as_str(),
                })
            }
            _ => None,
        }
    }

    /// Whether `type_name` names a model or class whose values have no printed form.
    fn nominal_has_no_printed_form(&self, type_name: &str) -> bool {
        if !matches!(
            self.lookup_semantic_type_info(type_name),
            Some(TypeInfo::Model(_) | TypeInfo::Class(_))
        ) {
            return false;
        }
        let adopts = |trait_id| self.type_implements_trait(type_name, core_traits::as_str(trait_id));
        !adopts(TraitId::Error) && !adopts(TraitId::Display) && !self.nominal_defines_str(type_name, 0)
    }

    /// Whether a model or class defines `__str__`: declared on the type, supplied by an adopted trait or one of its
    /// supertraits, or inherited from a class it extends.
    fn nominal_defines_str(&self, type_name: &str, depth: usize) -> bool {
        let str_method = magic_methods::as_str(MagicMethodId::Str);
        let (methods, adoptions, extends) = match self.lookup_semantic_type_info(type_name) {
            Some(TypeInfo::Model(model)) => (&model.methods, &model.trait_adoptions, None),
            Some(TypeInfo::Class(class)) => (&class.methods, &class.trait_adoptions, class.extends.as_deref()),
            // A type the checker cannot see into is given the benefit of the doubt.
            _ => return true,
        };
        if methods.contains_key(str_method) {
            return true;
        }
        let trait_supplies_str = adoptions.iter().any(|adoption| {
            std::iter::once(adoption.name.clone())
                .chain(
                    self.semantic_supertrait_closure(&adoption.name)
                        .into_iter()
                        .map(|(name, _)| name),
                )
                .any(|trait_name| {
                    self.lookup_semantic_trait_info(&trait_name)
                        .is_some_and(|info| info.methods.contains_key(str_method))
                })
        });
        if trait_supplies_str {
            return true;
        }
        match extends {
            Some(parent) if depth < MAX_EXTENDS_DEPTH => self.nominal_defines_str(parent, depth + 1),
            Some(_) => true,
            None => false,
        }
    }
}
