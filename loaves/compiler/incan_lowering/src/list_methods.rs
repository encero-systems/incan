//! Lower checked list arguments, list methods and builtins, and indexed list mutation.

use incan_lang::lang::surface::iterator_methods::IteratorMethodId;
use incan_lang::lang::surface::list_methods::ListMethodId;
use incan_mir_plan_native::{
    CalleeKind, Constant, ExternalFunction, ListLeaf, Operand, OperandKind, Place, PlanType, RvalueKind, SourceSpan,
    UnaryOp, assign as plan_assign,
};
use incan_semantics_core::IncanInputAccess;
use incan_semantics_core::body_ir::{
    ArgumentElement, Constant as SourceConstant, GlobalPlace, Operand as SourceOperand, OwnershipFact,
    Place as SourcePlace, PlaceElem, PlaceOperand, Rvalue,
};

use crate::builtin_methods::lower_generator_member;
use crate::builtins::list_call;
use crate::call_arguments::call_argument_operand;
use crate::calls::instantiated_external_call;
use crate::carrier_operands::{boundary_operand, element_plan_operand};
use crate::checked_values::{scalar_operand, scalar_place};
use crate::collections::is_collection;
use crate::decls::library_item;
use crate::element_borrows::{borrow_element_mut, shared_element_place};
use crate::element_places::is_element_place;
use crate::frame::{
    Builder, drop_owned, external_call, literal, place_type, read_place, temporary, terminate_call, value_type,
};
use crate::frozen_equality::{borrow_frozen_argument, frozen_reference_type};
use crate::generator_adapters::is_generator_adapter;
use crate::generator_consumers::is_generator_consumer;
use crate::generators::{generator_collect, generator_runtime_call, generator_shape};
use crate::hashed_collections::collection_length;
use crate::layout_types::shared_receiver_type;
use crate::list_iteration::{frozen_text_membership, list_root_place, shared_sequence_receiver};
use crate::list_values::{
    checked_list_argument, element_read, list_base_place, list_borrow, list_index_operand, value_operand,
};
use crate::lists::{list_element, list_shape, require_list_shape};
use crate::model_lists::prefix_place;
use crate::operand_borrows::{borrow_slice_operand, list_slice_view, lower_text_helper};
use crate::options::none_operand;
use crate::sequence_builtins::argument_type;
use crate::static_storage::{persist_static_value, read_static};

/// A static receiver's identity and the snapshot a method reads or changes before it is committed.
type StaticReceiver = Option<(GlobalPlace, Place)>;

/// Require one retained local receiver before constructing a runtime borrow.
///
/// 1. Check the argument position.
/// 2. Retain its exact local projection, refusing constants and spreads.
pub fn argument_place(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    index: usize,
    span: &SourceSpan,
) -> Result<Place, String> {
    // 1. Check the argument position.
    let Some(argument) = arguments.get(index) else {
        return Err("invalid Body IR list call arity".into());
    };
    // 2. Retain its exact local projection.
    match argument.as_one() {
        Some(SourceOperand::Place(read)) => list_root_place(builder, &read.place, span),
        Some(_) => Err("unsupported Body IR list receiver constant".into()),
        None => Err("unsupported Body IR list spread argument".into()),
    }
}

/// Like `argument_place` for a receiver read only through a shared borrow, retaining indexed storage in place.
pub fn shared_receiver_place(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    index: usize,
    span: &SourceSpan,
) -> Result<Place, String> {
    let Some(argument) = arguments.get(index) else {
        return Err("invalid Body IR list call arity".into());
    };
    if let Some(SourceOperand::Place(read)) = argument.as_one()
        && read.fact == OwnershipFact::Borrow
        && is_element_place(builder, &read.place)?
    {
        return match shared_element_place(builder, &read.place, span)? {
            Some(place) => Ok(place),
            None => materialized_receiver(builder, &read.place, span),
        };
    }
    argument_place(builder, arguments, index, span)
}

