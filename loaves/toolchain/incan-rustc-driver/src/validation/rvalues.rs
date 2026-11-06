//! Rvalue result validation, closure constructors, arrays, and binary and shift results.

use super::*;

/// Validate scalar and aggregate rvalues against their exact destination layout.
pub(super) fn rvalue(
    plan: &Plan,
    function: &Function,
    value: &RvalueKind,
    expected: Scalar,
) -> Result<Scalar, PlanError> {
    match value {
        RvalueKind::UnitFunction(name) => unit_function_result(plan, function, name),
        RvalueKind::Use(value) => operand(plan, function, value),
        RvalueKind::NativeAlias(value, source, target) => {
            require(
                function,
                operand(plan, function, value)?,
                scalar(source),
                "native alias input",
            )?;
            require(function, scalar(target), expected.clone(), "native alias destination")?;
            Ok(expected)
        }
        RvalueKind::FrozenText(value) => frozen_text_result(plan, function, value),
        RvalueKind::FrozenBytes(values) => {
            if values.iter().any(|value| u8::try_from(*value).is_err()) {
                return Err(invalid(
                    function,
                    "FrozenBytes construction requires unsigned literal bytes",
                ));
            }
            Ok(Scalar::FrozenBytes)
        }
        RvalueKind::FrozenCollection(ty, values) => {
            validate_frozen_values(plan, function, ty, values)?;
            Ok(scalar(ty))
        }
        RvalueKind::NumericCast(value, source, target) => numeric_cast_result(plan, function, value, source, target),
        RvalueKind::Discriminant(value) => {
            if !matches!(place(plan, function, value)?, Scalar::Enum(_)) {
                return Err(invalid(function, "discriminant requires an enum"));
            }
            Ok(Scalar::EnumTag)
        }
        RvalueKind::IsVariant(value, variant) => variant_test_result(plan, function, value, *variant),
        RvalueKind::TagToInt(value) => {
            require(
                function,
                operand(plan, function, value)?,
                Scalar::EnumTag,
                "discriminant conversion",
            )?;
            Ok(Scalar::Int)
        }
        RvalueKind::Enum(owner, variant, elements) => {
            enum_aggregate_result(plan, function, *owner, *variant, elements, expected)
        }
        RvalueKind::FunctionItem(callee) => function_item_result(plan, function, callee),
        RvalueKind::ClosureObject(value) => match place(plan, function, value)? {
            Scalar::Closure(signature) => Ok(Scalar::ClosureRef(signature)),
            Scalar::BorrowingClosure(signature) => Ok(Scalar::BorrowingClosureRef(signature)),
            _ => Err(invalid(
                function,
                "callable-object borrow requires an owned closure payload",
            )),
        },
        RvalueKind::ReifyFunction(value) => {
            let Scalar::FunctionItem(_, signature) = operand(plan, function, value)? else {
                return Err(invalid(function, "function-pointer coercion requires a function item"));
            };
            Ok(Scalar::FunctionPointer(signature))
        }
        RvalueKind::FunctionAddress(value) => {
            if !matches!(operand(plan, function, value)?, Scalar::FunctionPointer(_)) {
                return Err(invalid(
                    function,
                    "function address requires a checked function pointer",
                ));
            }
            Ok(Scalar::USize)
        }
        RvalueKind::IntToFloat(value) => {
            converted_scalar_result(plan, function, value, Scalar::Int, Scalar::Float, "int-to-float")
        }
        RvalueKind::FloatToInt(value) => {
            converted_scalar_result(plan, function, value, Scalar::Float, Scalar::Int, "float-to-int")
        }
        RvalueKind::BoolToInt(value) => {
            converted_scalar_result(plan, function, value, Scalar::Bool, Scalar::Int, "bool-to-int")
        }
        RvalueKind::Unary(op, value) => unary_result(plan, function, op, value),
        RvalueKind::Binary(op, left, right) => binary_rvalue_result(plan, function, op, left, right, expected),
        RvalueKind::Array(elements) => validate_array(plan, function, elements, &expected),
        RvalueKind::Tuple(elements) => tuple_aggregate_result(plan, function, elements, expected),
        RvalueKind::Range(elements) => range_aggregate_result(plan, function, elements, expected),
        RvalueKind::Model(index, elements) => model_aggregate_result(plan, function, *index, elements, expected),
        RvalueKind::RawAddressOf(value, mutable) => {
            Ok(Scalar::RawPointer(*mutable, vec![place(plan, function, value)?]))
        }
        RvalueKind::PointerCast(value, target) => pointer_cast_result(plan, function, value, target),
        RvalueKind::PointerOffset(pointer, offset) => {
            let pointer = operand(plan, function, pointer)?;
            if !matches!(pointer, Scalar::RawPointer(_, _)) || operand(plan, function, offset)? != Scalar::ISize {
                return Err(invalid(
                    function,
                    "pointer offset requires a pointer and native signed offset",
                ));
            }
            Ok(pointer)
        }
        RvalueKind::PointerAddress(pointer) => pointer_address_result(plan, function, pointer),
        RvalueKind::MutBorrow(value) => borrow_result(plan, function, value, true),
        RvalueKind::Borrow(value) => borrow_result(plan, function, value, false),
        RvalueKind::UnsizeTraitObject(value, source, target) => {
            trait_object_result(plan, function, value, source, target)
        }
        RvalueKind::UnsizeSlice(value) => match operand(plan, function, value)? {
            Scalar::StringArrayRef(_) => Ok(Scalar::StringSlice),
            Scalar::StrArrayRef(_) => Ok(Scalar::StrSlice),
            _ => Err(invalid(
                function,
                "slice coercion requires a shared formatting-array reference",
            )),
        },
    }
}

