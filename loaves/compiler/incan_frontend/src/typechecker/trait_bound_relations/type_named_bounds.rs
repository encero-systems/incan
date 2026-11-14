//! Bounds that name a type (#1033): the declaration such a bound records, and the one type that satisfies it.
//!
//! A bound is written in its callable's module and read wherever the callable is called. The names in scope there may
//! differ: the caller may not import an alias the bound spells, may import the bound's type under another name, or
//! may declare a type of the same name. The declaring module therefore records which type the bound names, and every
//! caller compares its type argument with that record instead of resolving the bound's spelling again.

use super::*;
use crate::symbols::{module_qualified_nominal_name, split_module_qualified_nominal_name};

impl TypeChecker {
    /// The `source_name` and `module_path` a bound that names a type records, read while its declaring module's names
    /// are in scope.
    ///
    /// Aliases are expanded first, so a bound on an alias records the alias's target. A model, class, enum or newtype
    /// is spelled by its declaring module ([`module_qualified_nominal_name`]) and carries that module's path. A
    /// builtin scalar is spelled by its canonical name and carries no path. `None` for a trait bound and for a type
    /// with neither form, such as an alias of a tuple, which keeps the spelling its callers resolve.
    pub(in crate::typechecker) fn type_bound_declaration(&self, bound: &str) -> Option<(String, Option<Vec<String>>)> {
        match self.bound_names_a_type(bound)? {
            ResolvedType::Named(name) => {
                let qualified = split_module_qualified_nominal_name(&name).map(|(module_path, _)| module_path);
                if let Some(module_path) = qualified {
                    return Some((name, Some(module_path)));
                }
                let (module_path, declaration_name) = self.nominal_declaration_in_scope(&name)?;
                Some((
                    module_qualified_nominal_name(&module_path, &declaration_name),
                    Some(module_path),
                ))
            }
            resolved => {
                let canonical = resolved.to_string();
                (builtin_scalar(&canonical).as_ref() == Some(&resolved)).then_some((canonical, None))
            }
        }
    }

    /// The type a checked bound names, as its declaring module recorded it with [`Self::type_bound_declaration`].
    ///
    /// `None` for a trait bound, and for a type bound whose declaring module recorded no declaration.
    pub(in crate::typechecker) fn declared_bound_type(bound: &TypeBoundInfo) -> Option<ResolvedType> {
        if bound.inferred || !bound.type_args.is_empty() {
            return None;
        }
        let source_name = bound.source_name.as_deref()?;
        if split_module_qualified_nominal_name(source_name).is_some() {
            return Some(ResolvedType::Named(source_name.to_string()));
        }
        if bound.module_path.is_some() || builtin_traits::from_str(&bound.name).is_some() {
            return None;
        }
        builtin_scalar(source_name)
    }

    /// What a checked bound names, for the remedy a violation offers: its recorded type, or whatever its spelling
    /// resolves to where the callable is called.
    pub(in crate::typechecker) fn checked_bound_target(&self, bound: &TypeBoundInfo) -> GenericBoundTarget {
        match Self::declared_bound_type(bound) {
            Some(_) => GenericBoundTarget::Type,
            None => self.generic_bound_target(&bound.name),
        }
    }

    /// Whether `ty` is the one type a bound names.
    ///
    /// The argument's aliases are expanded, as the bound's were. A nominal bound is met by the name this scope binds
    /// to the bound's declaration, under any import spelling, and by no other type of that name.
    pub(in crate::typechecker) fn type_is_bound_type(&self, ty: &ResolvedType, bound_type: &ResolvedType) -> bool {
        let actual = self.expand_type_aliases(ty.clone());
        if actual == *bound_type {
            return true;
        }
        let (ResolvedType::Named(actual_name), ResolvedType::Named(bound_name)) = (&actual, bound_type) else {
            return false;
        };
        let Some((module_path, declaration_name)) = split_module_qualified_nominal_name(bound_name) else {
            return false;
        };
        match split_module_qualified_nominal_name(actual_name) {
            Some(actual_declaration) => actual_declaration == (module_path, declaration_name),
            None => self.bare_name_names_declaration(actual_name, &module_path, declaration_name),
        }
    }
}

/// The builtin scalar a canonical spelling names. A collection base or an ownership mode is a named type, which a
/// bound records by its declaration instead.
fn builtin_scalar(name: &str) -> Option<ResolvedType> {
    ResolvedType::builtin_simple(name).filter(|resolved| !matches!(resolved, ResolvedType::Named(_)))
}

/// Whether two overloads are keyed on the same types: at least one type parameter is bound to a type, and position by
/// position their type parameters carry the same bounds, a type bound compared by the type it names.
///
/// Parameter names and defaults describe call syntax rather than dispatch identity, and return types take no part in
/// overload selection, so only parameter types, kinds and `mut` markers are compared beside the bounds.
pub(in crate::typechecker) fn type_keyed_overload_signatures_identical(
    left: &crate::symbols::FunctionInfo,
    right: &crate::symbols::FunctionInfo,
) -> bool {
    let bounds = |info: &crate::symbols::FunctionInfo| -> Vec<Vec<BoundKey>> {
        info.type_params
            .iter()
            .map(|type_param| {
                info.type_param_bound_details
                    .get(type_param)
                    .map(|bounds| bounds.iter().map(BoundKey::of).collect())
                    .unwrap_or_default()
            })
            .collect()
    };
    let left_bounds = bounds(left);
    left_bounds
        .iter()
        .flatten()
        .any(|bound| matches!(bound, BoundKey::Type(_)))
        && left_bounds == bounds(right)
        && left.params.len() == right.params.len()
        && left
            .params
            .iter()
            .zip(&right.params)
            .all(|(left, right)| left.ty == right.ty && left.kind == right.kind && left.is_mut == right.is_mut)
}

/// One bound of a type parameter as overload identity reads it.
#[derive(PartialEq)]
enum BoundKey {
    /// A bound that names a type, by that type.
    Type(ResolvedType),
    /// Any other bound, by its checked record.
    Trait(TypeBoundInfo),
}

impl BoundKey {
    /// Read one checked bound.
    fn of(bound: &TypeBoundInfo) -> Self {
        match TypeChecker::declared_bound_type(bound) {
            Some(named) => Self::Type(named),
            None => Self::Trait(bound.clone()),
        }
    }
}