/// Read a collection element once into temporary storage, retaining scalar copies and owned receiver values.
pub fn materialized_receiver(builder: &mut Builder, source: &SourcePlace, span: &SourceSpan) -> Result<Place, String> {
    if let Some(item) = element_read(builder, source, span)?
        && let OperandKind::Move(place) | OperandKind::Copy(place) = item.kind
    {
        return Ok(place);
    }
    Err("unsupported Body IR projected non-list receiver".into())
}

/// Materialize exactly one retained argument, refusing a spread before executing it.
pub fn argument_value(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    index: usize,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let Some(argument) = arguments.get(index) else {
        return Err("invalid Body IR list call arity".into());
    };
    match argument.as_one() {
        Some(value) => value_operand(builder, value, span),
        None => Err("unsupported Body IR list spread argument".into()),
    }
}

/// Select the checked destination or a typed temporary for a discarded call.
pub fn call_destination(
    builder: &mut Builder,
    destination: &Option<SourcePlace>,
    ty: PlanType,
    span: &SourceSpan,
) -> Result<Place, String> {
    match destination {
        Some(place) => {
            let root = list_root_place(builder, place, span)?;
            if place_type(&builder.locals, &root) == ty {
                return Ok(root);
            }
            scalar_place(builder, place, span)
        }
        None => Ok(temporary(builder, ty, span)),
    }
}

/// Borrow one checked positional list argument without admitting a spread.
pub fn borrowed_list_argument(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    index: usize,
    mutable: bool,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let Some(argument) = arguments.get(index) else {
        return Err("unsupported Body IR missing list argument".into());
    };
    match argument.as_one() {
        Some(value) => checked_list_argument(builder, value, mutable, span),
        None => Err("unsupported Body IR list argument spread".into()),
    }
}

/// Borrow stored list input or consume owned storage, never copying the complete source.
pub fn lower_list_iter(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 1 {
        return Err("invalid Body IR list iter arguments".into());
    }
    let (leaf, depth) = require_list_shape(
        &argument_type(builder, arguments, 0, span)?,
        "unsupported Body IR iter on non-list",
    )?;
    let element = list_element(&leaf, depth);
    let result = PlanType::Generator(leaf.clone(), depth - 1);
    let output = call_destination(builder, destination, result.clone(), span)?;
    if builder.locals[output.local as usize].ty != result {
        return Err("unsupported Body IR list iter destination".into());
    }
    drop_owned(builder, output.local, span);
    let list = PlanType::List(leaf.clone(), depth);
    match arguments[0].as_one() {
        Some(SourceOperand::Place(read)) => {
            let root = list_root_place(builder, &read.place, span)?;
            if read.fact == OwnershipFact::Move && place_type(&builder.locals, &root) == list {
                generator_runtime_call(
                    builder,
                    "list_generator",
                    element,
                    vec![list],
                    result,
                    vec![read_place(root, true, span)],
                    output.clone(),
                    span,
                );
            } else {
                let reference = list_borrow(builder, root, false, span)?;
                borrowed_list_generator_call(builder, reference, &leaf, depth, &output, span)?;
            }
        }
        Some(_) => {
            let owned = argument_value(builder, arguments, 0, span)?;
            generator_runtime_call(
                builder,
                "list_generator",
                element,
                vec![list],
                result,
                vec![owned],
                output.clone(),
                span,
            );
        }
        None => return Err("unsupported Body IR list iter spread".into()),
    }
    builder.owned.push(output.local);
    Ok(())
}

/// Convert one checked vector loan to its native slice and construct a lazy cursor without copying the list.
pub fn borrowed_list_generator_call(
    builder: &mut Builder,
    reference: Operand,
    leaf: &ListLeaf,
    depth: i64,
    output: &Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let element = list_element(leaf, depth);
    let slice_type = PlanType::SliceRef(vec![element.clone()]);
    let slice_reference = list_slice_view(builder, reference, std::slice::from_ref(&element), false, span)?;
    let result = PlanType::Generator(leaf.clone(), depth - 1);
    let path = "incan_std_primitives::iter::borrowed_list";
    builder.externals.push(ExternalFunction {
        path: path.into(),
        type_arguments: vec![element.clone()],
        parameters: vec![slice_type],
        return_type: result,
        span: span.clone(),
    });
    terminate_call(
        builder,
        CalleeKind::Instantiated(path.into(), vec![element]),
        vec![slice_reference],
        output.clone(),
        span,
    );
    Ok(())
}

