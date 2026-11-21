//! Scalar operators, comparisons and the checked rvalues an ordinary assignment stores.

use incan_mir_plan_native::{
    BinaryOp, CalleeKind, Carrier, Constant, ExternalFunction, ListLeaf, Operand, OperandKind, Place, PlanType,
    RvalueKind, SourceSpan, UnaryOp, assign as plan_assign, carrier_position,
};
use incan_semantics_core::SemanticSourceTargetKind;
use incan_semantics_core::body_ir::{
    BinOp, BodyIrModule, Constant as SourceConstant, Operand as SourceOperand, OwnershipFact, Rvalue, UnOp,
};

use crate::bytes_plan::bytes_literal;
use crate::call_arguments::shared_nominal_argument;
use crate::calls::instantiated_external_call;
use crate::checked_values::scalar_place;
use crate::collections::is_collection;
use crate::decimal_plan::decimal_comparison;
use crate::element_borrows::shared_element_place;
use crate::element_places::is_element_place;
use crate::enums::{unit_variant, validate_unit_comparison, validate_unit_ordering};
use crate::equality::{emit_collection_equality, equality_shape};
use crate::formatting::native_enum_owner;
use crate::frame::{Builder, literal, place_type, read_place, temporary, terminate_call, value_type};
use crate::frozen_equality::compare_frozen;
use crate::frozen_list_equality::{frozen_list_equality, supports_frozen_list_equality};
use crate::list_methods::materialized_receiver;
use crate::list_values::{list_borrow, value_operand};
use crate::lists::is_list;
use crate::matches::enum_operand;
use crate::model_construction::checked_aggregate;
use crate::models::model_place;
use crate::numerics::{integer_carrier, is_float_constant};
use crate::operand_borrows::borrow_text_operand;
use crate::optional_text_equality::optional_text_equality;
use crate::options::carrier_layout;
use crate::pending::untranslated;
use crate::result_carriers::retained_reference_value;
use crate::sequence_builtins::source_operand_type;
use crate::statements::is_text_or_nominal;
use crate::strings::emit_text_helper;
use crate::text_ownership::drop_borrowed_text_temporaries;
use crate::value_enums::value_variant;

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
        Rvalue::ValueEnumVariant(target) => {
            let (owner, variant) = value_variant(builder.enums(), builder.value_enums(), target)?;
            Ok(RvalueKind::Enum(owner as i64, variant as i64, Vec::new()))
        }
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

/// Lower a checked binary operation: an Option tested against None, enum comparisons, structural comparisons of
/// frozen values, collections, tuples, newtypes and text, and otherwise one native scalar operation.
fn scalar_binary(
    builder: &mut Builder,
    operator: BinOp,
    left: &SourceOperand,
    right: &SourceOperand,
    span: &SourceSpan,
) -> Result<RvalueKind, String> {
    if let Some(result) = optional_none_comparison(builder, operator, left, right, span)? {
        return Ok(result);
    }
    if enum_operand(builder, left, span)? || enum_operand(builder, right, span)? {
        return unit_enum_comparison(builder, operator, left, right, span);
    }
    if let Some(result) = structural_comparison(builder, operator, left, right, span)? {
        return Ok(RvalueKind::Use(result));
    }
    native_binary(builder, operator, left, right, span)
}

/// Select a structural comparison in the checked order: frozen, collection, tuple and newtype equality, then text,
/// then tuple ordering.
fn structural_comparison(
    builder: &mut Builder,
    operator: BinOp,
    left: &SourceOperand,
    right: &SourceOperand,
    span: &SourceSpan,
) -> Result<Option<Operand>, String> {
    if operator == BinOp::Eq || operator == BinOp::Ne {
        if let Some(result) = compare_frozen(builder, &operator, left, right, span)? {
            return Ok(Some(result));
        }
        if let Some(result) = collection_equality(builder, operator, left, right, span)? {
            return Ok(Some(result));
        }
        if let Some(result) = tuple_comparison(builder, operator, left, right, span)? {
            return Ok(Some(result));
        }
        if let Some(result) = newtype_equality(builder, operator, left, right, span)? {
            return Ok(Some(result));
        }
    }
    if let Some(result) = text_comparison(builder, operator, left, right, span)? {
        return Ok(Some(result));
    }
    if is_ordering(operator) {
        return tuple_comparison(builder, operator, left, right, span);
    }
    Ok(None)
}

