//! The `mut` marker on callable-type parameters (#1790): which `def` parameters carry it, and where it cannot be
//! written.
//!
//! `mut` on a parameter lets the function change the value. Changes to a collection, model or class object reach the
//! caller, so such a parameter is marked in the function's type, `(mut T, ...) -> R`, and function types agree on the
//! marker exactly. An `int`, `float` or `bool` parameter is the function's own copy: its changes stay local, it is not
//! marked, and a function type cannot mark one. A parameter of an imported Rust type is handed over whole and is not
//! marked either.

use crate::ast::{Param, ParamKind, Spanned, Type};
use crate::diagnostics::errors;
use crate::symbols::{ResolvedType, SymbolKind};

use super::TypeChecker;

/// Return whether a parameter of this type is the function's own copy of the argument, so that `mut` changes stay in
/// the function.
fn is_copied_scalar(ty: &ResolvedType) -> bool {
    matches!(ty, ResolvedType::Int | ResolvedType::Float | ResolvedType::Bool)
}

/// Return whether a `def` parameter is marked `mut` in the function's type, judged from the parameter and its resolved
/// type alone.
///
/// Only an ordinary parameter declared `mut` whose type is neither a copied scalar nor a Rust path is marked. The
/// stdlib loader, which has no symbol table for the module's Rust imports, uses this rule as it is;
/// [`TypeChecker::def_param_shows_changes_to_caller`] adds the Rust-import check.
pub(crate) fn def_param_is_marked(param: &Param, resolved: &ResolvedType) -> bool {
    param.is_mut
        && param.kind == ParamKind::Normal
        && !is_copied_scalar(resolved)
        && !matches!(resolved, ResolvedType::RustPath(_))
}

impl TypeChecker {
    /// Return whether a `def` parameter shows its changes to the caller, which is what the `mut` marker on a parameter
    /// of a callable type means.
    ///
    /// This is [`def_param_is_marked`] plus one check the symbol table allows: a parameter whose type names an
    /// imported Rust item is handed over whole, so it is not marked.
    pub(super) fn def_param_shows_changes_to_caller(&self, param: &Param, resolved: &ResolvedType) -> bool {
        if !def_param_is_marked(param, resolved) {
            return false;
        }
        let head = match &param.ty.node {
            Type::Simple(name) | Type::ConstrainedPrimitive(name, _) | Type::Generic(name, _) => Some(name.as_str()),
            Type::Qualified(segments) => segments.first().map(String::as_str),
            _ => None,
        };
        !head.is_some_and(|name| {
            self.lookup_symbol(name)
                .is_some_and(|symbol| matches!(symbol.kind, SymbolKind::RustItem(_)))
        })
    }

    /// Refuse the `mut` marker on an `int`, `float` or `bool` parameter of every function type written in `ty`.
    ///
    /// No `def` has such a type, since those parameters are the function's own copy, so the spelling can only
    /// describe a callable that changes a value its caller never sees. Each marker is reported once, however many
    /// times its annotation is resolved.
    pub(super) fn refuse_marked_copied_scalars(&mut self, ty: &Spanned<Type>) {
        match &ty.node {
            Type::Function(params, ret) => {
                for param in params {
                    if let Type::MutParam(inner) = &param.node {
                        let resolved =
                            self.expand_type_aliases(crate::symbols::resolve_type(&inner.node, &self.symbols));
                        if is_copied_scalar(&resolved)
                            && self
                                .receiver_plan_inputs
                                .reported_marked_scalars
                                .insert((param.span.start, param.span.end))
                        {
                            self.errors
                                .push(errors::mut_marker_on_copied_scalar(&inner.node.to_string(), param.span));
                        }
                    }
                    self.refuse_marked_copied_scalars(param);
                }
                self.refuse_marked_copied_scalars(ret);
            }
            Type::Generic(_, args) | Type::DottedGeneric(_, args) | Type::Tuple(args) => {
                for arg in args {
                    self.refuse_marked_copied_scalars(arg);
                }
            }
            Type::Ref(inner) | Type::RefMut(inner) | Type::MutParam(inner) => self.refuse_marked_copied_scalars(inner),
            _ => {}
        }
    }
}