/// Require a static text literal before checking its string-reference carrier; dynamic text remains a refusal.
pub(super) fn frozen_text_result(plan: &Plan, function: &Function, value: &Operand) -> Result<Scalar, PlanError> {
    if !matches!(value.kind, OperandKind::Literal(Constant::Text(_))) {
        return Err(invalid(function, "FrozenStr construction requires static literal text"));
    }
    require(
        function,
        operand(plan, function, value)?,
        Scalar::StrRef,
        "frozen literal",
    )?;
    Ok(Scalar::FrozenStr)
}

/// Resolve a retained planned or external function declaration and append its result to the callable carrier.
pub(super) fn function_item_result(plan: &Plan, function: &Function, callee: &Callee) -> Result<Scalar, PlanError> {
    let name = match &callee.kind {
        CalleeKind::Planned(name) | CalleeKind::External(name) => name,
        _ => return Err(invalid(function, "function item requires a retained declaration")),
    };
    let (mut parameters, result) = signature(plan, function, callee)?;
    parameters.push(result);
    Ok(Scalar::FunctionItem(name.clone(), parameters))
}

/// Check a fixed scalar conversion's exact input carrier before returning its declared output carrier.
pub(super) fn converted_scalar_result(
    plan: &Plan,
    function: &Function,
    value: &Operand,
    source: Scalar,
    target: Scalar,
    context: &str,
) -> Result<Scalar, PlanError> {
    require(function, operand(plan, function, value)?, source, context)?;
    Ok(target)
}

/// Resolve an executor callback by its retained name and require its exact no-argument, unit-returning signature.
pub(super) fn unit_function_result(plan: &Plan, function: &Function, name: &str) -> Result<Scalar, PlanError> {
    let callback = plan
        .functions
        .iter()
        .find(|value| value.name == name)
        .ok_or_else(|| PlanError::UnknownCallee(name.to_string()))?;
    if !callback.parameters.is_empty() || scalar(&callback.return_type) != Scalar::Unit {
        return Err(invalid(
            function,
            "executor callback must take no arguments and return unit",
        ));
    }
    Ok(Scalar::UnitFunction)
}

