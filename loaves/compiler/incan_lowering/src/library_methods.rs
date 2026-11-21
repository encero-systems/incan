//! Lower calls to library methods with their receivers, arguments, and results.

use incan_mir_plan_native::{Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign};
use incan_semantics_core::body_ir::{
    ArgumentBinding, ArgumentElement, BodyIrModule, LoafCallable, MethodTarget, Operand as SourceOperand,
    Place as SourcePlace,
};
use incan_semantics_core::{IncanType, generic};

use crate::call_arguments::call_parameter_type;
use crate::callable_calls::checked_type_arguments;
use crate::callable_values::lower_callable_bound_call;
use crate::calls::{instantiated_external_call, library_call_operands, native_delegation};
use crate::checked_values::scalar_place;
use crate::closures::return_representation;
use crate::derived_methods::{
    comparison_dunder, is_derived_clone, is_derived_display, lower_comparison_dunder, lower_derived_clone,
    receiver_declares_method, slot_name,
};
use crate::element_places::is_element_place;
use crate::frame::{Builder, place_type, read_place, temporary};
use crate::layout_types::{native_source_type, shared_receiver_type};
use crate::list_methods::call_destination;
use crate::method_calls::{is_json_serialize_method, receiver_fills_slot, receiver_method_identity};
use crate::models::model_place;
use crate::nominal_instances::{instance_arguments, template_base};
use crate::operand_borrows::lower_display_str;
use crate::pending::untranslated;
use crate::type_parameters::substitute;
use crate::unions::require_native_union_signature;

/// Call a stdlib method in the SDK crate that compiled it.
///
/// Borrow the retained receiver as the compiled crate declares it, then bind remaining arguments as for a local method.
/// The driver validates the complete signature against that crate's metadata.
///
/// An explicit trait implementation retains its native Self and trait arguments separately from owner binders. The
/// receiver closes owner placeholders, and the recorded native arguments select the exact compiled trait item.
pub fn lower_library_method(
    module: &BodyIrModule,
    builder: &mut Builder,
    delegation: &LoafCallable,
    target: &MethodTarget,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if !target.type_args.is_empty() && target.loaf_trait.is_none() {
        return Err("unsupported Body IR type arguments on a stdlib method".into());
    }
    if delegation.parameters.is_empty() || arguments.is_empty() {
        return Err("unsupported Body IR stdlib method without a receiver".into());
    }
    let names: Vec<String> = delegation.type_parameters.clone();
    let reference = library_receiver(module, builder, &delegation.parameters[0], &arguments[0], span)?;
    let closed = library_method_arguments(builder, delegation, target, &reference, &names)?;
    let mut parameters = vec![place_type(&builder.locals, &reference)];
    let mut ordinary_types: Vec<PlanType> = Vec::new();
    for index in 1..delegation.parameters.len() {
        let parameter = substitute(&delegation.parameters[index], &names, &closed)?;
        require_native_union_signature(&parameter, &builder.intrinsics, native_delegation(delegation))?;
        match delegation.declaration_frame() {
            Some(frame) => ordinary_types.push(call_parameter_type(builder, frame, &frame.params[index])?),
            None => ordinary_types.push(native_source_type(
                builder.constructors(),
                builder.nominals(),
                builder.enums(),
                &builder.unions,
                &parameter,
                &builder.intrinsics,
                "",
            )?),
        }
    }
    if sdk_callable_argument(builder, delegation, &target.binding, arguments)? {
        return Err(untranslated("sdk_generic_callables::sdk_owned_callable_bindings"));
    }
    parameters.extend(ordinary_types.iter().cloned());
    let result = library_method_result(builder, delegation, target, destination, &names, &closed, span)?;
    let ordinary = library_call_operands(
        module,
        builder,
        delegation,
        &target.binding,
        arguments,
        &ordinary_types,
        span,
        1,
        false,
    )?;
    let mut operands = vec![read_place(reference.clone(), true, span)];
    operands.extend(ordinary);
    let type_arguments = sdk_type_arguments(builder, delegation, &names, &closed)?;
    let output = call_destination(builder, destination, result.clone(), span)?;
    if fallible_factory_slot(target, &result) {
        return Err(untranslated("fallible_factories::fallible_factory"));
    }
    let Some(path) = delegation.rust_path() else {
        return Err("unsupported Body IR stdlib method native identity".into());
    };
    if let PlanType::Future(_) = result {
        return Err(untranslated("async_::construct_boxed_future"));
    }
    // Like a source method's, an owned result (text, an enum, a model, a collection) gets an owner in the caller, so a
    // discarded or matched result is dropped as well.
    instantiated_external_call(
        builder,
        &path,
        type_arguments,
        parameters,
        result,
        operands,
        output,
        span,
    )
}

