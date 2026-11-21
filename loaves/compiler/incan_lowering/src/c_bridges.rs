//! Lower checked C bridge operations without reopening source names, initialization state, or lifetimes.

use incan_mir_plan_native::{
    BinaryOp, Constant, ListLeaf, Operand, OperandKind, Place, PlanType, Projection, RvalueKind, SourceSpan,
    TerminatorKind, TupleElement, assign as plan_assign,
};
use incan_semantics_core::body_ir::{
    ArgumentElement, CheckedBridgeCall, CheckedBridgeOperation, Operand as SourceOperand, Place as SourcePlace,
};

use crate::builtin_calls::coerce_numeric_operand;
use crate::builtins::list_call;
use crate::checked_values::{scalar_operand, scalar_place};
use crate::enums::enum_owner;
use crate::frame::{Builder, drop_owned, external_call, forget_owned, literal, place_type, read_place, temporary};
use crate::layout_types::native_source_type;
use crate::list_values::{append_list_value, list_borrow, value_operand};
use crate::lists::{list_element, require_list_shape};
use crate::operand_borrows::borrow_slice_operand;
use crate::options::assign_carrier;
use crate::sequence_builtins::text_argument;
use crate::text_plan::owned_literal;
use crate::text_primitives::decode_utf8 as primitive_decode_utf8;
use crate::tuples::tuple_fields;

/// Preserve the checked bridge operation and exact native storage.
///
/// 1. Select its retained result place.
/// 2. Allocate or read only the slot authorized by the checker.
pub fn lower_checked_bridge(
    builder: &mut Builder,
    target: &CheckedBridgeCall,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    // 1. Every storage choice was retained by frontend projection.
    let ty = native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        &target.return_type,
        &builder.intrinsics,
        "",
    )?;
    let mut output = temporary(builder, ty.clone(), span);
    if let Some(place) = destination {
        output = scalar_place(builder, place, span)?;
    }
    // 2. A private slot is a handle to one allocation, never a copied scalar masquerading as a handle.
    match target.operation {
        CheckedBridgeOperation::NewOutputSlot => new_slot(builder, arguments, output, &ty, false, span),
        CheckedBridgeOperation::NewInOutSlot => new_slot(builder, arguments, output, &ty, true, span),
        CheckedBridgeOperation::TakeOutputSlot => take_slot(builder, arguments, output, ty, span),
        CheckedBridgeOperation::MakeCString => make_string(builder, arguments, output, ty, span),
        CheckedBridgeOperation::CStringPointer => string_pointer(builder, arguments, output, ty, span),
        CheckedBridgeOperation::CopyUtf8 => copy_utf8(builder, arguments, output, &ty, span),
        CheckedBridgeOperation::MakeSpan => make_span(builder, arguments, output, span),
        CheckedBridgeOperation::SpanConstPointer => span_pointer(builder, arguments, output, ty, false, span),
        CheckedBridgeOperation::SpanMutPointer => span_pointer(builder, arguments, output, ty, true, span),
        CheckedBridgeOperation::SpanCount => span_count(builder, arguments, output, span),
        CheckedBridgeOperation::FinishSpan => finish_span(builder, arguments, output, &ty, span),
    }
}

/// Read one checked, already evaluated bridge operand without adding a passing conversion.
fn argument(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    index: usize,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let Some(element) = arguments.get(index) else {
        return Err("invalid checked C bridge argument count".into());
    };
    match element.as_one() {
        Some(value) => scalar_operand(builder, value, span),
        None => Err("invalid checked C bridge spread".into()),
    }
}