/// Check the operand against its declared source carrier before admitting a numeric source and target pair.
pub(super) fn numeric_cast_result(
    plan: &Plan,
    function: &Function,
    value: &Operand,
    source: &PlanType,
    target: &PlanType,
) -> Result<Scalar, PlanError> {
    let source = scalar(source);
    let target = scalar(target);
    require(
        function,
        operand(plan, function, value)?,
        source.clone(),
        "numeric cast source",
    )?;
    let numeric = |ty: &Scalar| sized_numeric(ty).is_some() || matches!(ty, Scalar::Int | Scalar::Float);
    if !numeric(&source) || !numeric(&target) {
        return Err(invalid(function, "numeric cast requires numeric carriers"));
    }
    Ok(target)
}

/// Prove that the tested place is an enum and that its declaration owns the requested variant before returning bool.
pub(super) fn variant_test_result(
    plan: &Plan,
    function: &Function,
    value: &Place,
    variant: i64,
) -> Result<Scalar, PlanError> {
    let Scalar::Enum(owner) = place(plan, function, value)? else {
        return Err(invalid(function, "variant test requires an enum"));
    };
    let declaration = enum_declaration(plan, owner).ok_or_else(|| invalid(function, "unknown tested enum"))?;
    if usize::try_from(variant)
        .ok()
        .and_then(|index| declaration.variants.get(index))
        .is_none()
    {
        return Err(invalid(function, "unknown tested variant"));
    }
    Ok(Scalar::Bool)
}

/// Check the destination enum first, then variant arity and payloads in declaration order. The owned expected carrier
/// returns unchanged after every payload has been checked against the same retained plan declaration.
pub(super) fn enum_aggregate_result(
    plan: &Plan,
    function: &Function,
    owner: i64,
    variant: i64,
    elements: &[Operand],
    expected: Scalar,
) -> Result<Scalar, PlanError> {
    require(
        function,
        expected.clone(),
        Scalar::Enum(owner),
        "enum aggregate destination",
    )?;
    let declaration = enum_declaration(plan, owner).ok_or_else(|| invalid(function, "unknown constructed enum"))?;
    let variant = usize::try_from(variant)
        .ok()
        .and_then(|index| declaration.variants.get(index))
        .ok_or_else(|| invalid(function, "unknown constructed variant"))?;
    if variant.fields.len() != elements.len() {
        return Err(invalid(function, "enum payload count differs from declaration"));
    }
    for (element, field) in elements.iter().zip(&variant.fields) {
        require(
            function,
            operand(plan, function, element)?,
            scalar(field),
            "enum payload",
        )?;
    }
    Ok(expected)
}

/// Admit a unary operation only for its existing scalar carriers after resolving the operand's exact plan type.
pub(super) fn unary_result(
    plan: &Plan,
    function: &Function,
    op: &UnaryOp,
    value: &Operand,
) -> Result<Scalar, PlanError> {
    let ty = operand(plan, function, value)?;
    match op {
        UnaryOp::Not if ty == Scalar::Bool => Ok(ty),
        UnaryOp::Invert if integer(&ty) => Ok(ty),
        UnaryOp::Negate
            if matches!(
                ty,
                Scalar::Int
                    | Scalar::Float
                    | Scalar::I8
                    | Scalar::I16
                    | Scalar::I32
                    | Scalar::I128
                    | Scalar::F32
                    | Scalar::F64
            ) =>
        {
            Ok(ty)
        }
        _ => Err(invalid(function, "invalid unary operand type")),
    }
}

/// Resolve binary operands left to right. Shifts retain their distinct count rule before ordinary same-carrier checks.
pub(super) fn binary_rvalue_result(
    plan: &Plan,
    function: &Function,
    op: &BinaryOp,
    left: &Operand,
    right: &Operand,
    expected: Scalar,
) -> Result<Scalar, PlanError> {
    let ty = operand(plan, function, left)?;
    let count = operand(plan, function, right)?;
    if matches!(op, BinaryOp::ShiftLeft | BinaryOp::ShiftRight) {
        return shift_result(function, ty, &count);
    }
    require(function, count, ty.clone(), "binary operands")?;
    binary_result(function, op, ty, expected)
}

