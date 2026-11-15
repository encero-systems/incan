//! Callee signatures, generator and closure calls, and terminator validation.

use super::*;

/// Resolve a function reference against source-named declarations before invoking rustc.
pub(super) fn signature(plan: &Plan, function: &Function, callee: &Callee) -> Result<(Vec<Scalar>, Scalar), PlanError> {
    span(&callee.span)?;
    match &callee.kind {
        CalleeKind::InstallPanicHook(signature) => {
            if signature.len() != 2 || signature.last() != Some(&PlanType::Unit) {
                return Err(invalid(
                    function,
                    "native panic callback requires one input and no result",
                ));
            }
            Ok((
                vec![scalar(&PlanType::FunctionPointer(signature.clone()))],
                Scalar::Unit,
            ))
        }
        CalleeKind::FormatValue(ty, _) => {
            validate_model_type(plan, ty)?;
            Ok((vec![scalar(ty)], Scalar::String))
        }
        CalleeKind::SpawnGenerator(name, leaf, depth) => generator_spawn_signature(plan, name, leaf, *depth),
        CalleeKind::YieldGenerator(leaf, depth) => Ok((
            vec![
                Scalar::GeneratorYieldRef(Leaf::of(leaf), *depth),
                generator_element(leaf, *depth)?,
            ],
            Scalar::Unit,
        )),
        CalleeKind::CollectGenerator(leaf, depth) => Ok((
            vec![Scalar::Generator(Leaf::of(leaf), *depth)],
            Scalar::List(Leaf::of(leaf), depth + 1),
        )),
        CalleeKind::CallClosure(signature) => closure_call_signature(function, signature, false),
        CalleeKind::CallClosureOnce(signature) => closure_call_signature(function, signature, true),
        CalleeKind::CallBorrowingClosure(signature) => {
            // The same call through the shared view of an environment that holds a borrowed reference.
            let (mut inputs, result) = closure_call_signature(function, signature, false)?;
            inputs[0] = Scalar::BorrowingClosureRef(signature.iter().map(scalar).collect());
            Ok((inputs, result))
        }
        CalleeKind::CloneClosure(signature) => closure_clone_signature(function, signature),
        CalleeKind::ClosureBody(name, count) | CalleeKind::ClosureBodyOnce(name, count) => {
            lifted_closure_signature(plan, function, callee, name, *count)
        }
        CalleeKind::SharedCallableBody(signature) => Ok((
            vec![Scalar::Closure(signature.iter().map(scalar).collect())],
            Scalar::SharedCallable(signature.iter().map(scalar).collect()),
        )),
        CalleeKind::Value(value) => {
            let mut signature = match operand(plan, function, value)? {
                Scalar::FunctionPointer(signature) | Scalar::FunctionItem(_, signature) => signature,
                _ => return Err(invalid(function, "local call requires a callable type")),
            };
            let result = signature
                .pop()
                .ok_or_else(|| invalid(function, "local callable has no result type"))?;
            Ok((signature, result))
        }
        CalleeKind::StaticLoad(slot) => Ok((Vec::new(), scalar(&crate::statics::cell(plan, *slot)?.ty))),
        CalleeKind::StaticStore(slot) => Ok((vec![scalar(&crate::statics::cell(plan, *slot)?.ty)], Scalar::Unit)),
        CalleeKind::StaticForce(slot) => {
            crate::statics::cell(plan, *slot)?;
            Ok((Vec::new(), Scalar::Unit))
        }
        CalleeKind::ModuleInitialized(slot) | CalleeKind::ModuleComplete(slot) => {
            if !crate::statics::cell(plan, *slot)?.module_completion {
                return Err(invalid(function, "module completion requires its first computed cell"));
            }
            let result = if matches!(callee.kind, CalleeKind::ModuleInitialized(_)) {
                Scalar::Bool
            } else {
                Scalar::Unit
            };
            Ok((Vec::new(), result))
        }
        CalleeKind::FieldMetadata(owner, info) => {
            Ok((Vec::new(), scalar(&crate::field_metadata::result(plan, *owner, *info)?)))
        }
        CalleeKind::StaticMethod(slot, name) => {
            let method = crate::statics::method_target(plan, *slot, name)?;
            Ok((
                method
                    .parameters
                    .iter()
                    .skip(1)
                    .map(|parameter| scalar(&parameter.ty))
                    .collect(),
                scalar(&method.return_type),
            ))
        }
        CalleeKind::Planned(name) => planned_function_signature(plan, name),
        CalleeKind::CloneEnum(index, name) => clone_enum_signature(plan, *index, name),
        CalleeKind::CloneModel(index, name) => {
            let declaration = model(plan, *index).ok_or_else(|| PlanError::UnknownCallee(name.clone()))?;
            if declaration.name != *name {
                return Err(PlanError::UnknownCallee(name.clone()));
            }
            Ok((vec![Scalar::ModelRef(*index)], Scalar::Model(*index)))
        }
        CalleeKind::FutureMethod(..) => Err(invalid(
            function,
            "unsupported SDK method future outside its boxing shim",
        )),
        CalleeKind::Foreign(declaration) => {
            if !identifier(&declaration.name)
                || declaration.symbol.is_empty()
                || declaration.library.is_empty()
                || declaration.symbol.contains('\0')
                || declaration.library.contains('\0')
            {
                return Err(invalid(function, "invalid verified foreign declaration"));
            }
            Ok((
                declaration.parameters.iter().map(scalar).collect(),
                scalar(&declaration.return_type),
            ))
        }
        CalleeKind::External(path) | CalleeKind::Instantiated(path, _) => external_callee_signature(plan, callee, path),
    }
}