/// Allocate private scalar storage; physical zeroing grants no semantic Out initialization permission.
fn new_slot(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    output: Place,
    ty: &PlanType,
    initialized: bool,
    span: &SourceSpan,
) -> Result<(), String> {
    let value_type = slot_value_type(ty)?;
    let storage = temporary(builder, value_type.clone(), span);
    if initialized {
        let supplied = argument(builder, arguments, 0, span)?;
        let initial = coerce_numeric_operand(builder, supplied, &value_type, span)?;
        builder.emit(plan_assign(storage.clone(), RvalueKind::Use(initial), span));
    } else {
        if !arguments.is_empty() {
            return Err("invalid checked C fresh output arguments".into());
        }
        let zero = literal(Constant::Int(0), span);
        let cast = RvalueKind::NumericCast(zero, PlanType::Int, value_type);
        builder.emit(plan_assign(storage.clone(), cast, span));
    }
    builder.emit(plan_assign(output, RvalueKind::RawAddressOf(storage, true), span));
    Ok(())
}

/// Read the exact scalar stored behind the checker's mutable output handle.
fn slot_value_type(ty: &PlanType) -> Result<PlanType, String> {
    match ty {
        PlanType::RawPointer(true, elements) => match elements.as_slice() {
            [element] => Ok(element.clone()),
            _ => Err("invalid checked C slot pointee".into()),
        },
        _ => Err("invalid checked C slot handle".into()),
    }
}

/// Read a slot only after its initialized-state proof was retained as this exact operation.
fn take_slot(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    output: Place,
    ty: PlanType,
    span: &SourceSpan,
) -> Result<(), String> {
    let pointer = argument(builder, arguments, 0, span)?;
    let source = pointer_place(pointer)?;
    if source.projection != Projection::Whole {
        return Err("unsupported projected checked C slot handle".into());
    }
    let value = deref_place(&source, ty, span);
    builder.emit(plan_assign(
        output,
        RvalueKind::Use(read_place(value, false, span)),
        span,
    ));
    Ok(())
}

/// Read the physical place of a checked pointer or slice loan without a fallback allocation.
fn pointer_place(value: Operand) -> Result<Place, String> {
    match value.kind {
        OperandKind::Copy(place) | OperandKind::Move(place) => Ok(place),
        _ => Err("invalid checked C bridge pointer or slice loan".into()),
    }
}

/// The referent of a whole pointer or loan local.
fn deref_place(source: &Place, referent: PlanType, span: &SourceSpan) -> Place {
    Place {
        local: source.local,
        projection: Projection::Deref(referent),
        span: span.clone(),
    }
}

/// Validate and allocate owned terminated storage before a foreign pointer can be produced.
fn make_string(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    output: Place,
    ty: PlanType,
    span: &SourceSpan,
) -> Result<(), String> {
    let text = text_argument(builder, arguments, 0, span)?;
    let local = output.local;
    let path = "incan_stdlib_primitives::nul_terminated_utf8";
    external_call(builder, path, vec![PlanType::StringRef], ty, vec![text], output, span);
    builder.owned.push(local);
    Ok(())
}

/// Expose the checked invocation-long byte buffer loan without consuming or reallocating its owner.
fn string_pointer(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    output: Place,
    ty: PlanType,
    span: &SourceSpan,
) -> Result<(), String> {
    let view = buffer_loan(builder, arguments, span)?;
    lend_pointer(builder, view, output, ty, PlanType::U8, false, span)
}

/// Point at a slice loan's elements and cast that raw pointer to the checked carrier one bounded call receives.
fn lend_pointer(
    builder: &mut Builder,
    view: Operand,
    output: Place,
    ty: PlanType,
    element: PlanType,
    mutable: bool,
    span: &SourceSpan,
) -> Result<(), String> {
    let source = pointer_place(view)?;
    let slice_type = PlanType::NativeSlice(vec![element]);
    let borrowed = deref_place(&source, slice_type.clone(), span);
    let pointer = temporary(builder, PlanType::RawPointer(mutable, vec![slice_type]), span);
    builder.emit(plan_assign(
        pointer.clone(),
        RvalueKind::RawAddressOf(borrowed, mutable),
        span,
    ));
    let cast = RvalueKind::PointerCast(read_place(pointer, false, span), ty);
    builder.emit(plan_assign(output, cast, span));
    Ok(())
}