/// Lower a checker-selected iterator or generator adapter, consumer, or collection terminal.
pub fn lower_iterator_method(
    builder: &mut Builder,
    method: IteratorMethodId,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let receiver_type = argument_type(builder, arguments, 0, span)?;
    let Some((leaf, depth)) = generator_shape(&receiver_type) else {
        return Err("unsupported Body IR selected iterator Method".into());
    };
    if is_generator_adapter(method) || is_generator_consumer(method) {
        return lower_generator_member(builder, method, arguments, destination, receiver_type, span);
    }
    if method != IteratorMethodId::Collect {
        return Err("unsupported Body IR selected generator Method".into());
    }
    if arguments.len() != 1 {
        return Err("invalid Body IR generator collect arguments".into());
    }
    let receiver = argument_place(builder, arguments, 0, span)?;
    let output = call_destination(builder, destination, PlanType::List(leaf, depth + 1), span)?;
    drop_owned(builder, output.local, span);
    generator_collect(builder, receiver, output.clone(), span)?;
    builder.owned.push(output.local);
    Ok(())
}

/// Borrow the checker-selected runtime bytes receiver as an immutable native byte slice.
pub fn lower_bytes_as_slice(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let receiver_type = argument_type(builder, arguments, 0, span)?;
    let bytes = [
        PlanType::List(ListLeaf::U8, 1),
        PlanType::ListRef(ListLeaf::U8, 1),
        PlanType::ListMutRef(ListLeaf::U8, 1),
    ];
    if arguments.len() != 1 || !bytes.contains(&receiver_type) {
        return Err("invalid Body IR bytes.as_slice receiver or arity".into());
    }
    let Some(value) = arguments[0].as_one() else {
        return Err("invalid Body IR bytes.as_slice spread".into());
    };
    let reference = borrow_slice_operand(builder, value, &[PlanType::U8], false, span)?;
    let output = call_destination(builder, destination, PlanType::SliceRef(vec![PlanType::U8]), span)?;
    builder.emit(plan_assign(output, RvalueKind::Use(reference), span));
    Ok(())
}

/// The runtime helper, receiver exclusivity, result and parameters of one checked list method.
fn list_method_signature(
    method: ListMethodId,
    leaf: &ListLeaf,
    depth: i64,
    arity: usize,
) -> Result<(&'static str, bool, PlanType, Vec<PlanType>), String> {
    let element = list_element(leaf, depth);
    Ok(match method {
        ListMethodId::Append => ("append_list", true, PlanType::Unit, vec![element]),
        ListMethodId::Extend => (
            "extend_list",
            true,
            PlanType::Unit,
            vec![PlanType::ListRef(leaf.clone(), depth)],
        ),
        ListMethodId::Pop => ("pop_list", true, element, Vec::new()),
        ListMethodId::Remove => ("remove_list", true, PlanType::Unit, vec![PlanType::Int]),
        ListMethodId::Swap => ("swap_list", true, PlanType::Unit, vec![PlanType::Int, PlanType::Int]),
        ListMethodId::Clone => ("clone_list", false, PlanType::List(leaf.clone(), depth), Vec::new()),
        ListMethodId::Count if arity == 1 => ("length_list", false, PlanType::Int, Vec::new()),
        ListMethodId::Count => ("count_list", false, PlanType::Int, vec![element]),
        ListMethodId::Index => ("index_list", false, PlanType::Int, vec![element]),
        ListMethodId::Contains | ListMethodId::Reserve | ListMethodId::ReserveExact => {
            return Err("unsupported Body IR duplicate typed list dispatch".into());
        }
    })
}

