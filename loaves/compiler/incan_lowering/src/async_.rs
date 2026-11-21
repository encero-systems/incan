//! Source coroutines: their identities, captures, boxed futures and async entries.

use incan_mir_plan_native::{
    Callee, CalleeKind, EnumDeclaration, ExternalFunction, Function, Local, PlanType, SourceSpan, TerminatorKind,
    Unwind, block as plan_block, place as plan_place,
};
use incan_semantics_core::body_ir::{Body, BodyIrModule};
use incan_semantics_core::{IncanPrimitiveType, IncanType, SemanticSourceTargetKind};

use crate::classes::{function_name, is_program_entry};
use crate::layout_types::native_source_type;
use crate::native_type_identity::ConstructorIndex;
use crate::pending::untranslated;
use crate::unions::collect_unions;

/// Validate source coroutine identities and captures before native storage is selected by rustc.
pub fn validate_futures(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    intrinsics: &[EnumDeclaration],
) -> Result<(), String> {
    let _ = (constructors, intrinsics);
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
            return Err(untranslated("async_::capture_types"));
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

/// The boxed future constructor of one coroutine.
pub fn boxed_future_constructor(
    name: String,
    callee: CalleeKind,
    types: Vec<PlanType>,
    output: PlanType,
    span: SourceSpan,
) -> Result<Function, String> {
    let _ = (name, callee, types, output, span);
    Err(untranslated("async_::boxed_future_constructor"))
}

/// The native function `entry_name` that runs a coroutine to its checked output.
pub fn async_entry(
    module: &BodyIrModule,
    body: &Function,
    intrinsics: &mut Vec<EnumDeclaration>,
    entry_name: &str,
) -> Result<(Function, Function, ExternalFunction), String> {
    let _ = (module, body, intrinsics, entry_name);
    Err(untranslated("async_::async_entry"))
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
