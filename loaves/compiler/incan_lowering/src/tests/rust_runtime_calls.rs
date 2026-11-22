//! A Rust runtime publishes no governed bodies: its generic calls and its async functions stay native externals.

use std::sync::Arc;

use incan_mir_plan_native::{
    Callee, CalleeKind, EnumDeclaration, ExternalFunction, Function, ListLeaf, Local, PlanType, source_span,
};
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::BodyIrModule;

use crate::native_type_identity::ConstructorIndex;
use crate::nominal_instances::NominalInstance;
use crate::program_context::synthetic_program_context;
use crate::runtime_calls::{
    GovernedFrames, RuntimeInventories, RuntimeRedirects, retain_runtime_instances, runtime_redirects,
};
use crate::type_parameters::Instance;

/// The unit's inventories after runtime retention of a declarations-only module with no runtime bodies.
struct Retained {
    functions: Vec<Function>,
    externals: Vec<ExternalFunction>,
    instances: Vec<Instance>,
    redirects: RuntimeRedirects,
}

fn retain(functions: Vec<Function>, externals: Vec<ExternalFunction>) -> Result<Retained, String> {
    let constructors = ConstructorIndex::default();
    let mut module = BodyIrModule::declarations_only("main".into());
    let templates = BodyIrModule::declarations_only("main".into());
    let mut program = synthetic_program_context(&module, Arc::new(ConstructorIndex::default()));
    let mut retained = Retained {
        functions,
        externals,
        instances: Vec::new(),
        redirects: runtime_redirects(),
    };
    let mut nominal_instances: Vec<NominalInstance> = Vec::new();
    let mut intrinsics: Vec<EnumDeclaration> = Vec::new();
    let mut unions: Vec<IncanType> = Vec::new();
    let frames = GovernedFrames {
        template_constructors: &constructors,
        templates: &templates,
        program: &mut program,
        nominal_instances: &mut nominal_instances,
        instances: &mut retained.instances,
    };
    let inventories = RuntimeInventories {
        functions: &mut retained.functions,
        externals: &mut retained.externals,
        intrinsics: &mut intrinsics,
        unions: &mut unions,
    };
    retain_runtime_instances(&mut module, frames, inventories, &mut retained.redirects)?;
    Ok(retained)
}

/// `append_list[int]` keeps its exact external signature; nothing is redirected to a planned instance.
#[test]
fn generic_rust_runtime_call_stays_a_native_instantiated_external() -> Result<(), String> {
    let external = ExternalFunction {
        path: "incan_native_runtime::append_list".into(),
        type_arguments: vec![PlanType::Int],
        parameters: vec![PlanType::ListMutRef(ListLeaf::Int, 1), PlanType::Int],
        return_type: PlanType::Unit,
        span: source_span("main.incn", 1, 1),
    };
    let retained = retain(Vec::new(), vec![external.clone()])?;
    assert_eq!(
        retained.externals,
        [external],
        "the native generic runtime call lost its external"
    );
    assert!(
        retained.redirects.bindings.is_empty(),
        "a native generic runtime call was redirected"
    );
    assert!(
        retained.instances.is_empty() && retained.redirects.missing.is_empty(),
        "a native generic runtime call requested a body"
    );
    Ok(())
}

/// `race_pair`'s boxing shim keeps the native async function as its callee when no coroutine body is published.
#[test]
fn rust_runtime_async_function_stays_the_native_future_callee() -> Result<(), String> {
    let span = source_span("main.incn", 1, 1);
    let future = PlanType::Future(vec![PlanType::Int]);
    let types = vec![
        PlanType::Int,
        PlanType::Int,
        PlanType::Int,
        future.clone(),
        future.clone(),
    ];
    let callee = Callee {
        kind: CalleeKind::Instantiated("incan_native_runtime::race_pair".into(), types),
        span: span.clone(),
    };
    let locals = vec![Local {
        ty: future.clone(),
        span: span.clone(),
    }];
    let mut shim = Function::new(
        "__incan_future_ctor_race_0".into(),
        Vec::new(),
        future,
        locals,
        Vec::new(),
        span,
    );
    shim.future_callee = Some(callee.clone());
    let retained = retain(vec![shim], Vec::new())?;
    assert_eq!(
        retained.functions[0].future_callee,
        Some(callee),
        "the native runtime future callee was replaced"
    );
    assert!(
        retained.instances.is_empty(),
        "a native runtime future requested a coroutine instance"
    );
    Ok(())
}
