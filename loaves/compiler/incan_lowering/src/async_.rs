//! Source coroutines: their identities, captures, boxed futures and async entries.

use incan_mir_plan_native::{CalleeKind, EnumDeclaration, ExternalFunction, Function, PlanType, SourceSpan};
use incan_semantics_core::body_ir::{Body, BodyIrModule};
use incan_semantics_core::{IncanPrimitiveType, IncanType, SemanticSourceTargetKind};

use crate::classes::{function_name, is_program_entry};
use crate::native_type_identity::ConstructorIndex;
use crate::pending::untranslated;

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
