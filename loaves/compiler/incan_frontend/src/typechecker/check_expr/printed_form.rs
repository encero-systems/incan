//! One display rule for `print`/`println` arguments, `str(value)`, f-string `{value}` parts and `Display` bounds
//! (#1748).
//!
//! The display positions show a value alike. Scalars and `str` display their own text; a tuple, list, dict, set,
//! `Option` or `Result` displays its structure (lowering hands `print` and `str` the same rendering an f-string part
//! lowers to); a model, class, enum or newtype displays through `Display`, which it provides by a `__str__` method, by
//! its variants' values when it is an enum that declares them, or by `message()` when it adopts `Error`.
//! `@derive(Display)` provides nothing. A value with none of those has no printed form, so the checker refuses it in
//! every position with `INCAN-T0103` instead of leaving the build to fail on it. A `Display` bound asks the same of a
//! type argument, except that a structural value has no `Display` of its own.

use incan_lang::lang::magic_methods::{self, MagicMethodId};
use incan_lang::lang::traits::{self as core_traits, TraitId};
use incan_lang::lang::types::collections::CollectionTypeId;

use super::TypeChecker;
use crate::ast::{Expr, Span, Spanned};
use crate::diagnostics::CompileError;
use crate::diagnostics::errors::{self, DisplayPosition, UnprintableValue};
use crate::symbols::{ResolvedType, SymbolKind, TypeBoundInfo, TypeInfo};
use crate::typechecker::helpers::{collection_type_id, is_frozen_bytes};

/// How deep a class's `extends` chain is followed when looking for an inherited `__str__`.
const MAX_EXTENDS_DEPTH: usize = 64;