/// A native scalar binary operation over two operands of one numeric, boolean or decimal type.
fn native_binary(
    builder: &mut Builder,
    operator: BinOp,
    left: &SourceOperand,
    right: &SourceOperand,
    span: &SourceSpan,
) -> Result<RvalueKind, String> {
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
        return Ok(RvalueKind::Use(decimal_comparison(
            builder,
            operator,
            left_operand,
            right_operand,
            span,
        )?));
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

/// Select the four ordering comparisons.
fn is_ordering(operator: BinOp) -> bool {
    matches!(operator, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge)
}

/// The native trait method an aggregate comparison calls; equality and inequality both call `eq`.
fn ordering_path(operator: BinOp) -> &'static str {
    match operator {
        BinOp::Lt => "core::cmp::PartialOrd::lt",
        BinOp::Le => "core::cmp::PartialOrd::le",
        BinOp::Gt => "core::cmp::PartialOrd::gt",
        BinOp::Ge => "core::cmp::PartialOrd::ge",
        _ => "core::cmp::PartialEq::eq",
    }
}

/// Negate a native equality result for inequality.
fn negated_result(builder: &mut Builder, output: Place, span: &SourceSpan) -> Operand {
    let negated = temporary(builder, PlanType::Bool, span);
    builder.emit(plan_assign(
        negated.clone(),
        RvalueKind::Unary(UnaryOp::Not, read_place(output, false, span)),
        span,
    ));
    read_place(negated, false, span)
}

/// Select structural comparison for payload enums and exact native carriers; true preserves Rust's own ne.
fn external_enum_comparison_kind(builder: &Builder, owner: i64, operator: BinOp) -> Result<Option<bool>, String> {
    if owner < 0 {
        return Err("invalid Body IR enum comparison owner".into());
    }
    let equality = operator == BinOp::Eq || operator == BinOp::Ne;
    let Some(declaration) = builder.enums().get(owner as usize) else {
        let index = carrier_position(owner);
        let Some(intrinsic) = usize::try_from(index)
            .ok()
            .and_then(|slot| builder.intrinsics.get(slot))
        else {
            return Err("invalid Body IR intrinsic enum comparison owner".into());
        };
        if intrinsic.carrier != Carrier::Declared && equality {
            return Ok(Some(true));
        }
        return Ok(None);
    };
    if native_enum_owner(builder, owner) {
        return Ok(if equality { Some(true) } else { None });
    }
    if declaration.variants.iter().all(|variant| variant.fields.is_empty()) {
        return Ok(None);
    }
    if equality {
        if !declaration.derives.iter().any(|derive| derive == "PartialEq") {
            return Err("payload enum comparison requires a checked PartialEq implementation".into());
        }
        return Ok(Some(false));
    }
    if is_ordering(operator) {
        if !declaration
            .derives
            .iter()
            .any(|derive| derive == "PartialOrd" || derive == "Ord")
        {
            return Err("payload enum ordering requires a checked PartialOrd implementation".into());
        }
        return Ok(Some(false));
    }
    Ok(None)
}

