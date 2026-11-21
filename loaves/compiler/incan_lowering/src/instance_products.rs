//! Keep generic instance requests and closed generic instances in keyed plan products.
//!
//! A request names its generic by the declaration's stable identity, never by a source position, beside its closed
//! type arguments, owned callable slots and execution role. A caller's product carries the requests its lowering
//! made, so a reused caller still schedules its instances. A closed instance is its own product, keyed by its generic
//! body's checked witness and that request identity.

use std::collections::BTreeMap;

use incan_mir_plan_native::{
    ExternalFunction, Function, UnitNominals, decode_instance_request, encode_instance_request,
};
use incan_semantics_core::body_ir::{Body, BodyIrModule, CallableParamDefault};
use incan_semantics_core::stable_identity::StableDeclarationId;
use incan_semantics_core::stable_identity::source_signature::SourceSignatureContext;
use incan_semantics_core::{CompilerNodeId, IncanCallableParam, SymbolOrigin, encode_incan_symbol_identity};
use internal_models_native::compiler_records::RUNTIME_INSTANCE_IDENTITY;
use sha2::{Digest, Sha256};

use crate::checked_values::location;
use crate::compiler_record_adapters::{checked_type, checked_type_matches_native, native_checked_type};
use crate::nominal_records::retain_named_scalar;
use crate::plan_product_inputs::{graph_owner, ordinary_product_inputs, producer_generation};
use crate::scalar_store::{report_scalar, report_scalar_refusal, reported, retained_scalar};
use crate::trace_channel::frame_product;
use crate::type_parameters::{Instance, request_instance};

/// Separates a closed instance's request identity from its generic body's checked witness in the product key.
const INSTANCE_FUNCTION: &str = "incan.instance-function/2";
/// The toolchain Loaf whose generics lowering instantiates for list and copy operations.
const RUNTIME_LIBRARY: &str = "incan_native_runtime";
/// Stands in for a checker witness in a runtime generic's instance key; its checked meaning is keyed beside it.
const RUNTIME_BODY: &str = "runtime-body:incan_native_runtime";

/// The keyed-product transport of one records request, shared by every closed instance its unit lowers.
#[derive(Clone, Debug)]
pub struct InstanceProducts {
    pub directory: String,
    pub generation: Option<String>,
    pub admitted: BTreeMap<CompilerNodeId, String>,
    pub report: Option<String>,
    pub overflow_checks: bool,
}

/// A closed instance's reused native function with its runtime demand, carried frames and own requests.
pub struct InstanceProduct {
    pub function: Function,
    pub runtime: Vec<ExternalFunction>,
    pub frames: Vec<Function>,
    pub requests: Vec<Instance>,
}

/// The source frame a closed instance lowers in.
pub struct InstanceSource<'a> {
    pub source: &'a str,
    pub file: &'a str,
}

/// A freshly lowered closed instance: its function, runtime demand, synthesized frames and its own requests.
pub type LoweredInstance<'a> = (&'a Function, &'a [ExternalFunction], &'a [Function], &'a [Instance]);

/// A closed instance's product key inputs, generic positions and event identity.
type InstanceKey = (Vec<String>, Vec<(i64, i64)>, String);

/// Name one request without source positions; a generic or argument only a position names is refused.
///
/// A member's request is refused too. Its instance is named for the receiver it was requested on, such as each model
/// that adopts one trait default method, and this identity names no receiver, so a restored request could select
/// another receiver's instance.
pub fn request_identity(request: &Instance) -> Result<Vec<u8>, String> {
    if !graph_owner(&request.body) {
        return Err("generic instance of a member is named for its receiver".into());
    }
    let Some(stable) = signed_template(&request.body) else {
        return Err("generic instance has no checked declaration signature".into());
    };
    let declaration = stable.render_compact();
    let mut arguments = Vec::with_capacity(request.arguments.len());
    for native in &request.arguments {
        let argument = checked_type(native, "", "");
        if !checked_type_matches_native(&argument, native)? {
            return Err("generic instance argument differs from its native checked type".into());
        }
        arguments.push(argument);
    }
    encode_instance_request(
        &declaration,
        &arguments,
        &request.owned_callable_slots,
        request.original_callable,
    )
}

