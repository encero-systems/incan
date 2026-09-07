//! Narrow queries over checked types and literals that lowering maps to Body IR primitives.

use super::*;

/// Return the checked payload types of an intrinsic `Result[ok, error]` carrier.
///
/// This is deliberately a narrow query over the typechecker-owned semantic type. The Body-IR lowerer uses it only
/// to retain facts which direct execution cannot reconstruct: which intrinsic constructor is being formed, which
/// pattern payload is being bound, and whether `?` preserves the enclosing error type exactly. It does not infer
/// a conversion or admit a differently shaped generic carrier.
pub(super) fn result_type_parts(ty: &IncanType) -> Option<(&IncanType, &IncanType)> {
    let IncanType::Generic { base, args } = ty else {
        return None;
    };
    (collections::from_str(base) == Some(CollectionTypeId::Result)).then_some(())?;
    match args.as_slice() {
        [ok_type, error_type] => Some((ok_type, error_type)),
        _ => None,
    }
}
/// Return just the checked error channel for an intrinsic `Result` carrier.
pub(super) fn result_error_type(ty: &IncanType) -> Option<&IncanType> {
    result_type_parts(ty).map(|(_, error_type)| error_type)
}
/// Map only the compiler-owned intrinsic constructor spellings to Body-IR result variants.
pub(super) fn result_variant_kind(name: &str) -> Option<bir::ResultVariantKind> {
    match constructors::from_str(name) {
        Some(ConstructorId::Ok) => Some(bir::ResultVariantKind::Ok),
        Some(ConstructorId::Err) => Some(bir::ResultVariantKind::Err),
        _ => None,
    }
}
/// Whether a type is string-like enough to route binary operators through the compiler-owned string helpers
/// (mirrors `is_string_like_type` in `src/backend/ir/conversions.rs`, restated here so Body IR does not depend on
/// that Rust-emission-specific module — see this file's module docs).
pub(super) fn is_string_like(ty: &IncanType) -> bool {
    matches!(
        ty,
        IncanType::Primitive(IncanPrimitiveType::Str | IncanPrimitiveType::FrozenStr)
    )
}
/// Map a string-typed binary operator to its compiler-owned helper operation, or `None` for operators that have no
/// string-specific helper (arithmetic-only operators never reach here because `lower_binary` only checks this for
/// string-like operand types).
///
/// Membership lives here rather than in [`lower_binary_op`] because `in` is not one operation: between two strings
/// it asks for substring containment, over a collection it asks for element lookup. A single [`bir::BinOp`] variant
/// would have to pick one of those meanings and then apply it silently to the other. Routing string membership
/// through [`bir::HelperOp::StrContains`] instead makes the substring policy — the behavior `parity-987-0003`
/// records as `Preserved` — an explicit `Callee::Helper` call carrying its own runtime requirement, exactly as `+`
/// on two strings already is.
pub(super) fn string_helper_for_binop(op: ast::BinaryOp) -> Option<bir::HelperOp> {
    match op {
        ast::BinaryOp::Add => Some(bir::HelperOp::StrConcat),
        ast::BinaryOp::Eq => Some(bir::HelperOp::StrEq),
        ast::BinaryOp::NotEq => Some(bir::HelperOp::StrNe),
        ast::BinaryOp::Lt => Some(bir::HelperOp::StrLt),
        ast::BinaryOp::LtEq => Some(bir::HelperOp::StrLe),
        ast::BinaryOp::Gt => Some(bir::HelperOp::StrGt),
        ast::BinaryOp::GtEq => Some(bir::HelperOp::StrGe),
        ast::BinaryOp::In => Some(bir::HelperOp::StrContains),
        ast::BinaryOp::NotIn => Some(bir::HelperOp::StrNotContains),
        _ => None,
    }
}
/// Which builtin collection a checked type names, or `None` when it is not one of the three whose operators Body IR
/// represents.
///
/// Reads the collection registry rather than comparing base names against literals, so adding a collection to
/// `incan_core` cannot silently leave this mapping stale.
fn builtin_collection_id(ty: &IncanType) -> Option<CollectionTypeId> {
    let IncanType::Generic { base, args } = ty else {
        return None;
    };
    if args.is_empty() {
        return None;
    }
    match collections::from_str(base) {
        Some(id @ (CollectionTypeId::List | CollectionTypeId::Set | CollectionTypeId::Dict)) => Some(id),
        _ => None,
    }
}
/// Map a binary operator over a builtin collection to its compiler-owned helper operation, or `None` when the
/// operator has no collection meaning.
///
/// The collection is taken from the operand that *holds* it — the right side for membership, the left for
/// concatenation — because the helper names the container, not the element. `Dict` supports membership but not
/// concatenation, which is why the `Add` arm admits only `List`: this follows the surface the typechecker already
/// accepts rather than widening it. `str + str` never reaches here, because [`string_helper_for_binop`] is checked
/// first and a `str` is not a generic collection type.
///
/// This is the collection counterpart to [`string_helper_for_binop`], and exists for the same reason. `in` between
/// two strings asks for substring containment while `in` over a collection asks for element lookup, so neither can
/// be a [`bir::BinOp`] without silently applying one meaning to the other.
///
/// All seven helpers named here have a runtime function in `incan_stdlib::collections`, and the executor runs the
/// three list forms. Set and dict membership still refuse — not for want of a helper, but because the replacement
/// executor has no set or dict value at all, so their aggregates refuse before membership is reached. #1247 owns
/// that value gap and inherits the membership arms with it.
///
/// `+` earns a helper on a narrower ground, and the distinction matters because it is what keeps this table from
/// swallowing comparisons too. The test is not that a heap container cannot sit under a primitive -- `==` on two
/// lists does exactly that, faithfully -- but that `determine_binop_plan` routes list `+` to
/// `incan_stdlib::collections::list_concat` while emitting comparisons as an infix operator. A helper here is
/// therefore agreement with the Rust-emission backend, not a judgement about the operand's representation.
pub(super) fn collection_helper_for_binop(
    op: ast::BinaryOp,
    lhs_ty: &IncanType,
    rhs_ty: &IncanType,
) -> Option<bir::HelperOp> {
    // The container is whichever operand holds the collection: membership reads `needle in haystack`, so the
    // haystack is on the right, while concatenation joins two collections and takes the left. Selecting it here
    // keeps that rule in one place instead of at each call site.
    let container_ty = match op {
        ast::BinaryOp::In | ast::BinaryOp::NotIn => rhs_ty,
        _ => lhs_ty,
    };
    let collection = builtin_collection_id(container_ty)?;
    match (op, collection) {
        (ast::BinaryOp::In, CollectionTypeId::List) => Some(bir::HelperOp::ListContains),
        (ast::BinaryOp::NotIn, CollectionTypeId::List) => Some(bir::HelperOp::ListNotContains),
        (ast::BinaryOp::In, CollectionTypeId::Set) => Some(bir::HelperOp::SetContains),
        (ast::BinaryOp::NotIn, CollectionTypeId::Set) => Some(bir::HelperOp::SetNotContains),
        (ast::BinaryOp::In, CollectionTypeId::Dict) => Some(bir::HelperOp::DictContainsKey),
        (ast::BinaryOp::NotIn, CollectionTypeId::Dict) => Some(bir::HelperOp::DictNotContainsKey),
        (ast::BinaryOp::Add, CollectionTypeId::List) => Some(bir::HelperOp::ListConcat),
        _ => None,
    }
}
/// Map a surface binary operator to Body IR's primitive operator set — one machine-level combination of two
/// already-evaluated operands — or `None` for operators whose meaning is not a primitive.
///
/// The result type of a mapped operator is *not* this function's business: the typechecker already decided it (an
/// `int ** int` with a dynamic exponent resolves `float`, while a non-negative integer-literal exponent stays
/// `int`; `int & int` stays `int`), and lowering carries that decision through on the assigned temporary rather than
/// re-deriving it from the operator.
///
/// The match is exhaustive so that a new surface operator is a compile error here rather than a silent refusal.
/// Two groups deliberately return `None`, each for a different reason:
///
/// - **Membership** (`in` / `not in`) has no single primitive meaning — substring between strings, element lookup over
///   a collection — so it is always a runtime call, never a [`bir::BinOp`]. String operands map through
///   [`string_helper_for_binop`] and builtin collections through [`collection_helper_for_binop`]; membership over a
///   user-defined type is a resolved `__contains__` dispatch and never reaches this table.
/// - **`MatMul` and both pipes** are protocol hooks, never primitives. The typechecker resolves `@`, `|>`, and `<|`
///   through `__matmul__` / `__pipe_forward__` / `__pipe_backward__` and rejects the expression outright when no hook
///   resolves, so a well-typed program always reaches `lower_binary` with a recorded operator dispatch and is lowered
///   as the method call it is. They therefore need no entry here and carry no refusal: reaching this table with one of
///   them would mean the typechecker admitted an unresolved hook.
pub(super) fn lower_binary_op(op: ast::BinaryOp) -> Option<bir::BinOp> {
    match op {
        ast::BinaryOp::Add => Some(bir::BinOp::Add),
        ast::BinaryOp::Sub => Some(bir::BinOp::Sub),
        ast::BinaryOp::Mul => Some(bir::BinOp::Mul),
        ast::BinaryOp::Div => Some(bir::BinOp::Div),
        ast::BinaryOp::FloorDiv => Some(bir::BinOp::FloorDiv),
        ast::BinaryOp::Mod => Some(bir::BinOp::Mod),
        ast::BinaryOp::Pow => Some(bir::BinOp::Pow),
        ast::BinaryOp::BitAnd => Some(bir::BinOp::BitAnd),
        ast::BinaryOp::BitOr => Some(bir::BinOp::BitOr),
        ast::BinaryOp::BitXor => Some(bir::BinOp::BitXor),
        ast::BinaryOp::Shl => Some(bir::BinOp::Shl),
        ast::BinaryOp::Shr => Some(bir::BinOp::Shr),
        ast::BinaryOp::Eq => Some(bir::BinOp::Eq),
        ast::BinaryOp::NotEq => Some(bir::BinOp::Ne),
        ast::BinaryOp::Lt => Some(bir::BinOp::Lt),
        ast::BinaryOp::LtEq => Some(bir::BinOp::Le),
        ast::BinaryOp::Gt => Some(bir::BinOp::Gt),
        ast::BinaryOp::GtEq => Some(bir::BinOp::Ge),
        // `is` / `is not` stay distinct from `==` / `!=` even though the Rust-emission backend currently emits both
        // pairs identically. Body IR records which operator the source wrote; collapsing them would discard the
        // only place a later identity-versus-equality split could be decided against.
        ast::BinaryOp::Is => Some(bir::BinOp::Is),
        ast::BinaryOp::IsNot => Some(bir::BinOp::IsNot),
        ast::BinaryOp::And => Some(bir::BinOp::And),
        ast::BinaryOp::Or => Some(bir::BinOp::Or),
        ast::BinaryOp::In
        | ast::BinaryOp::NotIn
        | ast::BinaryOp::MatMul
        | ast::BinaryOp::PipeForward
        | ast::BinaryOp::PipeBackward => None,
    }
}
/// Lower a literal to a Body IR constant.
///
/// Every literal kind the AST can hold now maps to a [`bir::Constant`], byte strings included -- a `b"..."`
/// becomes [`bir::Constant::Bytes`] and never a [`bir::Constant::Str`], because the two are distinct source types
/// whose ownership and equality differ (see that variant's own docs for the owned-buffer rationale). Byte-string
/// *patterns* remain a separate, deliberately unsupported pattern-admission boundary; they do not make literal
/// expression lowering partial.
pub(super) fn lower_literal(lit: &ast::Literal) -> bir::Constant {
    match lit {
        ast::Literal::Int(int_lit) => bir::Constant::Int(int_lit.value),
        ast::Literal::Float(float_lit) => bir::Constant::Float(float_lit.repr.clone()),
        ast::Literal::Decimal(decimal_lit) => bir::Constant::Float(decimal_lit.repr.clone()),
        ast::Literal::String(s) => bir::Constant::Str(s.clone()),
        ast::Literal::Bool(b) => bir::Constant::Bool(*b),
        ast::Literal::None => bir::Constant::None,
        ast::Literal::Bytes(bytes) => bir::Constant::Bytes(bytes.clone()),
    }
}

