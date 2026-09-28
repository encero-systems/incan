//! Small helper utilities for expression lowering: pow exponent classification, literal extraction, the no-argument
//! `count()` on a list, and the `flat_map` callback that expands into a list.

use super::super::super::expr::{BuiltinFn, IrCallArg, IrCallArgKind, IrExprKind, VarAccess, VarRefKind};
use super::super::super::types::IrType;
use super::super::AstLowering;
use crate::TypedExpr;
use incan_frontend::ast::{self, Spanned};
use incan_lang::PowExponentKind;
use incan_lang::lang::types::collections::CollectionTypeId;

impl AstLowering {
    /// Determine `PowExponentKind` for a power expression's right operand.
    ///
    /// Used to implement Python-like `**` semantics where `int ** int` yields `Int` only for non-negative int literal
    /// exponents; otherwise `Float`.
    pub(in crate::lower) fn pow_exponent_kind(right_ast: &Spanned<ast::Expr>, right_ty: &IrType) -> PowExponentKind {
        let rhs_is_float = matches!(right_ty, IrType::Float);
        let rhs_int_literal = Self::extract_int_literal(right_ast);
        PowExponentKind::from_literal_info(rhs_is_float, rhs_int_literal)
    }

    /// Extract an integer literal value from an AST expression.
    pub(in crate::lower) fn extract_int_literal(expr: &Spanned<ast::Expr>) -> Option<i64> {
        match &expr.node {
            ast::Expr::Literal(ast::Literal::Int(il)) => Some(il.value),
            ast::Expr::Unary(ast::UnaryOp::Neg, inner) => {
                if let ast::Expr::Literal(ast::Literal::Int(il)) = &inner.node {
                    Some(-il.value)
                } else {
                    None
                }
            }
            ast::Expr::Paren(inner) => Self::extract_int_literal(inner),
            _ => None,
        }
    }

    /// Lower a no-argument `count()` on a list to the list's length (#1776).
    ///
    /// On a list the argument count picks the form, as the checker resolved it: `count(value)` is the list method the
    /// collection classification already selects, and `count()` is the iterator terminal, which counts every item of
    /// the list. That count is the list's length, so the call lowers as `len(items)` does: without materializing an
    /// iterator, whose adapter would require the element type to be cloneable and would copy the list to count it.
    pub(in crate::lower) fn lower_list_item_count(receiver: TypedExpr) -> IrExprKind {
        IrExprKind::BuiltinCall {
            func: BuiltinFn::Len,
            args: vec![receiver],
        }
    }

    /// Adapt the callback of `iter.flat_map(f)` to the `list` expansion the flat-map adapter stores.
    ///
    /// RFC 088 types the callback `(T) -> Iterable[U]`, while the adapter holds one expansion at a time as a
    /// `list[U]`. A callback returning another iterable (a set, a frozen collection, a generator, an iterator) is
    /// called through `list(f(item))`, which yields the expansion's items in the order the expansion yields them.
    /// `flattened_ty` is the checked `Iterator[U]` the call produces, which names `U`. A callback that already returns
    /// a list, or whose type lowering cannot see, is returned unchanged.
    pub(in crate::lower) fn flat_map_list_callback(callback: TypedExpr, flattened_ty: &IrType) -> TypedExpr {
        let IrType::Function { params, ret } = &callback.ty else {
            return callback;
        };
        let ([param_ty], IrType::NamedGeneric(_, flattened_args)) = (params.as_slice(), flattened_ty) else {
            return callback;
        };
        let [item_ty] = flattened_args.as_slice() else {
            return callback;
        };
        if matches!(ret.as_ref(), IrType::List(_) | IrType::Unknown) {
            return callback;
        }
        const ITEM: &str = "__incan_flat_map_item";
        let (param_ty, expansion_ty, list_ty) = (
            param_ty.clone(),
            ret.as_ref().clone(),
            IrType::List(Box::new(item_ty.clone())),
        );
        let span = callback.span;
        let item = TypedExpr::new(
            IrExprKind::Var {
                name: ITEM.to_string(),
                access: VarAccess::Move,
                ref_kind: VarRefKind::Value,
            },
            param_ty.clone(),
        );
        let expansion = TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(callback),
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: item,
                }],
                callable_signature: None,
                canonical_path: None,
            },
            expansion_ty,
        );
        let listed = TypedExpr::new(
            IrExprKind::BuiltinCall {
                func: BuiltinFn::CollectionConstructor(CollectionTypeId::List),
                args: vec![expansion],
            },
            list_ty.clone(),
        );
        TypedExpr::new(
            IrExprKind::Closure {
                params: vec![(ITEM.to_string(), param_ty.clone())],
                body: Box::new(listed),
                captures: Vec::new(),
                annotate_param_types: false,
            },
            IrType::Function {
                params: vec![param_ty],
                ret: Box::new(list_ty),
            },
        )
        .with_span(span)
    }
}
