//! Function body validation: locals, places, field projections, operands and control-flow edges.

use super::*;

/// Check the parameter-local convention and all nodes in one function. Exact scalar inputs borrow caller
/// storage; return types keep their separate lifetime and representation gate. Validated future boxing frames also
/// admit the exact native inputs whose loans their constructor lifetime retains.
pub(super) fn validate_function(plan: &Plan, function: &Function) -> Result<(), PlanError> {
    span(&function.span)?;
    let outputs = match carried_output(plan, function) {
        Some(carrier) => crate::declarations::standard_carrier(carrier).1,
        None => vec![function.return_type.clone()],
    };
    if outputs
        .iter()
        .chain(function.parameters.iter().map(|parameter| &parameter.ty))
        .any(|ty| borrowed_alias_in_source_type(plan, ty, &mut BTreeSet::new()))
        && function.signature_lifetimes.is_empty()
    {
        return Err(invalid(
            function,
            "native borrowed carrier in a source signature requires a retained lifetime contract",
        ));
    }
    validate_future_constructor(plan, function)?;
    if function.blocks.is_empty() || function.blocks[0].cleanup {
        return Err(invalid(
            function,
            "entry block must exist and cannot be a cleanup block",
        ));
    }
    if function.locals.len() <= function.parameters.len() {
        return Err(invalid(function, "locals must include return place and all parameters"));
    }
    let returned = scalar(&function.return_type);
    // A checked element reborrow may return an exclusive model reference; the lifetime-contract guard above still
    // applies.
    if scalar(&function.locals[0].ty) != returned || !source_return_type(&returned) {
        return Err(invalid(function, "return local must match a scalar return type"));
    }
    let mut parameters = BTreeSet::new();
    for (index, parameter) in function.parameters.iter().enumerate() {
        span(&parameter.span)?;
        if !identifier(&parameter.name) || !parameters.insert(&parameter.name) {
            return Err(invalid(function, "parameter name is invalid or duplicated"));
        }
        let parameter_type = scalar(&parameter.ty);
        let native_capture = function.future_callee.is_some() && external_signature_type(&parameter_type);
        if !source_parameter_type(&parameter_type) && !native_capture {
            return Err(invalid(
                function,
                format!("parameter `{}` has unsupported type {:?}", parameter.name, parameter.ty),
            ));
        }
        if scalar(&parameter.ty) != scalar(&function.locals[index + 1].ty) {
            return Err(invalid(
                function,
                format!(
                    "parameter `{}` local {:?} differs from its declaration {:?}",
                    parameter.name,
                    function.locals[index + 1].ty,
                    parameter.ty
                ),
            ));
        }
    }
    for local in &function.locals {
        if matches!(
            local.ty,
            PlanType::NativeStr | PlanType::NativeSlice(_) | PlanType::NativeTraitObject(..)
        ) {
            return Err(invalid(function, "unsized native referent cannot occupy a value frame"));
        }
        span(&local.span)?;
        validate_model_type(plan, &local.ty)?;
        validate_array_length(plan, function, scalar(&local.ty))?;
    }
    for block in &function.blocks {
        span(&block.span)?;
        for statement in &block.statements {
            span(&statement.span)?;
            match &statement.kind {
                StatementKind::Assign(destination, value) => {
                    writable_tuple_place(function, destination)?;
                    span(&value.span)?;
                    let expected = place(plan, function, destination)?;
                    let result = rvalue(plan, function, &value.kind, expected.clone())?;
                    require(function, result, expected, "assignment")?;
                }
                StatementKind::StorageLive(index) | StatementKind::StorageDead(index) => {
                    local(function, *index)?;
                    if *index
                        <= i64::try_from(function.parameters.len())
                            .map_err(|_| invalid(function, "too many parameters"))?
                    {
                        return Err(invalid(
                            function,
                            "storage markers cannot target return place or parameters",
                        ));
                    }
                }
            }
        }
        span(&block.terminator.span)?;
        terminator(plan, function, &block.terminator.kind, block.cleanup)?;
    }
    Ok(())
}