/// Require a tuple destination and its exact arity before checking fields in source order; preserve its owned carrier.
pub(super) fn tuple_aggregate_result(
    plan: &Plan,
    function: &Function,
    elements: &[Operand],
    expected: Scalar,
) -> Result<Scalar, PlanError> {
    let Scalar::Tuple(fields) = &expected else {
        return Err(invalid(function, "tuple aggregate requires a tuple destination"));
    };
    if fields.len() != elements.len() {
        return Err(invalid(function, "tuple element count differs from destination"));
    }
    for (element, field) in elements.iter().zip(fields) {
        require(
            function,
            operand(plan, function, element)?,
            field.clone(),
            "tuple element",
        )?;
    }
    Ok(expected)
}

/// Check the range destination and complete field count before comparing each operand with the plan's range layout.
pub(super) fn range_aggregate_result(
    plan: &Plan,
    function: &Function,
    elements: &[Operand],
    expected: Scalar,
) -> Result<Scalar, PlanError> {
    require(function, expected, Scalar::RangeValue, "range aggregate destination")?;
    let fields = range_field_types();
    if elements.len() != fields.len() {
        return Err(invalid(function, "range aggregate requires every declared range field"));
    }
    for (element, field) in elements.iter().zip(&fields) {
        require(
            function,
            operand(plan, function, element)?,
            &scalar(field),
            "range field",
        )?;
    }
    Ok(Scalar::RangeValue)
}

/// Check the retained aggregate declaration's destination, arity and fields without copying its field inventory.
pub(super) fn model_aggregate_result(
    plan: &Plan,
    function: &Function,
    index: i64,
    elements: &[Operand],
    expected: Scalar,
) -> Result<Scalar, PlanError> {
    require(
        function,
        expected.clone(),
        Scalar::Model(index),
        "model aggregate destination",
    )?;
    let model = model(plan, index).ok_or_else(|| invalid(function, "unknown constructed model"))?;
    if !model.unavailable_native_fields.is_empty() {
        return Err(invalid(
            function,
            "cannot construct a native layout with an opaque field",
        ));
    }
    if elements.len() != model.fields.len() {
        return Err(invalid(function, "model field count differs from declaration"));
    }
    for (element, field) in elements.iter().zip(&model.fields) {
        require(
            function,
            operand(plan, function, element)?,
            &scalar(&field.ty),
            "model field",
        )?;
    }
    Ok(Scalar::Model(index))
}

/// Preserve pointer mutability and the single-pointee requirement, checking the operand before the target carrier.
pub(super) fn pointer_cast_result(
    plan: &Plan,
    function: &Function,
    value: &Operand,
    target: &PlanType,
) -> Result<Scalar, PlanError> {
    let source = operand(plan, function, value)?;
    let Scalar::RawPointer(mutable, pointee) = scalar(target) else {
        return Err(invalid(function, "pointer cast requires a raw pointer destination"));
    };
    if pointee.len() != 1
        || !match source {
            Scalar::RawPointer(source_mutable, _) => !mutable || source_mutable,
            _ => false,
        }
    {
        return Err(invalid(function, "pointer cast requires a compatible raw pointer"));
    }
    Ok(scalar(target))
}

/// Admit a trait-object conversion only from the operand's own reference type to an object reference of the same
/// mutability. Whether the referent implements the object is rustc's to prove when it builds the conversion.
pub(super) fn trait_object_result(
    plan: &Plan,
    function: &Function,
    value: &Operand,
    source: &PlanType,
    target: &PlanType,
) -> Result<Scalar, PlanError> {
    require(
        function,
        operand(plan, function, value)?,
        scalar(source),
        "trait object conversion source",
    )?;
    let target = scalar(target);
    let preserved = match (&target, scalar(source)) {
        (Scalar::ScalarRef(inner), Scalar::ModelRef(_) | Scalar::ScalarRef(_))
        | (Scalar::ScalarMutRef(inner), Scalar::ModelMutRef(_) | Scalar::ScalarMutRef(_)) => {
            matches!(inner.as_slice(), [Scalar::NativeTraitObject(..)])
        }
        _ => false,
    };
    if !preserved {
        return Err(invalid(
            function,
            "a trait object conversion keeps its reference's mutability",
        ));
    }
    Ok(target)
}

