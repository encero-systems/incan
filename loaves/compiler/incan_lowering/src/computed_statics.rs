//! Computed module statics (RFC 052): their initializer names, module initialization keys and frame entry.
//!
//! Legacy declares each computed static as lazily initialized storage and, on entry to every function of the module,
//! forces the module's statics once in declaration order; a computed storage read forces its own static first. A plan
//! keeps the same order: each module with computed statics gets one planned initializer, and every function of the
//! module calls it on entry.

use incan_mir_plan_native::{
    Callee, CalleeKind, Constant, ExternalFunction, Function, Local, Operand, OperandKind, PlanType, RvalueKind,
    SourceSpan, TerminatorKind, Unwind, assign as plan_assign, block as plan_block, place as plan_place,
};
use incan_semantics_core::body_ir::{Body, StaticDeclaration, StaticInitializer};
use incan_semantics_core::{SemanticSourceTargetKind, canonical_module_identity};

use crate::frame::{Builder, literal, temporary, terminate_call};

/// Report whether a static's initializer is a computed body rather than an effect-free literal.
pub fn is_computed_static(declaration: &StaticDeclaration) -> bool {
    matches!(declaration.initial, StaticInitializer::Computed(_))
}

/// The declaring module of a static, read from its canonical identity.
pub fn static_owner(declaration: &StaticDeclaration) -> String {
    canonical_module_identity(&declaration.canonical).unwrap_or_default()
}

/// The first computed static slot module `owner` declares, which keys its initialization, or -1 for none.
pub fn module_static_key(statics: &[StaticDeclaration], owner: &str) -> i64 {
    for (slot, declaration) in statics.iter().enumerate() {
        if is_computed_static(declaration) && static_owner(declaration) == owner {
            return slot as i64;
        }
    }
    -1
}

/// Name the planned function initializing the statics of the module keyed `key`.
pub fn module_initializer_name(key: i64) -> String {
    format!("__incan_init_module_statics_{key}")
}

/// Call module `owner`'s static initializer, as legacy calls `__incan_init_module_statics` at a function entry.
pub fn initialize_module_statics(builder: &mut Builder, owner: &str, span: &SourceSpan) {
    let key = module_static_key(&builder.program.statics, owner);
    if key < 0 {
        return;
    }
    let output = temporary(builder, PlanType::Unit, span);
    terminate_call(
        builder,
        CalleeKind::Planned(module_initializer_name(key)),
        Vec::new(),
        output,
        span,
    );
}

/// Report whether a body initializes its module's statics on entry: every function and method does, as in legacy,
/// but a computed static's or constant's own initializer does not, because the module's initialization is what runs
/// it.
pub fn enters_module_initialization(body: &Body) -> bool {
    match &body.canonical {
        Some(identity) => {
            identity.kind != SemanticSourceTargetKind::Static && identity.kind != SemanticSourceTargetKind::Const
        }
        None => true,
    }
}

/// Keep computed values and literal lists or dictionaries in guarded cells without making literals effectful.
pub fn has_native_static_cell(declaration: &StaticDeclaration) -> bool {
    match declaration.initial {
        StaticInitializer::List(_) | StaticInitializer::Dict(_) => true,
        _ => is_computed_static(declaration),
    }
}

/// Name the planned function computing computed static `slot`'s first value.
pub fn static_initializer_name(slot: usize) -> String {
    format!("__incan_static_init_{slot}")
}

/// The runtime's module initialization claim and completion, which every module initializer calls.
pub fn module_initialization_externals(span: &SourceSpan) -> Vec<ExternalFunction> {
    vec![
        ExternalFunction {
            path: "incan_native_runtime::begin_module_initialization".into(),
            type_arguments: Vec::new(),
            parameters: vec![PlanType::Int],
            return_type: PlanType::Bool,
            span: span.clone(),
        },
        ExternalFunction {
            path: "incan_native_runtime::end_module_initialization".into(),
            type_arguments: Vec::new(),
            parameters: vec![PlanType::Int],
            return_type: PlanType::Unit,
            span: span.clone(),
        },
    ]
}

/// One call terminator of a module initializer, continuing at `target` and writing `destination`.
fn initializer_call(
    kind: CalleeKind,
    operands: Vec<Operand>,
    destination: i64,
    target: i64,
    span: &SourceSpan,
) -> TerminatorKind {
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
}

/// Build the initializer of the module keyed `key` without changing its declaration order or reentrancy behavior.
///
/// 1. Return immediately when the module has completed initialization.
/// 2. Use the runtime claim to force computed statics once in declaration order; reentrant calls return.
/// 3. Publish completion only after initialization succeeds.
pub fn module_initializer(statics: &[StaticDeclaration], key: i64, span: &SourceSpan) -> Function {
    let owner = static_owner(&statics[key as usize]);
    let forced: Vec<i64> = statics
        .iter()
        .enumerate()
        .filter(|(_, declaration)| is_computed_static(declaration) && static_owner(declaration) == owner)
        .map(|(slot, _)| slot as i64)
        .collect();
    // 1. Completed modules return after a single native atomic load.
    let done = 4 + forced.len() as i64 + 2;
    let probe = initializer_call(CalleeKind::ModuleInitialized(key), Vec::new(), 1, 1, span);
    let claim = initializer_call(
        CalleeKind::External("incan_native_runtime::begin_module_initialization".into()),
        vec![literal(Constant::Int(key), span)],
        1,
        3,
        span,
    );
    let claimed = || Operand {
        kind: OperandKind::Copy(plan_place(1, span)),
        span: span.clone(),
    };
    let mut blocks = vec![
        plan_block(Vec::new(), probe, span),
        plan_block(Vec::new(), TerminatorKind::SwitchBool(claimed(), 2, done), span),
        plan_block(Vec::new(), claim, span),
        plan_block(Vec::new(), TerminatorKind::SwitchBool(claimed(), done, 4), span),
    ];
    // 2. The original claim still controls first entry and reentrancy, in declaration order.
    for (index, slot) in forced.iter().enumerate() {
        let force = initializer_call(CalleeKind::StaticForce(*slot), Vec::new(), 2, 5 + index as i64, span);
        blocks.push(plan_block(Vec::new(), force, span));
    }
    let complete = initializer_call(
        CalleeKind::External("incan_native_runtime::end_module_initialization".into()),
        vec![literal(Constant::Int(key), span)],
        2,
        done - 1,
        span,
    );
    blocks.push(plan_block(Vec::new(), complete, span));
    // 3. Publish completion only after every static and the runtime completion record succeed.
    let publish = initializer_call(CalleeKind::ModuleComplete(key), Vec::new(), 2, done, span);
    blocks.push(plan_block(Vec::new(), publish, span));
    let unit = plan_assign(
        plan_place(0, span),
        RvalueKind::Use(literal(Constant::Unit, span)),
        span,
    );
    blocks.push(plan_block(vec![unit], TerminatorKind::Return, span));
    let local = |ty: PlanType| Local { ty, span: span.clone() };
    Function::new(
        module_initializer_name(key),
        Vec::new(),
        PlanType::Unit,
        vec![local(PlanType::Unit), local(PlanType::Bool), local(PlanType::Unit)],
        blocks,
        span.clone(),
    )
}