/// Lower one checker-selected list operation without interpreting its source or canonical spelling.
pub fn lower_list_method(
    builder: &mut Builder,
    method: ListMethodId,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if matches!(method, ListMethodId::Reserve | ListMethodId::ReserveExact) {
        let exact = method == ListMethodId::ReserveExact;
        return lower_list_reservation(builder, exact, arguments, destination, span);
    }
    if method == ListMethodId::Contains {
        return lower_list_helper(builder, false, arguments, destination, span);
    }
    let receiver_type = argument_type(builder, arguments, 0, span)?;
    let description = format!(
        "unsupported Body IR list Method {method:?} on non-list in {}",
        builder.name
    );
    let source_location = format!("{}:{}:{}", span.file, span.line, span.column);
    let (leaf, depth) = require_list_shape(&receiver_type, &format!("{description} at {source_location}"))?;
    let element = list_element(&leaf, depth);
    let (name, mutable, result, parameters) = list_method_signature(method, &leaf, depth, arguments.len())?;
    if arguments.len() != parameters.len() + 1 {
        return Err(format!("unsupported Body IR list Method arity {method:?}"));
    }
    let mut operands = Vec::with_capacity(parameters.len());
    for (index, parameter) in parameters.iter().enumerate() {
        if method == ListMethodId::Extend {
            operands.push(borrowed_list_argument(builder, arguments, index + 1, false, span)?);
        } else {
            match arguments[index + 1].as_one() {
                Some(value) => operands.push(boundary_operand(builder, value, parameter, span)?),
                None => return Err("unsupported Body IR list method spread argument".into()),
            }
        }
    }
    let (reference, cell) = list_method_receiver(builder, arguments, mutable, span)?;
    let receiver_type = if mutable {
        PlanType::ListMutRef(leaf, depth)
    } else {
        PlanType::ListRef(leaf, depth)
    };
    let output = call_destination(builder, destination, result.clone(), span)?;
    let mut all_parameters = vec![receiver_type];
    all_parameters.extend(parameters);
    let mut all_operands = vec![reference];
    all_operands.extend(operands);
    list_call(
        builder,
        name,
        element,
        all_parameters,
        result,
        all_operands,
        output,
        span,
    );
    if mutable && let Some((global, receiver)) = cell {
        persist_static_value(builder, &global, receiver, span)?;
    }
    Ok(())
}

/// Reserve additional capacity on the actual checked list receiver, preserving the existing int-to-usize contract.
fn lower_list_reservation(
    builder: &mut Builder,
    exact: bool,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 2 {
        return Err("invalid Body IR list reservation arity".into());
    }
    let ty = argument_type(builder, arguments, 0, span)?;
    let (leaf, depth) = require_list_shape(&ty, "invalid Body IR list reservation receiver")?;
    let count = argument_value(builder, arguments, 1, span)?;
    if value_type(builder, &count)? != PlanType::Int {
        return Err("invalid Body IR list reservation count".into());
    }
    let additional = temporary(builder, PlanType::USize, span);
    builder.emit(plan_assign(
        additional.clone(),
        RvalueKind::NumericCast(count, PlanType::Int, PlanType::USize),
        span,
    ));
    let (reference, cell) = list_method_receiver(builder, arguments, true, span)?;
    let output = call_destination(builder, destination, PlanType::Unit, span)?;
    let method = if exact { "reserve_exact" } else { "reserve" };
    instantiated_external_call(
        builder,
        &format!("alloc::vec::Vec::{method}"),
        vec![list_element(&leaf, depth)],
        vec![PlanType::ListMutRef(leaf, depth), PlanType::USize],
        PlanType::Unit,
        vec![reference, read_place(additional, false, span)],
        output,
        span,
    )?;
    if let Some((global, receiver)) = cell {
        persist_static_value(builder, &global, receiver, span)?;
    }
    Ok(())
}

