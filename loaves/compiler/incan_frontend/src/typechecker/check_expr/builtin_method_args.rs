//! Argument checks for the builtin list, dict and set methods (frozen or not).
//!
//! The checker types these methods from the method registry rather than from a declared signature, so nothing else
//! checks their arguments: an extra argument, as in `counts.get(name, 0)`, was dropped from the generated call, a
//! missing one, as in `items.remove()`, reached it, and a keyword argument was taken for a positional one (#1783).

use crate::ast::{CallArg, Span};
use crate::diagnostics::errors;
use crate::symbols::ResolvedType;
use crate::typechecker::TypeChecker;
use crate::typechecker::helpers::collection_type_id;
use incan_lang::lang::surface::dict_methods::{self, DictMethodId};
use incan_lang::lang::surface::method_arity::builtin_collection_method_arity;
use incan_lang::lang::types::collections::{self, CollectionTypeId};

impl TypeChecker {
    /// Refuse a call of a builtin collection method whose arguments the method does not take.
    ///
    /// Every argument of these methods is positional, and their number is the arity
    /// [`builtin_collection_method_arity`] states. `dict.contains_key` is left to its own validation, which checks the
    /// probe's type as well. A receiver that is not a builtin collection, or a method the registry does not name, is
    /// not checked here.
    pub(in crate::typechecker::check_expr) fn check_builtin_collection_method_args(
        &mut self,
        receiver: &ResolvedType,
        method: &str,
        args: &[CallArg],
        span: Span,
    ) {
        let collection = match receiver {
            ResolvedType::Generic(name, _) => collection_type_id(name.as_str()),
            ResolvedType::FrozenList(_) => Some(CollectionTypeId::FrozenList),
            ResolvedType::FrozenSet(_) => Some(CollectionTypeId::FrozenSet),
            ResolvedType::FrozenDict(_, _) => Some(CollectionTypeId::FrozenDict),
            _ => None,
        };
        let Some(collection) = collection else {
            return;
        };
        if collection == CollectionTypeId::Dict && dict_methods::from_str(method) == Some(DictMethodId::ContainsKey) {
            return;
        }
        let Some(arity) = builtin_collection_method_arity(collection, method) else {
            return;
        };

        let callee = format!("{}.{method}", collections::as_str(collection));
        for arg in args {
            match arg {
                CallArg::Positional(_) => {}
                CallArg::Named(name, _) => {
                    self.errors
                        .push(errors::unknown_keyword_argument(&callee, &name.node, name.span));
                }
                CallArg::PositionalUnpack(expr) => {
                    self.errors
                        .push(errors::call_unpack_without_rest(&callee, "*", expr.span));
                }
                CallArg::KeywordUnpack(expr) => {
                    self.errors
                        .push(errors::call_unpack_without_rest(&callee, "**", expr.span));
                }
            }
        }
        if args.len() != arity {
            self.errors
                .push(errors::builtin_arity(&callee, arity, args.len(), span));
        }
    }
}
