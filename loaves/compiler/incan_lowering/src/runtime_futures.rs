//! Governed async runtime bodies behind boxing shims: the coroutine step of the Incan `runtime_calls` module.

use incan_mir_plan_native::{Callee, CalleeKind, EnumDeclaration, ExternalFunction, Function, PlanType};
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::{Body, BodyIrModule};

use crate::async_::evaluator_name;
use crate::function_frames::frame_parameter_type;
use crate::layout_types::native_source_type;
use crate::native_type_identity::ConstructorIndex;
use crate::runtime_arguments::{RuntimeTypes, runtime_argument};
use crate::runtime_calls::{RuntimeRedirects, runtime_body, runtime_owned_slots};
use crate::type_parameters::{Instance, instantiate, request_instance};

/// Select governed async bodies behind boxing shims.
///
/// 1. Close their retained arguments. 2. Verify captures and queue the source coroutine.
///
/// Future constructors hold their callee outside ordinary terminators. Their exact capture ABI must agree with the
/// source coroutine; no ordinary return adapter may turn its deferred execution into an eager call. A callee whose
/// declaration this attempt has not loaded keeps its unbound selection.
pub fn retain_runtime_future_instances(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    functions: &mut [Function],
    instances: &mut Vec<Instance>,
    intrinsics: &[EnumDeclaration],
    unions: &[IncanType],
    redirects: &RuntimeRedirects,
) -> Result<(), String> {
    for function in functions.iter_mut() {
        let Some(callee) = &function.future_callee else {
            continue;
        };
        let CalleeKind::Instantiated(path, types) = &callee.kind else {
            continue;
        };
        if !path.starts_with("incan_native_runtime::") || redirects.deferred.contains_key(path) {
            continue;
        }
        let (path, types, span) = (path.clone(), types.clone(), callee.span.clone());
        // 1. Close the retained source declaration's arguments.
        let body = runtime_async_body(module, &path, types.len())?;
        let mut arguments = Vec::with_capacity(types.len());
        for argument in &types {
            let typed = RuntimeTypes {
                module,
                intrinsics,
                unions,
            };
            arguments.push(runtime_argument(&typed, argument)?);
        }
        let parameters: Vec<PlanType> = function
            .parameters
            .iter()
            .map(|parameter| parameter.ty.clone())
            .collect();
        let external = ExternalFunction {
            path: path.clone(),
            type_arguments: types,
            parameters: parameters.clone(),
            return_type: function.return_type.clone(),
            span: span.clone(),
        };
        let owned = runtime_owned_slots(&body, &external)?;
        let closed = instantiate(&body, &arguments, &owned)?;
        // 2. Verify the coroutine's exact captures and output before selecting its deferred evaluator.
        for (slot, parameter) in closed.params.iter().enumerate() {
            let expected = frame_parameter_type(
                constructors,
                &module.nominal_declarations,
                &module.enum_declarations,
                &closed,
                parameter,
                intrinsics,
                &span.file,
            )?;
            if parameters.get(slot) != Some(&expected) {
                return Err(format!("unsupported governed runtime future capture ABI for {path}"));
            }
        }
        let output = native_source_type(
            constructors,
            &module.nominal_declarations,
            &module.enum_declarations,
            unions,
            &closed.return_type,
            intrinsics,
            "",
        )?;
        if function.return_type != PlanType::Future(vec![output]) {
            return Err(format!("unsupported governed runtime future output ABI for {path}"));
        }
        let symbol = evaluator_name(module, &closed);
        if function.name.starts_with(&format!("__incan_future_ctor_{symbol}_")) {
            return Err(format!(
                "unsupported governed runtime body reenters {path}; its primitive must use Rust interop"
            ));
        }
        function.future_callee = Some(Callee {
            kind: CalleeKind::Planned(symbol),
            span,
        });
        request_instance(instances, &body, arguments, owned, false);
    }
    Ok(())
}

/// Require a unique governed coroutine and its complete declared type binding before selecting its evaluator.
fn runtime_async_body(module: &BodyIrModule, path: &str, arity: usize) -> Result<Body, String> {
    match runtime_body(module, path)? {
        Some(body) => {
            if !body.is_async || body.type_parameters.len() != arity {
                return Err(format!("unsupported governed runtime future declaration for {path}"));
            }
            Ok(body)
        }
        None => Err(format!("runtime publishes no governed async body for {path}")),
    }
}