/// A boxing shim retains an exact parameter frame and a proven source coroutine or canonical native callee.
pub(super) fn validate_future_constructor(plan: &Plan, function: &Function) -> Result<(), PlanError> {
    let Some(callee) = &function.future_callee else {
        return Ok(());
    };
    let PlanType::Future(outputs) = &function.return_type else {
        return Err(invalid(function, "unsupported future constructor return carrier"));
    };
    let [output] = outputs.as_slice() else {
        return Err(invalid(function, "unsupported future constructor output arity"));
    };
    if function.is_async
        || function.locals.len() != function.parameters.len() + 1
        || function.blocks.len() != 1
        || !function.blocks[0].statements.is_empty()
        || !matches!(function.blocks[0].terminator.kind, TerminatorKind::Unreachable)
    {
        return Err(invalid(function, "unsupported future boxing constructor frame"));
    }
    match &callee.kind {
        CalleeKind::Planned(name) => {
            let target = plan
                .functions
                .iter()
                .find(|target| target.name == *name)
                .ok_or_else(|| invalid(function, "unsupported missing future source declaration"))?;
            if !target.is_async
                || target.return_type != *output
                || target.parameters.len() != function.parameters.len()
                || target
                    .parameters
                    .iter()
                    .zip(&function.parameters)
                    .any(|(target, parameter)| target.ty != parameter.ty)
            {
                return Err(invalid(
                    function,
                    "unsupported future source signature differs from its constructor",
                ));
            }
        }
        CalleeKind::External(path) | CalleeKind::Instantiated(path, _) | CalleeKind::FutureMethod(path, _) => {
            if path.split("::").count() < 2 || !path.split("::").all(identifier) {
                return Err(invalid(function, "unsupported future native callee path"));
            }
            if let CalleeKind::Instantiated(_, arguments) | CalleeKind::FutureMethod(_, arguments) = &callee.kind {
                for argument in arguments {
                    validate_model_type(plan, argument)?;
                }
            }
            if matches!(callee.kind, CalleeKind::FutureMethod(..))
                && (path.split("::").count() < 3
                    || !function
                        .parameters
                        .first()
                        .is_some_and(|parameter| crate::futures::borrowed_parameter(&parameter.ty)))
            {
                return Err(invalid(
                    function,
                    "unsupported SDK method future without an exact receiver",
                ));
            }
        }
        _ => {
            return Err(invalid(function, "unsupported future constructor callee kind"));
        }
    }
    Ok(())
}

/// Refuse signed, oversized, or dangling local indices before constructing rustc index types.
pub(super) fn local(function: &Function, index: i64) -> Result<Scalar, PlanError> {
    usize::try_from(index)
        .ok()
        .and_then(|index| function.locals.get(index))
        .map(|local| scalar(&local.ty))
        .ok_or_else(|| PlanError::UnknownLocal {
            function: function.name.clone(),
            index,
        })
}

/// Reject assignments and call results that would write through a shared tuple reference.
///
/// Replacing a whole reference slot is distinct from changing its referent; projections preserve the shared root's
/// read-only capability even after their pointee type has been recovered.
pub(super) fn writable_tuple_place(function: &Function, destination: &Place) -> Result<(), PlanError> {
    if !matches!(destination.projection, Projection::Whole)
        && matches!(local(function, destination.local)?, Scalar::TupleRef(_))
    {
        return Err(invalid(function, "cannot write through a shared tuple reference"));
    }
    Ok(())
}

/// Recover a field path's owned shape from a shared or exclusive reference; non-dereferenced roots keep their
/// retained shape. Shared enum and tuple receivers permit reads; mutable reborrows remain guarded by `borrow_result`.
pub(super) fn field_owner(function: &Function, ty: Scalar, dereference: bool) -> Result<Scalar, PlanError> {
    if !dereference {
        return Ok(ty);
    }
    match ty {
        Scalar::ModelRef(owner) | Scalar::ModelMutRef(owner) => Ok(Scalar::Model(owner)),
        Scalar::EnumRef(owner) | Scalar::EnumMutRef(owner) => Ok(Scalar::Enum(owner)),
        Scalar::TupleRef(elements) | Scalar::TupleMutRef(elements) => Ok(Scalar::Tuple(elements)),
        Scalar::ScalarRef(elements) | Scalar::ScalarMutRef(elements) if elements.len() == 1 => Ok(elements[0].clone()),
        Scalar::StringRef | Scalar::StringMutRef => Ok(Scalar::String),
        Scalar::Boxed(elements) if elements.len() == 1 => Ok(elements[0].clone()),
        _ => Err(invalid(
            function,
            "field dereference requires a supported receiver reference",
        )),
    }
}