/// Admit address observation only for a thin raw pointer, retaining the existing refusal for unsized pointees.
pub(super) fn pointer_address_result(plan: &Plan, function: &Function, pointer: &Operand) -> Result<Scalar, PlanError> {
    let Scalar::RawPointer(_, elements) = operand(plan, function, pointer)? else {
        return Err(invalid(function, "address observation requires a raw pointer"));
    };
    if elements.len() != 1
        || matches!(
            elements.first(),
            Some(Scalar::NativeStr | Scalar::NativeSlice(_) | Scalar::NativeTraitObject(..))
        )
    {
        return Err(invalid(function, "address observation requires a thin raw pointer"));
    }
    Ok(Scalar::USize)
}

/// Form the exact borrowed carrier, rejecting mutable reborrows through a shared owner before native MIR construction.
pub(super) fn borrow_result(
    plan: &Plan,
    function: &Function,
    value: &Place,
    mutable: bool,
) -> Result<Scalar, PlanError> {
    let ty = place(plan, function, value)?;
    if mutable {
        return match ty {
            Scalar::ZipIterator(left, right) => Ok(Scalar::ZipIteratorMutRef(left, right)),
            Scalar::Tuple(elements) => {
                if shared_reference(&local(function, value.local)?) {
                    return Err(invalid(
                        function,
                        "cannot mutably borrow a tuple through a shared reference",
                    ));
                }
                Ok(Scalar::TupleMutRef(elements))
            }
            Scalar::Generator(leaf, depth) => Ok(Scalar::GeneratorMutRef(leaf, depth)),
            Scalar::ListIntoIter(leaf, depth) => Ok(Scalar::ListIntoIterMutRef(leaf, depth)),
            Scalar::Model(index) => {
                if matches!(local(function, value.local)?, Scalar::ModelRef(_)) {
                    return Err(invalid(function, "cannot mutably reborrow a shared receiver"));
                }
                Ok(Scalar::ModelMutRef(index))
            }
            Scalar::List(leaf, depth) => {
                if matches!(local(function, value.local)?, Scalar::ListRef(..)) {
                    return Err(invalid(function, "cannot mutably reborrow a shared list"));
                }
                Ok(Scalar::ListMutRef(leaf, depth))
            }
            Scalar::Set(leaf) => {
                if matches!(local(function, value.local)?, Scalar::SetRef(_)) {
                    return Err(invalid(function, "cannot mutably reborrow a shared set"));
                }
                Ok(Scalar::SetMutRef(leaf))
            }
            Scalar::Dict(key, item) => {
                if matches!(local(function, value.local)?, Scalar::DictRef(..)) {
                    return Err(invalid(function, "cannot mutably reborrow a shared dictionary"));
                }
                Ok(Scalar::DictMutRef(key, item))
            }
            Scalar::String => {
                if shared_reference(&local(function, value.local)?) {
                    return Err(invalid(
                        function,
                        "cannot mutably borrow text through a shared reference",
                    ));
                }
                Ok(Scalar::StringMutRef)
            }
            Scalar::Enum(index) => {
                if shared_reference(&local(function, value.local)?) {
                    return Err(invalid(
                        function,
                        "cannot mutably borrow an enum through a shared reference",
                    ));
                }
                Ok(Scalar::EnumMutRef(index))
            }
            ty if scalar_referent(&ty) => {
                // A whole shared-reference local is the reference itself, which `Read for &UnixStream` borrows
                // exclusively; only a place reached through the reference reborrows its shared referent.
                if !matches!(value.projection, Projection::Whole) && shared_reference(&local(function, value.local)?) {
                    return Err(invalid(function, "cannot mutably reborrow a shared scalar"));
                }
                Ok(Scalar::ScalarMutRef(vec![ty]))
            }
            Scalar::NativeSlice(elements) => {
                if shared_reference(&local(function, value.local)?) {
                    return Err(invalid(function, "cannot mutably reborrow a shared slice"));
                }
                Ok(Scalar::SliceMutRef(elements))
            }
            _ => Err(invalid(
                function,
                "mutable borrow requires a collection or nominal value",
            )),
        };
    }
    match ty {
        Scalar::ZipIterator(left, right) => Ok(Scalar::ZipIteratorRef(left, right)),
        Scalar::Tuple(elements) => Ok(Scalar::TupleRef(elements)),
        Scalar::GeneratorYield(leaf, depth) => Ok(Scalar::GeneratorYieldRef(leaf, depth)),
        Scalar::Enum(index) => Ok(Scalar::EnumRef(index)),
        Scalar::Model(index) => Ok(Scalar::ModelRef(index)),
        Scalar::List(leaf, depth) => Ok(Scalar::ListRef(leaf, depth)),
        Scalar::Set(leaf) => Ok(Scalar::SetRef(leaf)),
        Scalar::Dict(key, value) => Ok(Scalar::DictRef(key, value)),
        Scalar::String => Ok(Scalar::StringRef),
        Scalar::NativeSlice(elements) => Ok(Scalar::SliceRef(elements)),
        Scalar::NativeArray(elements, length) => Ok(Scalar::NativeArrayRef(elements, length)),
        Scalar::ValidationError => Ok(Scalar::ValidationErrorRef),
        Scalar::FrozenCollection(name, elements) => Ok(Scalar::FrozenCollectionRef(name, elements)),
        Scalar::StringArray(count) => Ok(Scalar::StringArrayRef(count)),
        Scalar::StrArray(count) => Ok(Scalar::StrArrayRef(count)),
        ty if scalar_referent(&ty) => Ok(Scalar::ScalarRef(vec![ty])),
        _ => Err(invalid(function, "shared borrow requires an owned formatting value")),
    }
}