/// Borrow ordinary receiver storage or a retained static snapshot after argument evaluation.
fn list_method_receiver(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    mutable: bool,
    span: &SourceSpan,
) -> Result<(Operand, StaticReceiver), String> {
    let cell = static_collection_receiver(builder, arguments, mutable, span)?;
    let reference = match &cell {
        Some((_, receiver)) => list_borrow(builder, receiver.clone(), mutable, span)?,
        None => borrowed_list_argument(builder, arguments, 0, mutable, span)?,
    };
    Ok((reference, cell))
}

/// Read static storage after method arguments, retaining exclusive-access authority for a subsequent commit.
pub fn static_collection_receiver(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    mutable: bool,
    span: &SourceSpan,
) -> Result<StaticReceiver, String> {
    let Some(first) = arguments.first() else {
        return Err("invalid Body IR static method arity".into());
    };
    match first.as_one() {
        Some(SourceOperand::Place(read)) => static_place_receiver(builder, read, mutable, span),
        _ => Ok(None),
    }
}

/// Materialize a checked global receiver while retaining its identity for the mutation's commit.
fn static_place_receiver(
    builder: &mut Builder,
    read: &PlaceOperand,
    mutable: bool,
    span: &SourceSpan,
) -> Result<StaticReceiver, String> {
    if read.place.global().is_none() {
        return Ok(None);
    }
    if !read.place.projection.is_empty() {
        return Err("unsupported Body IR projected static collection method".into());
    }
    if mutable && read.fact != OwnershipFact::MutBorrow {
        return Err("unsupported Body IR static collection method without mutable borrow fact".into());
    }
    let Some(global) = read.place.global() else {
        return Ok(None);
    };
    let operand = read_static(builder, &read.place, global, read.fact, span)?;
    match operand.kind {
        OperandKind::Move(place) => Ok(Some((global.clone(), place))),
        _ => Err("unsupported Body IR noncollection static method receiver".into()),
    }
}

/// Ask the runtime for one frozen collection's length through a shared reference to it.
#[allow(clippy::too_many_arguments)] // The Incan helper's seven parts of one frozen length read.
fn frozen_length(
    builder: &mut Builder,
    name: &str,
    elements: Vec<PlanType>,
    reference: PlanType,
    value: Operand,
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let output = call_destination(builder, destination, PlanType::Int, span)?;
    let path = match name {
        "FrozenSet" => "incan_native_runtime::frozen_set_length_ref",
        "FrozenDict" => "incan_native_runtime::frozen_dict_length_ref",
        _ => "incan_native_runtime::frozen_list_length_ref",
    };
    instantiated_external_call(
        builder,
        path,
        elements,
        vec![reference],
        PlanType::Int,
        vec![value],
        output,
        span,
    )
}

/// Read string, list, and hashed-collection lengths through the legacy runtime contracts.
pub fn lower_list_builtin(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 1 {
        return Err("unsupported Body IR len arity".into());
    }
    // An indexed nested list, such as `len(rows[1])`, is measured through the copy a shared list argument reads.
    if indexes_nested_list(builder, arguments) {
        let element = borrowed_list_argument(builder, arguments, 0, false, span)?;
        return list_length(builder, element, destination, span);
    }
    let ty = argument_type(builder, arguments, 0, span)?;
    match &ty {
        PlanType::NativeArray(_, count) => {
            let output = call_destination(builder, destination, PlanType::Int, span)?;
            builder.emit(plan_assign(
                output,
                RvalueKind::Use(literal(Constant::Int(*count), span)),
                span,
            ));
            return Ok(());
        }
        PlanType::FrozenCollection(name, elements) => {
            let value = borrow_frozen_argument(builder, &arguments[0], span)?;
            let reference = frozen_reference_type(&ty)?;
            return frozen_length(builder, name, elements.clone(), reference, value, destination, span);
        }
        PlanType::FrozenCollectionRef(name, elements) => {
            // A lent frozen parameter is already the reference the runtime reads.
            let Some(SourceOperand::Place(read)) = arguments[0].as_one() else {
                return Err("unsupported Body IR frozen reference length".into());
            };
            let value = read_place(scalar_place(builder, &read.place, span)?, false, span);
            return frozen_length(builder, name, elements.clone(), ty.clone(), value, destination, span);
        }
        _ => {}
    }
    if ty == PlanType::FrozenBytes {
        return frozen_bytes_length(builder, arguments, destination, span);
    }
    if matches!(ty, PlanType::String | PlanType::StringRef | PlanType::FrozenStr) {
        return lower_text_helper(builder, "length_text", PlanType::Int, arguments, destination, span);
    }
    let receiver = shared_receiver_place(builder, arguments, 0, span)?;
    if is_collection(&place_type(&builder.locals, &receiver)) {
        return collection_length(builder, receiver, destination, span);
    }
    require_list_shape(
        &place_type(&builder.locals, &receiver),
        "unsupported Body IR len on non-list",
    )?;
    let reference = list_borrow(builder, receiver, false, span)?;
    list_length(builder, reference, destination, span)
}