/// Validate one field against its retained tuple or nominal layout before allowing another path step.
pub(super) fn projected_field(
    plan: &Plan,
    function: &Function,
    owner: Scalar,
    slot: i64,
    field_type: &PlanType,
) -> Result<Scalar, PlanError> {
    validate_model_type(plan, field_type)?;
    let expected = match owner {
        Scalar::Tuple(elements) => usize::try_from(slot)
            .ok()
            .and_then(|slot| elements.get(slot))
            .cloned()
            .ok_or_else(|| invalid(function, "unknown tuple field"))?,
        Scalar::Model(owner) => {
            let model = model(plan, owner).ok_or_else(|| invalid(function, "unknown model owner"))?;
            if model.unavailable_native_fields.contains(&slot) {
                return Err(invalid(function, "cannot project an opaque native field"));
            }
            let field = usize::try_from(slot)
                .ok()
                .and_then(|slot| model.fields.get(slot))
                .ok_or_else(|| invalid(function, "unknown model field"))?;
            scalar(&field.ty)
        }
        Scalar::RangeValue => usize::try_from(slot)
            .ok()
            .and_then(|slot| range_field_types().get(slot).map(scalar))
            .ok_or_else(|| invalid(function, "unknown range field"))?,
        _ => {
            return Err(invalid(
                function,
                "field projection requires a matching nominal, tuple or range owner",
            ));
        }
    };
    require(function, scalar(field_type), &expected, "projected field type")?;
    Ok(expected)
}

/// Validate each enum downcast against the exact owner, active variant slot, and retained payload field type.
pub(super) fn projected_variant_field(
    plan: &Plan,
    function: &Function,
    owner: Scalar,
    variant: i64,
    slot: i64,
    field_type: &PlanType,
) -> Result<Scalar, PlanError> {
    validate_model_type(plan, field_type)?;
    let Scalar::Enum(owner) = owner else {
        return Err(invalid(function, "variant projection requires an enum owner"));
    };
    let declaration = enum_declaration(plan, owner).ok_or_else(|| invalid(function, "unknown enum owner"))?;
    let field = usize::try_from(variant)
        .ok()
        .and_then(|index| declaration.variants.get(index))
        .and_then(|variant| usize::try_from(slot).ok().and_then(|index| variant.fields.get(index)))
        .ok_or_else(|| invalid(function, "unknown enum payload field"))?;
    require(function, scalar(field_type), scalar(field), "enum payload projection")?;
    Ok(scalar(field))
}

