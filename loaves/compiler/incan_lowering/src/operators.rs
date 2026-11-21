//! Scalar operators, comparisons and the checked rvalues an ordinary assignment stores.

use incan_mir_plan_native::{BinaryOp, Operand, PlanType, RvalueKind, SourceSpan, UnaryOp, assign as plan_assign};
use incan_semantics_core::body_ir::{
    BinOp, BodyIrModule, Constant as SourceConstant, Operand as SourceOperand, Rvalue, TypedNumericConstant, UnOp,
};

use crate::checked_values::scalar_place;
use crate::collections::is_collection;
use crate::element_places::is_element_place;
use crate::enums::unit_variant;
use crate::frame::{Builder, place_type, read_place, temporary, value_type};
use crate::list_values::value_operand;
use crate::lists::is_list;
use crate::model_construction::checked_aggregate;
use crate::models::model_place;
use crate::numerics::{integer_carrier, is_float_constant, numeric_carrier};
use crate::pending::untranslated;
use crate::result_carriers::retained_reference_value;
use crate::statements::is_text_or_nominal;

/// The plan type an operand carries: its local's declared type, or its literal's type.
pub fn operand_type(builder: &Builder, operand: &Operand) -> Result<PlanType, String> {
    crate::frame::value_type(builder, operand)
}

/// Translate only native scalar operations; operators with runtime semantics are separate calls.
pub fn binary_operator(operator: BinOp) -> Result<BinaryOp, String> {
    Ok(match operator {
        BinOp::Add => BinaryOp::Add,
        BinOp::Sub => BinaryOp::Subtract,
        BinOp::Mul => BinaryOp::Multiply,
        BinOp::Div => BinaryOp::Divide,
        BinOp::Eq => BinaryOp::Equal,
        BinOp::Ne => BinaryOp::NotEqual,
        BinOp::Lt => BinaryOp::Less,
        BinOp::Le => BinaryOp::LessEqual,
        BinOp::Gt => BinaryOp::Greater,
        BinOp::Ge => BinaryOp::GreaterEqual,
        BinOp::FloorDiv => return Err("Body IR FloorDiv requires its runtime helper".into()),
        BinOp::Mod => return Err("Body IR Mod requires its runtime helper".into()),
        BinOp::Pow => return Err("unsupported Body IR Pow".into()),
        BinOp::BitAnd => BinaryOp::BitAnd,
        BinOp::BitOr => BinaryOp::BitOr,
        BinOp::BitXor => BinaryOp::BitXor,
        BinOp::Shl => BinaryOp::ShiftLeft,
        BinOp::Shr => BinaryOp::ShiftRight,
        BinOp::Is => return Err("unsupported Body IR Is".into()),
        BinOp::IsNot => return Err("unsupported Body IR IsNot".into()),
        BinOp::And => return Err("unsupported Body IR And".into()),
        BinOp::Or => return Err("unsupported Body IR Or".into()),
    })
}

/// Select the integer bit operations legacy emits as Rust infix operators.
fn is_bit_operator(operator: BinOp) -> bool {
    matches!(
        operator,
        BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr
    )
}

/// Admit integer bit operations over one integer type; a shift keeps its left type and counts with any integer.
fn bit_operation(
    operator: BinOp,
    left: Operand,
    left_type: &PlanType,
    right: Operand,
    right_type: &PlanType,
) -> Result<RvalueKind, String> {
    if !integer_carrier(left_type) || !integer_carrier(right_type) {
        return Err("unsupported Body IR non-integer bit operands".into());
    }
    let shift = operator == BinOp::Shl || operator == BinOp::Shr;
    if !shift && left_type != right_type {
        return Err("unsupported Body IR mixed integer bit operands".into());
    }
    Ok(RvalueKind::Binary(binary_operator(operator)?, left, right))
}

