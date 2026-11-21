//! Derived nominal formatting and comparisons over the retained source field layout.

use incan_mir_plan_native::{
    CalleeKind, Carrier, ExternalFunction, Operand, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign,
};
use incan_semantics_core::body_ir::{
    ArgumentElement, BinOp, Body, BodyIrModule, MethodTarget, NamedCallableTarget, NominalDeclaration,
    Operand as SourceOperand, Place as SourcePlace,
};
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolOrigin};

use crate::callable_calls::lower_callable_result;
use crate::checked_values::scalar_place;
use crate::decls::library_item;
use crate::element_places::{element_place_type, is_element_place};
use crate::formatting::native_format_text;
use crate::frame::{Builder, external_call, place_type, read_place, temporary, terminate_call};
use crate::layout_types::native_source_type;
use crate::list_methods::materialized_receiver;
use crate::options::carrier_layout;
use crate::sequence_builtins::text_argument;
use crate::text_ownership::drop_borrowed_text_temporaries;
use crate::type_parameters::substitute;

/// Locate structural model storage or materialize an indexed model once after checking its retained carrier.
pub fn nominal_place(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<Option<Place>, String> {
    let SourceOperand::Place(read) = value else {
        return Ok(None);
    };
    if is_element_place(builder, &read.place)? {
        return match element_place_type(builder, &read.place)? {
            PlanType::Model(..) => Ok(Some(materialized_receiver(builder, &read.place, span)?)),
            _ => Ok(None),
        };
    }
    if read.place.local_id().is_none() {
        return Ok(None);
    }
    let place = scalar_place(builder, &read.place, span)?;
    match place_type(&builder.locals, &place) {
        PlanType::Model(..) => Ok(Some(place)),
        _ => Ok(None),
    }
}

/// Borrow a nominal value for a runtime trait call, retaining its owner's initialized storage.
fn borrow_model(builder: &mut Builder, value: Place, ty: &PlanType, span: &SourceSpan) -> Result<Operand, String> {
    let PlanType::Model(owner, name) = ty else {
        return Err("unsupported Body IR model trait receiver".into());
    };
    let reference = temporary(builder, PlanType::ModelRef(*owner, name.clone()), span);
    builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(value), span));
    Ok(read_place(reference, false, span))
}

/// Retain the exact nominal instantiation and shared-reference signature for metadata validation.
fn model_call(
    builder: &mut Builder,
    name: &str,
    ty: &PlanType,
    result: PlanType,
    arguments: Vec<Operand>,
    destination: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let PlanType::Model(owner, model_name) = ty else {
        return Err("unsupported Body IR model trait instantiation".into());
    };
    let reference = PlanType::ModelRef(*owner, model_name.clone());
    let parameters = vec![reference; arguments.len()];
    let path = format!("incan_native_runtime::{name}");
    let owned = result == PlanType::String;
    let local = destination.local;
    builder.externals.push(ExternalFunction {
        path: path.clone(),
        type_arguments: vec![ty.clone()],
        parameters,
        return_type: result,
        span: span.clone(),
    });
    terminate_call(
        builder,
        CalleeKind::Instantiated(path, vec![ty.clone()]),
        arguments,
        destination,
        span,
    );
    if owned {
        builder.owned.push(local);
    }
    Ok(())
}

/// Render with the nominal's derived Debug implementation rather than reconstructing field text.
pub fn debug_model(builder: &mut Builder, value: Place, span: &SourceSpan) -> Result<Place, String> {
    let ty = place_type(&builder.locals, &value);
    let operand = borrow_model(builder, value, &ty, span)?;
    native_format_text(builder, operand, true, span)
}

/// Accept explicit Debug formatting only for a retained nominal place.
pub fn debug_model_operand(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<Place, String> {
    match nominal_place(builder, value, span)? {
        Some(place) => debug_model(builder, place, span),
        None => Err("unsupported Body IR nonmodel FormatStyle.Debug".into()),
    }
}

/// Select a trait operation without translating nominal comparisons into scalar MIR.
fn comparison_name(operator: BinOp) -> Result<&'static str, String> {
    match operator {
        BinOp::Eq => Ok("equal_model"),
        BinOp::Ne => Ok("unequal_model"),
        BinOp::Lt => Ok("less_model"),
        BinOp::Le => Ok("less_equal_model"),
        BinOp::Gt => Ok("greater_model"),
        BinOp::Ge => Ok("greater_equal_model"),
        _ => Err("unsupported Body IR model binary operator".into()),
    }
}