/// Whether the target may be a published standard fallible-iterator factory, whose runtime ABI differs.
fn fallible_factory_slot(target: &MethodTarget, result: &PlanType) -> bool {
    let Some(slot) = &target.canonical else {
        return false;
    };
    let named = matches!(
        slot.declaration_name.as_str(),
        "map" | "filter" | "flat_map" | "take" | "inspect" | "map_err" | "inspect_err"
    );
    named && matches!(result, PlanType::Model(..))
}

/// Whether an SDK generic value binder receives an owned callable environment, which keeps its own storage ABI.
fn sdk_callable_argument(
    builder: &Builder,
    delegation: &LoafCallable,
    binding: &ArgumentBinding,
    arguments: &[ArgumentElement],
) -> Result<bool, String> {
    let ArgumentBinding::Resolved {
        arguments: bindings,
        defaulted_slots,
    } = binding
    else {
        return Ok(false);
    };
    if !defaulted_slots.is_empty() {
        return Ok(false);
    }
    for (index, bound) in bindings.iter().enumerate() {
        let Some(IncanType::TypeVar(_)) = delegation.parameters.get(bound.slot + 1) else {
            continue;
        };
        let Some(argument) = arguments.get(index + 1) else {
            continue;
        };
        if let Some(SourceOperand::Place(read)) = argument.as_one()
            && read.place.projection.is_empty()
            && read.place.global().is_none()
            && let Some(local) = read.place.local_id()
        {
            let held = &builder.locals[(local.index() as i64 + builder.local_offset + 1) as usize].ty;
            if matches!(held, PlanType::Closure(_) | PlanType::ClosureRef(_)) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Retain explicit native argument order, or close declaration binders without constructing substitute Rust types.
fn sdk_type_arguments(
    builder: &Builder,
    delegation: &LoafCallable,
    names: &[String],
    closed: &[IncanType],
) -> Result<Vec<PlanType>, String> {
    let resolve = |ty: &IncanType| {
        native_source_type(
            builder.constructors(),
            builder.nominals(),
            builder.enums(),
            &builder.unions,
            ty,
            &builder.intrinsics,
            "",
        )
    };
    let mut result = Vec::new();
    if !delegation.native_type_arguments.is_empty() {
        for argument in &delegation.native_type_arguments {
            result.push(resolve(&substitute(argument, names, closed)?)?);
        }
        return Ok(result);
    }
    for argument in closed {
        result.push(resolve(argument)?);
    }
    Ok(result)
}

/// Close native Self from the concrete nominal frame or checked scalar receiver, then checked trait and method binders.
fn library_method_arguments(
    builder: &Builder,
    delegation: &LoafCallable,
    target: &MethodTarget,
    reference: &Place,
    names: &[String],
) -> Result<Vec<IncanType>, String> {
    let Some(selected) = &target.loaf_trait else {
        return library_receiver_arguments(builder, reference, names);
    };
    if delegation.rust_path().as_deref() != Some(selected.native_item.as_str())
        || names.first().map(String::as_str) != Some("Self")
    {
        return Err("the selected standard library trait method differs from its checked call target".into());
    }
    let mut closed = match place_type(&builder.locals, reference) {
        PlanType::ModelRef(..) | PlanType::ModelMutRef(..) => {
            library_receiver_arguments(builder, reference, &["Self".to_string()])?
        }
        _ => match &selected.receiver_type {
            Some(receiver) => checked_type_arguments(builder, std::slice::from_ref(receiver))?,
            None => return Err("the standard library scalar receiver has no checked Self type".into()),
        },
    };
    // A selected native signature has already closed its trait binders; only Self remains a source binder. Its
    // complete native argument vector is still substituted and validated at the external call boundary.
    if names.len() == 1 && !delegation.native_type_arguments.is_empty() {
        return Ok(closed);
    }
    closed.extend(checked_type_arguments(builder, &selected.type_arguments)?);
    closed.extend(checked_type_arguments(builder, &target.type_args)?);
    if closed.len() != names.len() {
        return Err("the standard library trait method type argument count is unsupported".into());
    }
    Ok(closed)
}

/// Use the frontend-retained concrete SDK factory result, with native metadata still checking the exact return carrier.
fn library_method_result(
    builder: &mut Builder,
    delegation: &LoafCallable,
    target: &MethodTarget,
    destination: &Option<SourcePlace>,
    names: &[String],
    closed: &[IncanType],
    span: &SourceSpan,
) -> Result<PlanType, String> {
    if target.loaf_trait.is_some()
        && let Some(place) = destination
    {
        let output = scalar_place(builder, place, span)?;
        return Ok(place_type(&builder.locals, &output));
    }
    let returned = substitute(&delegation.return_type, names, closed)?;
    require_native_union_signature(&returned, &builder.intrinsics, native_delegation(delegation))?;
    let result = native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        &returned,
        &builder.intrinsics,
        "",
    )?;
    // A published method's declaration frame represents its result as a function's does: a direct returned view is a
    // reference to its source place.
    match delegation.declaration_frame() {
        Some(frame) => return_representation(frame, result),
        None => Ok(result),
    }
}

/// Close SDK owner binders from the retained receiver; a trait's Self binder takes that entire concrete owner.
fn library_receiver_arguments(builder: &Builder, receiver: &Place, names: &[String]) -> Result<Vec<IncanType>, String> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let owner = match place_type(&builder.locals, receiver) {
        PlanType::ModelRef(index, _) | PlanType::ModelMutRef(index, _) => index,
        _ => return Err("the standard library method receiver type arguments are unsupported".into()),
    };
    let declaration = &builder.nominals()[owner as usize];
    let arguments = instance_arguments(declaration)?;
    if names.len() == 1 && names[0] == "Self" {
        if arguments.is_empty() {
            return Ok(vec![IncanType::Named(declaration.name.clone())]);
        }
        return Ok(vec![generic(template_base(declaration), arguments)]);
    }
    if arguments.len() == names.len() {
        return Ok(arguments);
    }
    Err("the standard library method owner type arguments are unsupported".into())
}

/// Borrow the receiver place as the stdlib method declares it, shared or exclusive, never moving its owner.
fn library_receiver(
    module: &BodyIrModule,
    builder: &mut Builder,
    declared: &IncanType,
    argument: &ArgumentElement,
    span: &SourceSpan,
) -> Result<Place, String> {
    let Some(value) = argument.as_one() else {
        return Err("unsupported Body IR stdlib method spread receiver".into());
    };
    let SourceOperand::Place(read) = value else {
        return Err("unsupported Body IR stdlib method nonplace receiver".into());
    };
    if is_element_place(builder, &read.place)? {
        return Err(untranslated("library_methods::library_receiver element"));
    }
    let _ = module;
    let mut receiver = model_place(
        builder.tables(),
        &builder.locals,
        &read.place,
        span,
        builder.local_offset,
    )?;
    receiver.local += builder.local_offset;
    let source = place_type(&builder.locals, &receiver);
    match declared {
        IncanType::Ref(_) => {
            let reference = temporary(builder, shared_receiver_type(source)?, span);
            builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(receiver), span));
            Ok(reference)
        }
        IncanType::RefMut(_) => match source {
            PlanType::Model(owner, name) => {
                let reference = temporary(builder, PlanType::ModelMutRef(owner, name), span);
                builder.emit(plan_assign(reference.clone(), RvalueKind::MutBorrow(receiver), span));
                Ok(reference)
            }
            _ => Err(format!(
                "the mutable standard library receiver requires a nominal layout; body={}; receiver={source:?}; at={}:{}:{}",
                builder.name, span.file, span.line, span.column
            )),
        },
        _ => Err("unsupported Body IR by-value stdlib method receiver".into()),
    }
}

