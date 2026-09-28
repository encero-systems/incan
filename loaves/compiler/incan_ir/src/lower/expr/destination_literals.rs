//! Values lowered in the type their destination gives them: an integer literal the checker typed as a binary float
//! (#1831), a collection literal declared in a generic body with its annotation's type parameters (#1847), and a
//! value written to an `Option` place wrapped in the `Some` layers that place adds (#1858, #1860).

use super::super::super::TypedExpr;
use super::super::super::expr::{
    IrCallArg, IrCallArgKind, IrDictEntry, IrExprKind, IrListEntry, UnaryOp, VarAccess, VarRefKind,
};
use super::super::super::types::{IrType, union_member_type_matches};
use super::super::AstLowering;
use incan_frontend::ast;
use incan_lang::lang::surface::constructors::{self, ConstructorId};
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
            ast::Expr::Literal(ast::Literal::Int(literal)) if literal.suffix.is_none() => {
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

    /// Cast a suffixed source float to its checked exact width when no destination annotation would seed Rust
    /// inference.
    ///
    /// Legacy IR stores an ordinary float literal as an unsuffixed `f64` token. The checker still records `3.14f32`
    /// as `f32`; spelling the conversion in IR keeps an inferred binding physically `f32` without changing the frozen
    /// emitter.
    pub(super) fn write_suffixed_float_literal_as_cast(lowered: &mut TypedExpr, source: &ast::Expr) {
        let ast::Expr::Literal(ast::Literal::Float(literal)) = source else {
            return;
        };
        if literal.suffix.is_none() || !matches!(lowered.kind, IrExprKind::Float(_)) {
            return;
        }
        let target = lowered.ty.clone();
        let original = std::mem::replace(&mut lowered.kind, IrExprKind::Unit);
        lowered.kind = IrExprKind::Cast {
            expr: Box::new(TypedExpr::new(original, IrType::Float)),
            to_type: target,
        };
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

    /// Wrap a value the checker recorded as written to an `Option` place in the `Some` layers that place adds (#1858).
    ///
    /// The checker records the place's `Option` type for the value of a field or index assignment, a model or class
    /// constructor field and a `return`, where it accepts a value of the `Option`'s payload type (`box.count = 5` for
    /// an `Option[int]` field). The backend writes these places with the value as lowered, so the value itself becomes
    /// `Some(5)`. Inside an imported trait default body the facts are not read, since its spans belong to the defining
    /// module (see [`Self::lower_expr_spanned`]).
    pub(in crate::lower) fn wrap_in_recorded_option_destination(&self, value: TypedExpr, span: ast::Span) -> TypedExpr {
        if self.active_imported_trait_defaults.last().copied().unwrap_or(false) {
            return value;
        }
        let Some(destination) = self
            .type_info
            .as_ref()
            .and_then(|info| info.option_destination_type(span))
        else {
            return value;
        };
        let destination = self.lower_resolved_type(destination);
        Self::wrap_value_in_option_destination(value, &destination)
    }

    /// Wrap `value` in one `Some` per `Option` layer that `destination` holds around the value's own type (#1858,
    /// #1860).
    ///
    /// An `int` written to an `Option[Option[int]]` place becomes `Some(Some(5))`, a value already of the destination
    /// type is left as it is, and so is a `None` literal, which is the outer `None` of any `Option` destination. A
    /// value whose type is not one of the destination's `Option` payloads (a union member, a reference, an unknown
    /// type) is left as it is too: this wraps only a value the destination holds exactly, and each layer takes the
    /// destination's own `Option` type, so a `str` literal in an `Option[str]` place is an `Option[str]` value.
    pub(in crate::lower) fn wrap_value_in_option_destination(value: TypedExpr, destination: &IrType) -> TypedExpr {
        if matches!(value.kind, IrExprKind::None) {
            return value;
        }
        let mut layers = Vec::new();
        let mut place = destination;
        while !union_member_type_matches(place, &value.ty) {
            let IrType::Option(payload) = place else {
                return value;
            };
            layers.push(place.clone());
            place = payload;
        }
        layers.into_iter().rev().fold(value, |payload, option_ty| {
            Self::some_constructor_call(payload, option_ty)
        })
    }

    /// Wrap a value written to a local binding whose `Option` type holds the value two or more layers deep in all of
    /// those layers (#1860).
    ///
    /// Writing a binding already wraps a value of the binding `Option`'s own payload type in one `Some`, so a value one
    /// layer short is left as it is. `a: Option[Option[int]] = 5` lowers its value to `Some(Some(5))`, a value of the
    /// binding's own type, which the write then stores unchanged.
    pub(in crate::lower) fn wrap_value_in_nested_option_binding(value: TypedExpr, binding_ty: &IrType) -> TypedExpr {
        if let IrType::Option(payload) = binding_ty
            && union_member_type_matches(payload, &value.ty)
        {
            return value;
        }
        Self::wrap_value_in_option_destination(value, binding_ty)
    }

    /// Build the `Some(payload)` constructor call of type `option_ty`, spelled as a source `Some(...)` call lowers.
    fn some_constructor_call(payload: TypedExpr, option_ty: IrType) -> TypedExpr {
        let span = payload.span;
        let mut call = TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: constructors::as_str(ConstructorId::Some).to_string(),
                        access: VarAccess::Move,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Unknown,
                )),
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: payload,
                }],
                callable_signature: None,
                canonical_path: None,
            },
            option_ty,
        );
        call.span = span;
        call
    }
}
