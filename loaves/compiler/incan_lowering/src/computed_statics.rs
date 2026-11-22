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
use incan_semantics_core::body_ir::{
    Body, LinkedStaticAccessors, StaticDeclaration, StaticInitializer, loaf_crate_name,
};
use incan_semantics_core::{SemanticSourceTargetKind, SymbolOrigin, canonical_module_identity};

use crate::frame::{Builder, external_call, literal, temporary, terminate_call};

const BEGIN_MODULE_INITIALIZATION: &str = "incan_native_runtime::begin_unit_module_initialization";
const END_MODULE_INITIALIZATION: &str = "incan_native_runtime::end_unit_module_initialization";

/// Report whether a static participates in its declaring module's computed initialization.
pub fn is_computed_static(declaration: &StaticDeclaration) -> bool {
    matches!(declaration.initial, StaticInitializer::Computed(_))
        || linked_static(declaration).is_some_and(|accessors| accessors.initialize_module.is_some())
}

/// Read the producer bindings of this static, when it was supplied by a compiled Loaf publishing native storage.
pub(crate) fn linked_static(declaration: &StaticDeclaration) -> Option<&LinkedStaticAccessors> {
    match &declaration.initial {
        StaticInitializer::Linked(accessors) => Some(accessors),
        _ => None,
    }
}

/// Resolve a recorded crate-relative operation against the exact package that declares its storage.
pub(crate) fn linked_static_item(declaration: &StaticDeclaration, item: &str) -> Option<String> {
    let SymbolOrigin::Package { library, .. } = &declaration.canonical.origin else {
        return None;
    };
    Some(format!("{}::{item}", loaf_crate_name(library)))
}

/// Select the actual guard for a synthesized method, either in this plan or in the static's declaring Loaf.
pub(crate) fn guarded_static_call(
    builder: &Builder,
    slot: usize,
    method: String,
    mutable: bool,
) -> Result<CalleeKind, String> {
    let declaration = &builder.program.statics[slot];
    let Some(accessors) = linked_static(declaration) else {
        return Ok(CalleeKind::StaticMethod(slot as i64, method));
    };
    let guard = accessors
        .guarded
        .as_ref()
        .ok_or("linked static has no producer guard")?;
    let accessor = if mutable {
        guard
            .with_mut
            .as_deref()
            .ok_or("linked static has no exclusive producer guard")?
    } else {
        guard.with_ref.as_str()
    };
    let item = linked_static_item(declaration, accessor).ok_or("linked static has no declaring Loaf")?;
    Ok(CalleeKind::LinkedStaticMethod(item, method))
}

/// The declaring module of a static, read from its canonical identity.
pub fn static_owner(declaration: &StaticDeclaration) -> String {
    canonical_module_identity(&declaration.canonical).unwrap_or_default()
}

/// The first computed static slot module `owner` declares, which keys its initialization, or -1 for none.
pub fn module_static_key(statics: &[StaticDeclaration], owner: &str) -> i64 {
    for (slot, declaration) in statics.iter().enumerate() {
        if matches!(declaration.initial, StaticInitializer::Computed(_)) && static_owner(declaration) == owner {
            return slot as i64;
        }
    }
    -1
}

/// Name the planned function initializing the statics of module `owner` by the module's stable identity, as other
/// generated names are. A static slot would shift whenever another module gains or loses a static, renaming this
/// initializer under every saved product whose frames enter it.
pub fn module_initializer_name(owner: &str) -> String {
    format!("__incan_init_module_statics_{}", hex::encode(owner.as_bytes()))
}

/// The unit operand of a runtime call that keys a process-wide cell by unit and slot.
///
/// Every unit (a Loaf, or the program) numbers its statics from 0, and the runtime keeps one cell table per carrier
/// for the whole process, so each cell is keyed by its unit and its slot in that unit. The unit is the compiled crate's
/// stable id, which only the native adapter knows: the plan carries 0 here, and the adapter supplies the crate's id as
/// the first argument of every call [`is_unit_keyed_runtime_call`] names.
pub fn unit_operand(span: &SourceSpan) -> Operand {
    literal(Constant::Numeric("0".into(), PlanType::U64), span)
}