/// Measure a frozen byte string through the runtime's frozen-bytes length helper.
fn frozen_bytes_length(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let Some(value) = arguments[0].as_one() else {
        return Err("unsupported Body IR spread FrozenBytes length".into());
    };
    let operand = scalar_operand(builder, value, span)?;
    let mut output = temporary(builder, PlanType::Int, span);
    if let Some(place) = destination {
        output = scalar_place(builder, place, span)?;
    }
    external_call(
        builder,
        "incan_native_runtime::length_frozen_bytes",
        vec![PlanType::FrozenBytes],
        PlanType::Int,
        vec![operand],
        output,
        span,
    );
    Ok(())
}

/// Recognize one index into a local list of lists, such as `rows[1]`.
fn indexes_nested_list(builder: &Builder, arguments: &[ArgumentElement]) -> bool {
    let Some(SourceOperand::Place(read)) = arguments[0].as_one() else {
        return false;
    };
    if read.place.projection.len() != 1 || !matches!(read.place.projection[0], PlaceElem::Index(_)) {
        return false;
    }
    let Some(local) = read.place.local_id() else {
        return false;
    };
    let slot = (local.index() as i64 + 1 + builder.local_offset) as usize;
    matches!(list_shape(&builder.locals[slot].ty), Some((_, depth)) if depth > 1)
}

/// Read a borrowed list's length through the runtime.
fn list_length(
    builder: &mut Builder,
    reference: Operand,
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let (leaf, depth) = require_list_shape(&value_type(builder, &reference)?, "unsupported Body IR len on non-list")?;
    let output = call_destination(builder, destination, PlanType::Int, span)?;
    list_call(
        builder,
        "length_list",
        list_element(&leaf, depth),
        vec![PlanType::ListRef(leaf, depth)],
        PlanType::Int,
        vec![reference],
        output,
        span,
    );
    Ok(())
}

/// Borrow both operands for native PartialEq, retaining haystack-first order and explicit negation.
///
/// `x in xs`, `x not in xs` and `xs.contains(x)` all lower here, so one admission policy serves every spelling.
pub fn lower_list_helper(
    builder: &mut Builder,
    negated: bool,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 2 {
        return Err("unsupported Body IR list membership arity".into());
    }
    if frozen_text_membership(builder, negated, arguments, destination, span)? {
        return Ok(());
    }
    let receiver = shared_sequence_receiver(builder, arguments, span, true)?;
    let (leaf, depth) = require_list_shape(
        &place_type(&builder.locals, &receiver),
        "unsupported Body IR membership on non-list",
    )?;
    admit_membership_equality(builder, &leaf)?;
    let element = list_element(&leaf, depth);
    let needle_type = match shared_receiver_type(element.clone()) {
        Ok(reference_type) => reference_type,
        Err(reason) => {
            return Err(format!(
                "{reason} in {} membership needle at {}:{}",
                builder.name, span.file, span.line
            ));
        }
    };
    let Some(value) = arguments[1].as_one() else {
        return Err("unsupported Body IR list membership needle spread".into());
    };
    let argument = call_argument_operand(builder, value, &needle_type, span, false, IncanInputAccess::Inferred)?;
    let reference = list_borrow(builder, receiver, false, span)?;
    let slice_type = PlanType::SliceRef(vec![element.clone()]);
    let slice_reference = list_slice_view(builder, reference, std::slice::from_ref(&element), false, span)?;
    let output = call_destination(builder, destination, PlanType::Bool, span)?;
    instantiated_external_call(
        builder,
        "incan_std_primitives::collections::list_contains",
        vec![element],
        vec![slice_type, needle_type],
        PlanType::Bool,
        vec![slice_reference, argument],
        output.clone(),
        span,
    )?;
    if negated {
        let value = read_place(output.clone(), false, span);
        builder.emit(plan_assign(output, RvalueKind::Unary(UnaryOp::Not, value), span));
    }
    Ok(())
}

