//! Checked serde reconstruction rooted in exact source hook bodies and authored Incan error mapping.

use std::sync::Arc;

use incan_lang::lang::types::collections::{CollectionTypeId, from_str as collection_type};
use incan_mir_plan_native::{
    CalleeKind, EnumDeclaration, ExternalFunction, Function, Parameter, PlanType, SourceSpan, TerminatorKind,
    carrier_position, is_carrier_index, place as plan_place, source_span as plan_span,
};
use incan_semantics_core::body_ir::{Body, BodyIrModule};
use incan_semantics_core::{IncanPrimitiveType, IncanType, SemanticSourceTargetKind, generic};

use crate::calls::instantiated_external_call;
use crate::checked_json_contracts::{checked_deserialize_name, checked_owner_arguments, selected_deserialize_hook};
use crate::classes::function_name;
use crate::decls::library_item;
use crate::frame::{Builder, drop_all_owned, forget_owned, read_place, temporary, terminate_call};
use crate::layout_types::native_source_type;
use crate::native_type_identity::ConstructorIndex;
use crate::nominal_instances::NominalInstance;
use crate::options::{result_layout, retain_body_carriers, retain_carrier};
use crate::string_input::owned_string_frame;
use crate::type_parameters::{Instance, merge_instance_requests, request_instance};
use crate::unions::collect_unions;

/// The selected source hook and the checked owner and result carriers one ingress function joins.
struct CheckedIngress<'a> {
    hook: &'a Body,
    selected: &'a Body,
    arguments: Vec<IncanType>,
    name: String,
    owner: PlanType,
    hook_result: PlanType,
    result: PlanType,
}

/// Close checked ingress roots from `start`, so a resumed plan visits only newly resolved declarations.
pub fn lower_checked_deserialize(
    constructors: &Arc<ConstructorIndex>,
    module: &BodyIrModule,
    instances: &[NominalInstance],
    file: &str,
    intrinsics: &mut Vec<EnumDeclaration>,
    start: usize,
) -> Result<(Vec<Function>, Vec<ExternalFunction>, Vec<Instance>), String> {
    let mut functions = Vec::new();
    let mut externals = Vec::new();
    let mut pending: Vec<Instance> = Vec::new();
    for owner_index in start..module.nominal_declarations.len() {
        let declaration = &module.nominal_declarations[owner_index];
        if declaration.canonical.kind != SemanticSourceTargetKind::Newtype || !library_item(declaration).is_empty() {
            continue;
        }
        let name = checked_deserialize_name(declaration)?;
        if name.is_empty() {
            continue;
        }
        let arguments = checked_owner_arguments(declaration, instances)?;
        let (hook, selected) = selected_deserialize_hook(module, declaration, &arguments)?;
        retain_body_carriers(constructors, module, intrinsics, &selected, file)?;
        let owner = PlanType::Model(owner_index as i64, declaration.name.to_string());
        let success = checked_success(&selected)?;
        let output = generic(
            "Result".into(),
            vec![success, IncanType::Primitive(IncanPrimitiveType::Str)],
        );
        retain_carrier(constructors, module, intrinsics, &output, file)?;
        let nominals = &module.nominal_declarations;
        let enums = &module.enum_declarations;
        let hook_result = native_source_type(
            constructors,
            nominals,
            enums,
            &collect_unions(module),
            &selected.return_type,
            intrinsics,
            "",
        )?;
        let result = native_source_type(
            constructors,
            nominals,
            enums,
            &collect_unions(module),
            &output,
            intrinsics,
            "",
        )?;
        let span = plan_span(file, 1, 1);
        let ingress = CheckedIngress {
            hook: &hook,
            selected: &selected,
            arguments,
            name,
            owner,
            hook_result,
            result,
        };
        let (function, runtime, scheduled) =
            checked_json_function(constructors.clone(), module, ingress, intrinsics, &span)?;
        functions.push(function);
        externals.extend(runtime);
        merge_instance_requests(&mut pending, &scheduled);
    }
    Ok((functions, externals, pending))
}