/// Whether a runtime call keys a process-wide cell by its first argument, the unit, and its second, a unit-local
/// static slot or module key.
pub fn is_unit_keyed_runtime_call(path: &str) -> bool {
    matches!(
        path,
        BEGIN_MODULE_INITIALIZATION
            | END_MODULE_INITIALIZATION
            | "incan_native_runtime::read_static_bool_in_unit"
            | "incan_native_runtime::write_static_bool_in_unit"
            | "incan_native_runtime::read_static_float_in_unit"
            | "incan_native_runtime::write_static_float_in_unit"
            | "incan_native_runtime::read_static_int_in_unit"
            | "incan_native_runtime::write_static_int_in_unit"
            | "incan_native_runtime::read_static_int_set_in_unit"
            | "incan_native_runtime::write_static_int_set_in_unit"
            | "incan_native_runtime::read_static_text_in_unit"
            | "incan_native_runtime::write_static_text_in_unit"
            | "incan_native_runtime::read_static_text_int_dict_in_unit"
            | "incan_native_runtime::write_static_text_int_dict_in_unit"
    )
}

#[cfg(test)]
mod unit_key_tests {
    use super::is_unit_keyed_runtime_call;

    /// The adapter injects a unit ID only when a call names a candidate runtime entry point.
    #[test]
    fn only_unit_keyed_runtime_calls_take_the_unit() {
        assert!(is_unit_keyed_runtime_call(
            "incan_native_runtime::read_static_int_in_unit"
        ));
        assert!(is_unit_keyed_runtime_call(
            "incan_native_runtime::write_static_text_int_dict_in_unit"
        ));
        assert!(is_unit_keyed_runtime_call(
            "incan_native_runtime::begin_unit_module_initialization"
        ));
        assert!(is_unit_keyed_runtime_call(
            "incan_native_runtime::end_unit_module_initialization"
        ));
        assert!(!is_unit_keyed_runtime_call("incan_native_runtime::read_static_int"));
        assert!(!is_unit_keyed_runtime_call(
            "incan_native_runtime::begin_module_initialization"
        ));
        assert!(!is_unit_keyed_runtime_call("incan_native_runtime::read_line"));
        assert!(!is_unit_keyed_runtime_call("caselib::read_static_int_in_unit"));
    }
}

/// Call the declaring unit's module initializer. Seed-era storage records without a producer binding still use
/// the local initializer during the transition.
pub fn initialize_module_statics(builder: &mut Builder, owner: &str, span: &SourceSpan) {
    let initializer = builder.program.statics.iter().find_map(|declaration| {
        if !is_computed_static(declaration) || static_owner(declaration) != owner {
            return None;
        }
        match &declaration.initial {
            StaticInitializer::Computed(_) => Some(CalleeKind::Planned(module_initializer_name(owner))),
            StaticInitializer::Linked(accessors) => {
                let path = accessors.initialize_module.as_deref()?;
                linked_static_item(declaration, path).map(CalleeKind::External)
            }
            _ => None,
        }
    });
    let Some(initializer) = initializer else {
        return;
    };
    let output = temporary(builder, PlanType::Unit, span);
    if let CalleeKind::External(path) = initializer {
        external_call(builder, &path, Vec::new(), PlanType::Unit, Vec::new(), output, span);
    } else {
        terminate_call(builder, initializer, Vec::new(), output, span);
    }
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
        StaticInitializer::Linked(ref accessors) => accessors.guarded.is_some(),
        StaticInitializer::Computed(_) => true,
        _ => false,
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
            path: BEGIN_MODULE_INITIALIZATION.into(),
            type_arguments: Vec::new(),
            parameters: vec![PlanType::U64, PlanType::Int],
            return_type: PlanType::Bool,
            span: span.clone(),
        },
        ExternalFunction {
            path: END_MODULE_INITIALIZATION.into(),
            type_arguments: Vec::new(),
            parameters: vec![PlanType::U64, PlanType::Int],
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
        .filter(|(_, declaration)| {
            matches!(declaration.initial, StaticInitializer::Computed(_)) && static_owner(declaration) == owner
        })
        .map(|(slot, _)| slot as i64)
        .collect();
    // 1. Completed modules return after a single native atomic load.
    let done = 4 + forced.len() as i64 + 2;
    let probe = initializer_call(CalleeKind::ModuleInitialized(key), Vec::new(), 1, 1, span);
    let claim = initializer_call(
        CalleeKind::External(BEGIN_MODULE_INITIALIZATION.into()),
        vec![unit_operand(span), literal(Constant::Int(key), span)],
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
        CalleeKind::External(END_MODULE_INITIALIZATION.into()),
        vec![unit_operand(span), literal(Constant::Int(key), span)],
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
        module_initializer_name(&owner),
        Vec::new(),
        PlanType::Unit,
        vec![local(PlanType::Unit), local(PlanType::Bool), local(PlanType::Unit)],
        blocks,
        span.clone(),
    )
}
