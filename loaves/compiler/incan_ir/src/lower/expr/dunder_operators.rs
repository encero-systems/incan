//! A dunder call that has no Rust method to call lowers to the operation it defines (#1561).
//!
//! The backend implements `PartialEq` from each type's `__eq__` (the method is the `PartialEq::eq` slot), so no type
//! has a Rust method named `__eq__`: `a.__eq__(b)` is `a == b` for every receiver, a concrete value, `self` in an
//! expanded trait default and a type parameter alike. The `__ne__` default of the builtin `Eq` trait is
//! `not self.__eq__(other)`, so a call that reaches that default rather than a method of the receiver's own type is
//! `a != b`. The other comparison dunders and `__str__` are methods of a source type or trait, except through a type
//! parameter's builtin bound: `T with Ord`, `T with Eq` and `T with Display` lower to Rust's `Ord`, `PartialEq` and
//! `Display`, which name no dunder, so there `a.__lt__(b)` is `a < b` and `value.__str__()` is the value's display
//! text. An ordering dunder of the builtin `Ord` trait is also its operator on a type this compilation emits no
//! projection of that dunder for: one that derives `Ord`, whose Rust `PartialOrd` is derived, and an adopter declared
//! in another module, whose Rust `PartialOrd` calls the dunder its `Ord` impl has.

use super::super::super::TypedExpr;
use super::super::super::expr::{
    BinOp, FormatPart, FormatStyle, IrCallArg, IrCallArgKind, IrExprKind, IrMethodDispatch, UnaryOp,
};
use super::super::super::types::IrType;
use super::super::AstLowering;
use super::trait_declaration_name;
use incan_lang::lang::keywords::{self, KeywordId};
use incan_lang::lang::magic_methods::{self, ComparisonDunderId, MagicMethodId};
use incan_lang::lang::traits::{self as builtin_traits, TraitId};
use incan_lang::lang::{stdlib, trait_bounds};

impl AstLowering {
    /// Lower a dunder call to the operation it defines when no Rust method of its name exists to call, or return the
    /// receiver and arguments unchanged.
    ///
    /// `dispatch` is the checked dispatch of the call. `__ne__` is rewritten when its dispatch reaches the builtin
    /// `Eq` trait's default, which an `Ord` adopter that imports only `Ord` reaches without the trait in scope. The
    /// ordering dunders are rewritten for a receiver whose type is a type parameter and whose dispatch goes through a
    /// trait the builtin registry maps to a Rust trait, and for a dispatch through the builtin `Ord` trait that does
    /// not reach a local adopter's projection (a derived `Ord`, or an adopter from another module). `__str__` is
    /// rewritten only through a type parameter's bound.
    pub(in crate::lower) fn lower_dunder_as_operation(
        &self,
        method: &str,
        receiver: TypedExpr,
        dispatch: Option<&IrMethodDispatch>,
        args: Vec<IrCallArg>,
    ) -> Result<(IrExprKind, IrType), (TypedExpr, Vec<IrCallArg>)> {
        let through_builtin_bound = Self::receiver_is_type_parameter(&receiver.ty)
            && matches!(dispatch, Some(IrMethodDispatch::Trait(trait_dispatch))
                if trait_bounds::rust_to_incan(&trait_dispatch.trait_path).is_some());
        if magic_methods::from_str(method) == Some(MagicMethodId::Str) && args.is_empty() && through_builtin_bound {
            let text = IrExprKind::Format {
                parts: vec![FormatPart::Expr {
                    expr: receiver,
                    style: FormatStyle::Display,
                }],
            };
            return Ok((text, IrType::String));
        }
        let reaches_stdlib_trait = |trait_id: TraitId| {
            matches!(dispatch, Some(IrMethodDispatch::Trait(trait_dispatch))
                if builtin_traits::from_str(trait_declaration_name(trait_dispatch)) == Some(trait_id)
                    && trait_dispatch.trait_module_path.as_ref().and_then(|path| path.first()).map(String::as_str)
                        == Some(stdlib::STDLIB_ROOT))
        };
        let through_eq_default = reaches_stdlib_trait(TraitId::Eq);
        let ordered = through_builtin_bound
            || (reaches_stdlib_trait(TraitId::Ord)
                && !self.receiver_adopts_the_builtin_source_trait(&receiver, dispatch));
        let op = match magic_methods::comparison_from_str(method) {
            Some(ComparisonDunderId::Eq) => Some(BinOp::Eq),
            Some(ComparisonDunderId::Ne) if through_builtin_bound || through_eq_default => Some(BinOp::Ne),
            Some(ComparisonDunderId::Lt) if ordered => Some(BinOp::Lt),
            Some(ComparisonDunderId::Le) if ordered => Some(BinOp::Le),
            Some(ComparisonDunderId::Gt) if ordered => Some(BinOp::Gt),
            Some(ComparisonDunderId::Ge) if ordered => Some(BinOp::Ge),
            _ => None,
        };
        let single_positional =
            matches!(args.as_slice(), [arg] if arg.name.is_none() && matches!(arg.kind, IrCallArgKind::Positional));
        match op {
            Some(op) if single_positional => {
                let mut args = args;
                let Some(other) = args.pop() else {
                    return Err((receiver, args));
                };
                let comparison = IrExprKind::BinOp {
                    op,
                    left: Box::new(Self::dereferenced_self(receiver)),
                    right: Box::new(Self::dereferenced_self(other.expr)),
                };
                Ok((comparison, IrType::Bool))
            }
            _ => Err((receiver, args)),
        }
    }

    /// Return `operand` dereferenced when it is the method's `self`, which the backend always binds by reference, so a
    /// comparison with an owned value compares the values.
    fn dereferenced_self(operand: TypedExpr) -> TypedExpr {
        if !matches!(&operand.kind, IrExprKind::Var { name, .. } if name == keywords::as_str(KeywordId::SelfKw)) {
            return operand;
        }
        let ty = operand.ty.clone();
        TypedExpr::new(
            IrExprKind::UnaryOp {
                op: UnaryOp::Deref,
                operand: Box::new(operand),
            },
            ty,
        )
    }

    /// Whether a receiver's type is a type parameter, seen through references.
    fn receiver_is_type_parameter(ty: &IrType) -> bool {
        match ty {
            IrType::Ref(inner) | IrType::RefMut(inner) => Self::receiver_is_type_parameter(inner),
            IrType::Generic(_) => true,
            _ => false,
        }
    }
}