/// Resolve checked arithmetic, list dereferences, and nominal projections, verifying reference shape and canonical
/// pointee or field type.
pub(super) fn place(plan: &Plan, function: &Function, value: &Place) -> Result<Scalar, PlanError> {
    span(&value.span)?;
    let ty = local(function, value.local)?;
    match &value.projection {
        Projection::Whole => Ok(ty),
        Projection::VariantField(variant, slot, field_type) => {
            validate_model_type(plan, field_type)?;
            let Scalar::Enum(owner) = ty else {
                return Err(invalid(function, "variant projection requires an enum owner"));
            };
            let declaration = enum_declaration(plan, owner).ok_or_else(|| invalid(function, "unknown enum owner"))?;
            let field = usize::try_from(*variant)
                .ok()
                .and_then(|index| declaration.variants.get(index))
                .and_then(|variant| usize::try_from(*slot).ok().and_then(|index| variant.fields.get(index)))
                .ok_or_else(|| invalid(function, "unknown enum payload field"))?;
            require(function, scalar(field_type), scalar(field), "enum payload projection")?;
            Ok(scalar(field_type))
        }
        Projection::Deref(field_type) => {
            let pointee = match ty {
                Scalar::SliceRef(elements) | Scalar::SliceMutRef(elements) => Scalar::NativeSlice(elements),
                Scalar::NativeArrayRef(elements, length) | Scalar::NativeArrayMutRef(elements, length) => {
                    Scalar::NativeArray(elements, length)
                }
                Scalar::ScalarRef(elements) | Scalar::ScalarMutRef(elements) | Scalar::RawPointer(_, elements) => {
                    let [inner] = elements.as_slice() else {
                        return Err(invalid(function, "unsupported scalar reference arity"));
                    };
                    inner.clone()
                }
                Scalar::ValidationErrorRef => Scalar::ValidationError,
                Scalar::FrozenCollectionRef(name, elements) => Scalar::FrozenCollection(name, elements),
                Scalar::ModelRef(owner) | Scalar::ModelMutRef(owner) => Scalar::Model(owner),
                Scalar::EnumRef(owner) => Scalar::Enum(owner),
                Scalar::ZipIteratorRef(left, right) | Scalar::ZipIteratorMutRef(left, right) => {
                    Scalar::ZipIterator(left, right)
                }
                Scalar::GeneratorMutRef(leaf, depth) => Scalar::Generator(leaf, depth),
                Scalar::ListIntoIterMutRef(leaf, depth) => Scalar::ListIntoIter(leaf, depth),
                Scalar::StringRef | Scalar::StringMutRef => Scalar::String,
                Scalar::EnumMutRef(owner) => Scalar::Enum(owner),
                Scalar::TupleRef(elements) | Scalar::TupleMutRef(elements) => Scalar::Tuple(elements),
                Scalar::ListRef(leaf, depth) | Scalar::ListMutRef(leaf, depth) => Scalar::List(leaf, depth),
                Scalar::SetRef(leaf) | Scalar::SetMutRef(leaf) => Scalar::Set(leaf),
                Scalar::DictRef(key, value) | Scalar::DictMutRef(key, value) => Scalar::Dict(key, value),
                _ => {
                    return Err(invalid(
                        function,
                        "dereference requires a collection or nominal reference",
                    ));
                }
            };
            require(function, scalar(field_type), &pointee, "dereferenced owner")?;
            Ok(pointee)
        }
        Projection::Field(slot, field_type) | Projection::DerefField(slot, field_type) => {
            let owner = field_owner(function, ty, matches!(value.projection, Projection::DerefField(..)))?;
            projected_field(plan, function, owner, *slot, field_type)
        }
        Projection::Fields(fields) | Projection::DerefFields(fields) => {
            if fields.is_empty() {
                return Err(invalid(function, "field path must retain at least one field"));
            }
            let mut owner = field_owner(function, ty, matches!(value.projection, Projection::DerefFields(_)))?;
            for field in fields {
                let dereferences = usize::try_from(field.dereferences)
                    .map_err(|_| invalid(function, "field dereference count must be nonnegative"))?;
                for _ in 0..dereferences {
                    owner = field_owner(function, owner, true)?;
                }
                if field.slot == -1 {
                    if dereferences == 0 || field.variant != -1 {
                        return Err(invalid(
                            function,
                            "dereference-only step requires a loan and no variant",
                        ));
                    }
                    require(function, scalar(&field.ty), &owner, "dereference step")?;
                    continue;
                }
                owner = match field.variant {
                    -1 => projected_field(plan, function, owner, field.slot, &field.ty)?,
                    variant if variant >= 0 => {
                        projected_variant_field(plan, function, owner, variant, field.slot, &field.ty)?
                    }
                    _ => return Err(invalid(function, "invalid field path variant")),
                };
            }
            Ok(owner)
        }
        Projection::NumericValue(kind)
            if sized_numeric(&scalar(kind)).is_some_and(|kind| ty == Scalar::CheckedNumeric(kind)) =>
        {
            Ok(scalar(kind))
        }
        Projection::Overflow if matches!(ty, Scalar::CheckedNumeric(_)) => Ok(Scalar::Bool),
        Projection::Value if ty == Scalar::CheckedInt => Ok(Scalar::Int),
        Projection::Overflow if ty == Scalar::CheckedInt => Ok(Scalar::Bool),
        _ => Err(invalid(function, "only checked integer results support projections")),
    }
}

/// Check an edge's index and whether it crosses the normal/cleanup boundary correctly.
pub(super) fn edge(function: &Function, index: i64, cleanup: bool) -> Result<(), PlanError> {
    let block = usize::try_from(index)
        .ok()
        .and_then(|index| function.blocks.get(index))
        .ok_or_else(|| PlanError::UnknownBlock {
            function: function.name.clone(),
            index,
        })?;
    if block.cleanup != cleanup {
        return Err(invalid(function, "edge crosses normal and cleanup control flow"));
    }
    Ok(())
}