/// Require the exact enum declaration and its checked Clone derive before admitting the borrowed clone call.
pub(super) fn clone_enum_signature(plan: &Plan, index: i64, name: &str) -> Result<(Vec<Scalar>, Scalar), PlanError> {
    let declaration = enum_declaration(plan, index).ok_or_else(|| PlanError::UnknownCallee(name.into()))?;
    if declaration.name != name
        || !declaration
            .derives
            .iter()
            .any(|derive| derives::from_str(derive) == Some(DeriveId::Clone))
    {
        return Err(PlanError::UnknownCallee(name.into()));
    }
    Ok((vec![Scalar::EnumRef(index)], Scalar::Enum(index)))
}

/// Select the retained external by exact path and native argument carriers; a plain external admits only the
/// non-generic declaration. Parameter/result projection follows the existing first matching declaration rule.
pub(super) fn external_callee_signature(
    plan: &Plan,
    callee: &Callee,
    path: &str,
) -> Result<(Vec<Scalar>, Scalar), PlanError> {
    let external = plan
        .externals
        .iter()
        .find(|external| {
            external.path == path
                && match &callee.kind {
                    CalleeKind::Instantiated(_, types) => {
                        external.type_arguments.iter().map(scalar).eq(types.iter().map(scalar))
                    }
                    _ => external.type_arguments.is_empty(),
                }
        })
        .ok_or_else(|| PlanError::UnknownCallee(path.to_string()))?;
    Ok((
        external.parameters.iter().map(scalar).collect(),
        scalar(&external.return_type),
    ))
}

/// Resolve a generator producer and validate its yield-handle input before projecting the caller's remaining inputs.
pub(super) fn generator_spawn_signature(
    plan: &Plan,
    name: &str,
    leaf: &ListLeaf,
    depth: i64,
) -> Result<(Vec<Scalar>, Scalar), PlanError> {
    let producer = plan
        .functions
        .iter()
        .find(|function| function.name == name)
        .ok_or_else(|| PlanError::UnknownCallee(name.to_string()))?;
    if producer.parameters.is_empty()
        || scalar(&producer.parameters[0].ty) != Scalar::GeneratorYield(Leaf::of(leaf), depth)
        || scalar(&producer.return_type) != Scalar::Unit
    {
        return Err(invalid(
            producer,
            "generator producer signature differs from yield handle contract",
        ));
    }
    Ok((
        producer.parameters[1..]
            .iter()
            .map(|parameter| scalar(&parameter.ty))
            .collect(),
        Scalar::Generator(Leaf::of(leaf), depth),
    ))
}

