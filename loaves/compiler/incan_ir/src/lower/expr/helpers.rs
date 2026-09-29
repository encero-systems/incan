//! Small helper utilities for expression lowering: pow exponent classification, literal extraction, the no-argument
//! `count()` on a list, the `flat_map` callback that expands into a nested iterator, and the `Ok(...)` or `Err(...)`
//! a `Result` combinator's closure returns.

use super::super::super::expr::{
    BuiltinFn, IrCallArg, IrCallArgKind, IrExprKind, IrGeneratorClause, IteratorMethodKind, MethodKind, Pattern,
    VarAccess, VarRefKind,
};
use super::super::super::types::IrType;
use super::super::AstLowering;
use super::frozen_reads::owned_frozen_iteration_source;
use crate::TypedExpr;
use incan_frontend::ast::{self, Spanned};
use incan_lang::PowExponentKind;
use incan_lang::lang::surface::constructors::{self, ConstructorId};
use incan_lang::lang::surface::result_methods::ResultMethodId;
use incan_lang::lang::traits::{self as builtin_traits, TraitId};
use incan_lang::lang::types::collections::{self as collection_types, CollectionTypeId};

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

    /// Adapt the callback of `iter.flat_map(f)` to the nested iterator the flat-map adapter polls.
    ///
    /// RFC 088 types the callback `(T) -> Iterable[U]`, while the adapter keeps the expansion of the current source
    /// item as an `Iterator[U]` and draws one item from it per request, so no expansion is collected ahead of the
    /// consumer: a generator runs no further than the items taken from it, and an unbounded expansion still yields. A
    /// callback returning a generator or an iterator hands over that iterator and is returned unchanged. Every other
    /// callback returns its expansion through [`Self::flat_map_nested_iterator`]: a closure literal has its body
    /// wrapped, so Rust still infers its parameter types from the adapter's callback slot, and any other callable is
    /// called from a closure that wraps the result. `flattened_ty` is the checked `Iterator[U]` the call produces,
    /// which names `U`. A callback whose type lowering cannot see is returned unchanged.
    pub(in crate::lower) fn flat_map_iterator_callback(&self, callback: TypedExpr, flattened_ty: &IrType) -> TypedExpr {
        let IrType::Function { params, ret } = &callback.ty else {
            return callback;
        };
        let ([param_ty], IrType::NamedGeneric(_, flattened_args)) = (params.as_slice(), flattened_ty) else {
            return callback;
        };
        let [item_ty] = flattened_args.as_slice() else {
            return callback;
        };
        let returns_generator = matches!(ret.as_ref(), IrType::NamedGeneric(name, _)
            if collection_types::from_str(name) == Some(CollectionTypeId::Generator));
        if returns_generator || ret.is_iterator_protocol() || self.receiver_adopts_iterator_protocol(ret) {
            return callback;
        }
        let (param_ty, expansion_ty, item_ty) = (param_ty.clone(), ret.as_ref().clone(), item_ty.clone());
        let span = callback.span;
        let (params, expansion, captures, annotate_param_types) = match callback.kind {
            IrExprKind::Closure {
                params,
                body,
                captures,
                annotate_param_types,
            } => (params, *body, captures, annotate_param_types),
            kind => {
                const ITEM: &str = "__incan_flat_map_item";
                let callback = TypedExpr { kind, ..callback };
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
                (vec![(ITEM.to_string(), param_ty.clone())], expansion, Vec::new(), false)
            }
        };
        let nested = Self::flat_map_nested_iterator(expansion, item_ty);
        let nested_ty = nested.ty.clone();
        TypedExpr::new(
            IrExprKind::Closure {
                params,
                body: Box::new(nested),
                captures,
                annotate_param_types,
            },
            IrType::Function {
                params: vec![param_ty],
                ret: Box::new(nested_ty),
            },
        )
        .with_span(span)
    }

    /// Turn one `flat_map` expansion into the `Iterator[U]` the flat-map adapter polls one item at a time.
    ///
    /// A list is iterated through `expansion.iter()`, over the list the callback returned, and a frozen collection
    /// through `list(expansion).iter()`, the owned items the checker typed (see [`owned_frozen_iteration_source`]).
    /// Any other iterable, such as a set or a value whose type lowering cannot see, becomes the generator expression
    /// `(value for value in expansion)`, which draws from the expansion only as it is polled.
    fn flat_map_nested_iterator(expansion: TypedExpr, item_ty: IrType) -> TypedExpr {
        let expansion = owned_frozen_iteration_source(expansion);
        if matches!(expansion.ty, IrType::List(_)) {
            return TypedExpr::new(
                IrExprKind::KnownMethodCall {
                    receiver: Box::new(expansion),
                    kind: MethodKind::Iterator(IteratorMethodKind::Iter),
                    args: Vec::new(),
                },
                IrType::NamedGeneric(builtin_traits::as_str(TraitId::Iterator).to_string(), vec![item_ty]),
            );
        }
        const VALUE: &str = "__incan_flat_map_value";
        let value = TypedExpr::new(
            IrExprKind::Var {
                name: VALUE.to_string(),
                access: VarAccess::Move,
                ref_kind: VarRefKind::Value,
            },
            item_ty.clone(),
        );
        TypedExpr::new(
            IrExprKind::Generator {
                element: Box::new(value),
                clauses: vec![IrGeneratorClause::For {
                    pattern: Pattern::Var(VALUE.to_string()),
                    iterable: Box::new(expansion),
                }],
            },
            IrType::NamedGeneric(
                collection_types::as_str(CollectionTypeId::Generator).to_string(),
                vec![item_ty],
            ),
        )
    }

    /// Spell each `Ok(...)` or `Err(...)` that the closure `callback` of the `Result` combinator `method` returns with
    /// both of its type arguments, when the closure's checked result leaves a side of it open (#1561).
    ///
    /// Rust types a closure's result from its body alone, so the `Ok(1)` of `e.or_else((m) => Ok(1))` names no error
    /// type and the call fails to build (E0282). The closure's checked result gives each side the checker fixed, from
    /// the call's context included; the receiver fixes `or_else`'s success type and `and_then`'s error type; a side
    /// nothing fixes is `None` (`()`), the type an open side of a bound `Ok(...)` or `Err(...)` takes. A closure whose
    /// result is known, and one that returns anything else, is left as it is.
    pub(in crate::lower) fn pin_result_combinator_callback(
        callback: &mut TypedExpr,
        method: ResultMethodId,
        receiver_ty: &IrType,
    ) {
        match &mut callback.kind {
            IrExprKind::Block { value: Some(value), .. } => {
                Self::pin_result_combinator_callback(value, method, receiver_ty);
                callback.ty = value.ty.clone();
            }
            IrExprKind::Closure { body, .. } => {
                let IrType::Function { ret, .. } = &mut callback.ty else {
                    return;
                };
                let IrType::Result(ok, err) = ret.as_mut() else {
                    return;
                };
                if !matches!(**ok, IrType::Unknown) && !matches!(**err, IrType::Unknown) {
                    return;
                }
                let mut receiver_ty = receiver_ty;
                while let IrType::Ref(inner) | IrType::RefMut(inner) = receiver_ty {
                    receiver_ty = inner.as_ref();
                }
                if let IrType::Result(receiver_ok, receiver_err) = receiver_ty {
                    if method == ResultMethodId::OrElse && matches!(**ok, IrType::Unknown) {
                        *ok = receiver_ok.clone();
                    }
                    if method == ResultMethodId::AndThen && matches!(**err, IrType::Unknown) {
                        *err = receiver_err.clone();
                    }
                }
                if let Some(pinned) = Self::pin_returned_result_constructor(body, &**ok, &**err) {
                    **ret = pinned;
                }
            }
            _ => {}
        }
    }

    /// Give the `Ok(...)` or `Err(...)` that `expr` produces, directly, as a block's value or as both branches of an
    /// `if`, the type arguments `ok` and `err`, with an open side other than the constructor's own payload as `None`,
    /// and return the result type it then has. `None` when `expr` produces no such constructor call.
    fn pin_returned_result_constructor(expr: &mut TypedExpr, ok: &IrType, err: &IrType) -> Option<IrType> {
        let unit_when_open = |ty: &IrType| match ty {
            IrType::Unknown => IrType::Unit,
            ty => ty.clone(),
        };
        let pinned = match &mut expr.kind {
            IrExprKind::Call {
                func,
                type_args,
                args,
                canonical_path: None,
                ..
            } if type_args.is_empty() && args.len() == 1 => {
                let IrExprKind::Var { name, .. } = &func.kind else {
                    return None;
                };
                let (ok, err) = match constructors::from_str(name)? {
                    ConstructorId::Ok => (ok.clone(), unit_when_open(err)),
                    ConstructorId::Err => (unit_when_open(ok), err.clone()),
                    _ => return None,
                };
                *type_args = vec![ok.clone(), err.clone()];
                IrType::Result(Box::new(ok), Box::new(err))
            }
            IrExprKind::Block { value: Some(value), .. } => Self::pin_returned_result_constructor(value, ok, err)?,
            IrExprKind::If {
                then_branch,
                else_branch: Some(else_branch),
                ..
            } => {
                let then_ty = Self::pin_returned_result_constructor(then_branch, ok, err);
                let else_ty = Self::pin_returned_result_constructor(else_branch, ok, err);
                then_ty.or(else_ty)?
            }
            _ => return None,
        };
        expr.ty = pinned.clone();
        Some(pinned)
    }
}