/// Lower a numeric literal with the canonical type selected by the typechecker.
///
/// Ordinary `int` and `float` keep their compact compatibility variants. Explicit sized numerics and decimals use
/// [`bir::Constant::TypedNumeric`] so wide integer magnitude, float width/rounding, and decimal scale cannot be lost
/// before a replacement backend sees them.
pub(super) fn lower_checked_literal(lit: &ast::Literal, ty: &IncanType) -> bir::Constant {
    use incan_core::lang::types::numerics::{NumericFamily, NumericTypeId, info_for};

    let typed = match (lit, ty) {
        (ast::Literal::Int(value), IncanType::Primitive(IncanPrimitiveType::Numeric(kind))) => {
            match info_for(*kind).family {
                NumericFamily::SignedInteger => i128::try_from(value.magnitude)
                    .ok()
                    .map(|value| bir::TypedNumericConstant::Signed { kind: *kind, value }),
                NumericFamily::UnsignedInteger => Some(bir::TypedNumericConstant::Unsigned {
                    kind: *kind,
                    value: value.magnitude,
                }),
                NumericFamily::BinaryFloat => match kind {
                    NumericTypeId::F32 => {
                        let value = value.magnitude as f32;
                        value
                            .is_finite()
                            .then_some(bir::TypedNumericConstant::F32 { bits: value.to_bits() })
                    }
                    NumericTypeId::F64 => {
                        let value = value.magnitude as f64;
                        value
                            .is_finite()
                            .then_some(bir::TypedNumericConstant::F64 { bits: value.to_bits() })
                    }
                    _ => None,
                },
                NumericFamily::Bool => None,
            }
        }
        (ast::Literal::Float(value), IncanType::Primitive(IncanPrimitiveType::Numeric(NumericTypeId::F32))) => {
            let normalized = value.repr.replace('_', "");
            normalized
                .parse::<f32>()
                .ok()
                .filter(|value| value.is_finite())
                .map(|value| bir::TypedNumericConstant::F32 { bits: value.to_bits() })
        }
        (ast::Literal::Float(value), IncanType::Primitive(IncanPrimitiveType::Numeric(NumericTypeId::F64))) => {
            value.value.is_finite().then_some(bir::TypedNumericConstant::F64 {
                bits: value.value.to_bits(),
            })
        }
        (ast::Literal::Decimal(value), IncanType::Decimal { precision, scale }) => {
            decimal_constant(&value.body, *precision, *scale)
        }
        _ => None,
    };
    typed
        .map(bir::Constant::TypedNumeric)
        .unwrap_or_else(|| lower_literal(lit))
}