/// Borrow matching carriers; native traits are verified by rustc, while source traits require retained witnesses.
fn compare_places(
    builder: &mut Builder,
    destination: Place,
    operator: BinOp,
    first: Place,
    right: &SourceOperand,
    span: &SourceSpan,
) -> Result<bool, String> {
    let Some(second) = nominal_place(builder, right, span)? else {
        return Err("unsupported Body IR mixed model comparison".into());
    };
    let ty = place_type(&builder.locals, &first);
    if ty != place_type(&builder.locals, &second) {
        return Err("unsupported Body IR distinct model comparison".into());
    }
    let name = comparison_name(operator)?;
    let PlanType::Model(owner, _) = &ty else {
        return Err("unsupported Body IR model comparison carrier".into());
    };
    let equality = matches!(operator, BinOp::Eq | BinOp::Ne);
    let (required, count) = if equality { ("PartialEq", 1) } else { ("PartialOrd", 5) };
    let declaration = &builder.nominals()[*owner as usize];
    if library_item(declaration).is_empty()
        && !declaration.derives.iter().any(|derive| derive == required)
        && builder.program.comparison_functions[*owner as usize].len() < count
    {
        return Err(format!(
            "unsupported Body IR model comparison without derived {required}"
        ));
    }
    let arguments = vec![
        borrow_model(builder, first, &ty, span)?,
        borrow_model(builder, second, &ty, span)?,
    ];
    model_call(builder, name, &ty, PlanType::Bool, arguments, destination, span)?;
    Ok(true)
}

/// Use native Rust comparison implementations or retained source derives and implementation frames.
pub fn compare_models(
    builder: &mut Builder,
    destination: Place,
    operator: BinOp,
    left: &SourceOperand,
    right: &SourceOperand,
    span: &SourceSpan,
) -> Result<bool, String> {
    match nominal_place(builder, left, span)? {
        None => Ok(false),
        Some(first) => compare_places(builder, destination, operator, first, right, span),
    }
}

/// Prefer a source carrier's checked string method across Loaf boundaries; native carriers use their Rust
/// implementation.
pub fn display_model_operand(
    builder: &mut Builder,
    value: &SourceOperand,
    span: &SourceSpan,
) -> Result<Option<Place>, String> {
    let Some(place) = nominal_place(builder, value, span)? else {
        return Ok(None);
    };
    let ty = place_type(&builder.locals, &place);
    let PlanType::Model(owner, _) = &ty else {
        return Err("unsupported Body IR model display carrier".into());
    };
    let declaration = &builder.nominals()[*owner as usize];
    if declaration.display_method.is_some() {
        return Ok(Some(display_checked_model(builder, *owner, place, span)?));
    }
    if library_item(declaration).is_empty() && !declaration.derives.iter().any(|derive| derive == "Display") {
        return Err("unsupported Body IR model display without derived Display".into());
    }
    let argument = borrow_model(builder, place, &ty, span)?;
    Ok(Some(native_format_text(builder, argument, false, span)?))
}

/// Call the exact validated source body or SDK item; a crate call keeps its metadata-checked shared signature.
fn display_checked_model(builder: &mut Builder, owner: i64, place: Place, span: &SourceSpan) -> Result<Place, String> {
    // The call consumes its selected callee, while the program mapping remains shared.
    let callee = builder
        .program
        .display_functions
        .iter()
        .find(|mapping| mapping.owner == owner)
        .map(|mapping| mapping.callee.clone());
    let Some(callee) = callee else {
        let name = &builder.nominals()[owner as usize].name;
        return Err(format!(
            "unsupported Body IR unavailable checked display body on {name}"
        ));
    };
    let ty = place_type(&builder.locals, &place);
    let argument = borrow_model(builder, place, &ty, span)?;
    let result = temporary(builder, PlanType::String, span);
    match callee {
        CalleeKind::Planned(symbol) => {
            terminate_call(
                builder,
                CalleeKind::Planned(symbol),
                vec![argument],
                result.clone(),
                span,
            );
        }
        CalleeKind::External(path) => {
            let reference = PlanType::ModelRef(owner, builder.nominals()[owner as usize].name.to_string());
            external_call(
                builder,
                &path,
                vec![reference],
                PlanType::String,
                vec![argument],
                result.clone(),
                span,
            );
        }
        _ => return Err("unsupported Body IR checked display callee kind".into()),
    }
    builder.owned.push(result.local);
    Ok(result)
}

/// Serialize a retained model with checked serde capability, borrowing its original storage.
pub fn serialize_model_operand(
    builder: &mut Builder,
    value: &SourceOperand,
    destination: Place,
    span: &SourceSpan,
) -> Result<bool, String> {
    let Some(place) = nominal_place(builder, value, span)? else {
        return Ok(false);
    };
    let ty = place_type(&builder.locals, &place);
    let PlanType::Model(owner, _) = &ty else {
        return Err("unsupported Body IR model JSON carrier".into());
    };
    if !builder.nominals()[*owner as usize]
        .derives
        .iter()
        .any(|derive| derive == "serde::Serialize")
    {
        return Err("unsupported Body IR model JSON serialization without serde::Serialize".into());
    }
    let argument = borrow_model(builder, place, &ty, span)?;
    model_call(
        builder,
        "serialize_model",
        &ty,
        PlanType::String,
        vec![argument],
        destination,
        span,
    )?;
    Ok(true)
}