/// Materialize indexed scalar operands and refuse collection operators before native validation.
pub fn scalar_operator_operand(
    builder: &mut Builder,
    value: &SourceOperand,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let operand = value_operand(builder, value, span)?;
    let ty = value_type(builder, &operand)?;
    if is_list(&ty) || is_collection(&ty) {
        return Err("unsupported Body IR list scalar operator".into());
    }
    Ok(operand)
}

/// Borrow an owner's place for a destination declared as a reference to it, rather than reading a copy.
fn owner_borrow(
    builder: &mut Builder,
    operand: &SourceOperand,
    expected: &PlanType,
    span: &SourceSpan,
) -> Result<Option<Operand>, String> {
    let SourceOperand::Place(read) = operand else {
        return Ok(None);
    };
    if read.place.global().is_some() || is_element_place(builder, &read.place)? {
        return Ok(None);
    }
    let place = scalar_place(builder, &read.place, span)?;
    let owner = place_type(&builder.locals, &place);
    match (expected, &owner) {
        (PlanType::ModelRef(index, _), PlanType::Model(owner_index, _))
        | (PlanType::EnumRef(index, _), PlanType::Enum(owner_index, _)) => {
            if index != owner_index {
                return Ok(None);
            }
        }
        _ => return Ok(None),
    }
    let reference = temporary(builder, expected.clone(), span);
    builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(place), span));
    Ok(Some(read_place(reference, false, span)))
}

/// Read the checked expression; the retained destination carrier selects a proven constructor's closed layout.
pub fn scalar_rvalue(
    module: &BodyIrModule,
    builder: &mut Builder,
    value: &Rvalue,
    expected: &PlanType,
    span: &SourceSpan,
) -> Result<RvalueKind, String> {
    match value {
        Rvalue::Use(operand) => {
            if let Some(reference) = retained_reference_value(builder, operand, expected, span)? {
                return Ok(RvalueKind::Use(reference));
            }
            match owner_borrow(builder, operand, expected, span)? {
                Some(reference) => Ok(RvalueKind::Use(reference)),
                None => Ok(RvalueKind::Use(value_operand(builder, operand, span)?)),
            }
        }
        Rvalue::UnaryOp(operator, operand) => match operator {
            UnOp::Not => Ok(RvalueKind::Unary(
                UnaryOp::Not,
                scalar_operator_operand(builder, operand, span)?,
            )),
            UnOp::Neg => Ok(RvalueKind::Unary(
                UnaryOp::Negate,
                scalar_operator_operand(builder, operand, span)?,
            )),
            UnOp::Invert => {
                let inverted = scalar_operator_operand(builder, operand, span)?;
                if !integer_carrier(&value_type(builder, &inverted)?) {
                    return Err("unsupported Body IR non-integer unary operator Invert".into());
                }
                Ok(RvalueKind::Unary(UnaryOp::Invert, inverted))
            }
        },
        Rvalue::BinaryOp(operator, left, right) => scalar_binary(builder, *operator, left, right, span),
        Rvalue::IsInstance(_) => Err(untranslated("carrier_operands::lower_union_isinstance")),
        Rvalue::Aggregate(kind, elements) => checked_aggregate(module, builder, kind, elements, expected, span),
        Rvalue::Dict(_) => Err("unsupported Body IR Dict".into()),
        Rvalue::ValueEnumVariant(_) => Err(untranslated("value_enums::value_variant")),
        Rvalue::FieldlessEnumVariant(target) => {
            let (owner, variant) = unit_variant(builder.enums(), target)?;
            Ok(RvalueKind::Enum(owner as i64, variant as i64, Vec::new()))
        }
        Rvalue::ResultVariant(_) => Err("unsupported Body IR ResultVariant".into()),
        Rvalue::Format(_) => Err("unsupported Body IR Format".into()),
        Rvalue::Closure { .. } => Err("unsupported Body IR Closure".into()),
        Rvalue::Generator { .. } => Err("unsupported Body IR Generator".into()),
        Rvalue::Match { .. } => Err("unsupported Body IR Match".into()),
    }
}