/// Normalize one typechecked decimal literal into the same coefficient/literal-scale pair as `Decimal128`.
fn decimal_constant(body: &str, precision: u8, scale: u8) -> Option<bir::TypedNumericConstant> {
    let parsed = incan_core::numeric_values::parse_decimal_literal_body(body)?;
    Some(bir::TypedNumericConstant::Decimal {
        precision,
        scale,
        coefficient: parsed.coefficient,
        literal_scale: parsed.literal_scale,
    })
}

/// Fold a checked negative exact-numeric literal into one typed constant without applying a general runtime
/// negation rule. Ordinary float/int negation remains an operation and keeps its existing replacement boundary.
pub(super) fn lower_checked_negative_literal(lit: &ast::Literal, ty: &IncanType) -> Option<bir::Constant> {
    use incan_core::lang::types::numerics::{NumericFamily, NumericTypeId, info_for};

    let value = match (lit, ty) {
        (ast::Literal::Int(value), IncanType::Primitive(IncanPrimitiveType::Numeric(kind)))
            if info_for(*kind).family == NumericFamily::SignedInteger =>
        {
            let signed = if value.magnitude == (1_u128 << 127) {
                i128::MIN
            } else {
                -i128::try_from(value.magnitude).ok()?
            };
            bir::TypedNumericConstant::Signed {
                kind: *kind,
                value: signed,
            }
        }
        (ast::Literal::Int(value), IncanType::Primitive(IncanPrimitiveType::Numeric(NumericTypeId::F32))) => {
            let value = -(value.magnitude as f32);
            value
                .is_finite()
                .then_some(bir::TypedNumericConstant::F32 { bits: value.to_bits() })?
        }
        (ast::Literal::Int(value), IncanType::Primitive(IncanPrimitiveType::Numeric(NumericTypeId::F64))) => {
            let value = -(value.magnitude as f64);
            value
                .is_finite()
                .then_some(bir::TypedNumericConstant::F64 { bits: value.to_bits() })?
        }
        (ast::Literal::Float(value), IncanType::Primitive(IncanPrimitiveType::Numeric(NumericTypeId::F32))) => {
            let normalized = value.repr.replace('_', "");
            let value = -normalized.parse::<f32>().ok()?;
            value
                .is_finite()
                .then_some(bir::TypedNumericConstant::F32 { bits: value.to_bits() })?
        }
        (ast::Literal::Float(value), IncanType::Primitive(IncanPrimitiveType::Numeric(NumericTypeId::F64))) => {
            let value = -value.value;
            value
                .is_finite()
                .then_some(bir::TypedNumericConstant::F64 { bits: value.to_bits() })?
        }
        _ => return None,
    };
    Some(bir::Constant::TypedNumeric(value))
}
/// Canonical base name of the checked range-value type, as the typechecker spells it (`Range[int]`).
///
/// `TypeChecker::check_range_expr` (`src/frontend/typechecker/check_expr/control_flow.rs`) produces this spelling,
/// but a type spelling is not evidence of a Body-IR aggregate layout: parameters, returns, and user declarations
/// can cross the frontend boundary with the same generic base. [`BodyBuilder::lower_for`] therefore combines this
/// helper with a local provenance check before reading [`bir::AggregateKind::Range`] fields. The `range()` builtin
/// is deliberately *not* this type: it resolves to a plain `Named("Range")` iterator
/// (`src/frontend/symbols.rs`) and keeps its existing iteration path.
pub(super) const RANGE_TYPE_BASE: &str = incan_core::lang::surface::types::RANGE_TYPE_NAME;
/// The per-iteration increment of every range the surface can spell.
///
/// There is no step spelling in the language (`start..end` and `start..=end` are the only forms the parser
/// produces), so this is a single shared constant rather than a lowered operand: it is both the
/// [`bir::AggregateKind::RANGE_FIELD_STEP`] operand a range value carries and the increment
/// [`BodyBuilder::lower_for`]'s normalized counting loop adds to its index, which keeps the two from drifting.
pub(super) const RANGE_UNIT_STEP: i64 = 1;
/// The element type a checked range value yields per iteration, or `None` when `ty` is not a range value.
///
/// Used to recognise a range-shaped type and recover a checked loop item type. A caller must not use this
/// type-level fact alone as permission to project a range aggregate's fields; see [`RANGE_TYPE_BASE`].
pub(super) fn range_value_element_type(ty: &IncanType) -> Option<&IncanType> {
    match ty {
        IncanType::Generic { base, args } if base == RANGE_TYPE_BASE => args.first(),
        _ => None,
    }
}