/// Validate one callable object's signature before exposing its receiver and packed inputs. Consuming and borrowed
/// calls keep their distinct carrier and refusal text while sharing the same admitted-signature rule.
pub(super) fn closure_call_signature(
    function: &Function,
    signature: &[PlanType],
    consuming: bool,
) -> Result<(Vec<Scalar>, Scalar), PlanError> {
    let signature: Vec<_> = signature.iter().map(scalar).collect();
    if !callable_object_signature(&signature) {
        let reason = if consuming {
            "consuming callable-object signature is unsupported"
        } else {
            "callable-object call requires admitted inputs and a result"
        };
        return Err(invalid(function, reason));
    }
    let (result, inputs) = signature.split_last().ok_or_else(|| {
        invalid(
            function,
            if consuming {
                "consuming callable-object has no result"
            } else {
                "callable-object call has no result type"
            },
        )
    })?;
    let receiver = if consuming {
        Scalar::ClosureOnce(signature.clone())
    } else {
        Scalar::ClosureRef(signature.clone())
    };
    Ok((vec![receiver, Scalar::Tuple(inputs.to_vec())], result.clone()))
}

/// Check a cloneable callable's admitted signature before retaining its borrowed input and owned output carriers.
pub(super) fn closure_clone_signature(
    function: &Function,
    signature: &[PlanType],
) -> Result<(Vec<Scalar>, Scalar), PlanError> {
    let signature: Vec<_> = signature.iter().map(scalar).collect();
    if !callable_object_signature(&signature) {
        return Err(invalid(
            function,
            "callable-object clone requires admitted inputs and a result",
        ));
    }
    Ok((vec![Scalar::ClosureRef(signature.clone())], Scalar::Closure(signature)))
}

/// Separate capture inputs from a retained lifted function's callable signature, preserving consuming closure
/// precedence over a native closure result's Send/Sync facts. Invalid capture counts retain the caller's diagnostic.
pub(super) fn lifted_closure_signature(
    plan: &Plan,
    function: &Function,
    callee: &Callee,
    name: &str,
    count: i64,
) -> Result<(Vec<Scalar>, Scalar), PlanError> {
    let lifted = plan
        .functions
        .iter()
        .find(|lifted| lifted.name == name)
        .ok_or_else(|| PlanError::UnknownCallee(name.to_string()))?;
    let count = usize::try_from(count)
        .ok()
        .filter(|count| *count <= lifted.parameters.len())
        .ok_or_else(|| invalid(function, "closure capture count exceeds the lifted parameters"))?;
    let mut object: Vec<_> = lifted.parameters[count..]
        .iter()
        .map(|parameter| scalar(&parameter.ty))
        .collect();
    object.push(scalar(&lifted.return_type));
    let result = if matches!(callee.kind, CalleeKind::ClosureBodyOnce(..)) {
        Scalar::ClosureOnce(object)
    } else if let PlanType::NativeClosure(_, send, sync) = &function.return_type {
        Scalar::NativeClosure(object, *send, *sync)
    } else if let PlanType::BorrowingClosure(_) = &function.return_type {
        Scalar::BorrowingClosure(object)
    } else {
        Scalar::Closure(object)
    };
    Ok((
        lifted.parameters[..count]
            .iter()
            .map(|parameter| scalar(&parameter.ty))
            .collect(),
        result,
    ))
}

/// Project the exact parameter/result carriers of a retained planned function without copying its declaration.
pub(super) fn planned_function_signature(plan: &Plan, name: &str) -> Result<(Vec<Scalar>, Scalar), PlanError> {
    let function = plan
        .functions
        .iter()
        .find(|function| function.name == name)
        .ok_or_else(|| PlanError::UnknownCallee(name.to_string()))?;
    Ok((
        function
            .parameters
            .iter()
            .map(|parameter| scalar(&parameter.ty))
            .collect(),
        scalar(&function.return_type),
    ))
}

/// Compare the yielded element with the same primitive/list vocabulary used for ordinary values.
pub(super) fn generator_element(leaf: &ListLeaf, depth: i64) -> Result<Scalar, PlanError> {
    let element = checked_list_leaf_type(leaf)?;
    if depth != 0 {
        return Ok(Scalar::List(Leaf::of(leaf), depth));
    }
    Ok(scalar(&element))
}

/// Captured spawn is a constructor boundary, not an arbitrary closure operation in an ordinary planned body.
/// Its source-ordered owned parameters supply the entire environment exactly once.
pub(super) fn validate_generator_constructor(function: &Function, arguments: &[Operand]) -> Result<(), PlanError> {
    if function.parameters.len() != arguments.len()
        || function.locals.len() != arguments.len() + 1
        || function.blocks.iter().any(|block| !block.statements.is_empty())
    {
        return Err(invalid(
            function,
            "captured generator spawn requires an exact constructor frame",
        ));
    }
    for (index, argument) in arguments.iter().enumerate() {
        let OperandKind::Move(place) = &argument.kind else {
            return Err(invalid(function, "generator capture must move its owned parameter"));
        };
        if usize::try_from(place.local).map_err(|_| invalid(function, "negative generator capture slot"))? != index + 1
            || !matches!(place.projection, Projection::Whole)
        {
            return Err(invalid(
                function,
                "generator capture order differs from constructor parameters",
            ));
        }
    }
    Ok(())
}