/// Borrow the single checked C string buffer through its retained ownership fact.
fn buffer_loan(builder: &mut Builder, arguments: &[ArgumentElement], span: &SourceSpan) -> Result<Operand, String> {
    let [element] = arguments else {
        return Err("invalid checked C string pointer arguments".into());
    };
    match element.as_one() {
        Some(value) => borrow_slice_operand(builder, value, &[PlanType::U8], false, span),
        None => Err("invalid checked C string pointer spread".into()),
    }
}

/// The blocks of one bounded UTF-8 copy.
struct ScanBlocks {
    header: i64,
    inspect: i64,
    append: i64,
    found: i64,
    missing: i64,
    null_view: i64,
    invalid_bound: i64,
    after: i64,
}

/// Copy only bytes preceding a terminator within the caller's checked bound.
///
/// 1. Preserve the raw view and explicit bound.
/// 2. Scan one byte per bounded iteration into owned storage.
/// 3. Validate UTF-8 or return the bounded-view error.
fn copy_utf8(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    output: Place,
    ty: &PlanType,
    span: &SourceSpan,
) -> Result<(), String> {
    // 1. The checker retains the unsafe scope and scoped-view lifetime.
    let pointer = argument(builder, arguments, 0, span)?;
    let bound = argument(builder, arguments, 1, span)?;
    let pointer_type = PlanType::RawPointer(false, vec![PlanType::U8]);
    let base = temporary(builder, pointer_type.clone(), span);
    builder.emit(plan_assign(
        base.clone(),
        RvalueKind::PointerCast(pointer, pointer_type),
        span,
    ));
    let bytes_type = PlanType::List(ListLeaf::U8, 1);
    let copied = temporary(builder, bytes_type.clone(), span);
    list_call(
        builder,
        "empty_list",
        PlanType::U8,
        Vec::new(),
        bytes_type,
        Vec::new(),
        copied.clone(),
        span,
    );
    let index = temporary(builder, PlanType::Int, span);
    builder.emit(plan_assign(
        index.clone(),
        RvalueKind::Use(literal(Constant::Int(0), span)),
        span,
    ));
    let blocks = ScanBlocks {
        header: builder.new_block(span),
        inspect: builder.new_block(span),
        append: builder.new_block(span),
        found: builder.new_block(span),
        missing: builder.new_block(span),
        null_view: builder.new_block(span),
        invalid_bound: builder.new_block(span),
        after: builder.new_block(span),
    };
    begin_scan(builder, &base, &bound, &blocks, span);
    // 2. The bound is checked before every read; a terminator stops reads immediately.
    scan_until_terminator(builder, &base, &index, bound, &copied, &blocks, span)?;
    // 3. Both outcomes initialize one ordinary owned Result and release the temporary buffer.
    builder.select(blocks.found);
    decoded_result(builder, &copied, &output, ty, blocks.after, span)?;
    builder.select(blocks.missing);
    refuse_result(
        builder,
        &output,
        ty,
        "C string has no terminator within max_bytes",
        blocks.after,
        span,
    )?;
    builder.select(blocks.null_view);
    refuse_result(builder, &output, ty, "C string pointer is null", blocks.after, span)?;
    builder.select(blocks.invalid_bound);
    refuse_result(
        builder,
        &output,
        ty,
        "max_bytes is not a valid bound",
        blocks.after,
        span,
    )?;
    builder.select(blocks.after);
    drop_owned(builder, copied.local, span);
    Ok(())
}