/// Admit elements whose native PartialEq is Incan's structural equality: scalars, text, enums, tuples, lists,
/// dictionaries, sets, opaque native records and source models with derived equality. A source model with its own
/// `__eq__` has no native PartialEq to call, and a callable has no equality.
fn admit_membership_equality(builder: &Builder, leaf: &ListLeaf) -> Result<(), String> {
    match leaf {
        ListLeaf::Model(owner, _) => {
            let declaration = &builder.nominals()[*owner as usize];
            if library_item(declaration).is_empty() && !declaration.derives.iter().any(|name| name == "PartialEq") {
                return Err("unsupported Body IR list membership nominal equality".into());
            }
            Ok(())
        }
        ListLeaf::FunctionPointer(_) => Err("unsupported Body IR list membership callable equality".into()),
        _ => Ok(()),
    }
}

/// Write one checked list element through the legacy mutable indexing helper.
pub fn indexed_assignment(
    builder: &mut Builder,
    destination: &SourcePlace,
    value: &Rvalue,
    span: &SourceSpan,
) -> Result<(), String> {
    indexed_assignment_at(builder, destination, 0, value, span)
}

/// Write one checked list element whose list is reached after `start` structural steps (zero for a bare list).
pub fn indexed_assignment_at(
    builder: &mut Builder,
    destination: &SourcePlace,
    start: usize,
    value: &Rvalue,
    span: &SourceSpan,
) -> Result<(), String> {
    if destination.projection.len() <= start {
        return Err("unsupported Body IR list assignment without index".into());
    }
    // Materialize the replacement before borrowing storage that its value might itself read.
    let assigned = assignment_value(builder, value, span)?;
    let mut receiver = list_base_place(builder, destination, span)?;
    if start > 0 {
        receiver = prefix_place(builder, destination, start, span)?;
    }
    for slot in start..destination.projection.len() - 1 {
        let index = list_index_operand(builder, destination, slot, span)?;
        receiver = borrow_element_mut(builder, receiver, index, span)?;
    }
    let (leaf, depth) = require_list_shape(
        &place_type(&builder.locals, &receiver),
        "unsupported Body IR indexed assignment on non-list",
    )?;
    let element = list_element(&leaf, depth);
    let argument = match value {
        Rvalue::Use(SourceOperand::Constant(SourceConstant::None)) => none_operand(builder, &element, span)?,
        _ => element_plan_operand(builder, assigned, &element, span)?,
    };
    let subscript = list_index_operand(builder, destination, destination.projection.len() - 1, span)?;
    let reference = list_borrow(builder, receiver, true, span)?;
    let unit = temporary(builder, PlanType::Unit, span);
    list_call(
        builder,
        "set_list",
        element.clone(),
        vec![PlanType::ListMutRef(leaf, depth), PlanType::Int, element],
        PlanType::Unit,
        vec![reference, subscript, argument],
        unit,
        span,
    );
    Ok(())
}

/// Materialize the checked single value before borrowing its indexing destination mutably.
fn assignment_value(builder: &mut Builder, value: &Rvalue, span: &SourceSpan) -> Result<Operand, String> {
    match value {
        Rvalue::Use(operand) => value_operand(builder, operand, span),
        _ => Err("unsupported Body IR indexed assignment rvalue".into()),
    }
}
