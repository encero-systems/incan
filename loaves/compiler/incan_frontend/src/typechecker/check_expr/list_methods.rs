//! Choose between a builtin list method and the RFC 088 iterator method of the same name by argument count (#1776).
//!
//! `count` is both a list method (`items.count(value)`, how many items equal `value`) and an iterator terminal
//! (`items.count()`, how many items there are). Both are valid on a list, and the argument count picks one: no argument
//! is the terminal, one value is the list method. `index(value)` has only the list form. A call whose argument count
//! fits no form is refused with a diagnostic naming the forms that exist. This is resolution between two builtin
//! surfaces, not overloading of user-declared methods.

use crate::ast::{CallArg, Span};
use crate::diagnostics::errors;
use crate::symbols::ResolvedType;
use crate::typechecker::helpers::collection_type_id;
use incan_lang::lang::surface::{iterator_methods, list_methods};
use incan_lang::lang::types::collections::CollectionTypeId;

use super::TypeChecker;

impl TypeChecker {
    /// Resolve a list method whose name an iterator method may share, choosing the form by argument count.
    ///
    /// Returns `None` for every receiver other than a list and every method other than `count` and `index`; the
    /// ordinary method resolution handles those. For `count` and `index` on a list:
    ///
    /// - no argument, when an iterator terminal of that name exists: the terminal, typed by the iterator surface and
    ///   without consuming the list;
    /// - one value argument: the list method, whose value must be compatible with the list's element type;
    /// - anything else: refused with [`errors::list_method_argument_count`], and typed as the list method's `int` so
    ///   checking continues.
    pub(super) fn resolve_list_method_by_argument_count(
        &mut self,
        base_ty: &ResolvedType,
        method: &str,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
    ) -> Option<ResolvedType> {
        let ResolvedType::Generic(name, type_args) = base_ty else {
            return None;
        };
        if collection_type_id(name.as_str()) != Some(CollectionTypeId::List) {
            return None;
        }
        let value_arity = match list_methods::from_str(method)? {
            list_methods::ListMethodId::Count | list_methods::ListMethodId::Index => 1,
            _ => return None,
        };
        let iterator_form = iterator_methods::from_str(method).is_some_and(iterator_methods::is_terminal);

        if args.is_empty() && iterator_form {
            return self.resolve_iterator_protocol_method_call(base_ty, method, args, arg_types, span);
        }
        if args.len() == value_arity {
            let element_ty = type_args.first().cloned().unwrap_or(ResolvedType::Unknown);
            if let Some(value_ty) = arg_types.first()
                && !self.types_compatible(value_ty, &element_ty)
            {
                self.errors.push(errors::type_mismatch(
                    &element_ty.to_string(),
                    &value_ty.to_string(),
                    span,
                ));
            }
            return Some(ResolvedType::Int);
        }
        self.errors.push(errors::list_method_argument_count(
            method,
            iterator_form,
            args.len(),
            span,
        ));
        Some(ResolvedType::Int)
    }
}
