//! Canonical scalar conversion admission, with legacy Rust cast semantics.

use incan_mir_plan_native::{Constant, Operand, PlanType, RvalueKind, SizedNumeric};
use incan_semantics_core::NumericTypeId;
use incan_semantics_core::body_ir::TypedNumericConstant;

/// Preserve truncating, saturating float casts and boolean zero/one conversion.
pub fn integer_conversion(value: Operand, ty: &PlanType) -> Result<RvalueKind, String> {
    match ty {
        PlanType::Int => Ok(RvalueKind::Use(value)),
        PlanType::Float => Ok(RvalueKind::FloatToInt(value)),
        PlanType::Bool => Ok(RvalueKind::BoolToInt(value)),
        PlanType::String => Err("unsupported Body IR Int string parsing".into()),
        _ => Err("unsupported Body IR Int input type".into()),
    }
}

/// Admit legacy numeric carriers; bool-to-float stays refused.
pub fn float_conversion(value: Operand, ty: &PlanType) -> Result<RvalueKind, String> {
    match ty {
        PlanType::Int => Ok(RvalueKind::IntToFloat(value)),
        PlanType::Float => Ok(RvalueKind::Use(value)),
        PlanType::String => Err("unsupported Body IR Float string parsing".into()),
        _ => Err("unsupported Body IR Float input type".into()),
    }
}

/// Translate the checker-owned numeric identity without decoding display spellings.
pub fn numeric_type(kind: NumericTypeId) -> Result<PlanType, String> {
    Ok(match kind {
        NumericTypeId::I8 => PlanType::I8,
        NumericTypeId::I16 => PlanType::I16,
        NumericTypeId::I32 => PlanType::I32,
        NumericTypeId::I128 => PlanType::I128,
        NumericTypeId::U8 => PlanType::U8,
        NumericTypeId::U16 => PlanType::U16,
        NumericTypeId::U32 => PlanType::U32,
        NumericTypeId::U64 => PlanType::U64,
        NumericTypeId::U128 => PlanType::U128,
        NumericTypeId::F32 => PlanType::F32,
        NumericTypeId::F64 => PlanType::F64,
        NumericTypeId::ISize => PlanType::ISize,
        NumericTypeId::USize => PlanType::USize,
        NumericTypeId::I64 => PlanType::Int,
        NumericTypeId::Bool => return Err("unsupported Body IR numeric carrier bool".into()),
    })
}

/// Retain exact integer digits and IEEE bits without a host-width conversion.
pub fn numeric_constant(value: &TypedNumericConstant) -> Result<Constant, String> {
    match value {
        TypedNumericConstant::Signed { kind, value } => Ok(Constant::Numeric(value.to_string(), numeric_type(*kind)?)),
        TypedNumericConstant::Unsigned { kind, value } => {
            Ok(Constant::Numeric(value.to_string(), numeric_type(*kind)?))
        }
        TypedNumericConstant::F32 { bits } => Ok(Constant::Numeric(bits.to_string(), PlanType::F32)),
        TypedNumericConstant::F64 { bits } => Ok(Constant::Numeric(bits.to_string(), PlanType::F64)),
        _ => Err("unsupported Body IR decimal constant".into()),
    }
}

/// Keep a numeric default's exact carrier; zero is also the positive-zero IEEE bit pattern for sized floats.
pub fn numeric_zero(ty: &PlanType) -> Result<Constant, String> {
    match ty {
        PlanType::Int => return Ok(Constant::Int(0)),
        PlanType::Float => return Ok(Constant::Float(0.0)),
        _ => {}
    }
    if has_sized_numeric(ty) {
        return Ok(Constant::Numeric("0".into(), ty.clone()));
    }
    Err("unsupported Body IR nonnumeric default".into())
}

/// The named carrier when a scalar has a sized numeric representation.
pub fn sized_numeric(ty: &PlanType) -> Option<SizedNumeric> {
    Some(match ty {
        PlanType::I8 => SizedNumeric::I8,
        PlanType::I16 => SizedNumeric::I16,
        PlanType::I32 => SizedNumeric::I32,
        PlanType::I128 => SizedNumeric::I128,
        PlanType::U8 => SizedNumeric::U8,
        PlanType::U16 => SizedNumeric::U16,
        PlanType::U32 => SizedNumeric::U32,
        PlanType::U64 => SizedNumeric::U64,
        PlanType::U128 => SizedNumeric::U128,
        PlanType::F32 => SizedNumeric::F32,
        PlanType::F64 => SizedNumeric::F64,
        PlanType::ISize => SizedNumeric::ISize,
        PlanType::USize => SizedNumeric::USize,
        _ => return None,
    })
}

pub fn has_sized_numeric(ty: &PlanType) -> bool {
    sized_numeric(ty).is_some()
}

/// Name the matching monomorphic legacy-runtime boundary.
pub fn numeric_name(ty: &PlanType) -> &'static str {
    match ty {
        PlanType::I8 => "i8",
        PlanType::I16 => "i16",
        PlanType::I32 => "i32",
        PlanType::I128 => "i128",
        PlanType::U8 => "u8",
        PlanType::U16 => "u16",
        PlanType::U32 => "u32",
        PlanType::U64 => "u64",
        PlanType::U128 => "u128",
        PlanType::F32 => "f32",
        PlanType::F64 => "f64",
        PlanType::ISize => "isize",
        PlanType::USize => "usize",
        _ => "",
    }
}

/// Distinguish width-specific integers from exact floating carriers.
pub fn sized_integer(ty: &PlanType) -> bool {
    has_sized_numeric(ty) && *ty != PlanType::F32 && *ty != PlanType::F64
}

/// Select only the carriers admitting overflow-checked unary negation.
pub fn sized_signed_integer(ty: &PlanType) -> bool {
    matches!(
        ty,
        PlanType::I8 | PlanType::I16 | PlanType::I32 | PlanType::I128 | PlanType::ISize
    )
}

/// Recognize the ordinary and sized integer carriers that admit bit operations.
pub fn integer_carrier(ty: &PlanType) -> bool {
    *ty == PlanType::Int || sized_integer(ty)
}

/// Recognize ordinary and sized numeric carriers at checked assignment boundaries.
pub fn numeric_carrier(ty: &PlanType) -> bool {
    has_sized_numeric(ty) || *ty == PlanType::Int || *ty == PlanType::Float
}

/// Whether a typed numeric literal is one of the float carriers.
pub fn is_float_constant(value: &TypedNumericConstant) -> bool {
    matches!(
        value,
        TypedNumericConstant::F32 { .. } | TypedNumericConstant::F64 { .. }
    )
}