/// Lower a standard-library slot through the behavior a derive or a held callable gives the receiver.
///
/// A slot the receiver's own method fills is left to ordinary dispatch, because that method is what legacy calls. A
/// receiver type that declares a method of the slot's name without such a record also uses ordinary dispatch. A
/// checked Serialize default that still names the protocol itself keeps the canonical JSON implementation; retaining
/// that default is not an override.
pub fn lower_standard_slot(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &MethodTarget,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<bool, String> {
    let json = is_json_serialize_method(target);
    let mut json_default = false;
    if receiver_fills_slot(module, target, builder, &arguments[0], span)? {
        if !json {
            return Ok(false);
        }
        let selected = receiver_method_identity(module, target, builder, &arguments[0], span)?;
        if selected != target.canonical {
            return Ok(false);
        }
        // A retained default points at the protocol itself; only a distinct implementation overrides that slot.
        json_default = true;
    }
    let comparison = comparison_dunder(target);
    let standard_slot = json || is_derived_clone(target) || is_derived_display(target) || comparison.is_some();
    if standard_slot
        && !json_default
        && receiver_declares_method(module, builder, &arguments[0], &slot_name(target), span)?
    {
        return Ok(false);
    }
    if json {
        return Err(untranslated("model_values::serialize_model_operand"));
    }
    if is_derived_clone(target) {
        lower_derived_clone(builder, arguments, destination, span)?;
        return Ok(true);
    }
    if let Some(operator) = comparison {
        lower_comparison_dunder(module, builder, operator, arguments, destination, span)?;
        return Ok(true);
    }
    if is_derived_display(target) {
        lower_display_str(builder, arguments, destination, span)?;
        return Ok(true);
    }
    lower_callable_bound_call(builder, target, arguments, destination, span)
}
