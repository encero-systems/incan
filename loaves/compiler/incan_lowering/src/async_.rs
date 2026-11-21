//! Source coroutines: their identities, captures, boxed futures and async entries.

use incan_mir_plan_native::{
    Callee, CalleeKind, Carrier, EnumDeclaration, ExternalFunction, Function, Local, Operand, Parameter, Place,
    PlanType, Projection, SourceSpan, TerminatorKind, Unwind, block as plan_block, carrier_position,
    place as plan_place,
};
use incan_semantics_core::body_ir::{Body, BodyIrModule};
use incan_semantics_core::{
    IncanCallableParamKind, IncanInputAccess, IncanPrimitiveType, IncanType, SemanticSourceTargetKind,
};

use crate::classes::{frame_type, function_name, is_program_entry};
use crate::explicit_arguments::direct_view_access;
use crate::frame::{
    Builder, cleanup_exit, drop_all_owned, forget_moved_arguments, forget_owned, place_type, read_place, terminate_call,
};
use crate::layout_types::{mutable_receiver_type, native_source_type, shared_receiver_type};
use crate::native_type_identity::ConstructorIndex;
use crate::unions::collect_unions;

/// Validate source coroutine identities and captures before native storage is selected by rustc.
pub fn validate_futures(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    intrinsics: &[EnumDeclaration],
) -> Result<(), String> {
    for body in &module.bodies {
        if !body.is_async {
            continue;
        }
        match &body.canonical {
            Some(identity) => {
                if identity.kind != SemanticSourceTargetKind::Function
                    && identity.kind != SemanticSourceTargetKind::Method
                {
                    return Err("unsupported Body IR async callable identity".into());
                }
            }
            None => return Err("unsupported Body IR async body without canonical identity".into()),
        }
        if body.type_parameters.is_empty() {
            capture_types(constructors, module, body, intrinsics)?;
        }
    }
    Ok(())
}

/// Separate coroutine and integer main bodies from their process entry and construction shims.
pub fn evaluator_name(module: &BodyIrModule, body: &Body) -> String {
    let name = function_name(module, body);
    if body.is_async {
        return format!("__incan_async_body_{name}");
    }
    if is_program_entry(module, body) && body.return_type == IncanType::Primitive(IncanPrimitiveType::Int) {
        return format!("__incan_integer_{name}");
    }
    name
}

/// The synchronous function that drives one async test's coroutine for the native test harness.
pub fn async_test_name(native: &str) -> String {
    format!("__incan_test_{native}")
}

/// Retain owned captures, including declaration-owned rest packs, and at most one nominal loan. An explicit borrowed
/// input is captured as its loan, which the future holds until it completes.
fn capture_types(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    body: &Body,
    intrinsics: &[EnumDeclaration],
) -> Result<Vec<PlanType>, String> {
    let mut fields = Vec::with_capacity(body.params.len());
    let mut loans = 0;
    for parameter in &body.params {
        let local = &body.locals[parameter.local.index() as usize];
        let mut ty = frame_type(
            constructors,
            &module.nominal_declarations,
            &module.enum_declarations,
            local,
            intrinsics,
            "",
        )?;
        match parameter.input_access {
            IncanInputAccess::Shared | IncanInputAccess::InferredShared => ty = shared_receiver_type(ty)?,
            IncanInputAccess::Exclusive => ty = mutable_receiver_type(ty)?,
            _ => {}
        }
        validate_collection_capture(parameter.kind, &ty)?;
        if matches!(ty, PlanType::ModelRef(..) | PlanType::ModelMutRef(..)) {
            loans += 1;
        }
        if parameter.mutable && ty == PlanType::String {
            return Err("unsupported Body IR async mutable string capture".into());
        }
        fields.push(ty);
    }
    if loans > 1 {
        return Err("unsupported Body IR async future with multiple borrowed captures".into());
    }
    Ok(fields)
}

/// Rest parameters own their checked packs; ordinary collection parameters retain caller-owned borrows.
fn validate_collection_capture(kind: IncanCallableParamKind, ty: &PlanType) -> Result<(), String> {
    match ty {
        PlanType::List(..) if kind != IncanCallableParamKind::RestPositional => {
            Err("unsupported Body IR async borrowed list capture".into())
        }
        PlanType::Dict(..) if kind != IncanCallableParamKind::RestKeyword => {
            Err("unsupported Body IR async borrowed dictionary capture".into())
        }
        PlanType::Set(_) => Err("unsupported Body IR async borrowed set capture".into()),
        _ => Ok(()),
    }
}