/// Borrow payload enums and native carriers for the structural comparison selected by their checked derives.
fn native_enum_comparison(
    builder: &mut Builder,
    operator: BinOp,
    left: &Place,
    right: &Place,
    span: &SourceSpan,
) -> Result<Option<Operand>, String> {
    let equality = operator == BinOp::Eq || operator == BinOp::Ne;
    if !equality && !is_ordering(operator) {
        return Ok(None);
    }
    let ty = place_type(&builder.locals, left);
    if ty != place_type(&builder.locals, right) {
        if equality && let Some(result) = optional_text_equality(builder, left, right, operator == BinOp::Ne, span)? {
            return Ok(Some(result));
        }
        return Err("unsupported Body IR mixed enum comparison".into());
    }
    let PlanType::Enum(owner, name) = &ty else {
        return Ok(None);
    };
    let reference = PlanType::EnumRef(*owner, name.clone());
    let Some(native) = external_enum_comparison_kind(builder, *owner, operator)? else {
        return Ok(None);
    };
    let mut operands = Vec::with_capacity(2);
    for source in [left, right] {
        let borrowed = temporary(builder, reference.clone(), span);
        builder.emit(plan_assign(borrowed.clone(), RvalueKind::Borrow(source.clone()), span));
        operands.push(read_place(borrowed, false, span));
    }
    let path = if native && operator == BinOp::Ne {
        "core::cmp::PartialEq::ne"
    } else {
        ordering_path(operator)
    };
    builder.externals.push(ExternalFunction {
        path: path.to_owned(),
        type_arguments: vec![ty.clone(), ty.clone()],
        parameters: vec![reference.clone(), reference],
        return_type: PlanType::Bool,
        span: span.clone(),
    });
    let output = temporary(builder, PlanType::Bool, span);
    terminate_call(
        builder,
        CalleeKind::Instantiated(path.to_owned(), vec![ty.clone(), ty]),
        operands,
        output.clone(),
        span,
    );
    if operator == BinOp::Ne && !native {
        return Ok(Some(negated_result(builder, output, span)));
    }
    Ok(Some(read_place(output, false, span)))
}

/// Compare payload carriers through their checked structural trait, or fieldless source tags without consuming either
/// receiver.
///
/// Fieldless ordering compares the tags, whose source variant indices define the derived order.
fn unit_enum_comparison(
    builder: &mut Builder,
    operator: BinOp,
    left: &SourceOperand,
    right: &SourceOperand,
    span: &SourceSpan,
) -> Result<RvalueKind, String> {
    if operator != BinOp::Eq && operator != BinOp::Ne && !is_ordering(operator) {
        return Err("unsupported Body IR enum BinaryOp".into());
    }
    let left_place = enum_comparison_place(builder, left, span)?;
    let right_place = enum_comparison_place(builder, right, span)?;
    if let Some(result) = native_enum_comparison(builder, operator, &left_place, &right_place, span)? {
        return Ok(RvalueKind::Use(result));
    }
    let left_type = place_type(&builder.locals, &left_place);
    let right_type = place_type(&builder.locals, &right_place);
    validate_unit_comparison(builder.enums(), &left_type, &right_type)?;
    if is_ordering(operator) {
        validate_unit_ordering(builder.enums(), &left_type)?;
    }
    let left_tag = enum_comparison_tag(builder, &left_place, span);
    let right_tag = enum_comparison_tag(builder, &right_place, span);
    Ok(RvalueKind::Binary(
        binary_operator(operator)?,
        read_place(left_tag, false, span),
        read_place(right_tag, false, span),
    ))
}

/// Compare a checked Option against None by its tag without copying or equating its payload.
fn optional_none_comparison(
    builder: &mut Builder,
    operator: BinOp,
    left: &SourceOperand,
    right: &SourceOperand,
    span: &SourceSpan,
) -> Result<Option<RvalueKind>, String> {
    if !matches!(operator, BinOp::Eq | BinOp::Ne | BinOp::Is | BinOp::IsNot) {
        return Ok(None);
    }
    let other = if none_constant(right) {
        left
    } else if none_constant(left) {
        right
    } else {
        return Ok(None);
    };
    let equal = operator == BinOp::Eq || operator == BinOp::Is;
    if none_constant(other) {
        return Ok(Some(RvalueKind::Use(literal(Constant::Bool(equal), span))));
    }
    let source = enum_comparison_place(builder, other, span)?;
    let ty = place_type(&builder.locals, &source);
    if ty == PlanType::Unit {
        return Ok(Some(RvalueKind::Use(literal(Constant::Bool(equal), span))));
    }
    let (_, declaration) = carrier_layout(builder, &ty)?;
    if declaration.carrier != Carrier::Option {
        return Err("invalid Body IR None comparison carrier".into());
    }
    let tag = enum_comparison_tag(builder, &source, span);
    let comparison = if equal { BinaryOp::Equal } else { BinaryOp::NotEqual };
    Ok(Some(RvalueKind::Binary(
        comparison,
        read_place(tag, false, span),
        literal(Constant::Int(0), span),
    )))
}