impl TypeChecker {
    /// Refuse one displayed operand whose checked type has no printed form.
    ///
    /// `position` is where the source displays it and words the message; `operand` names the value in the message
    /// when the source spells it as a plain name or as `self`. Every value with a printed form, and every type the
    /// checker cannot classify (a type parameter, a Rust type, an unknown), is left alone.
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
            Expr::SelfExpr => Some("self"),
            _ => None,
        };
        let error = errors::value_has_no_printed_form(position, name, value, operand.span);
        self.errors.push(error);
    }

    /// Refuse a type argument bound to a `Display` bound when the display rule gives its values no printed form, or
    /// return `None` when the bound's ordinary refusal applies.
    ///
    /// A structural value (a tuple, list, dict, set, `Option` or `Result`) prints in the display positions but has no
    /// `Display` of its own, so it falls to the ordinary refusal.
    pub(in crate::typechecker) fn display_bound_refusal(
        &self,
        callee: &str,
        type_param: &str,
        type_arg: &ResolvedType,
        span: Span,
    ) -> Option<CompileError> {
        let type_arg = self.expand_type_aliases(type_arg.clone());
        let value = self.unprintable_value(&type_arg)?;
        Some(errors::value_has_no_printed_form(
            DisplayPosition::Bound { callee, type_param },
            None,
            value,
            span,
        ))
    }

    /// Whether a type satisfies a `Display` bound under the display rule, or `None` when the rule leaves the answer to
    /// the bound's ordinary check.
    ///
    /// The rule answers for `bytes` and `FrozenBytes` (#1838: the two share one display contract), which have no
    /// printed form, and for a model, class, enum or newtype, which provides `Display` exactly as
    /// [`Self::nominal_provides_display`] states.
    pub(in crate::typechecker) fn display_bound_satisfied(&self, ty: &ResolvedType) -> Option<bool> {
        match ty {
            ResolvedType::Bytes => Some(false),
            _ if is_frozen_bytes(ty) => Some(false),
            ResolvedType::Named(type_name) | ResolvedType::Generic(type_name, _) => {
                self.nominal_provides_display(type_name)
            }
            _ => None,
        }
    }

    /// Classify a value type with no printed form, or return `None` when every display position renders it.
    ///
    /// A union value, a generator, a function, `bytes` or `FrozenBytes` (#1838: the two share one display contract),
    /// and a model, class, enum or newtype that provides no `Display` have none.
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
            _ if is_frozen_bytes(ty) => Some(UnprintableValue::Bytes),
            ResolvedType::Named(type_name) | ResolvedType::Generic(type_name, _)
                if self.nominal_provides_display(type_name) == Some(false) =>
            {
                Some(UnprintableValue::Nominal {
                    type_name: type_name.as_str(),
                })
            }
            _ => None,
        }
    }

    /// Whether the values of a model, class, enum or newtype provide `Display`, or `None` when `type_name` names none
    /// of those (a newtype over a Rust type included).
    ///
    /// A type provides `Display` by a `__str__(self) -> str` method, by each variant's value when it is an enum that
    /// declares values, or by `message()` when it adopts `Error` and has no `__str__`. `@derive(Display)` provides
    /// nothing.
    fn nominal_provides_display(&self, type_name: &str) -> Option<bool> {
        let own_display = self.nominal_own_display(type_name)?;
        Some(own_display || self.type_implements_trait(type_name, core_traits::as_str(TraitId::Error)))
    }

    /// Whether a model, class, enum or newtype has a `Display` of its own, which an `Error` adopter displays instead of
    /// its `message()`, or `None` when `type_name` names none of those (a newtype over a Rust type included).
    ///
    /// Its own `Display` is a `__str__` (see [`Self::nominal_defines_str`]), the values of an enum that declares them,
    /// or a `Display` the type adopts or takes from a Rust derive macro (see [`Self::nominal_adopts_display`]).
    /// `@derive(Display)` provides nothing.
    pub(in crate::typechecker::check_expr) fn nominal_own_display(&self, type_name: &str) -> Option<bool> {
        let (declares_values, adoptions, derives) = match self.lookup_semantic_type_info(type_name)? {
            TypeInfo::Model(info) => (false, &info.trait_adoptions, &info.derives),
            TypeInfo::Class(info) => (false, &info.trait_adoptions, &info.derives),
            TypeInfo::Enum(info) => (info.value_enum.is_some(), &info.trait_adoptions, &info.derives),
            TypeInfo::Newtype(info) if !info.is_rusttype => (false, &info.trait_adoptions, &info.derives),
            _ => return None,
        };
        Some(
            declares_values
                || self.nominal_defines_str(type_name, 0)
                || self.nominal_adopts_display(type_name, adoptions, derives),
        )
    }

    /// Whether a model, class, enum or newtype takes `Display` by a `with` adoption or from a Rust derive macro.
    ///
    /// A `with Display` adoption, direct or through a supertrait, provides it: the language's `Display` by the
    /// `__str__` it requires, Rust's `std::fmt::Display` by the `fmt` the type implements or forwards. So does a Rust
    /// derive macro named `Display`, forwarded with `@rust.derive(...)` or imported from a Rust crate and named in
    /// `@derive(...)`. The compiler's own `@derive(Display)` is neither, and provides nothing.
    fn nominal_adopts_display(&self, type_name: &str, adoptions: &[TypeBoundInfo], derives: &[String]) -> bool {
        let display = core_traits::as_str(TraitId::Display);
        let adopted = adoptions.iter().any(|adoption| {
            self.trait_name_matches(&adoption.name, display)
                || adoption
                    .source_name
                    .as_deref()
                    .is_some_and(|source_name| self.trait_name_matches(source_name, display))
                || self
                    .semantic_supertrait_closure(&adoption.name)
                    .iter()
                    .any(|(name, _)| self.trait_name_matches(name, display))
        });
        let names_display = |path: &str| path.rsplit("::").next() == Some(display);
        let imported_rust_derive = derives.iter().any(|derive| {
            self.lookup_symbol(derive).is_some_and(
                |symbol| matches!(&symbol.kind, SymbolKind::RustItem(info) if names_display(info.path.as_str())),
            )
        });
        let forwarded_rust_derive = self
            .local_rust_derive_paths
            .get(type_name)
            .is_some_and(|paths| paths.iter().any(|path| names_display(path)));
        adopted || imported_rust_derive || forwarded_rust_derive
    }

    /// Whether a model, class, enum or newtype defines `__str__`: declared on the type, supplied by an adopted trait or
    /// one of its supertraits, or inherited from a class it extends.
    fn nominal_defines_str(&self, type_name: &str, depth: usize) -> bool {
        let str_method = magic_methods::as_str(MagicMethodId::Str);
        let (methods, adoptions, extends) = match self.lookup_semantic_type_info(type_name) {
            Some(TypeInfo::Model(model)) => (&model.methods, &model.trait_adoptions, None),
            Some(TypeInfo::Class(class)) => (&class.methods, &class.trait_adoptions, class.extends.as_deref()),
            Some(TypeInfo::Enum(info)) => (&info.methods, &info.trait_adoptions, None),
            Some(TypeInfo::Newtype(info)) => (&info.methods, &info.trait_adoptions, None),
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