/// Encode the requests a lowering made, in request order, for the product that carries them.
pub fn encode_requests(requests: &[Instance]) -> Result<Vec<Vec<u8>>, String> {
    requests.iter().map(request_identity).collect()
}

/// Rebuild a product's requests against this unit's generic templates; a generic no longer present is a miss.
pub fn restore_requests(encoded: &[Vec<u8>], context: &BodyIrModule) -> Option<Vec<Instance>> {
    let mut restored = Vec::new();
    for row in encoded {
        let (declaration, authored, slots, original) = decode_instance_request(row).ok()?;
        let mut arguments = Vec::with_capacity(authored.len());
        for ty in &authored {
            arguments.push(native_checked_type(ty).ok()?);
        }
        let position = template_position(context, &declaration)?;
        request_instance(&mut restored, &context.bodies[position], arguments, slots, original);
    }
    Some(restored)
}

/// Read a closed instance's product when its generic body and request identity are unchanged.
///
/// 1. Key the instance by its generic body's witness and the request identity; an unkeyable instance has none.
/// 2. Restore the product at the template's current node positions, with the requests it carries.
/// 3. Report it as a reuse, unless an earlier pass of this request already reported it.
pub fn reused_instance(
    products: &InstanceProducts,
    context: &BodyIrModule,
    request: &Instance,
    body: &Body,
    frame: &InstanceSource<'_>,
    nominals: &UnitNominals,
) -> Result<Option<InstanceProduct>, String> {
    // 1. Only a module-level generic the checker witnessed has a product.
    let Some((inputs, positions, _identity)) = instance_key(products, context, request, frame)? else {
        return Ok(None);
    };
    // A runtime retry drains the instances again. The first pass already reused or retained this product and reported
    // which, so the retry restores the same product and keeps that verdict.
    let settled = reported(products.report.as_deref(), &inputs);
    let current = location(&body.span, frame.source, frame.file);
    // 2. A missing or stale product, or a generic it requests that is gone, is an ordinary miss.
    let Some((function, runtime, _references, frames, encoded)) =
        retained_scalar(&products.directory, &inputs, nominals, Some(&current), &positions)
    else {
        return Ok(None);
    };
    let Some(requests) = restore_requests(&encoded, context) else {
        return Ok(None);
    };
    // 3. Only the first pass that reads the product counts it.
    if !settled {
        report_scalar(products.report.as_deref())?;
    }
    frame_product("reused", "instance", &function.name);
    Ok(Some(InstanceProduct {
        function,
        runtime,
        frames,
        requests,
    }))
}

/// Retain a freshly lowered closed instance as its own product, or report why it has none.
///
/// Every instance of a records request ends in a product event or a refusal, so publication sees each plan function.
pub fn retain_instance_product(
    products: &InstanceProducts,
    context: &BodyIrModule,
    request: &Instance,
    (function, runtime, frames, pending): LoweredInstance<'_>,
    frame: &InstanceSource<'_>,
    nominals: &UnitNominals,
) -> Result<(), String> {
    let report = products.report.as_deref();
    let Some((inputs, positions, _identity)) = instance_key(products, context, request, frame)? else {
        return report_scalar_refusal(report);
    };
    frame_product("lowered", "instance", &function.name);
    let encoded = match encode_requests(pending) {
        Ok(encoded) => encoded,
        Err(_) => return report_scalar_refusal(report),
    };
    match retain_named_scalar(
        &products.directory,
        &inputs,
        function,
        runtime,
        nominals,
        &positions,
        frames,
        &encoded,
    ) {
        Ok(_) => report_scalar(report),
        Err(_) => report_scalar_refusal(report),
    }
}