/// Pin a closure constructor to the one shape its native body implements: rustc builds that body from the injected
/// boxing expression, so any other statement or edge in the plan would silently not execute.
pub(super) fn validate_closure_constructor(function: &Function) -> Result<(), PlanError> {
    let Some((_, count)) = crate::closures::constructor(function) else {
        return Ok(());
    };
    let refused = || {
        invalid(
            function,
            "closure constructor must box its captures in exactly one call",
        )
    };
    let [first, second] = function.blocks.as_slice() else {
        return Err(refused());
    };
    if !first.statements.is_empty()
        || !second.statements.is_empty()
        || first.cleanup
        || second.cleanup
        || !matches!(second.terminator.kind, TerminatorKind::Return)
        || !matches!(
            function.return_type,
            PlanType::Closure(_)
                | PlanType::ClosureOnce(_)
                | PlanType::NativeClosure(..)
                | PlanType::BorrowingClosure(_)
        )
        || usize::try_from(count).ok() != Some(function.parameters.len())
    {
        return Err(refused());
    }
    let TerminatorKind::Call(_, arguments, destination, 1, Unwind::Continue) = &first.terminator.kind else {
        return Err(refused());
    };
    let consumes_environment = matches!(function.return_type, PlanType::ClosureOnce(_));
    let TerminatorKind::Call(callee, ..) = &first.terminator.kind else {
        return Err(refused());
    };
    if consumes_environment != matches!(callee.kind, CalleeKind::ClosureBodyOnce(..))
        || (consumes_environment && function.parameters.is_empty())
    {
        return Err(refused());
    }
    // Each capture comes from its own parameter. Consuming environments move every capture; reusable environments move
    // the snapshots selected by Incan's capture policy and copy scalars.
    let forwarded = arguments.iter().enumerate().all(|(index, argument)| {
        let (moved, place) = match &argument.kind {
            OperandKind::Copy(place) => (false, place),
            OperandKind::Move(place) => (true, place),
            _ => return false,
        };
        let owned = function
            .parameters
            .get(index)
            .is_some_and(|parameter| crate::plan::owned_capture(parameter.ty.clone()));
        moved == (owned || consumes_environment)
            && matches!(place.projection, Projection::Whole)
            && usize::try_from(place.local).ok() == Some(index + 1)
    });
    if !forwarded
        || arguments.len() != function.parameters.len()
        || destination.local != 0
        || !matches!(destination.projection, Projection::Whole)
    {
        return Err(refused());
    }
    Ok(())
}