/// Recognize only the retained None constant; ordinary unit values cannot authorize an Option tag comparison.
fn none_constant(value: &SourceOperand) -> bool {
    matches!(value, SourceOperand::Constant(SourceConstant::None))
}

/// Retain an indexed checked value for a shared discriminant read; structural values keep their place.
fn enum_comparison_place(builder: &mut Builder, operand: &SourceOperand, span: &SourceSpan) -> Result<Place, String> {
    let SourceOperand::Place(read) = operand else {
        return Err("unsupported Body IR nonplace enum comparison".into());
    };
    if is_element_place(builder, &read.place)? {
        if read.fact == OwnershipFact::Borrow
            && let Some(place) = shared_element_place(builder, &read.place, span)?
        {
            return Ok(place);
        }
        return materialized_receiver(builder, &read.place, span);
    }
    scalar_place(builder, &read.place, span)
}

/// Materialize one native discriminant as the integer tag type used by checked match dispatch.
fn enum_comparison_tag(builder: &mut Builder, source: &Place, span: &SourceSpan) -> Place {
    let raw = temporary(builder, PlanType::EnumTag, span);
    let tag = temporary(builder, PlanType::Int, span);
    builder.emit(plan_assign(raw.clone(), RvalueKind::Discriminant(source.clone()), span));
    builder.emit(plan_assign(
        tag.clone(),
        RvalueKind::TagToInt(read_place(raw, false, span)),
        span,
    ));
    tag
}

/// Borrow checked collection storage, materializing a byte literal or an indexed or sliced operand once before its
/// shared read. Callers classify both operands first, so only owners, projections and byte literals arrive here.
fn comparison_collection_place(
    builder: &mut Builder,
    value: &SourceOperand,
    span: &SourceSpan,
) -> Result<Place, String> {
    let read = match value {
        SourceOperand::Constant(constant) => return bytes_literal(builder, constant, span),
        SourceOperand::Place(read) => read,
    };
    if read.fact != OwnershipFact::Borrow {
        return Err("unsupported Body IR collection equality without retained shared read".into());
    }
    if is_element_place(builder, &read.place)? {
        if let Some(place) = shared_element_place(builder, &read.place, span)? {
            return Ok(place);
        }
        return materialized_receiver(builder, &read.place, span);
    }
    let place = scalar_place(builder, &read.place, span)?;
    let ty = place_type(&builder.locals, &place);
    if !read.place.projection.is_empty() && equality_shape(&ty).as_ref() != Some(&ty) {
        return Err("unsupported Body IR projected collection reference equality".into());
    }
    Ok(place)
}

/// Read a comparison operand's carrier without evaluating it; a byte literal is an owned unsigned byte list.
fn comparison_operand_type(
    builder: &mut Builder,
    value: &SourceOperand,
    span: &SourceSpan,
) -> Result<PlanType, String> {
    match value {
        SourceOperand::Constant(_) => Ok(PlanType::List(ListLeaf::U8, 1)),
        SourceOperand::Place(_) => source_operand_type(builder, value, span),
    }
}