/// Validate terminator values and every normal and unwinding successor.
pub(super) fn terminator(
    plan: &Plan,
    function: &Function,
    value: &TerminatorKind,
    cleanup: bool,
) -> Result<(), PlanError> {
    match value {
        TerminatorKind::Await(future, destination, target, cancelled, action, drop_action) => {
            if !function.is_async || cleanup {
                return Err(invalid(function, "unsupported await outside a coroutine body"));
            }
            let future_type = place(plan, function, future)?;
            match future_type {
                Scalar::Future(outputs) => {
                    let [output] = outputs.as_slice() else {
                        return Err(invalid(function, "unsupported future output arity"));
                    };
                    require(
                        function,
                        place(plan, function, destination)?,
                        output,
                        "await destination",
                    )?;
                }
                Scalar::Model(_) => {
                    // The checked suspension retains the output; coroutine expansion verifies Future::poll against it.
                    place(plan, function, destination)?;
                }
                _ => {
                    return Err(invalid(function, "unsupported await without an SDK future carrier"));
                }
            }
            edge(function, *target, false)?;
            edge(function, *cancelled, false)?;
            unwind(function, action, false)?;
            unwind(function, drop_action, false)
        }
        TerminatorKind::CoroutineDrop if function.is_async && !cleanup => Ok(()),
        TerminatorKind::CoroutineDrop => Err(invalid(function, "unsupported coroutine drop outside a coroutine body")),
        TerminatorKind::Goto(target) => edge(function, *target, cleanup),
        TerminatorKind::SwitchBool(condition, false_target, true_target) => {
            require(
                function,
                operand(plan, function, condition)?,
                &Scalar::Bool,
                "switch condition",
            )?;
            edge(function, *false_target, cleanup)?;
            edge(function, *true_target, cleanup)
        }
        TerminatorKind::Call(callee, arguments, destination, target, action) => {
            writable_tuple_place(function, destination)?;
            if matches!(callee.kind, CalleeKind::SpawnGenerator(..)) && !arguments.is_empty() {
                validate_generator_constructor(function, arguments)?;
            }
            let (parameters, result) = signature(plan, function, callee)?;
            if parameters.len() != arguments.len() {
                return Err(invalid(
                    function,
                    "call argument count differs from the callee signature",
                ));
            }
            for (argument, parameter) in arguments.iter().zip(parameters) {
                require(
                    function,
                    operand(plan, function, argument)?,
                    &parameter,
                    "call argument",
                )?;
            }
            require(
                function,
                place(plan, function, destination)?,
                &result,
                "call destination",
            )?;
            edge(function, *target, cleanup)?;
            unwind(function, action, cleanup)
        }
        TerminatorKind::Drop(value, target, action) => {
            place(plan, function, value)?;
            edge(function, *target, cleanup)?;
            unwind(function, action, cleanup)
        }
        TerminatorKind::AssertOverflow(condition, op, left, right, target, action) => {
            require(
                function,
                operand(plan, function, condition)?,
                &Scalar::Bool,
                "overflow flag",
            )?;
            let left_type = operand(plan, function, left)?;
            if left_type != Scalar::Int && !sized_integer(&left_type) {
                return Err(invalid(function, "overflow assertion requires an integer carrier"));
            }
            require(
                function,
                operand(plan, function, right)?,
                left_type,
                "overflow right operand",
            )?;
            if !matches!(op, BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply) {
                return Err(invalid(
                    function,
                    "overflow assertion requires add, subtract, or multiply",
                ));
            }
            edge(function, *target, cleanup)?;
            unwind(function, action, cleanup)
        }
        TerminatorKind::Return if cleanup => Err(invalid(function, "cleanup block cannot return normally")),
        TerminatorKind::UnwindResume if !cleanup => Err(invalid(function, "unwind resume requires a cleanup block")),
        TerminatorKind::Return | TerminatorKind::Unreachable | TerminatorKind::UnwindResume => Ok(()),
    }
}