/// The bounded loop: stop at the bound, stop at a terminator, otherwise append the byte and advance.
fn scan_until_terminator(
    builder: &mut Builder,
    base: &Place,
    index: &Place,
    bound: Operand,
    copied: &Place,
    blocks: &ScanBlocks,
    span: &SourceSpan,
) -> Result<(), String> {
    builder.select(blocks.header);
    let within = temporary(builder, PlanType::Bool, span);
    let less = RvalueKind::Binary(BinaryOp::Less, read_place(index.clone(), false, span), bound);
    builder.emit(plan_assign(within.clone(), less, span));
    let test = read_place(within, false, span);
    builder.finish(TerminatorKind::SwitchBool(test, blocks.missing, blocks.inspect), span);
    builder.select(blocks.inspect);
    let byte = scan_byte(builder, base, index, span);
    let terminated = temporary(builder, PlanType::Bool, span);
    let zero = temporary(builder, PlanType::U8, span);
    let cast = RvalueKind::NumericCast(literal(Constant::Int(0), span), PlanType::Int, PlanType::U8);
    builder.emit(plan_assign(zero.clone(), cast, span));
    let equal = RvalueKind::Binary(
        BinaryOp::Equal,
        read_place(byte.clone(), false, span),
        read_place(zero, false, span),
    );
    builder.emit(plan_assign(terminated.clone(), equal, span));
    let test = read_place(terminated, false, span);
    builder.finish(TerminatorKind::SwitchBool(test, blocks.append, blocks.found), span);
    builder.select(blocks.append);
    append_list_value(builder, copied, read_place(byte, false, span), &ListLeaf::U8, 1, span)?;
    increment_index(builder, index, span);
    builder.finish(TerminatorKind::Goto(blocks.header), span);
    Ok(())
}

/// Decode the copied bytes and initialize `Ok(text)`, or `Err(message)` for invalid UTF-8, then continue at `after`.
fn decoded_result(
    builder: &mut Builder,
    copied: &Place,
    output: &Place,
    ty: &PlanType,
    after: i64,
    span: &SourceSpan,
) -> Result<(), String> {
    let reference = list_borrow(builder, copied.clone(), false, span)?;
    let replace = literal(Constant::Bool(false), span);
    let decoded = primitive_decode_utf8(builder, reference, PlanType::ListRef(ListLeaf::U8, 1), replace, span);
    let decoded_type = PlanType::Tuple(vec![TupleElement::Bool, TupleElement::String]);
    let fields = tuple_fields(&decoded, &decoded_type, span)?;
    let (Some(failed), Some(text)) = (fields.first(), fields.get(1)) else {
        return Err("invalid checked C UTF-8 decode result".into());
    };
    let invalid_utf8 = builder.new_block(span);
    let valid_utf8 = builder.new_block(span);
    let decoded_after = builder.new_block(span);
    let test = read_place(failed.clone(), false, span);
    builder.finish(TerminatorKind::SwitchBool(test, invalid_utf8, valid_utf8), span);
    forget_owned(builder, decoded.local);
    let decoded_owners = builder.owned.clone();
    let owner = enum_owner(ty)?;
    builder.select(invalid_utf8);
    assign_carrier(
        builder,
        output.clone(),
        owner,
        1,
        vec![read_place(text.clone(), true, span)],
        span,
    );
    builder.finish(TerminatorKind::Goto(decoded_after), span);
    let decoded_retained = builder.owned.clone();
    builder.select(valid_utf8);
    builder.owned = decoded_owners;
    assign_carrier(
        builder,
        output.clone(),
        owner,
        0,
        vec![read_place(text.clone(), true, span)],
        span,
    );
    builder.finish(TerminatorKind::Goto(decoded_after), span);
    builder.select(decoded_after);
    builder.owned = decoded_retained;
    builder.finish(TerminatorKind::Goto(after), span);
    Ok(())
}

/// Initialize a checked bridge result's `Err` with one refusal message, then continue after the operation.
fn refuse_result(
    builder: &mut Builder,
    output: &Place,
    ty: &PlanType,
    message: &str,
    after: i64,
    span: &SourceSpan,
) -> Result<(), String> {
    let error = owned_literal(builder, message, span);
    let owner = enum_owner(ty)?;
    assign_carrier(
        builder,
        output.clone(),
        owner,
        1,
        vec![read_place(error, true, span)],
        span,
    );
    builder.finish(TerminatorKind::Goto(after), span);
    Ok(())
}