/// Classify both checked layouts before evaluating either operand, then compare their shared collection views.
fn collection_equality(
    builder: &mut Builder,
    operator: BinOp,
    left: &SourceOperand,
    right: &SourceOperand,
    span: &SourceSpan,
) -> Result<Option<Operand>, String> {
    if !local_comparison_operand(left) || !local_comparison_operand(right) {
        return Ok(None);
    }
    let left_type = comparison_operand_type(builder, left, span)?;
    let left_shape = equality_shape(&left_type);
    let right_type = comparison_operand_type(builder, right, span)?;
    let right_shape = equality_shape(&right_type);
    let (Some(first), Some(second)) = (left_shape, right_shape) else {
        return Ok(None);
    };
    let unequal = operator == BinOp::Ne;
    if supports_frozen_list_equality(&first, &second) {
        let first_place = comparison_collection_place(builder, left, span)?;
        let second_place = comparison_collection_place(builder, right, span)?;
        return Ok(Some(frozen_list_equality(
            builder,
            first_place,
            second_place,
            unequal,
            span,
        )?));
    }
    if first != second {
        return Err("unsupported Body IR collection equality across different layouts".into());
    }
    let left_place = comparison_collection_place(builder, left, span)?;
    let right_place = comparison_collection_place(builder, right, span)?;
    let references = vec![
        list_borrow(builder, left_place, false, span)?,
        list_borrow(builder, right_place, false, span)?,
    ];
    Ok(Some(emit_collection_equality(
        builder, &first, references, unequal, span,
    )?))
}

/// Compare matching structural tuples through their native comparison traits without consuming fields.
///
/// 1. Require the same checked tuple layout on both sides.
/// 2. Borrow each operand once, retaining model-field and parameter storage.
/// 3. Call native equality or ordering and negate equality for inequality.
fn tuple_comparison(
    builder: &mut Builder,
    operator: BinOp,
    left: &SourceOperand,
    right: &SourceOperand,
    span: &SourceSpan,
) -> Result<Option<Operand>, String> {
    // 1. Scalars and other aggregate kinds keep their own comparison paths.
    if !local_comparison_operand(left) || !local_comparison_operand(right) {
        return Ok(None);
    }
    let left_type = source_operand_type(builder, left, span)?;
    let Some((owned, reference)) = tuple_comparison_layout(&left_type) else {
        return Ok(None);
    };
    let right_type = source_operand_type(builder, right, span)?;
    if tuple_comparison_layout(&right_type) != Some((owned.clone(), reference.clone())) {
        return Err("unsupported Body IR tuple comparison across different layouts".into());
    }
    // 2. The shared argument boundary preserves existing references and borrows owned tuple fields.
    let operands = vec![
        shared_nominal_argument(builder, left, &reference, span)?,
        shared_nominal_argument(builder, right, &reference, span)?,
    ];
    // 3. Aggregate comparison is a trait call, never a primitive MIR binary operation.
    let output = temporary(builder, PlanType::Bool, span);
    instantiated_external_call(
        builder,
        ordering_path(operator),
        vec![owned.clone(), owned],
        vec![reference.clone(), reference],
        PlanType::Bool,
        operands,
        output.clone(),
        span,
    )?;
    if operator == BinOp::Ne {
        return Ok(Some(negated_result(builder, output, span)));
    }
    Ok(Some(read_place(output, false, span)))
}

/// Normalize tuple storage and its shared view while preserving every structural field carrier.
fn tuple_comparison_layout(ty: &PlanType) -> Option<(PlanType, PlanType)> {
    match ty {
        PlanType::Tuple(fields) | PlanType::TupleRef(fields) | PlanType::TupleMutRef(fields) => {
            Some((PlanType::Tuple(fields.clone()), PlanType::TupleRef(fields.clone())))
        }
        _ => None,
    }
}

/// Recognize checked local and byte-literal comparison operands without collecting borrowed values into owned storage.
fn local_comparison_operand(value: &SourceOperand) -> bool {
    match value {
        SourceOperand::Place(read) => read.place.local_id().is_some(),
        SourceOperand::Constant(SourceConstant::Bytes(_)) => true,
        SourceOperand::Constant(_) => false,
    }
}

