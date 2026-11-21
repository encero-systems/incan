//! Per-instantiation decorator bindings and their cached frames.

use incan_mir_plan_native::{
    Callee, CalleeKind, Constant, EnumDeclaration, ExternalFunction, Function, Local, Operand, PlanType, RvalueKind,
    SourceSpan, StaticCell, TerminatorKind, assign as plan_assign, place as plan_place,
};
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::{BodyIrModule, DecoratedBinding, DecoratedInitialization};

use crate::classes::body_module_identity;
use crate::frame::{Builder, external_call, literal, read_place, temporary, terminate_call};
use crate::program_context::ProgramContext;
use crate::string_input::{SyntheticFrame, synthetic_builder};
use crate::text_plan::owned_literal;

/// Admit a synchronous per-instantiation binding with its initializer; its frames check the closed signature.
pub fn validate_instance_binding(binding: &DecoratedBinding) -> Result<(), String> {
    if binding.initialization != DecoratedInitialization::PerInstantiation {
        return Err("unsupported Body IR cached decorator outside per-instantiation policy".into());
    }
    let body = &binding.original_body;
    if body.is_async || body.is_generator() {
        return Err("unsupported Body IR generic decorator on an async or generator callable".into());
    }
    if binding.initializer.is_none() {
        return Err("unsupported Body IR generic decorator without a per-instantiation initializer".into());
    }
    Ok(())
}

/// Honor the kernel-validated lifetime and complete initializer field without inferring a replacement from storage.
pub fn validate_binding_policy(binding: &DecoratedBinding) -> Result<(), String> {
    match binding.initialization {
        DecoratedInitialization::PerInstantiation => validate_instance_binding(binding),
        DecoratedInitialization::PerInvocation => {
            Err("unsupported Body IR retired per-invocation decorator lifetime".into())
        }
        DecoratedInitialization::ModuleStorage => {
            if !binding.original_body.type_parameters.is_empty() || binding.initializer.is_some() {
                return Err("unsupported Body IR module decorator lifetime".into());
            }
            Ok(())
        }
    }
}

/// The source role of a frame: the normal callable, a decorated binding's original body, or its initializer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecoratorFrame {
    Normal,
    Original,
    Initializer,
}

/// Give authenticated hidden frames distinct symbols while retaining the normal closed instance's key.
pub fn frame_name(base: String, role: DecoratorFrame) -> String {
    match role {
        DecoratorFrame::Normal => base,
        DecoratorFrame::Original => format!("{base}__incan_original"),
        DecoratorFrame::Initializer => format!("{base}__incan_initializer"),
    }
}

