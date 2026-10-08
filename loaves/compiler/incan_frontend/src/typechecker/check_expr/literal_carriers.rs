//! Retain checker-settled intrinsic carrier types at source literal spans.
//!
//! Bindings and peer collection members may settle a constructor after its first check. Body IR reads each
//! expression's own type, so that proof must reach the constructor and its literal descendants before lowering.

use crate::ast::{CallArg, DictEntry, Expr, ListEntry, Literal, Spanned};
use crate::symbols::ResolvedType;
use crate::typechecker::helpers::collection_type_id;
use incan_lang::lang::surface::constructors::{self, ConstructorId};
use incan_lang::lang::types::collections::CollectionTypeId;

use super::{TypeChecker, fill_open_result_parts};

impl TypeChecker {
    /// Retain only compatible open Option/Result parts of checked source literals, using the type already settled
    /// by their binding or peers. Concrete leaves, arbitrary calls, identifiers and spreads remain untouched.
    pub(in crate::typechecker) fn retain_literal_carrier_types(
        &mut self,
        value: &Spanned<Expr>,
        settled: &ResolvedType,
    ) {
        let constructor = self.literal_carrier_constructor(value);
        if constructor.is_none()
            && !matches!(
                value.node,
                Expr::List(_)
                    | Expr::Dict(_)
                    | Expr::Set(_)
                    | Expr::Tuple(_)
                    | Expr::Paren(_)
                    | Expr::Literal(Literal::None)
            )
        {
            return;
        }
        let Some(mut checked) = self.type_info.expr_type(value.span).cloned() else {
            return;
        };
        if !self.types_compatible(&checked, settled) {
            return;
        }
        fill_open_result_parts(&mut checked, settled, false);
        if self.type_info.expr_type(value.span) != Some(&checked) {
            self.record_expr_type(value.span, checked.clone());
        }

        // Each child takes the corresponding part of this literal's checked storage type, never a spelling-based
        // reconstruction or the type of an unrelated use of the value.
        match (&value.node, &checked) {
            (Expr::Paren(inner), _) => self.retain_literal_carrier_types(inner, &checked),
            (Expr::List(entries), ResolvedType::Generic(name, args))
                if collection_type_id(name) == Some(CollectionTypeId::List) && args.len() == 1 =>
            {
                for entry in entries {
                    if let ListEntry::Element(child) = entry {
                        self.retain_literal_carrier_types(child, &args[0]);
                    }
                }
            }
            (Expr::Set(items), ResolvedType::Generic(name, args))
                if collection_type_id(name) == Some(CollectionTypeId::Set) && args.len() == 1 =>
            {
                for child in items {
                    self.retain_literal_carrier_types(child, &args[0]);
                }
            }
            (Expr::Dict(entries), ResolvedType::Generic(name, args))
                if collection_type_id(name) == Some(CollectionTypeId::Dict) && args.len() == 2 =>
            {
                for entry in entries {
                    if let DictEntry::Pair(key, child) = entry {
                        self.retain_literal_carrier_types(key, &args[0]);
                        self.retain_literal_carrier_types(child, &args[1]);
                    }
                }
            }
            (Expr::Tuple(items), ResolvedType::Tuple(types)) if items.len() == types.len() => {
                for (child, ty) in items.iter().zip(types) {
                    self.retain_literal_carrier_types(child, ty);
                }
            }
            (Expr::Call(_, _, args) | Expr::Constructor(_, args), _) => {
                let payload = match constructor {
                    Some(ConstructorId::Some) => checked.option_inner_type(),
                    Some(ConstructorId::Ok) => checked.result_ok_type(),
                    Some(ConstructorId::Err) => checked.result_err_type(),
                    _ => None,
                };
                if let (Some(payload), [CallArg::Positional(child)]) = (payload, args.as_slice()) {
                    self.retain_literal_carrier_types(child, payload);
                }
            }
            _ => {}
        }
    }

    /// Recognize only an unshadowed intrinsic constructor with one ordinary payload argument. A same-spelled user
    /// binding and a generic or unpacked call provide no constructor witness for retaining open payload parts.
    fn literal_carrier_constructor(&self, value: &Spanned<Expr>) -> Option<ConstructorId> {
        let (name, args) = match &value.node {
            Expr::Call(callee, type_args, args) if type_args.is_empty() => {
                let Expr::Ident(name) = &callee.node else {
                    return None;
                };
                (name, args)
            }
            Expr::Constructor(name, args) => (name, args),
            _ => return None,
        };
        if !matches!(args.as_slice(), [CallArg::Positional(_)]) || self.has_non_builtin_call_root_binding(name) {
            return None;
        }
        match constructors::from_str(name)? {
            constructor @ (ConstructorId::Some | ConstructorId::Ok | ConstructorId::Err) => Some(constructor),
            _ => None,
        }
    }
}