/// Key one closed instance by its generic body's witness and request identity, with the generic's positions.
///
/// A generic of the toolchain's runtime Loaf has no checker witness: no consumer checks its published body. It keys by
/// its own checked meaning under the installed producer instead, and its event names the generic as a runtime
/// instance.
fn instance_key(
    products: &InstanceProducts,
    context: &BodyIrModule,
    request: &Instance,
    frame: &InstanceSource<'_>,
) -> Result<Option<InstanceKey>, String> {
    if !graph_owner(&request.body) {
        return Ok(None);
    }
    let Some(canonical) = &request.body.canonical else {
        return Ok(None);
    };
    let mut identity = encode_incan_symbol_identity(canonical);
    let mut unchecked = None;
    if runtime_generic(&canonical.origin) {
        identity = format!("{RUNTIME_INSTANCE_IDENTITY}{identity}");
        unchecked = Some(RUNTIME_BODY);
    }
    let selected = ordinary_product_inputs(
        context,
        &request.body,
        frame.source,
        frame.file,
        products.overflow_checks,
        Some(&products.directory),
        &products.admitted,
        products.report.is_some(),
        products.generation.as_deref(),
        unchecked,
    )?;
    let Some((mut inputs, positions)) = selected else {
        return Ok(None);
    };
    let Ok(named) = request_identity(request) else {
        return Ok(None);
    };
    inputs.push(INSTANCE_FUNCTION.to_owned());
    inputs.push(format!("sha256:{}", hex::encode(Sha256::digest(&named))));
    Ok(Some((inputs, positions, identity)))
}

/// Whether a generic is published by the toolchain's runtime Loaf, whose bodies are fixed for an installation.
fn runtime_generic(origin: &SymbolOrigin) -> bool {
    matches!(origin, SymbolOrigin::Package { library, .. } if library == RUNTIME_LIBRARY)
}

/// Project the checked generic overload through the same source signature used by the checker.
fn signed_template(body: &Body) -> Option<StableDeclarationId> {
    let canonical = body.canonical.as_ref()?;
    let parameters: Vec<IncanCallableParam> = body
        .params
        .iter()
        .map(|param| IncanCallableParam {
            name: None,
            ty: param.ty.clone(),
            kind: param.kind.clone(),
            has_default: !matches!(param.default, CallableParamDefault::Required),
            is_partial_preset: false,
            is_mut: param.mutable,
            native_owned: param.native_owned,
            input_access: param.input_access,
        })
        .collect();
    let signature = SourceSignatureContext {
        nominals: body.named_type_identities.as_ref(),
        binders: &body.type_parameters,
    };
    let value = signature.callable(&parameters, &body.return_type, body.is_async)?;
    StableDeclarationId::from_canonical(canonical, Some(value), None)
}

/// Restore one uniquely matching overload. Older unsigned requests require a unique declaration.
fn template_position(context: &BodyIrModule, declaration: &str) -> Option<usize> {
    let name = declaration.split('|').next().unwrap_or_default();
    let mut selected = None;
    for (position, body) in context.bodies.iter().enumerate() {
        let Some(canonical) = &body.canonical else {
            continue;
        };
        let Some(stable) = StableDeclarationId::from_canonical(canonical, None, None) else {
            continue;
        };
        if stable.render_compact() != name {
            continue;
        }
        if name != declaration {
            let Some(signed) = signed_template(body) else {
                continue;
            };
            if signed.render_compact() != declaration {
                continue;
            }
        }
        if selected.is_some() {
            return None;
        }
        selected = Some(position);
    }
    selected
}

/// Share one records request's keyed-product transport with its closed instances; other requests keep none.
pub fn instance_transport(
    directory: Option<String>,
    admitted: &BTreeMap<CompilerNodeId, String>,
    report: Option<String>,
    overflow_checks: bool,
) -> Option<InstanceProducts> {
    let root = directory?;
    let generation = producer_generation(Some(&root));
    Some(InstanceProducts {
        directory: root,
        admitted: admitted.clone(),
        report,
        overflow_checks,
        generation,
    })
}