/// Construct a source or SDK future now and pin it, preserving deferred body execution and native obligations.
pub fn construct_boxed_future(
    builder: &mut Builder,
    callee: CalleeKind,
    types: Vec<PlanType>,
    output: PlanType,
    arguments: Vec<Operand>,
    destination: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let name = format!("__incan_future_ctor_{}_{}", builder.name, builder.synthesized.len());
    let constructor = boxed_future_constructor(name.clone(), callee, types, output, span.clone())?;
    builder.synthesized.push(constructor);
    forget_moved_arguments(builder, &arguments);
    let local = destination.local;
    terminate_call(builder, CalleeKind::Planned(name), arguments, destination, span);
    builder.owned.push(local);
    Ok(())
}

/// Retain one exact boxed-future ABI for both call-site shims and a library's exported async constructor.
pub fn boxed_future_constructor(
    name: String,
    callee: CalleeKind,
    types: Vec<PlanType>,
    output: PlanType,
    span: SourceSpan,
) -> Result<Function, String> {
    let result = PlanType::Future(vec![output]);
    let mut parameters = Vec::with_capacity(types.len());
    let mut locals = vec![Local {
        ty: result.clone(),
        span: span.clone(),
    }];
    for (index, ty) in types.into_iter().enumerate() {
        parameters.push(Parameter {
            name: format!("capture{index}"),
            ty: ty.clone(),
            span: span.clone(),
        });
        locals.push(Local { ty, span: span.clone() });
    }
    let blocks = vec![plan_block(Vec::new(), TerminatorKind::Unreachable, &span)];
    let mut function = Function::new(name, parameters, result, locals, blocks, span.clone());
    function.future_callee = Some(Callee { kind: callee, span });
    Ok(function)
}

/// Capture evaluated arguments by value and return a native coroutine with its physical output ABI.
pub fn construct_future(
    module: &BodyIrModule,
    builder: &mut Builder,
    body: &Body,
    arguments: Vec<Operand>,
    destination: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    if is_program_entry(module, body) {
        return Err("unsupported Body IR calling async main".into());
    }
    let fields = capture_types(builder.constructors(), module, body, &builder.intrinsics)?;
    let mut output = native_source_type(
        builder.constructors(),
        &module.nominal_declarations,
        &module.enum_declarations,
        &collect_unions(module),
        &body.return_type,
        &builder.intrinsics,
        "",
    )?;
    if let Some(access) = direct_view_access(body) {
        output = match access {
            IncanInputAccess::Shared => shared_receiver_type(output)?,
            IncanInputAccess::Exclusive => mutable_receiver_type(output)?,
            _ => return Err("invalid Body IR returned view access".into()),
        };
        if destination.projection != Projection::Whole {
            return Err("unsupported Body IR projected async returned view carrier".into());
        }
        builder.locals[destination.local as usize].ty = PlanType::Future(vec![output.clone()]);
    }
    construct_boxed_future(
        builder,
        CalleeKind::Planned(evaluator_name(module, body)),
        fields,
        output,
        arguments,
        destination,
        span,
    )
}

/// Retain a native suspension and an explicit cancellation edge; the adapter emits only the pinned poll plumbing.
pub fn await_future(builder: &mut Builder, future: Place, destination: Place, span: &SourceSpan) -> Result<(), String> {
    // The checked suspension already retains its output. Rustc validates the native Future obligation and exact poll
    // output.
    let incoming = builder.owned.clone();
    let current = builder.current;
    let cancelled = builder.new_block(span);
    builder.select(cancelled);
    drop_all_owned(builder, span);
    builder.finish(TerminatorKind::CoroutineDrop, span);
    builder.current = current;
    builder.owned = incoming;
    let after = builder.new_block(span);
    let unwind = Unwind::Cleanup(cleanup_exit(builder, span));
    forget_owned(builder, future.local);
    if matches!(
        place_type(&builder.locals, &destination),
        PlanType::String
            | PlanType::Future(_)
            | PlanType::Enum(..)
            | PlanType::Model(..)
            | PlanType::List(..)
            | PlanType::Set(_)
            | PlanType::Dict(..)
    ) {
        builder.owned.push(destination.local);
    }
    let drop_unwind = Unwind::Cleanup(cleanup_exit(builder, span));
    builder.finish(
        TerminatorKind::Await(future, destination, after, cancelled, unwind, drop_unwind),
        span,
    );
    builder.select(after);
    Ok(())
}