/// Refuse a null view, and a bound that is not positive or not a target pointer offset, before any read.
fn begin_scan(builder: &mut Builder, base: &Place, bound: &Operand, blocks: &ScanBlocks, span: &SourceSpan) {
    let address = temporary(builder, PlanType::USize, span);
    let pointer_address = RvalueKind::PointerAddress(read_place(base.clone(), false, span));
    builder.emit(plan_assign(address.clone(), pointer_address, span));
    let zero = temporary(builder, PlanType::USize, span);
    let cast = RvalueKind::NumericCast(literal(Constant::Int(0), span), PlanType::Int, PlanType::USize);
    builder.emit(plan_assign(zero.clone(), cast, span));
    let nonnull = temporary(builder, PlanType::Bool, span);
    let not_equal = RvalueKind::Binary(
        BinaryOp::NotEqual,
        read_place(address, false, span),
        read_place(zero, false, span),
    );
    builder.emit(plan_assign(nonnull.clone(), not_equal, span));
    let check_bound = builder.new_block(span);
    let test = read_place(nonnull, false, span);
    builder.finish(TerminatorKind::SwitchBool(test, blocks.null_view, check_bound), span);
    builder.select(check_bound);
    let positive = temporary(builder, PlanType::Bool, span);
    let no_bytes = literal(Constant::Int(0), span);
    let greater = RvalueKind::Binary(BinaryOp::Greater, bound.clone(), no_bytes);
    builder.emit(plan_assign(positive.clone(), greater, span));
    let check_offset = builder.new_block(span);
    let test = read_place(positive, false, span);
    builder.finish(
        TerminatorKind::SwitchBool(test, blocks.invalid_bound, check_offset),
        span,
    );
    builder.select(check_offset);
    let offset = temporary(builder, PlanType::ISize, span);
    let narrowed = RvalueKind::NumericCast(bound.clone(), PlanType::Int, PlanType::ISize);
    builder.emit(plan_assign(offset.clone(), narrowed, span));
    let restored = temporary(builder, PlanType::Int, span);
    let widened = RvalueKind::NumericCast(read_place(offset, false, span), PlanType::ISize, PlanType::Int);
    builder.emit(plan_assign(restored.clone(), widened, span));
    let fits = temporary(builder, PlanType::Bool, span);
    let equal = RvalueKind::Binary(BinaryOp::Equal, read_place(restored, false, span), bound.clone());
    builder.emit(plan_assign(fits.clone(), equal, span));
    let test = read_place(fits, false, span);
    builder.finish(
        TerminatorKind::SwitchBool(test, blocks.invalid_bound, blocks.header),
        span,
    );
}

/// Form one in-bound pointer and copy its byte without carrying a foreign reference beyond the read.
fn scan_byte(builder: &mut Builder, base: &Place, index: &Place, span: &SourceSpan) -> Place {
    let offset = temporary(builder, PlanType::ISize, span);
    let cast = RvalueKind::NumericCast(read_place(index.clone(), false, span), PlanType::Int, PlanType::ISize);
    builder.emit(plan_assign(offset.clone(), cast, span));
    let base_type = place_type(&builder.locals, base);
    let address = temporary(builder, base_type, span);
    let moved = RvalueKind::PointerOffset(read_place(base.clone(), false, span), read_place(offset, false, span));
    builder.emit(plan_assign(address.clone(), moved, span));
    let source = deref_place(&address, PlanType::U8, span);
    let byte = temporary(builder, PlanType::U8, span);
    builder.emit(plan_assign(
        byte.clone(),
        RvalueKind::Use(read_place(source, false, span)),
        span,
    ));
    byte
}

/// Advance after a successful read; the next loop edge rechecks the caller's bound.
fn increment_index(builder: &mut Builder, index: &Place, span: &SourceSpan) {
    let checked = temporary(builder, PlanType::CheckedInt, span);
    let add = RvalueKind::Binary(
        BinaryOp::Add,
        read_place(index.clone(), false, span),
        literal(Constant::Int(1), span),
    );
    builder.emit(plan_assign(checked.clone(), add, span));
    let value = Place {
        local: checked.local,
        projection: Projection::Value,
        span: span.clone(),
    };
    builder.emit(plan_assign(
        index.clone(),
        RvalueKind::Use(read_place(value, false, span)),
        span,
    ));
}

