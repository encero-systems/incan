//! Literals lowered in the type their destination gives them: an integer literal the checker typed as a binary float
//! (#1831), and a collection literal declared in a generic body with its annotation's type parameters (#1847).

use super::super::super::TypedExpr;
use super::super::super::expr::{IrDictEntry, IrExprKind, IrListEntry, UnaryOp};
use super::super::super::types::IrType;
use super::super::AstLowering;
use incan_frontend::ast;
use incan_lang::lang::types::numerics::NumericTypeId;

impl AstLowering {
    /// Write an integer literal whose checked type is `float`, `f32` or `f64` as the float literal of the same value.
    ///
    /// The checker gives an integer literal the float type its destination expects, wherever the destination is: a
    /// declaration (`x: float = 1`), a reassignment of a `float` binding (`f = 1`), a parameter, a return, a field, or
    /// an element of a tuple or a list whose element type is a float. It refuses every other integer-to-float
    /// assignment (`f = n` with `n: int`). The generated Rust has no integer-to-float literal coercion, so the
    /// literal is lowered as a float literal of its checked type. A negated literal (`-3`) keeps its negation,
    /// around a float operand; the checker records the float type on the negation, not on the literal inside it.
    /// `source` is the expression `lowered` was lowered from, whose literal carries the full magnitude of an
    /// integer too wide for `i64`. Every other expression is left unchanged.
    pub(super) fn write_float_typed_int_literal_as_float(lowered: &mut TypedExpr, source: &ast::Expr) {
        if !Self::is_binary_float_type(&lowered.ty) {
            return;
        }
        let float_ty = lowered.ty.clone();
        match source {
            ast::Expr::Literal(ast::Literal::Int(literal)) => {
                Self::write_int_literal_as_float(lowered, literal, float_ty);
            }
            ast::Expr::Unary(ast::UnaryOp::Neg, operand_source) => {
                let ast::Expr::Literal(ast::Literal::Int(literal)) = &operand_source.node else {
                    return;
                };
                if let IrExprKind::UnaryOp {
                    op: UnaryOp::Neg,
                    operand,
                } = &mut lowered.kind
                {
                    Self::write_int_literal_as_float(operand, literal, float_ty);
                }
            }
            _ => {}
        }
    }

    /// Give one target's copy of a chained value built from literals the target's own type at each integer literal,
    /// down through the elements of its list, set, tuple and dict literals.
    ///
    /// The checker checks such a value once per target but records one type per source span, the last target's, so
    /// `n = f = 1` over an `int` and a `float` lowers both copies as the float `1.0`, and `f = n = 1` lowers both as
    /// the integer `1`. Each integer literal of the copy, and each negation of one, is rewritten for `target`: the
    /// float literal of the same value where `target` is `float`, `f32` or `f64` at that position, and the integer
    /// literal as written where it is any other type. `source` is the expression the copy was lowered from. A
    /// container literal is walked only when `target` is the same kind of container; every other expression is left
    /// unchanged.
    pub(in crate::lower) fn retype_chained_integer_literals(
        lowered: &mut TypedExpr,
        source: &ast::Expr,
        target: &IrType,
    ) {
        match source {
            ast::Expr::Paren(inner) => {
                return Self::retype_chained_integer_literals(lowered, &inner.node, target);
            }
            ast::Expr::Literal(ast::Literal::Int(literal)) => {
                Self::retype_integer_literal(lowered, literal, target);
                return;
            }
            _ => {}
        }
        match (source, &mut lowered.kind, target) {
            (
                ast::Expr::Unary(ast::UnaryOp::Neg, operand_source),
                IrExprKind::UnaryOp {
                    op: UnaryOp::Neg,
                    operand,
                },
                _,
            ) => {
                let ast::Expr::Literal(ast::Literal::Int(literal)) = &operand_source.node else {
                    return;
                };
                if Self::retype_integer_literal(operand, literal, target) {
                    lowered.ty = operand.ty.clone();
                }
            }
            (ast::Expr::List(entries), IrExprKind::List(lowered_entries), IrType::List(element_ty)) => {
                for (entry, lowered_entry) in entries.iter().zip(lowered_entries.iter_mut()) {
                    if let (ast::ListEntry::Element(item), IrListEntry::Element(lowered_item)) = (entry, lowered_entry)
                    {
                        Self::retype_chained_integer_literals(lowered_item, &item.node, element_ty);
                    }
                }
            }
            (ast::Expr::Set(items), IrExprKind::Set(lowered_items), IrType::Set(element_ty)) => {
                for (item, lowered_item) in items.iter().zip(lowered_items.iter_mut()) {
                    Self::retype_chained_integer_literals(lowered_item, &item.node, element_ty);
                }
            }
            (ast::Expr::Tuple(items), IrExprKind::Tuple(lowered_items), IrType::Tuple(element_types)) => {
                for ((item, lowered_item), element_ty) in items.iter().zip(lowered_items.iter_mut()).zip(element_types)
                {
                    Self::retype_chained_integer_literals(lowered_item, &item.node, element_ty);
                }
            }
            (ast::Expr::Dict(entries), IrExprKind::Dict(lowered_entries), IrType::Dict(key_ty, value_ty)) => {
                for (entry, lowered_entry) in entries.iter().zip(lowered_entries.iter_mut()) {
                    if let (ast::DictEntry::Pair(key, value), IrDictEntry::Pair(lowered_key, lowered_value)) =
                        (entry, lowered_entry)
                    {
                        Self::retype_chained_integer_literals(lowered_key, &key.node, key_ty);
                        Self::retype_chained_integer_literals(lowered_value, &value.node, value_ty);
                    }
                }
            }
            _ => {}
        }
    }

