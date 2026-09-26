//! Argument checks for the builtin list, dict and set methods (frozen or not).
//!
//! These methods are typed from the method registry rather than from a declared signature. Every argument is
//! positional, and their number is the arity `method_arity` states; a keyword or unpacked argument, or another count,
//! is refused (#1783).

use crate::ast::{CallArg, Span};
use crate::diagnostics::errors;
use crate::symbols::ResolvedType;
use crate::typechecker::TypeChecker;
use crate::typechecker::helpers::collection_type_id;
use incan_lang::lang::surface::method_arity::builtin_collection_method_arity;
use incan_lang::lang::types::collections::{self, CollectionTypeId};

impl TypeChecker {
    /// Refuse a call of a builtin collection method whose arguments the method does not take.
    ///
    /// Every argument of these methods is positional, and their number is the arity
    /// [`builtin_collection_method_arity`] states. A receiver that is not a builtin collection, or a method the
    /// registry does not name, is not checked here.
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