// Typed spans: a span is its owned element vector; pointers and counts are views of that vector.

/// Move the constructor's owned buffer into the span, reading it once through its checked ownership fact.
fn make_span(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    require_span_operands(arguments, 1, "constructor")?;
    let Some(value) = arguments[0].as_one() else {
        return Err("invalid checked C span constructor spread".into());
    };
    let buffer = value_operand(builder, value, span)?;
    let local = output.local;
    builder.emit(plan_assign(output, RvalueKind::Use(buffer), span));
    builder.owned.push(local);
    Ok(())
}

/// Lend the span's elements as the raw pointer one bounded call receives, without moving or reallocating them.
fn span_pointer(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    output: Place,
    ty: PlanType,
    mutable: bool,
    span: &SourceSpan,
) -> Result<(), String> {
    let element = pointer_element(&ty)?;
    require_span_operands(arguments, 1, "pointer")?;
    let Some(value) = arguments[0].as_one() else {
        return Err("invalid checked C span pointer spread".into());
    };
    let view = borrow_slice_operand(builder, value, std::slice::from_ref(&element), mutable, span)?;
    lend_pointer(builder, view, output, ty, element, mutable, span)
}

/// Read the span's element count, which is also a mutable span's capacity, as the `usize` a C count takes.
fn span_count(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    require_span_operands(arguments, 1, "count")?;
    let buffer = span_buffer(builder, arguments, span)?;
    let count = read_place(buffer_length(builder, buffer, span)?, false, span);
    builder.emit(plan_assign(
        output,
        RvalueKind::NumericCast(count, PlanType::Int, PlanType::USize),
        span,
    ));
    Ok(())
}

/// Keep the first `written` elements of a mutable span, as `into_bytes` and `into_f32s` state.
///
/// 1. Read the span's capacity and the written count.
/// 2. Refuse a negative count or one above the capacity.
/// 3. Copy the written prefix into the `Ok` result; the span's own buffer is released with the span.
fn finish_span(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    output: Place,
    ty: &PlanType,
    span: &SourceSpan,
) -> Result<(), String> {
    // 1. The count is compared in `int`, which every checked integer count converts to, through one copied local.
    require_span_operands(arguments, 2, "finish")?;
    let buffer = span_buffer(builder, arguments, span)?;
    let capacity = buffer_length(builder, buffer.clone(), span)?;
    let count = argument(builder, arguments, 1, span)?;
    let supplied = coerce_numeric_operand(builder, count, &PlanType::Int, span)?;
    let written = temporary(builder, PlanType::Int, span);
    builder.emit(plan_assign(written.clone(), RvalueKind::Use(supplied), span));
    let owner = enum_owner(ty)?;
    let keep = builder.new_block(span);
    let refuse = builder.new_block(span);
    let after = builder.new_block(span);
    // 2. Both bounds are checked before any element is read.
    within_capacity(builder, &written, &capacity, keep, refuse, span);
    builder.select(refuse);
    refuse_result(
        builder,
        &output,
        ty,
        "written count is outside the span capacity",
        after,
        span,
    )?;
    // 3. The prefix is an owned copy, so the result never aliases the span's storage.
    builder.select(keep);
    let kept = written_prefix(builder, buffer, &written, span)?;
    assign_carrier(builder, output, owner, 0, vec![read_place(kept, true, span)], span);
    builder.finish(TerminatorKind::Goto(after), span);
    builder.select(after);
    Ok(())
}

/// Refuse a span operation whose checked operand count is not the operation's own.
fn require_span_operands(arguments: &[ArgumentElement], count: usize, role: &str) -> Result<(), String> {
    if arguments.len() != count {
        return Err(format!("invalid checked C span {role} arguments"));
    }
    Ok(())
}