/// Bound dynamic name lookup to its registration module independently of signature and cache lifetime policy.
pub fn callable_name_profile(module: &BodyIrModule, owner: &str) -> Result<bool, String> {
    for binding in &module.decorated_bindings {
        if binding.initializer.is_some() && body_module_identity(&binding.original_body)? != owner {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Name the frame that publishes a closed instance once and returns its decorated callable.
pub fn binding_accessor_name(base: &str) -> String {
    format!("{base}__incan_binding")
}

/// The pointer signature of the frame being built, read from its parameter slots and then its return slot.
pub fn builder_signature(builder: &crate::frame::Builder) -> Vec<incan_mir_plan_native::PlanType> {
    let mut signature: Vec<_> = (1..=builder.parameter_count as usize)
        .map(|index| builder.locals[index].ty.clone())
        .collect();
    signature.push(builder.locals[0].ty.clone());
    signature
}

/// Name the storage cell's initializer, which holds the original until publication replaces it.
fn binding_seed_name(base: &str) -> String {
    format!("{base}__incan_binding_seed")
}

/// Name the frame that runs the instance's decorators and stores their result in its cell.
fn binding_publish_name(base: &str) -> String {
    format!("{base}__incan_binding_publish")
}

/// The reified pointer signature of a planned frame: its parameter types, then its result.
pub fn frame_signature(function: &Function) -> Vec<PlanType> {
    let mut signature: Vec<PlanType> = function
        .parameters
        .iter()
        .map(|parameter| parameter.ty.clone())
        .collect();
    signature.push(function.return_type.clone());
    signature
}

/// Materialize an already authenticated planned item through the existing function-pointer coercion.
pub fn reified_pointer(builder: &mut Builder, name: &str, signature: &[PlanType], span: &SourceSpan) -> Operand {
    let item = temporary(
        builder,
        PlanType::FunctionItem(name.to_string(), signature.to_vec()),
        span,
    );
    let callee = Callee {
        kind: CalleeKind::Planned(name.to_string()),
        span: span.clone(),
    };
    builder.emit(plan_assign(item.clone(), RvalueKind::FunctionItem(callee), span));
    let pointer = temporary(builder, PlanType::FunctionPointer(signature.to_vec()), span);
    builder.emit(plan_assign(
        pointer.clone(),
        RvalueKind::ReifyFunction(read_place(item, false, span)),
        span,
    ));
    read_place(pointer, false, span)
}

/// The names one closed decorated instance's cache frames use: its own base, original and initializer frames.
pub struct CachedBinding<'a> {
    pub base: String,
    pub original: String,
    pub initializer: String,
    pub signature: Vec<PlanType>,
    pub slot: i64,
    pub unions: &'a [IncanType],
    pub intrinsics: &'a [EnumDeclaration],
}

/// Plan a closed instance's storage cell and the frames that seed, publish and read it.
///
/// 1. Seed the cell with the original, so reaching the storage never runs user code.
/// 2. Publish by running the initializer frame and storing the decorated callable it returns.
/// 3. Read through the runtime claim, which publishes once per instance, then load the cell.
pub fn cached_binding_frames(
    program: &ProgramContext,
    binding: &DecoratedBinding,
    cached: &CachedBinding<'_>,
    span: &SourceSpan,
) -> Result<(StaticCell, Vec<Function>, Vec<ExternalFunction>), String> {
    let module_identity = body_module_identity(&binding.original_body)?;
    let Some(identity) = &binding.original_body.canonical else {
        return Err("unsupported Body IR generic decorator without a declaration identity".into());
    };
    let declaration = identity.declaration_name.to_string();
    let base = &cached.base;
    let signature = &cached.signature;
    let pointer = PlanType::FunctionPointer(signature.clone());
    let frame = |name: String, result: PlanType| cache_frame(program, name, result, &module_identity, cached, span);
    // 1. The seed is never observed: every read waits for publication first.
    let mut seed = frame(binding_seed_name(base), pointer.clone());
    let seeded = reified_pointer(&mut seed, &cached.original, signature, span);
    seed.emit(plan_assign(plan_place(0, span), RvalueKind::Use(seeded), span));
    seed.finish(TerminatorKind::Return, span);
    // 2. Publication replaces the seed only after every decorator has returned.
    let mut publish = frame(binding_publish_name(base), PlanType::Unit);
    let decorated = temporary(&mut publish, pointer.clone(), span);
    terminate_call(
        &mut publish,
        CalleeKind::Planned(cached.initializer.clone()),
        Vec::new(),
        decorated.clone(),
        span,
    );
    let stored = temporary(&mut publish, PlanType::Unit, span);
    terminate_call(
        &mut publish,
        CalleeKind::StaticStore(cached.slot),
        vec![read_place(decorated, false, span)],
        stored,
        span,
    );
    publish.emit(plan_assign(
        plan_place(0, span),
        RvalueKind::Use(literal(Constant::Unit, span)),
        span,
    ));
    publish.finish(TerminatorKind::Return, span);
    // 3. The publisher's address identifies this unit's cell to the runtime claim.
    let mut access = frame(binding_accessor_name(base), pointer.clone());
    claim_binding(&mut access, &module_identity, cached, &declaration, span);
    terminate_call(
        &mut access,
        CalleeKind::StaticLoad(cached.slot),
        Vec::new(),
        plan_place(0, span),
        span,
    );
    access.finish(TerminatorKind::Return, span);
    let externals = access.externals.clone();
    let cell = StaticCell {
        slot: cached.slot,
        ty: pointer.clone(),
        initializer: binding_seed_name(base),
        span: span.clone(),
        module_completion: false,
    };
    let frames = vec![
        finished_frame(seed, pointer.clone(), span),
        finished_frame(publish, PlanType::Unit, span),
        finished_frame(access, pointer, span),
    ];
    Ok((cell, frames, externals))
}

/// Ask the runtime to publish this instance once, keyed by its publisher's and original's addresses.
fn claim_binding(
    access: &mut Builder,
    module_identity: &str,
    cached: &CachedBinding<'_>,
    declaration: &str,
    span: &SourceSpan,
) {
    let module_text = owned_literal(access, module_identity, span);
    let instance_text = owned_literal(access, &cached.base, span);
    let name_text = owned_literal(access, declaration, span);
    let publisher = reified_pointer(access, &binding_publish_name(&cached.base), &[PlanType::Unit], span);
    let publisher_address = function_address(access, publisher.clone(), span);
    let original = reified_pointer(access, &cached.original, &cached.signature, span);
    let original_address = function_address(access, original, span);
    let claimed = temporary(access, PlanType::Unit, span);
    external_call(
        access,
        "incan_native_runtime::initialize_binding",
        vec![
            PlanType::String,
            PlanType::String,
            PlanType::FunctionPointer(vec![PlanType::Unit]),
            PlanType::USize,
            PlanType::USize,
            PlanType::String,
        ],
        PlanType::Unit,
        vec![
            read_place(module_text, true, span),
            read_place(instance_text, true, span),
            publisher,
            publisher_address,
            original_address,
            read_place(name_text, true, span),
        ],
        claimed,
        span,
    );
}

/// Expose a reified pointer's address, which keys the runtime's per-unit claim and callable-name registry.
fn function_address(builder: &mut Builder, pointer: Operand, span: &SourceSpan) -> Operand {
    let address = temporary(builder, PlanType::USize, span);
    builder.emit(plan_assign(address.clone(), RvalueKind::FunctionAddress(pointer), span));
    read_place(address, false, span)
}

/// Open a parameterless synthetic frame in the declaring module; it has no source dispatch and no owners.
fn cache_frame(
    program: &ProgramContext,
    name: String,
    result: PlanType,
    module_identity: &str,
    cached: &CachedBinding<'_>,
    span: &SourceSpan,
) -> Builder {
    let frame = SyntheticFrame {
        locals: vec![Local {
            ty: result,
            span: span.clone(),
        }],
        parameter_count: 0,
        file: span.file.to_string(),
        module_identity: module_identity.to_string(),
        unions: cached.unions.to_vec(),
        intrinsics: cached.intrinsics.to_vec(),
        name,
        owned: Vec::new(),
    };
    let mut builder = synthetic_builder(program.clone(), frame);
    builder.new_block(span);
    builder
}

/// Retain a completed synthetic frame's storage and blocks as a planned function without parameters.
fn finished_frame(builder: Builder, result: PlanType, span: &SourceSpan) -> Function {
    Function::new(
        builder.name,
        Vec::new(),
        result,
        builder.locals,
        builder.blocks,
        span.clone(),
    )
}