/// Read the already validated standard Result success without reconstructing a nominal identity by spelling.
fn checked_success(hook: &Body) -> Result<IncanType, String> {
    if let IncanType::Generic { base, args } = &hook.return_type
        && collection_type(base) == Some(CollectionTypeId::Result)
        && args.len() == 2
    {
        return Ok(args[0].clone());
    }
    Err("unsupported Body IR checked Deserialize hook Result".into())
}

/// Invoke the previously validated source hook once, then return its actual displayed error through owned Result.
fn checked_json_function(
    constructors: Arc<ConstructorIndex>,
    module: &BodyIrModule,
    ingress: CheckedIngress<'_>,
    intrinsics: &[EnumDeclaration],
    span: &SourceSpan,
) -> Result<(Function, Vec<ExternalFunction>, Vec<Instance>), String> {
    let CheckedIngress {
        hook,
        selected,
        arguments,
        name,
        owner,
        hook_result,
        result,
    } = ingress;
    let mut builder = owned_string_frame(constructors, module, &name, result.clone(), intrinsics, span);
    let checked_layout = checked_result_layout(&builder, &hook_result)?;
    let result_layout_value = checked_result_layout(&builder, &result)?;
    if checked_layout.variants[0].fields != [owner.clone()]
        || result_layout_value.variants[0].fields != [owner.clone()]
        || result_layout_value.variants[1].fields != [PlanType::String]
    {
        return Err("unsupported Body IR checked Deserialize Result owner disagreement".into());
    }
    let error = checked_layout.variants[1].fields[0].clone();
    if error != PlanType::ValidationError {
        return Err("unsupported Body IR checked Deserialize error carrier".into());
    }
    let checked = temporary(&mut builder, hook_result.clone(), span);
    terminate_call(
        &mut builder,
        CalleeKind::Planned(function_name(module, selected)),
        vec![read_place(plan_place(1, span), true, span)],
        checked.clone(),
        span,
    );
    builder.owned.push(checked.local);
    instantiated_external_call(
        &mut builder,
        "incan_native_runtime::checked_json_result",
        vec![owner, error],
        vec![hook_result],
        result.clone(),
        vec![read_place(checked, true, span)],
        plan_place(0, span),
        span,
    )?;
    forget_owned(&mut builder, 0);
    drop_all_owned(&mut builder, span);
    builder.finish(TerminatorKind::Return, span);
    let function = Function::new(
        name,
        vec![Parameter {
            name: "value".into(),
            ty: PlanType::String,
            span: span.clone(),
        }],
        result,
        builder.locals,
        builder.blocks,
        span.clone(),
    );
    let mut pending: Vec<Instance> = Vec::new();
    if !hook.type_parameters.is_empty() {
        request_instance(&mut pending, hook, arguments, Vec::new(), false);
    }
    Ok((function, builder.externals, pending))
}

/// Check bounds and the full standard Result shape before reading checked ingress payloads.
fn checked_result_layout(builder: &Builder, ty: &PlanType) -> Result<EnumDeclaration, String> {
    let PlanType::Enum(owner, name) = ty else {
        return Err("unsupported Body IR checked Deserialize Result carrier".into());
    };
    if !is_carrier_index(*owner) || carrier_position(*owner) >= builder.intrinsics.len() as i64 {
        return Err("unsupported Body IR checked Deserialize Result index".into());
    }
    if builder.intrinsics[carrier_position(*owner) as usize].name != *name {
        return Err("unsupported Body IR checked Deserialize Result name".into());
    }
    // A Result carrier lays out Ok then Err; its variant spellings are presentation only.
    let (_, declaration) = result_layout(builder, ty)?;
    if declaration.variants.len() != 2 {
        return Err("unsupported Body IR checked Deserialize Result variants".into());
    }
    if declaration.variants[0].fields.len() != 1 || declaration.variants[1].fields.len() != 1 {
        return Err("unsupported Body IR checked Deserialize Result payloads".into());
    }
    Ok(declaration)
}