/// Cleanup edges must name cleanup blocks; cleanup code cannot install a nested cleanup edge.
pub(super) fn unwind(function: &Function, action: &Unwind, cleanup: bool) -> Result<(), PlanError> {
    match action {
        Unwind::Continue if !cleanup => Ok(()),
        Unwind::Cleanup(index) if !cleanup => edge(function, *index, true),
        Unwind::Terminate if cleanup => Ok(()),
        _ => Err(invalid(function, "cleanup code cannot unwind again")),
    }
}

/// Infer a source operand's exact scalar representation.
pub(super) fn operand(plan: &Plan, function: &Function, value: &Operand) -> Result<Scalar, PlanError> {
    span(&value.span)?;
    match &value.kind {
        OperandKind::Copy(value) => {
            let ty = place(plan, function, value)?;
            if let Scalar::Model(index) = ty
                && plan
                    .models
                    .get(usize::try_from(index).map_err(|_| invalid(function, "invalid Copy model index"))?)
                    .is_some_and(|model| {
                        model
                            .derives
                            .iter()
                            .any(|derive| derives::from_str(derive) == Some(DeriveId::Copy))
                    })
            {
                return Ok(ty);
            }
            if matches!(ty, Scalar::Enum(index) if enum_declaration(plan, index).is_none_or(|value| {
                !value
                    .derives
                    .iter()
                    .any(|derive| derives::from_str(derive) == Some(DeriveId::Copy))
            })) {
                return Err(invalid(function, "non-Copy enum cannot be copied"));
            }
            if owns_values(&ty)
                || matches!(
                    ty,
                    Scalar::String
                        | Scalar::ZipIterator(_, _)
                        | Scalar::ZipIteratorMutRef(_, _)
                        | Scalar::Generator(_, _)
                        | Scalar::GeneratorMutRef(_, _)
                        | Scalar::ListIntoIter(_, _)
                        | Scalar::ListIntoIterMutRef(_, _)
                        | Scalar::GeneratorYield(_, _)
                        | Scalar::StringArray(_)
                        | Scalar::Model(_)
                        | Scalar::StringMutRef
                        | Scalar::SliceMutRef(_)
                        | Scalar::EnumMutRef(_)
                        | Scalar::TupleMutRef(_)
                        | Scalar::List(_, _)
                        | Scalar::ListMutRef(_, _)
                        | Scalar::Set(_)
                        | Scalar::SetMutRef(_)
                        | Scalar::Dict(_, _)
                        | Scalar::DictMutRef(_, _)
                )
            {
                return Err(invalid(
                    function,
                    format!("non-Copy value {ty:?} at {value:?} cannot be copied"),
                ));
            }
            Ok(ty)
        }
        OperandKind::Move(value) => place(plan, function, value),
        OperandKind::Literal(value) => Ok(match value {
            Constant::RustItem(_, ty) => scalar(ty),
            Constant::Int(_) => Scalar::Int,
            Constant::Numeric(_, ty) => scalar(ty),
            Constant::Float(_) => Scalar::Float,
            Constant::Bool(_) => Scalar::Bool,
            Constant::Unit => Scalar::Unit,
            Constant::TypeToken(represented) => Scalar::TypeToken(represented.iter().map(scalar).collect()),
            Constant::Text(_) => Scalar::StrRef,
        }),
    }
}

/// Reject implicit copies of any recursively owned value, including tuple string fields.
pub(super) fn owns_values(ty: &Scalar) -> bool {
    match ty {
        Scalar::ScalarMutRef(_) | Scalar::SliceMutRef(_) | Scalar::ModelMutRef(_) => true,
        Scalar::String
        | Scalar::StringArray(_)
        | Scalar::Model(_)
        | Scalar::ClosureOnce(_)
        | Scalar::Closure(_)
        | Scalar::NativeClosure(..)
        | Scalar::BorrowingClosure(_)
        | Scalar::SharedCallable(_)
        | Scalar::ValidationError
        | Scalar::Boxed(_)
        | Scalar::Future(_) => true,
        Scalar::Tuple(elements) => elements.iter().any(owns_values),
        _ => false,
    }
}

/// Keep type mismatches explicit rather than allowing rustc MIR validation to ICE.
pub(super) fn require(
    function: &Function,
    actual: Scalar,
    expected: impl std::borrow::Borrow<Scalar>,
    context: &str,
) -> Result<(), PlanError> {
    let expected = expected.borrow();
    if &actual != expected {
        return Err(invalid(
            function,
            format!("{context}: expected {expected:?}, found {actual:?}"),
        ));
    }
    Ok(())
}