/// Compare a source or linked newtype through its retained PartialEq selection, which native metadata verifies.
fn newtype_equality(
    builder: &mut Builder,
    operator: BinOp,
    left: &SourceOperand,
    right: &SourceOperand,
    span: &SourceSpan,
) -> Result<Option<Operand>, String> {
    for value in [left, right] {
        match value {
            SourceOperand::Place(_) | SourceOperand::Constant(SourceConstant::NumericNewtype(_)) => {}
            SourceOperand::Constant(_) => return Ok(None),
        }
    }
    let ty = source_operand_type(builder, left, span)?;
    let PlanType::Model(owner, name) = &ty else {
        return Ok(None);
    };
    let reference = PlanType::ModelRef(*owner, name.clone());
    let Some(declaration) = usize::try_from(*owner)
        .ok()
        .and_then(|slot| builder.nominals().get(slot))
    else {
        return Err("unsupported Body IR equality without retained nominal declaration".into());
    };
    if declaration.canonical.kind != SemanticSourceTargetKind::Newtype {
        return Ok(None);
    }
    if !declaration.derives.iter().any(|derive| derive == "PartialEq") {
        return Err("unsupported Body IR newtype equality without retained PartialEq".into());
    }
    if source_operand_type(builder, right, span)? != ty {
        return Err("unsupported Body IR equality between different newtypes".into());
    }
    let operands = vec![
        newtype_comparison_operand(builder, left, &ty, &reference, span)?,
        newtype_comparison_operand(builder, right, &ty, &reference, span)?,
    ];
    let path = "core::cmp::PartialEq::eq";
    builder.externals.push(ExternalFunction {
        path: path.to_owned(),
        type_arguments: vec![ty.clone(), ty.clone()],
        parameters: vec![reference.clone(), reference],
        return_type: PlanType::Bool,
        span: span.clone(),
    });
    let output = temporary(builder, PlanType::Bool, span);
    terminate_call(
        builder,
        CalleeKind::Instantiated(path.to_owned(), vec![ty.clone(), ty]),
        operands,
        output.clone(),
        span,
    );
    if operator == BinOp::Ne {
        return Ok(Some(negated_result(builder, output, span)));
    }
    Ok(Some(read_place(output, false, span)))
}

/// Materialize one checked newtype operand and borrow its retained carrier for equality.
fn newtype_comparison_operand(
    builder: &mut Builder,
    value: &SourceOperand,
    ty: &PlanType,
    reference: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let selected = value_operand(builder, value, span)?;
    let source = match selected.kind {
        OperandKind::Move(place) | OperandKind::Copy(place) => place,
        OperandKind::Literal(_) => return Err("unsupported Body IR newtype equality without an owned place".into()),
    };
    if place_type(&builder.locals, &source) != *ty {
        return Err("unsupported Body IR newtype equality carrier mismatch".into());
    }
    let borrowed = temporary(builder, reference.clone(), span);
    builder.emit(plan_assign(borrowed.clone(), RvalueKind::Borrow(source), span));
    Ok(read_place(borrowed, false, span))
}

/// Classify text from retained storage, including indexed elements, before materializing either comparison value.
fn text_comparison_operand(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<bool, String> {
    let ty = source_operand_type(builder, value, span)?;
    Ok(matches!(
        ty,
        PlanType::String | PlanType::StringRef | PlanType::StrRef | PlanType::FrozenStr
    ))
}

/// Bind a checked comparison in a closed text instance to the same borrowed runtime operation as concrete text.
fn text_comparison(
    builder: &mut Builder,
    operator: BinOp,
    left: &SourceOperand,
    right: &SourceOperand,
    span: &SourceSpan,
) -> Result<Option<Operand>, String> {
    let name = match operator {
        BinOp::Eq => "equal_text",
        BinOp::Ne => "unequal_text",
        BinOp::Lt => "less_text",
        BinOp::Le => "less_equal_text",
        BinOp::Gt => "greater_text",
        BinOp::Ge => "greater_equal_text",
        _ => return Ok(None),
    };
    if !text_comparison_operand(builder, left, span)? || !text_comparison_operand(builder, right, span)? {
        return Ok(None);
    }
    let incoming = builder.owned.clone();
    let operands = vec![
        borrow_text_operand(builder, left, span)?,
        borrow_text_operand(builder, right, span)?,
    ];
    let output = emit_text_helper(
        builder,
        name,
        PlanType::Bool,
        vec![PlanType::StringRef, PlanType::StringRef],
        operands,
        span,
    );
    drop_borrowed_text_temporaries(builder, &incoming, output.local, span);
    Ok(Some(read_place(output, false, span)))
}