/// Reject unsupported hashed leaves and invalid dimensions before constructing native types, including unused locals.
///
/// A model's physical hashed storage requires its declared Hash layout (see [`hashed_model_layout`]); actual Eq and
/// Hash implementations are verified against native callee bounds before MIR. Enum payload derivation keeps the
/// stricter [`hashable_model`] and [`hashable_enum`] authority. Lowering separately refuses keyed dictionary lookups
/// whose runtime also bounds Display.
pub(super) fn validate_array_length(plan: &Plan, function: &Function, ty: Scalar) -> Result<(), PlanError> {
    match ty {
        Scalar::Generator(_, depth)
        | Scalar::GeneratorMutRef(_, depth)
        | Scalar::GeneratorYield(_, depth)
        | Scalar::GeneratorYieldRef(_, depth)
            if depth < 0 =>
        {
            Err(invalid(function, "generator element depth must be nonnegative"))
        }
        Scalar::Set(Leaf::Tuple(elements))
        | Scalar::SetRef(Leaf::Tuple(elements))
        | Scalar::SetMutRef(Leaf::Tuple(elements))
        | Scalar::Dict(Leaf::Tuple(elements), _)
        | Scalar::DictRef(Leaf::Tuple(elements), _)
        | Scalar::DictMutRef(Leaf::Tuple(elements), _)
            if elements.contains(&Scalar::Float) =>
        {
            Err(invalid(function, "floating-point tuple hashed keys lack Eq and Hash"))
        }
        Scalar::Set(Leaf::Enum(index))
        | Scalar::SetRef(Leaf::Enum(index))
        | Scalar::SetMutRef(Leaf::Enum(index))
        | Scalar::Dict(Leaf::Enum(index), _)
        | Scalar::DictRef(Leaf::Enum(index), _)
        | Scalar::DictMutRef(Leaf::Enum(index), _)
            if hashable_enum(plan, index) =>
        {
            Ok(())
        }
        Scalar::Set(Leaf::Enum(_))
        | Scalar::SetRef(Leaf::Enum(_))
        | Scalar::SetMutRef(Leaf::Enum(_))
        | Scalar::Dict(Leaf::Enum(_), _)
        | Scalar::DictRef(Leaf::Enum(_), _)
        | Scalar::DictMutRef(Leaf::Enum(_), _) => Err(invalid(
            function,
            "enum hashed keys require retained Eq and Hash authority",
        )),
        Scalar::Set(Leaf::Model(index))
        | Scalar::SetRef(Leaf::Model(index))
        | Scalar::SetMutRef(Leaf::Model(index))
        | Scalar::Dict(Leaf::Model(index), _)
        | Scalar::DictRef(Leaf::Model(index), _)
        | Scalar::DictMutRef(Leaf::Model(index), _)
            if hashed_model_layout(plan, index) =>
        {
            Ok(())
        }
        Scalar::Set(Leaf::Model(_))
        | Scalar::SetRef(Leaf::Model(_))
        | Scalar::SetMutRef(Leaf::Model(_))
        | Scalar::Dict(Leaf::Model(_), _)
        | Scalar::DictRef(Leaf::Model(_), _)
        | Scalar::DictMutRef(Leaf::Model(_), _) => Err(invalid(
            function,
            "model hashed storage requires retained Hash layout authority",
        )),
        Scalar::Set(Leaf::Numeric(element))
        | Scalar::SetRef(Leaf::Numeric(element))
        | Scalar::SetMutRef(Leaf::Numeric(element))
        | Scalar::Dict(Leaf::Numeric(element), _)
        | Scalar::DictRef(Leaf::Numeric(element), _)
        | Scalar::DictMutRef(Leaf::Numeric(element), _)
            if matches!(element, Numeric::F32 | Numeric::F64) =>
        {
            Err(invalid(function, "floating-point hashed keys lack Eq and Hash"))
        }
        Scalar::Set(Leaf::Float)
        | Scalar::SetRef(Leaf::Float)
        | Scalar::SetMutRef(Leaf::Float)
        | Scalar::Dict(Leaf::Float, _)
        | Scalar::DictRef(Leaf::Float, _)
        | Scalar::DictMutRef(Leaf::Float, _) => Err(invalid(function, "floating-point hashed keys lack Eq and Hash")),
        Scalar::StringArray(count)
        | Scalar::StrArray(count)
        | Scalar::StringArrayRef(count)
        | Scalar::StrArrayRef(count)
            if count < 0 =>
        {
            Err(invalid(function, "formatting array length must be nonnegative"))
        }
        Scalar::List(_, depth)
        | Scalar::ListRef(_, depth)
        | Scalar::ListMutRef(_, depth)
        | Scalar::ListIntoIter(_, depth)
        | Scalar::ListIntoIterMutRef(_, depth)
            if depth < 1 =>
        {
            Err(invalid(function, "list depth must be positive"))
        }
        _ => Ok(()),
    }
}