/// Whether an identity is the serde JSON protocol member `name` in source or in the compiled standard data Loaf.
fn is_json_protocol_member(identity: Option<&CanonicalSymbolId>, name: &str) -> bool {
    let Some(identity) = identity else {
        return false;
    };
    if identity.kind != SemanticSourceTargetKind::Method || identity.declaration_name != name {
        return false;
    }
    match &identity.origin {
        SymbolOrigin::Module(path) => path.len() == 3 && path[0] == "std" && path[1] == "serde" && path[2] == "json",
        SymbolOrigin::Package { library, module_path } => {
            library == "incan_stdlib_data"
                && module_path.len() == 2
                && module_path[0] == "serde"
                && module_path[1] == "json"
        }
        _ => false,
    }
}

/// Recognize the JSON protocol default in source and compiled SDK identities, never a same-name override.
pub fn is_json_serialize_method(target: &MethodTarget) -> bool {
    is_json_protocol_member(target.canonical.as_ref(), "to_json")
}

/// Refuse the serde protocol default named `method` for a model receiver that declares its own `method` body.
///
/// A call that reaches the protocol default through a type parameter must run the adopter's own method instead, and
/// Body IR records no implementation slot for an imported protocol trait, so that dispatch stays refused.
pub fn refuse_overridden_json_default(
    module: &BodyIrModule,
    builder: &Builder,
    receiver: &PlanType,
    method: &str,
) -> Result<(), String> {
    if let PlanType::Model(owner, _) = receiver {
        let declaration = &builder.nominals()[*owner as usize];
        if module
            .bodies
            .iter()
            .any(|body| declares_own_method(declaration, body, method))
        {
            return Err(format!(
                "unsupported Body IR JSON protocol default over an adopter's own {method}"
            ));
        }
    }
    Ok(())
}

/// Whether a body is a method spelled `method` declared inside the model `owner`, as an adopter's override is.
fn declares_own_method(owner: &NominalDeclaration, body: &Body, method: &str) -> bool {
    let Some(identity) = &body.canonical else {
        return false;
    };
    if identity.kind != SemanticSourceTargetKind::Method || identity.declaration_name != method {
        return false;
    }
    if identity.origin != owner.canonical.origin {
        return false;
    }
    let member = &identity.declaration_span;
    let declaration = &owner.canonical.declaration_span;
    declaration.start <= member.start && member.end <= declaration.end
}

/// Recognize the JSON protocol's `from_json` default in source and compiled SDK identities, never an override.
pub fn is_json_deserialize_method(target: &NamedCallableTarget) -> bool {
    is_json_protocol_member(target.canonical.as_ref(), "from_json")
}

/// Parse a model that derives serde's Deserialize through legacy's serde decoding and its error text.
pub fn lower_model_from_json(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &NamedCallableTarget,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let ty = static_receiver_type(builder, target)?;
    let PlanType::Model(owner, _) = &ty else {
        return Err("unsupported Body IR JSON deserialization receiver".into());
    };
    if !builder.nominals()[*owner as usize]
        .derives
        .iter()
        .any(|derive| derive == "serde::Deserialize")
    {
        return Err("unsupported Body IR model JSON deserialization without serde::Deserialize".into());
    }
    refuse_overridden_json_default(module, builder, &ty, "from_json")?;
    if arguments.len() != 1 || !target.type_args.is_empty() {
        return Err("invalid Body IR from_json arguments".into());
    }
    let Some(place) = destination else {
        return Err("unsupported Body IR discarded from_json".into());
    };
    let output = scalar_place(builder, place, span)?;
    let result = place_type(&builder.locals, &output);
    require_parsed_result(builder, &result, &ty)?;
    let incoming = builder.owned.clone();
    let text = text_argument(builder, arguments, 0, span)?;
    let path = "incan_native_runtime::deserialize_model";
    builder.externals.push(ExternalFunction {
        path: path.into(),
        type_arguments: vec![ty.clone()],
        parameters: vec![PlanType::StringRef],
        return_type: result.clone(),
        span: span.clone(),
    });
    let local = output.local;
    let callee = CalleeKind::Instantiated(path.into(), vec![ty]);
    lower_callable_result(builder, callee, vec![text], output, result, span)?;
    drop_borrowed_text_temporaries(builder, &incoming, local, span);
    Ok(())
}

/// The checked static owner, closed in the active instance when it names a type parameter.
fn static_receiver_type(builder: &Builder, target: &NamedCallableTarget) -> Result<PlanType, String> {
    let Some(ty) = &target.receiver_type else {
        return Err("unsupported Body IR from_json without a receiver type".into());
    };
    let closed = substitute(ty, &builder.type_parameters, &builder.type_arguments)?;
    native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        &closed,
        &builder.intrinsics,
        "",
    )
}

/// Require the destination to be the `Result[Model, str]` carrier legacy's `from_json` returns.
fn require_parsed_result(builder: &Builder, result: &PlanType, payload: &PlanType) -> Result<(), String> {
    let (_, declaration) = carrier_layout(builder, result)?;
    if declaration.carrier != Carrier::Result
        || declaration.variants[0].fields != [payload.clone()]
        || declaration.variants[1].fields != [PlanType::String]
    {
        return Err("unsupported Body IR from_json destination carrier".into());
    }
    Ok(())
}