    /// Rewrite one lowered copy of the integer `literal` for the scalar `target`, returning whether it changed: a float
    /// target takes the float literal of the same value in its own type, and any other target takes back the integer
    /// literal a float copy was rewritten from.
    fn retype_integer_literal(lowered: &mut TypedExpr, literal: &ast::IntLiteral, target: &IrType) -> bool {
        if Self::is_binary_float_type(target) {
            if !matches!(
                lowered.kind,
                IrExprKind::Int(_) | IrExprKind::IntLiteral(_) | IrExprKind::Float(_)
            ) {
                return false;
            }
            lowered.kind = IrExprKind::Float(literal.magnitude as f64);
            lowered.ty = target.clone();
            return true;
        }
        if !matches!(lowered.kind, IrExprKind::Float(_)) {
            return false;
        }
        lowered.kind = if literal.fits_i64() {
            IrExprKind::Int(literal.value)
        } else {
            IrExprKind::IntLiteral(literal.repr.clone())
        };
        lowered.ty = IrType::Int;
        true
    }

    /// Replace a lowered integer literal with the float literal of `literal`'s magnitude, typed `float_ty`.
    ///
    /// The checker has already refused a magnitude that is not finite in the float type, so the conversion to `f64`
    /// only rounds a magnitude wider than the float's mantissa, as the float literal of that value would.
    fn write_int_literal_as_float(lowered: &mut TypedExpr, literal: &ast::IntLiteral, float_ty: IrType) {
        if !matches!(lowered.kind, IrExprKind::Int(_) | IrExprKind::IntLiteral(_)) {
            return;
        }
        lowered.kind = IrExprKind::Float(literal.magnitude as f64);
        lowered.ty = float_ty;
    }

    /// Return whether `ty` is one of the binary float types: `float`, `f32` or `f64`.
    fn is_binary_float_type(ty: &IrType) -> bool {
        matches!(
            ty,
            IrType::Float | IrType::Numeric(NumericTypeId::F32 | NumericTypeId::F64)
        )
    }

    /// Give a tuple, list, dict or set literal declared with an annotation the annotation's type, when the literal's
    /// own checked type mentions a type parameter of the enclosing generic body.
    ///
    /// Inside `def mk[T](x: T)`, `pair: tuple[T, Option[T]] = (x, None)` checks `(x, None)` as `(T, Option[T])`, and
    /// lowering spells that `T` as a type parameter ([`IrType::Generic`]). The emitter types a `None` element from its
    /// container's element type and reads a type parameter there as not yet inferred, so it wrote `None::<()>`. The
    /// annotation spells the body's own `T` as the fixed type it is in that body, so the literal takes the
    /// annotation's type and its `None` is written `None::<T>`. Only the literal itself is retyped: its nested
    /// literals take their element types from it. A literal whose type mentions no type parameter, or whose annotation
    /// is not the same kind of collection (an `Option` or a union around it), is left as it is.
    pub(in crate::lower) fn give_literal_its_annotated_type_parameters(
        lowered: &mut TypedExpr,
        annotation: Option<&IrType>,
    ) {
        let Some(annotation) = annotation else {
            return;
        };
        let literal = matches!(
            lowered.kind,
            IrExprKind::Tuple(_) | IrExprKind::List(_) | IrExprKind::Dict(_) | IrExprKind::Set(_)
        );
        let same_kind = match (&lowered.ty, annotation) {
            (IrType::Tuple(items), IrType::Tuple(annotated)) => items.len() == annotated.len(),
            (IrType::List(_), IrType::List(_))
            | (IrType::Set(_), IrType::Set(_))
            | (IrType::Dict(_, _), IrType::Dict(_, _)) => true,
            _ => false,
        };
        if literal && same_kind && lowered.ty.contains_generic_parameter() {
            lowered.ty = annotation.clone();
        }
    }
}