/// Check a formatting array's exact length and each element against its declared destination type.
pub(super) fn validate_array(
    plan: &Plan,
    function: &Function,
    values: &[Operand],
    expected: &Scalar,
) -> Result<Scalar, PlanError> {
    let (element, count) = match expected {
        Scalar::StringArray(count) => (Scalar::String, *count),
        Scalar::StrArray(count) => (Scalar::StrRef, *count),
        _ => {
            return Err(invalid(
                function,
                "array expression requires a formatting-array destination",
            ));
        }
    };
    if usize::try_from(count).ok() != Some(values.len()) {
        return Err(invalid(function, "formatting array length differs from its elements"));
    }
    for value in values {
        require(
            function,
            operand(plan, function, value)?,
            &element,
            "formatting array element",
        )?;
    }
    Ok(match element {
        Scalar::String => Scalar::StringArray(count),
        _ => Scalar::StrArray(count),
    })
}

/// Integer arithmetic retains its exact carrier unless an explicit pair destination requests checked arithmetic.
/// Lowering selects that destination from the unit policy or an explicit helper, independently of rustc defaults.
pub(super) fn binary_result(
    function: &Function,
    op: &BinaryOp,
    ty: Scalar,
    expected: Scalar,
) -> Result<Scalar, PlanError> {
    if let Some(kind) = sized_numeric(&ty) {
        return match op {
            BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply if sized_integer(&ty) => {
                let checked = Scalar::CheckedNumeric(kind);
                Ok(if expected == checked { checked } else { ty })
            }
            BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply | BinaryOp::Divide
                if matches!(ty, Scalar::F32 | Scalar::F64) =>
            {
                Ok(ty)
            }
            BinaryOp::Equal
            | BinaryOp::NotEqual
            | BinaryOp::Less
            | BinaryOp::LessEqual
            | BinaryOp::Greater
            | BinaryOp::GreaterEqual => Ok(Scalar::Bool),
            BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor if sized_integer(&ty) => Ok(ty),
            _ => Err(invalid(function, "unsupported sized numeric operator")),
        };
    }
    match op {
        BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor if ty == Scalar::Int => Ok(Scalar::Int),
        BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply if ty == Scalar::Int => {
            Ok(if expected == Scalar::CheckedInt {
                Scalar::CheckedInt
            } else {
                Scalar::Int
            })
        }
        BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply | BinaryOp::Divide if ty == Scalar::Float => {
            Ok(Scalar::Float)
        }
        BinaryOp::Equal | BinaryOp::NotEqual if matches!(ty, Scalar::Int | Scalar::Float | Scalar::Bool) => {
            Ok(Scalar::Bool)
        }
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual
            if matches!(ty, Scalar::Int | Scalar::Float) =>
        {
            Ok(Scalar::Bool)
        }
        _ => Err(invalid(function, "invalid binary operand type")),
    }
}

/// A shift keeps its left operand's integer type and counts with any integer type, as Rust's own shifts do.
pub(super) fn shift_result(function: &Function, ty: Scalar, count: &Scalar) -> Result<Scalar, PlanError> {
    if integer(&ty) && integer(count) {
        Ok(ty)
    } else {
        Err(invalid(function, "shift operands must be integers"))
    }
}