/// The element a checked span pointer addresses.
fn pointer_element(ty: &PlanType) -> Result<PlanType, String> {
    if let PlanType::RawPointer(_, elements) = ty
        && let [element] = elements.as_slice()
    {
        return Ok(element.clone());
    }
    Err("invalid checked C span pointer carrier".into())
}

/// The span local's own element vector, the operation's first operand; a span is only read through its local.
fn span_buffer(builder: &mut Builder, arguments: &[ArgumentElement], span: &SourceSpan) -> Result<Place, String> {
    match arguments.first().and_then(ArgumentElement::as_one) {
        Some(SourceOperand::Place(read)) => scalar_place(builder, &read.place, span),
        _ => Err("invalid checked C span receiver".into()),
    }
}

/// Read the span vector's length through the runtime without consuming the span.
fn buffer_length(builder: &mut Builder, buffer: Place, span: &SourceSpan) -> Result<Place, String> {
    let (leaf, depth) = require_list_shape(&place_type(&builder.locals, &buffer), "invalid checked C span storage")?;
    let reference = list_borrow(builder, buffer, false, span)?;
    let count = temporary(builder, PlanType::Int, span);
    let element = list_element(&leaf, depth);
    let parameters = vec![PlanType::ListRef(leaf, depth)];
    list_call(
        builder,
        "length_list",
        element,
        parameters,
        PlanType::Int,
        vec![reference],
        count.clone(),
        span,
    );
    Ok(count)
}

/// Branch to `keep` only for `0 <= written <= capacity`, and to `refuse` otherwise.
fn within_capacity(
    builder: &mut Builder,
    written: &Place,
    capacity: &Place,
    keep: i64,
    refuse: i64,
    span: &SourceSpan,
) {
    let count = read_place(written.clone(), false, span);
    let nonnegative = temporary(builder, PlanType::Bool, span);
    let at_least = RvalueKind::Binary(BinaryOp::GreaterEqual, count.clone(), literal(Constant::Int(0), span));
    builder.emit(plan_assign(nonnegative.clone(), at_least, span));
    let check_capacity = builder.new_block(span);
    let test = read_place(nonnegative, false, span);
    builder.finish(TerminatorKind::SwitchBool(test, refuse, check_capacity), span);
    builder.select(check_capacity);
    let fits = temporary(builder, PlanType::Bool, span);
    let at_most = RvalueKind::Binary(BinaryOp::LessEqual, count, read_place(capacity.clone(), false, span));
    builder.emit(plan_assign(fits.clone(), at_most, span));
    let test = read_place(fits, false, span);
    builder.finish(TerminatorKind::SwitchBool(test, refuse, keep), span);
}

/// Copy the first `written` elements of the span vector into a new owned vector.
fn written_prefix(builder: &mut Builder, buffer: Place, written: &Place, span: &SourceSpan) -> Result<Place, String> {
    let (leaf, depth) = require_list_shape(&place_type(&builder.locals, &buffer), "invalid checked C span storage")?;
    let reference = list_borrow(builder, buffer, false, span)?;
    let kept = temporary(builder, PlanType::List(leaf.clone(), depth), span);
    let operands = vec![
        reference,
        literal(Constant::Bool(true), span),
        literal(Constant::Int(0), span),
        literal(Constant::Bool(true), span),
        read_place(written.clone(), false, span),
        literal(Constant::Bool(false), span),
        literal(Constant::Int(1), span),
    ];
    let parameters = vec![
        PlanType::ListRef(leaf.clone(), depth),
        PlanType::Bool,
        PlanType::Int,
        PlanType::Bool,
        PlanType::Int,
        PlanType::Bool,
        PlanType::Int,
    ];
    let element = list_element(&leaf, depth);
    let result = PlanType::List(leaf, depth);
    list_call(
        builder,
        "slice_list",
        element,
        parameters,
        result,
        operands,
        kept.clone(),
        span,
    );
    Ok(kept)
}