/// A native scalar binary operation over two numeric or boolean operands of one type.
///
/// Comparisons of Options, enums, frozen values, collections, tuples, newtypes and text each take their own runtime
/// operation; every one of those paths declines numeric and boolean operands, so those reach the native operator.
fn scalar_binary(
    builder: &mut Builder,
    operator: BinOp,
    left: &SourceOperand,
    right: &SourceOperand,
    span: &SourceSpan,
) -> Result<RvalueKind, String> {
    if !plain_scalar_operand(builder, left, span)? || !plain_scalar_operand(builder, right, span)? {
        return Err(untranslated(
            "operators::scalar_rvalue comparison of nonscalar operands",
        ));
    }
    if operator == BinOp::Div && (!float_operand(builder, left, span)? || !float_operand(builder, right, span)?) {
        return Err("unsupported Body IR non-float true division operands".into());
    }
    let left_operand = scalar_operator_operand(builder, left, span)?;
    let right_operand = scalar_operator_operand(builder, right, span)?;
    let left_type = value_type(builder, &left_operand)?;
    let right_type = value_type(builder, &right_operand)?;
    if let PlanType::FrozenCollection(..) = left_type {
        return Err("unsupported Body IR frozen collection comparison".into());
    }
    if is_bit_operator(operator) {
        return bit_operation(operator, left_operand, &left_type, right_operand, &right_type);
    }
    if left_type != right_type {
        return Err("unsupported Body IR mixed numeric arithmetic without retained coercion".into());
    }
    if operator == BinOp::Is || operator == BinOp::IsNot {
        if left_type != PlanType::Bool {
            return Err("unsupported Body IR scalar identity outside booleans".into());
        }
        let comparison = if operator == BinOp::IsNot {
            BinaryOp::NotEqual
        } else {
            BinaryOp::Equal
        };
        return Ok(RvalueKind::Binary(comparison, left_operand, right_operand));
    }
    if left_type == PlanType::Decimal {
        return Err(untranslated("decimal_plan::decimal_comparison"));
    }
    // A closed type-parameter instance can reach here with text or a nominal where the checker chose a plain operator
    // for the open parameter; those need a runtime operation the checker never selected.
    if is_text_or_nominal(&left_type) {
        return Err(format!(
            "unsupported Body IR binary operator {operator:?} on text or nominal operands {left_type:?} at {span:?}"
        ));
    }
    Ok(RvalueKind::Binary(
        binary_operator(operator)?,
        left_operand,
        right_operand,
    ))
}

/// A numeric or boolean literal, or a local's numeric or boolean storage, read without materializing anything.
fn plain_scalar_operand(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<bool, String> {
    match value {
        SourceOperand::Constant(constant) => Ok(match constant {
            SourceConstant::Int(_) | SourceConstant::Float(_) | SourceConstant::Bool(_) => true,
            SourceConstant::TypedNumeric(number) => !matches!(number, TypedNumericConstant::Decimal { .. }),
            _ => false,
        }),
        SourceOperand::Place(read) => {
            if read.place.global().is_some() || is_element_place(builder, &read.place)? {
                return Ok(false);
            }
            let place = scalar_place(builder, &read.place, span)?;
            let ty = place_type(&builder.locals, &place);
            Ok(numeric_carrier(&ty) || ty == PlanType::Bool)
        }
    }
}

/// Require a retained float representation; integer true-division coercions remain outside this slice.
fn float_operand(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<bool, String> {
    match value {
        SourceOperand::Place(read) => {
            let mut place = model_place(
                builder.tables(),
                &builder.locals,
                &read.place,
                span,
                builder.local_offset,
            )?;
            place.local += builder.local_offset;
            let ty = place_type(&builder.locals, &place);
            Ok(matches!(ty, PlanType::Float | PlanType::F32 | PlanType::F64))
        }
        SourceOperand::Constant(SourceConstant::Float(_)) => Ok(true),
        SourceOperand::Constant(SourceConstant::TypedNumeric(number)) => Ok(is_float_constant(number)),
        SourceOperand::Constant(_) => Ok(false),
    }
}