/// Return the coroutine's checked output through the native function `entry_name`, preserving Result termination
/// after executor setup: `main` for a program, the test wrapper for an async test.
pub fn async_entry(
    _module: &BodyIrModule,
    body: &Function,
    intrinsics: &mut Vec<EnumDeclaration>,
    entry_name: &str,
) -> Result<(Function, Function, ExternalFunction), String> {
    let span = &body.span;
    let future = PlanType::Future(vec![body.return_type.clone()]);
    let constructor = boxed_future_constructor(
        format!("__incan_{entry_name}_future"),
        CalleeKind::Planned(body.name.clone()),
        Vec::new(),
        body.return_type.clone(),
        span.clone(),
    )?;
    let mut path = "incan_native_runtime::run_future_entry";
    let mut arguments = vec![future.clone()];
    if body.return_type != PlanType::Unit {
        let PlanType::Enum(owner, _) = &body.return_type else {
            return Err("unsupported Body IR async main output".into());
        };
        let index = carrier_position(*owner);
        let Some(declaration) = usize::try_from(index).ok().and_then(|slot| intrinsics.get(slot)) else {
            return Err("unsupported Body IR async main outside a standard Result".into());
        };
        if declaration.carrier != Carrier::Result || declaration.variants.len() != 2 {
            return Err("unsupported Body IR async main outside a standard Result".into());
        }
        let errors = &declaration.variants[1].fields;
        if errors.len() != 1 {
            return Err("unsupported Body IR async main error arity".into());
        }
        arguments = vec![errors[0].clone(), future.clone()];
        path = "incan_native_runtime::run_result_future_entry";
    }
    let call = |kind: CalleeKind, operands, destination: i64, target: i64| {
        TerminatorKind::Call(
            Callee {
                kind,
                span: span.clone(),
            },
            operands,
            plan_place(destination, span),
            target,
            Unwind::Continue,
        )
    };
    let blocks = vec![
        plan_block(
            Vec::new(),
            call(CalleeKind::Planned(constructor.name.clone()), Vec::new(), 1, 1),
            span,
        ),
        plan_block(
            Vec::new(),
            call(
                CalleeKind::Instantiated(path.into(), arguments.clone()),
                vec![read_place(plan_place(1, span), true, span)],
                0,
                2,
            ),
            span,
        ),
        plan_block(Vec::new(), TerminatorKind::Return, span),
    ];
    let locals = vec![
        Local {
            ty: body.return_type.clone(),
            span: span.clone(),
        },
        Local {
            ty: future.clone(),
            span: span.clone(),
        },
    ];
    let entry = Function::new(
        entry_name.to_string(),
        Vec::new(),
        body.return_type.clone(),
        locals,
        blocks,
        span.clone(),
    );
    let external = ExternalFunction {
        path: path.into(),
        type_arguments: arguments,
        parameters: vec![future],
        return_type: body.return_type.clone(),
        span: span.clone(),
    };
    Ok((entry, constructor, external))
}

/// Use the same retained output ABI for source and SDK futures; construction identity stays with the call.
pub fn future_type(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    ty: &IncanType,
    intrinsics: &[EnumDeclaration],
) -> Result<Option<PlanType>, String> {
    let IncanType::Generic { base, args } = ty else {
        return Ok(None);
    };
    if base != "Awaitable" {
        return Ok(None);
    }
    if args.len() != 1 {
        return Err("unsupported Body IR async future output arity".into());
    }
    let unions = collect_unions(module);
    let carrier = native_source_type(
        constructors,
        &module.nominal_declarations,
        &module.enum_declarations,
        &unions,
        ty,
        intrinsics,
        "",
    )?;
    Ok(Some(carrier))
}

/// Identify owned pinned future storage independently of its retained output layout.
pub fn is_future(ty: &PlanType) -> bool {
    matches!(ty, PlanType::Future(_))
}

/// Run the checked integer main for its effects; its value is program metadata, never the process exit code.
pub fn integer_entry(body: &Function, entry_name: String) -> Function {
    let span = &body.span;
    let call = TerminatorKind::Call(
        Callee {
            kind: CalleeKind::Planned(body.name.clone()),
            span: span.clone(),
        },
        Vec::new(),
        plan_place(1, span),
        1,
        Unwind::Continue,
    );
    Function::new(
        entry_name,
        Vec::new(),
        PlanType::Unit,
        vec![
            Local {
                ty: PlanType::Unit,
                span: span.clone(),
            },
            Local {
                ty: body.return_type.clone(),
                span: span.clone(),
            },
        ],
        vec![
            plan_block(Vec::new(), call, span),
            plan_block(Vec::new(), TerminatorKind::Return, span),
        ],
        span.clone(),
    )
}
