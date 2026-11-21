//! Method calls: checked receivers, selected implementations and their ordinary arguments.

use incan_lang::lang::surface::iterator_methods::{IteratorMethodId, as_str as iterator_method_name};
use incan_mir_plan_native::{
    CalleeKind, Operand, Place, PlanType, Projection, RvalueKind, SourceSpan, assign as plan_assign,
};
use incan_semantics_core::body_ir::{
    ArgumentBinding, ArgumentElement, Body, BodyIrModule, MethodTarget, Operand as SourceOperand, OwnershipFact,
    Place as SourcePlace, ReflectionOperation,
};
use incan_semantics_core::{CanonicalSymbolId, IncanType, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};

use crate::async_::construct_future;
use crate::builtin_methods::lower_builtin_method;
use crate::call_arguments::{SlotPolicy, call_parameter_type, has_defaulted_slots, ordered_slot_operands};
use crate::callable_calls::{checked_type_arguments, emit_selected_body_call};
use crate::checked_values::scalar_place;
use crate::classes::{frame_type, function_name, receiver_mode};
use crate::decls::{library_enum_item, library_item};
use crate::defaults::{DefaultCall, default_arguments};
use crate::element_borrows::{element_method_owner, element_method_receiver};
use crate::element_places::{element_place_type, is_element_place};
use crate::explicit_arguments::parameter_accesses;
use crate::frame::{Builder, dereferenced_place, place_type, read_place, temporary};
use crate::generic_callables::owned_callable_slots;
use crate::iterator_methods::standard_protocol as standard_iterator_protocol;
use crate::layout_types::native_source_type;
use crate::library_methods::{lower_library_method, lower_standard_slot};
use crate::list_methods::materialized_receiver;
use crate::models::model_place;
use crate::pending::untranslated;
use crate::reflection::lower_reflection;
use crate::result_carriers::lower_carrier_method;
use crate::result_combinators::{lower_result_combinator, result_combinator};
use crate::sequence_builtins::argument_type;
use crate::static_methods::{StaticCall, lower_static_method_body, lower_static_sdk_method};
use crate::static_storage::computed_static_type;
use crate::traits::{is_lazy_method_body, receiver_selects_slot_arguments};
use crate::type_parameters::{instantiate, request_instance};
use crate::value_enum_methods::{lower_enum_message, lower_value_enum_getter_from, lower_value_enum_method};
use crate::value_enums::{enum_message_layout, value_getter_owner, value_method_owner};

/// Close retained source trait arguments in the caller's checked frame before choosing an implementation.
pub fn lower_method_call(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &MethodTarget,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let mut selected = target.clone();
    if let Some(trait_arguments) = &target.trait_arguments {
        selected.trait_arguments = Some(checked_type_arguments(builder, trait_arguments)?);
    }
    lower_checked_method_call(module, builder, &selected, arguments, destination, span)
}

/// Borrow the proven receiver and call its canonical method body with bound ordinary arguments.
fn lower_checked_method_call(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &MethodTarget,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if target.has_builtin_selection() && !target.has_valid_builtin_selection() {
        return Err("unsupported Body IR builtin method operation without matching canonical provenance".into());
    }
    if target.intrinsic_carrier.is_some() {
        return lower_carrier_method(builder, target, arguments, destination, span);
    }
    // Builtin registry members and source methods both retain identities; their origin selects the admission path.
    let Some(identity) = &target.canonical else {
        return Err(format!(
            "unsupported Body IR Method without canonical identity for {} at {}:{}:{} in {}",
            target.name, builder.file, span.line, span.column, builder.name
        ));
    };
    if identity.origin == SymbolOrigin::Builtin {
        return lower_builtin_origin_method(module, builder, target, arguments, destination, span);
    }
    if arguments.is_empty() {
        return Err("invalid Body IR Method missing receiver".into());
    }
    match special_method_route(module, builder, target, arguments, destination, span)? {
        MethodRoute::Lowered => return Ok(()),
        MethodRoute::Library(index, selected) => {
            let callable = &module.loaf_callables[index];
            return lower_library_method(module, builder, callable, &selected, arguments, destination, span);
        }
        MethodRoute::SelectedLibrary => {
            let Some(selection) = &target.native_trait_delegation else {
                return Err("invalid Body IR selected library method without a delegation".into());
            };
            return lower_library_method(module, builder, selection, target, arguments, destination, span);
        }
        MethodRoute::Reflection => {
            lower_reflection(builder, target, arguments, destination, span)?;
            return Ok(());
        }
        MethodRoute::EnumMessage(index) => return lower_enum_message(builder, index, arguments, destination, span),
        MethodRoute::ValueGetter(owner) => {
            return lower_value_enum_method(builder, owner, arguments, destination, span);
        }
        MethodRoute::Element(owner) => {
            return lower_element_method(module, builder, target, owner, arguments, destination, span);
        }
        MethodRoute::Retained => {}
    }
    lower_retained_method_call(module, builder, target, arguments, destination, span)
}

/// The dispatch route a method call selects before its retained body, if any: static receivers, reflection, standard
/// slots, compiled Loaf implementations, retained iterator defaults, SDK delegations, element receivers and generated
/// enum members each lower through their own step.
fn special_method_route(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &MethodTarget,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<MethodRoute, String> {
    let receiver = &arguments[0];
    if lower_global_method(module, builder, target, arguments, destination, span)? {
        return Ok(MethodRoute::Lowered);
    }
    match &target.operation {
        None | Some(ReflectionOperation::Builtin(_)) => {}
        Some(_) => return Ok(MethodRoute::Reflection),
    }
    // Any other slot leaves the standard-slot step without effects, so only these targets enter it.
    if (standard_slot(target) || is_std_primitives_method(target, "traits", "callable", "__call__"))
        && lower_standard_slot(module, builder, target, arguments, destination, span)?
    {
        return Ok(MethodRoute::Lowered);
    }
    let mut implementation = target.canonical.clone();
    if let Some(owner) = receiver_owner(module, builder, receiver, span)? {
        let selected = owner_method_identity(module, target, &owner)?;
        if Some(&selected) != target.canonical.as_ref()
            && let Some(index) = compiled_callable_index(module, &selected)
        {
            return Ok(MethodRoute::Library(index, implementation_target(target, selected)));
        }
        implementation = Some(selected);
    }
    if retained_iterator_default(module, builder, target, receiver, span)? {
        return Ok(MethodRoute::Retained);
    }
    if !target.has_valid_native_trait_selection() {
        return Err("the selected standard library trait signature is invalid".into());
    }
    if selected_library_method(module, builder, target, receiver, span)? {
        return Ok(MethodRoute::SelectedLibrary);
    }
    // A list or dictionary element receiver is reborrowed in its container; no native place names it.
    if let Some(owner) = element_method_owner(builder, receiver)? {
        return element_route(module, target, owner);
    }
    if let Some(index) = enum_message_layout(module, builder, target, receiver, span)? {
        return Ok(MethodRoute::EnumMessage(index));
    }
    if let Some(owner) = value_method_owner(module, builder, target, receiver, span)? {
        return Ok(MethodRoute::ValueGetter(owner));
    }
    if !receiver_fills_source_slot(module, builder, target, receiver, span)?
        && let Some(identity) = implementation
        && let Some(index) = compiled_callable_index(module, &identity)
    {
        return Ok(MethodRoute::Library(index, implementation_target(target, identity)));
    }
    Ok(MethodRoute::Retained)
}

/// Whether a checked SDK instantiation lowers the call while source implementations keep priority, including over
/// collection elements whose model is declared here (`lower_selected_library_method`).
fn selected_library_method(
    module: &BodyIrModule,
    builder: &Builder,
    target: &MethodTarget,
    receiver: &ArgumentElement,
    span: &SourceSpan,
) -> Result<bool, String> {
    if target.native_trait_delegation.is_none() || receiver_fills_source_slot(module, builder, target, receiver, span)?
    {
        return Ok(false);
    }
    if let Some(PlanType::Model(owner, _)) = element_method_owner(builder, receiver)?
        && library_item(&module.nominal_declarations[owner as usize]).is_empty()
    {
        return Ok(false);
    }
    Ok(true)
}

/// The route of a method on a list or dictionary element: a retained SDK model element calls its authenticated SDK
/// method (`lower_sdk_element_method`); any other nominal element borrows itself in its container.
fn element_route(module: &BodyIrModule, target: &MethodTarget, owner: PlanType) -> Result<MethodRoute, String> {
    let sdk_model = match &owner {
        PlanType::Model(index, _) => usize::try_from(*index)
            .ok()
            .and_then(|slot| module.nominal_declarations.get(slot))
            .is_some_and(|declaration| !library_item(declaration).is_empty()),
        _ => false,
    };
    if sdk_model {
        let selected = owner_method_identity(module, target, &owner)?;
        if let Some(index) = compiled_callable_index(module, &selected) {
            return Ok(MethodRoute::Library(index, implementation_target(target, selected)));
        }
    }
    Ok(MethodRoute::Element(owner))
}

/// Call a method on a list or dictionary element: a value enum's generated `value()` reads the element once; any other
/// method borrows the element as the selected body's receiver.
fn lower_element_method(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &MethodTarget,
    owner: PlanType,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if lower_element_value_getter(module, builder, target, &owner, arguments, destination, span)? {
        return Ok(());
    }
    let index = owner_method_index(module, target, &owner, builder)?;
    let reference = element_method_receiver(module, builder, index, &arguments[0], span)?;
    call_method_body(module, builder, target, index, reference, arguments, destination, span)
}

/// Answer a value enum's generated `value()` on a list or dictionary element's enum, `items[index].phase.value()`, from
/// one read of that element: the getter has no body to call (#2995). Declared methods keep the element route.
fn lower_element_value_getter(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &MethodTarget,
    owner: &PlanType,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<bool, String> {
    let PlanType::Enum(layout, _) = owner else {
        return Ok(false);
    };
    let Some(identity) = &target.canonical else {
        return Ok(false);
    };
    if identity.declaration_name != "value" || identity.kind != SemanticSourceTargetKind::Method {
        return Ok(false);
    }
    let Some(getter) = value_getter_owner(module, target, *layout)? else {
        return Ok(false);
    };
    let Some(SourceOperand::Place(read)) = arguments[0].as_one() else {
        return Ok(false);
    };
    let source = materialized_receiver(builder, &read.place, span)?;
    lower_value_enum_getter_from(builder, getter, source, destination, span)?;
    Ok(true)
}

/// Whether the checker selected a standard derive or comparison slot, which a derive or JSON protocol implements.
fn standard_slot(target: &MethodTarget) -> bool {
    let json = is_json_serialize_method(target);
    json || is_std_primitives_method(target, "derives", "copying", "clone")
        || is_std_primitives_method(target, "derives", "string", "__str__")
        || ["__eq__", "__ne__", "__lt__", "__le__", "__gt__", "__ge__"]
            .iter()
            .any(|name| is_std_primitives_method(target, "derives", "comparison", name))
}

/// Whether the checker selected `name` of `std.<section>.<module>`, in source or compiled SDK identities.
pub fn is_std_primitives_method(target: &MethodTarget, section: &str, module: &str, name: &str) -> bool {
    let Some(identity) = &target.canonical else {
        return false;
    };
    if identity.kind != SemanticSourceTargetKind::Method || identity.declaration_name != name {
        return false;
    }
    match &identity.origin {
        SymbolOrigin::Module(path) => path.len() == 3 && path[0] == "std" && path[1] == section && path[2] == module,
        SymbolOrigin::Package { library, module_path } => {
            library == "incan_stdlib_primitives"
                && module_path.len() == 2
                && module_path[0] == section
                && module_path[1] == module
        }
        _ => false,
    }
}

/// The compiled Loaf callable that carries this exact canonical implementation, if any.
fn compiled_callable_index(module: &BodyIrModule, identity: &CanonicalSymbolId) -> Option<usize> {
    module
        .loaf_callables
        .iter()
        .position(|callable| callable.canonical == *identity)
}

/// Name the retained concrete implementation rather than its trait slot when calling an already compiled method.
fn implementation_target(target: &MethodTarget, canonical: CanonicalSymbolId) -> MethodTarget {
    let mut selected = target.clone();
    if selected.canonical.as_ref() != Some(&canonical) {
        // The selected item is the implementation's ordinary compiled entry, not the trait's native generic item.
        selected.loaf_trait = None;
        selected.native_trait_delegation = None;
    }
    selected.canonical = Some(canonical);
    selected
}

/// How a method call lowers once its special routes are excluded.
enum MethodRoute {
    Lowered,
    Library(usize, MethodTarget),
    SelectedLibrary,
    Reflection,
    EnumMessage(usize),
    ValueGetter(usize),
    Element(PlanType),
    Retained,
}

/// Read the receiver's retained model or source enum from the checked type of the place it reads.
///
/// `None` is only a receiver that names no retained nominal: a trait object, a primitive or a literal. A place that
/// cannot be read is an error, never a fallback to the trait's checked slot.
fn receiver_owner(
    module: &BodyIrModule,
    builder: &Builder,
    argument: &ArgumentElement,
    span: &SourceSpan,
) -> Result<Option<PlanType>, String> {
    match argument.as_one() {
        None => Err("unsupported Body IR Method spread receiver".into()),
        Some(SourceOperand::Place(read)) => {
            if let Some(global) = read.place.global() {
                return Ok(nominal_owner(
                    module,
                    &computed_static_type(builder, &read.place, global)?,
                ));
            }
            if is_element_place(builder, &read.place)? {
                return Ok(nominal_owner(module, &element_place_type(builder, &read.place)?));
            }
            let mut path = model_place(
                builder.tables(),
                &builder.locals,
                &read.place,
                span,
                builder.local_offset,
            )?;
            path.local += builder.local_offset;
            Ok(nominal_owner(module, &place_type(&builder.locals, &path)))
        }
        Some(_) => Ok(None),
    }
}

/// A retained model or source enum owns trait slots, including a receiver reached through stored reference.
fn nominal_owner(module: &BodyIrModule, actual: &PlanType) -> Option<PlanType> {
    match actual {
        PlanType::Model(owner, name) | PlanType::ModelRef(owner, name) | PlanType::ModelMutRef(owner, name) => {
            Some(PlanType::Model(*owner, name.clone()))
        }
        PlanType::Enum(owner, name) | PlanType::EnumRef(owner, name)
            if (*owner as usize) < module.enum_declarations.len() =>
        {
            Some(PlanType::Enum(*owner, name.clone()))
        }
        _ => None,
    }
}

/// The retained declaration identity a model or source enum receiver owner fills its trait slots under.
fn owner_identity(module: &BodyIrModule, owner: &PlanType) -> Result<CanonicalSymbolId, String> {
    match owner {
        PlanType::Model(index, _) => {
            if let Some(declaration) = module.nominal_declarations.get(*index as usize) {
                return Ok(declaration.canonical.clone());
            }
        }
        PlanType::Enum(index, _) => {
            if let Some(declaration) = module.enum_declarations.get(*index as usize) {
                return Ok(declaration.canonical.clone());
            }
        }
        _ => {}
    }
    Err("unsupported Body IR trait-object receiver".into())
}

/// Keep a checked source adopter on its own slot even when its required published default is unavailable.
fn receiver_fills_source_slot(
    module: &BodyIrModule,
    builder: &Builder,
    target: &MethodTarget,
    argument: &ArgumentElement,
    span: &SourceSpan,
) -> Result<bool, String> {
    let Some(owner) = receiver_owner(module, builder, argument, span)? else {
        return Ok(false);
    };
    let source = match &owner {
        PlanType::Model(index, _) => module
            .nominal_declarations
            .get(*index as usize)
            .is_some_and(|declaration| library_item(declaration).is_empty()),
        PlanType::Enum(index, _) => module
            .enum_declarations
            .get(*index as usize)
            .is_some_and(|declaration| library_enum_item(declaration).is_empty()),
        _ => false,
    };
    if !source {
        return Ok(false);
    }
    let identity = owner_identity(module, &owner)?;
    Ok(module.trait_implementations.iter().any(|implementation| {
        implementation.owner == identity && Some(&implementation.method) == target.canonical.as_ref()
    }))
}

/// Select one concrete owner's retained implementation for instance and associated trait calls.
fn owner_method_identity(
    module: &BodyIrModule,
    target: &MethodTarget,
    owner: &PlanType,
) -> Result<CanonicalSymbolId, String> {
    let Some(checked) = &target.canonical else {
        return Err("unsupported Body IR unproven Method".into());
    };
    let identity = owner_identity(module, owner)?;
    let mut canonical = checked.clone();
    for declaration in &module.nominal_declarations {
        if declaration.canonical != identity {
            continue;
        }
        if let Some(inheritance) = &declaration.class_inheritance
            && let Some(implementation) = inheritance.methods.iter().find(|entry| entry.method == canonical)
        {
            canonical = implementation.implementation.clone();
        }
    }
    let mut selected: Option<&incan_semantics_core::body_ir::TraitImplementation> = None;
    for implementation in &module.trait_implementations {
        if implementation.owner != identity || implementation.method != canonical {
            continue;
        }
        let matches = match &target.trait_arguments {
            Some(arguments) => *arguments == implementation.slot_arguments,
            None => receiver_selects_slot_arguments(module, implementation, owner),
        };
        if !matches {
            continue;
        }
        match selected {
            Some(previous) => {
                if previous.implementation != implementation.implementation
                    || previous.type_arguments != implementation.type_arguments
                {
                    return Err("unsupported Body IR trait method at several instantiations for one receiver".into());
                }
            }
            None => selected = Some(implementation),
        }
    }
    match selected {
        Some(implementation) => Ok(implementation.implementation.clone()),
        None => {
            if !method_belongs_to_owner(&canonical, &identity)
                && let Some(declared) = declared_owner_method(module, &identity, &target.name)?
            {
                return Ok(declared);
            }
            Ok(canonical)
        }
    }
}

/// Select one retained method body declared directly by the concrete receiver when no trait record names it.
fn declared_owner_method(
    module: &BodyIrModule,
    owner: &CanonicalSymbolId,
    name: &str,
) -> Result<Option<CanonicalSymbolId>, String> {
    let mut selected: Option<&CanonicalSymbolId> = None;
    for body in &module.bodies {
        let Some(identity) = &body.canonical else {
            continue;
        };
        if identity.origin != owner.origin
            || identity.kind != SemanticSourceTargetKind::Method
            || identity.declaration_name != name
            || identity.declaration_span.start < owner.declaration_span.start
            || identity.declaration_span.end > owner.declaration_span.end
        {
            continue;
        }
        match selected {
            Some(previous) => {
                if previous != identity {
                    return Err("unsupported Body IR several declared methods for one receiver slot".into());
                }
            }
            None => selected = Some(identity),
        }
    }
    Ok(selected.cloned())
}

/// Preserve an exact checked overload when its canonical declaration is already inside the receiver owner.
fn method_belongs_to_owner(method: &CanonicalSymbolId, owner: &CanonicalSymbolId) -> bool {
    method.origin == owner.origin
        && method.kind == SemanticSourceTargetKind::Method
        && method.declaration_span.start >= owner.declaration_span.start
        && method.declaration_span.end <= owner.declaration_span.end
}

/// Borrow the concrete receiver and execute its selected body, naming the frame when its receiver is refused.
fn lower_retained_method_call(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &MethodTarget,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let index = method_index(module, target, builder, &arguments[0], span)
        .map_err(|error| format!("{error} for {} in {}", target.name, builder.name))?;
    let (receiver_type, borrow) = method_receiver(module, builder, index, &arguments[0], span)?;
    let reference = temporary(builder, receiver_type, span);
    builder.emit(plan_assign(reference.clone(), borrow, span));
    call_method_body(module, builder, target, index, reference, arguments, destination, span)
}

/// Select the checked implementation using the concrete receiver's retained layout identity.
fn method_index(
    module: &BodyIrModule,
    target: &MethodTarget,
    builder: &Builder,
    argument: &ArgumentElement,
    span: &SourceSpan,
) -> Result<usize, String> {
    match receiver_owner(module, builder, argument, span)? {
        Some(owner) => owner_method_index(module, target, &owner, builder),
        None => Err(format!(
            "unsupported Body IR trait-object receiver at {}:{}",
            span.line, span.column
        )),
    }
}

/// Find the selected body's exact receiver frame; a compiled Loaf implementation uses its callable record instead.
pub fn owner_method_index(
    module: &BodyIrModule,
    target: &MethodTarget,
    owner: &PlanType,
    builder: &Builder,
) -> Result<usize, String> {
    let identity = owner_identity(module, owner)?;
    let selected_identity = owner_method_identity(module, target, owner)?;
    let candidates = builder.program.body_indices.candidates(&selected_identity);
    let mut selected_index: Option<usize> = None;
    for index in &candidates {
        let body = &module.bodies[*index];
        if body.canonical.as_ref() != Some(&selected_identity) || body.params.is_empty() {
            continue;
        }
        let receiver = frame_type(
            builder.constructors(),
            builder.nominals(),
            builder.enums(),
            &body.locals[0],
            &builder.intrinsics,
            "",
        )?;
        if !receiver_matches(&receiver, owner) {
            continue;
        }
        if receiver_mode(&body.locals[0].origin).is_none() {
            return Err("unsupported Body IR Method without receiver".into());
        }
        match selected_index {
            Some(previous) => {
                // Repeated publication of the same checked body is harmless; differing executable facts are ambiguous.
                if *body != module.bodies[previous] {
                    return Err("unsupported Body IR conflicting selected method executable bodies".into());
                }
            }
            None => selected_index = Some(*index),
        }
    }
    if let Some(index) = selected_index {
        return Ok(index);
    }
    let _ = identity;
    Err(untranslated("traits::owner_method_index refusal text"))
}

/// Match the declared receiver frame against the caller's checked concrete model or enum carrier.
fn receiver_matches(receiver: &PlanType, owner: &PlanType) -> bool {
    match (owner, receiver) {
        (PlanType::Model(owner_index, _), PlanType::ModelRef(index, _) | PlanType::ModelMutRef(index, _)) => {
            index == owner_index
        }
        (PlanType::Enum(owner_index, _), PlanType::EnumRef(index, _)) => index == owner_index,
        _ => false,
    }
}

/// Lower the checked receiver to a shared borrow or mutable reborrow without moving its owner.
fn method_receiver(
    module: &BodyIrModule,
    builder: &Builder,
    index: usize,
    argument: &ArgumentElement,
    span: &SourceSpan,
) -> Result<(PlanType, RvalueKind), String> {
    let Some(value) = argument.as_one() else {
        return Err("unsupported Body IR Method spread receiver".into());
    };
    let body = &module.bodies[index];
    let receiver_type = frame_type(
        builder.constructors(),
        &module.nominal_declarations,
        &module.enum_declarations,
        &body.locals[0],
        &builder.intrinsics,
        "",
    )?;
    let SourceOperand::Place(read) = value else {
        return Err("unsupported Body IR Method nonplace receiver".into());
    };
    if !matches!(read.fact, OwnershipFact::Borrow | OwnershipFact::MutBorrow) {
        return Err("unsupported Body IR Method receiver ownership".into());
    }
    let mut receiver = model_place(
        builder.tables(),
        &builder.locals,
        &read.place,
        span,
        builder.local_offset,
    )?;
    receiver.local += builder.local_offset;
    // A receiver stored as a carried reference field.
    if place_type(&builder.locals, &receiver) == receiver_type {
        match &receiver_type {
            PlanType::ModelRef(owner, name) => {
                let referent = dereferenced_place(&receiver, PlanType::Model(*owner, name.clone()), span)?;
                return Ok((receiver_type, RvalueKind::Borrow(referent)));
            }
            PlanType::ModelMutRef(owner, name) => {
                let referent = dereferenced_place(&receiver, PlanType::Model(*owner, name.clone()), span)?;
                return Ok((receiver_type, RvalueKind::MutBorrow(referent)));
            }
            PlanType::EnumRef(owner, name) => {
                let referent = dereferenced_place(&receiver, PlanType::Enum(*owner, name.clone()), span)?;
                return Ok((receiver_type, RvalueKind::Borrow(referent)));
            }
            _ => {}
        }
    }
    match &receiver_type {
        PlanType::ModelRef(..) | PlanType::EnumRef(..) => Ok((receiver_type, RvalueKind::Borrow(receiver))),
        PlanType::ModelMutRef(..) => Ok((receiver_type, RvalueKind::MutBorrow(receiver))),
        _ => Err("invalid Body IR Method receiver type".into()),
    }
}

/// Invoke the selected method with its checked receiver and arguments, retaining async captures in a future.
#[allow(clippy::too_many_arguments)]
fn call_method_body(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &MethodTarget,
    index: usize,
    reference: Place,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let typed = instantiated_method_body(module, builder, target, index, arguments, span)?;
    let body = &typed;
    retain_sdk_factory_result(module, builder, target, body, destination, span)?;
    if source_equality_target(module, target, body) {
        return Err(untranslated("comparison_methods::source_equality_value"));
    }
    let ordinary = method_ordinary_operands(module, builder, target, arguments, body, span)?;
    let mut operands = vec![read_place(reference, true, span)];
    operands.extend(ordinary);
    if body.is_async {
        let Some(place) = destination else {
            return Err("unsupported Body IR async method call without carrier".into());
        };
        let output = scalar_place(builder, place, span)?;
        return construct_future(module, builder, body, operands, output, span);
    }
    let callee = CalleeKind::Planned(function_name(module, body));
    emit_selected_body_call(module, builder, body, callee, operands, destination, span)
}

/// Keep a selected SDK default's concrete result where its checked interface still uses a tuple iterator frame.
fn retain_sdk_factory_result(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &MethodTarget,
    body: &Body,
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if target.loaf_trait.is_none() || body.canonical != target.canonical || !target.has_valid_native_trait_selection() {
        return Ok(());
    }
    let Some(delegation) = &target.native_trait_delegation else {
        return Ok(());
    };
    if target.canonical.as_ref() != Some(&delegation.canonical) {
        return Ok(());
    }
    let Some(place) = destination else {
        return Ok(());
    };
    let output = scalar_place(builder, place, span)?;
    if output.projection != Projection::Whole {
        return Ok(());
    }
    if !matches!(place_type(&builder.locals, &output), PlanType::ZipIterator(..)) {
        return Ok(());
    }
    let result = native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        &body.return_type,
        &builder.intrinsics,
        "",
    )?;
    let PlanType::Model(owner, _) = &result else {
        return Ok(());
    };
    if library_item(&module.nominal_declarations[*owner as usize]).is_empty() {
        return Ok(());
    }
    let declared = checked_type_arguments(builder, std::slice::from_ref(&delegation.return_type))?;
    let Some(declared) = declared.first() else {
        return Err("the standard library factory result differs from its destination type".into());
    };
    let expected = native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        declared,
        &builder.intrinsics,
        "",
    )?;
    if expected != place_type(&builder.locals, &output) {
        return Err("the standard library factory result differs from its destination type".into());
    }
    builder.locals[output.local as usize].ty = result;
    Ok(())
}

/// Whether an explicit equality source member keeps its owned input, which a borrowing wrapper adapts.
fn source_equality_target(module: &BodyIrModule, target: &MethodTarget, body: &Body) -> bool {
    if body.canonical.is_none() {
        return false;
    }
    if target.canonical == body.canonical
        && module
            .nominal_declarations
            .iter()
            .any(|owner| owner.comparison_equality == body.canonical)
    {
        return true;
    }
    module.trait_implementations.iter().any(|implementation| {
        Some(&implementation.implementation) == body.canonical.as_ref()
            && implementation.method.declaration_name == "__eq__"
            && module
                .enum_declarations
                .iter()
                .any(|declaration| implementation.owner == declaration.canonical)
    })
}

/// Bind the ordinary slots once, excluding the method's separately prepared receiver.
pub fn method_ordinary_operands(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &MethodTarget,
    arguments: &[ArgumentElement],
    body: &Body,
    span: &SourceSpan,
) -> Result<Vec<Operand>, String> {
    // A selected body with only its receiver proves there are no ordinary slots to bind. Keep every resolved binding
    // on the normal validation path, so malformed slot/default records still refuse.
    if body.params.len() == 1 && arguments.len() == 1 && matches!(target.binding, ArgumentBinding::UnresolvedPositional)
    {
        return Ok(Vec::new());
    }
    let mut types = Vec::with_capacity(body.params.len().saturating_sub(1));
    for parameter in body.params.iter().skip(1) {
        types.push(call_parameter_type(builder, body, parameter)?);
    }
    if let Some(values) = method_partial_arguments(module, builder, target, arguments, body, &types, span)? {
        return Ok(values);
    }
    if matches!(target.binding, ArgumentBinding::Resolved { .. }) && has_defaulted_slots(&target.binding) {
        let call = DefaultCall {
            body,
            parameter_offset: 1,
            owned: false,
        };
        return default_arguments(module, builder, &target.binding, &arguments[1..], &types, &call, span);
    }
    let policy = SlotPolicy {
        offset: 1,
        accesses: parameter_accesses(&body.params, 1)?,
        ..SlotPolicy::default()
    };
    ordered_slot_operands(builder, &target.binding, arguments, &types, span, &policy)
}

/// A builtin registry member: a model-collecting iterator calls its retained default, a Result combinator lowers its
/// callable, and every other member takes its builtin lowering.
fn lower_builtin_origin_method(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &MethodTarget,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if target.builtin_iterator() == Some(IteratorMethodId::Collect)
        && let PlanType::Model(..) = argument_type(builder, arguments, 0, span)?
    {
        let selected = concrete_iterator_collect(module, builder, target, arguments, span)?;
        return lower_retained_method_call(module, builder, &selected, arguments, destination, span);
    }
    if let Some(combinator) = target.builtin_result().and_then(result_combinator) {
        return lower_result_combinator(module, builder, combinator, arguments, destination, span);
    }
    lower_builtin_method(builder, target, arguments, destination, span)
}

/// Recognize the JSON protocol default in source and compiled SDK identities, never a same-name override.
pub fn is_json_serialize_method(target: &MethodTarget) -> bool {
    let Some(identity) = &target.canonical else {
        return false;
    };
    if identity.kind != SemanticSourceTargetKind::Method || identity.declaration_name != "to_json" {
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

/// Whether a retained implementation record fills the selected slot for the receiver's concrete nominal.
///
/// Such a slot dispatches to that body, ahead of any behavior a standard derive gives the slot, because the adopter's
/// own method is what legacy calls. A receiver that is not a retained nominal has no record.
pub fn receiver_fills_slot(
    module: &BodyIrModule,
    target: &MethodTarget,
    builder: &Builder,
    argument: &ArgumentElement,
    span: &SourceSpan,
) -> Result<bool, String> {
    let Some(owner) = receiver_owner(module, builder, argument, span)? else {
        return Ok(false);
    };
    let identity = owner_identity(module, &owner)?;
    Ok(module.trait_implementations.iter().any(|implementation| {
        implementation.owner == identity && Some(&implementation.method) == target.canonical.as_ref()
    }))
}

/// Use the same checked adopter selection for a compiled method and a retained executable body.
pub fn receiver_method_identity(
    module: &BodyIrModule,
    target: &MethodTarget,
    builder: &Builder,
    argument: &ArgumentElement,
    span: &SourceSpan,
) -> Result<Option<CanonicalSymbolId>, String> {
    match receiver_owner(module, builder, argument, span)? {
        Some(owner) => Ok(Some(owner_method_identity(module, target, &owner)?)),
        None => Ok(target.canonical.clone()),
    }
}

/// Close only the selected method's retained parameters and queue its exact executable instance.
pub fn instantiated_method_body(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &MethodTarget,
    index: usize,
    arguments: &[ArgumentElement],
    span: &SourceSpan,
) -> Result<Body, String> {
    let body = &module.bodies[index];
    if !body.type_parameters.is_empty() {
        let closed = sdk_forwarded_type_arguments(module, builder, target, body, arguments, span)?;
        let mut typed = instantiate(body, &closed, &[])?;
        let owned = owned_callable_slots(builder, body, &typed, arguments, &target.binding, 1)?;
        request_instance(&mut builder.instances, body, closed.clone(), owned.clone(), false);
        if !owned.is_empty() {
            typed = instantiate(body, &closed, &owned)?;
        }
        return Ok(typed);
    }
    if !target.type_args.is_empty() {
        return Err("unsupported Body IR type arguments on a method without type parameters".into());
    }
    if is_lazy_method_body(module, body) {
        request_instance(&mut builder.instances, body, Vec::new(), Vec::new(), false);
    }
    Ok(body.clone())
}

/// Recover a scalar generic forwarding argument omitted by frozen SDK Self-call facts (#1698).
///
/// Both caller and callee must be authenticated defaults of the standard Iterator protocol. The callee's first
/// ordinary parameter and result name its sole binder. Its actual bound argument must have exactly the primitive
/// carrier of the caller's one already checked generic argument. Binder spellings from the two declarations are never
/// compared, and every remaining argument still passes normal instantiated signature validation.
fn sdk_forwarded_type_arguments(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &MethodTarget,
    body: &Body,
    arguments: &[ArgumentElement],
    span: &SourceSpan,
) -> Result<Vec<IncanType>, String> {
    let closed = checked_type_arguments(builder, &target.type_args)?;
    if !closed.is_empty() || body.type_parameters.len() != 1 || builder.callable_type_arguments.len() != 1 {
        return Ok(closed);
    }
    if body.params.len() < 2 || arguments.len() != body.params.len() || body.canonical != target.canonical {
        return Ok(closed);
    }
    if !iterator_default_pair(module, target.canonical.as_ref(), builder.callable_identity.as_ref()) {
        return Ok(closed);
    }
    match &body.params[1].ty {
        IncanType::TypeVar(name) if *name == body.type_parameters[0] && body.return_type == body.params[1].ty => {}
        _ => return Ok(closed),
    }
    let candidate = builder.callable_type_arguments[0].clone();
    if !matches!(candidate, IncanType::Primitive(_)) {
        return Ok(closed);
    }
    let ArgumentBinding::Resolved {
        arguments: bindings,
        defaulted_slots,
        ..
    } = &target.binding
    else {
        return Ok(closed);
    };
    if !defaulted_slots.is_empty() || bindings.len() + 1 != arguments.len() {
        return Ok(closed);
    }
    let mut position = None;
    for (index, binding) in bindings.iter().enumerate() {
        if binding.slot == 0 {
            if position.is_some() {
                return Ok(closed);
            }
            position = Some(index + 1);
        }
    }
    let Some(position) = position else {
        return Ok(closed);
    };
    let actual = argument_type(builder, arguments, position, span)?;
    let expected = native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        &candidate,
        &builder.intrinsics,
        "",
    )?;
    if actual != expected {
        return Ok(closed);
    }
    Ok(vec![candidate])
}

/// Whether the selected callee and the calling frame are both member methods declared inside the standard Iterator
/// protocol.
fn iterator_default_pair(
    module: &BodyIrModule,
    selected: Option<&CanonicalSymbolId>,
    caller: Option<&CanonicalSymbolId>,
) -> bool {
    let (Some(selected), Some(caller)) = (selected, caller) else {
        return false;
    };
    if selected.kind != SemanticSourceTargetKind::Method
        || caller.kind != SemanticSourceTargetKind::Method
        || selected.namespace != SymbolNamespace::Member
        || caller.namespace != SymbolNamespace::Member
    {
        return false;
    }
    module.trait_declarations.iter().any(|protocol| {
        standard_iterator_protocol(protocol)
            && selected.origin == protocol.origin
            && caller.origin == protocol.origin
            && protocol.declaration_span.start <= selected.declaration_span.start
            && selected.declaration_span.end <= protocol.declaration_span.end
            && protocol.declaration_span.start <= caller.declaration_span.start
            && caller.declaration_span.end <= protocol.declaration_span.end
    })
}

/// Preserve an explicit equality source member's owned input and drop timing while its native trait slot borrows; its
/// owned-equality wrapper is `comparison_methods::source_equality_value`.
pub fn source_method_symbol(module: &BodyIrModule, target: &MethodTarget, body: &Body) -> Result<String, String> {
    if source_equality_target(module, target, body) {
        return Err(untranslated("comparison_methods::source_equality_value"));
    }
    Ok(function_name(module, body))
}

/// Prefer retained Incan Iterator identity and collection defaults absent from the native SDK trait.
fn retained_iterator_default(
    module: &BodyIrModule,
    builder: &Builder,
    target: &MethodTarget,
    receiver: &ArgumentElement,
    span: &SourceSpan,
) -> Result<bool, String> {
    let Some(identity) = &target.canonical else {
        return Ok(false);
    };
    if identity.kind != SemanticSourceTargetKind::Method || identity.namespace != SymbolNamespace::Member {
        return Ok(false);
    }
    if !matches!(
        target.builtin_iterator(),
        Some(IteratorMethodId::Iter | IteratorMethodId::Collect)
    ) {
        return Ok(false);
    }
    let standard = module.trait_declarations.iter().any(|protocol| {
        standard_iterator_protocol(protocol)
            && protocol.origin == identity.origin
            && protocol.declaration_span.start <= identity.declaration_span.start
            && identity.declaration_span.end <= protocol.declaration_span.end
    });
    if !standard || !target.has_valid_native_trait_selection() {
        return Ok(false);
    }
    receiver_fills_retained_slot(module, target, builder, receiver, span)
}

/// Resolve a builtin opaque-Iterator terminal only through its concrete SDK owner's unique retained default (#1698).
fn concrete_iterator_collect(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &MethodTarget,
    arguments: &[ArgumentElement],
    span: &SourceSpan,
) -> Result<MethodTarget, String> {
    if arguments.len() != 1
        || !target.type_args.is_empty()
        || target.loaf_trait.is_some()
        || target.native_trait_delegation.is_some()
    {
        return Err("unsupported Body IR opaque Iterator.collect call contract".into());
    }
    let PlanType::Model(owner, _) = argument_type(builder, arguments, 0, span)? else {
        return Err("unsupported Body IR opaque Iterator.collect without concrete receiver".into());
    };
    let declaration = &module.nominal_declarations[owner as usize];
    if library_item(declaration).is_empty() {
        return Err("opaque Iterator.collect requires a standard library owner".into());
    }
    let mut selected = target.clone();
    let mut matches = 0;
    let selected_method = iterator_method_name(IteratorMethodId::Collect);
    for protocol in module
        .trait_declarations
        .iter()
        .filter(|protocol| standard_iterator_protocol(protocol))
    {
        for implementation in &module.trait_implementations {
            let method = &implementation.method;
            if implementation.owner != declaration.canonical
                || method.origin != protocol.origin
                || method.namespace != SymbolNamespace::Member
                || method.kind != SemanticSourceTargetKind::Method
                || method.declaration_name != selected_method
            {
                continue;
            }
            if !(protocol.declaration_span.start <= method.declaration_span.start
                && method.declaration_span.end <= protocol.declaration_span.end)
            {
                continue;
            }
            if implementation.implementation != *method {
                return Err("unsupported Body IR opaque Iterator.collect with a nondefault implementation".into());
            }
            matches += 1;
            selected.canonical = Some(method.clone());
        }
    }
    if matches != 1 || !receiver_fills_retained_slot(module, &selected, builder, &arguments[0], span)? {
        return Err("unsupported Body IR opaque Iterator.collect without one retained standard default".into());
    }
    let index = method_index(module, &selected, builder, &arguments[0], span)?;
    if module.bodies[index].params.len() != 1 {
        return Err("unsupported Body IR opaque Iterator.collect with ordinary parameters".into());
    }
    Ok(selected)
}

/// Prefer an adopter's checked implementation only when its executable body is retained in this program.
fn receiver_fills_retained_slot(
    module: &BodyIrModule,
    target: &MethodTarget,
    builder: &Builder,
    argument: &ArgumentElement,
    span: &SourceSpan,
) -> Result<bool, String> {
    let Some(owner) = receiver_owner(module, builder, argument, span)? else {
        return Ok(false);
    };
    let identity = owner_identity(module, &owner)?;
    Ok(module.trait_implementations.iter().any(|implementation| {
        implementation.owner == identity
            && Some(&implementation.method) == target.canonical.as_ref()
            && module
                .bodies
                .iter()
                .any(|body| body.canonical.as_ref() == Some(&implementation.implementation))
    }))
}

/// Call a computed model static's selected method under its actual storage lock, preserving mutation and unwinding
/// rather than copying a receiver out of storage.
fn lower_global_method(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &MethodTarget,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<bool, String> {
    let Some(SourceOperand::Place(read)) = arguments[0].as_one() else {
        return Ok(false);
    };
    let Some(global) = read.place.global() else {
        return Ok(false);
    };
    let call = StaticCall {
        target,
        read,
        global,
        arguments,
        destination,
    };
    if !receiver_fills_retained_slot(module, target, builder, &arguments[0], span)?
        && let Some(callable) = module
            .loaf_callables
            .iter()
            .find(|callable| Some(&callable.canonical) == target.canonical.as_ref())
    {
        lower_static_sdk_method(module, builder, callable, &call, span)?;
        return Ok(true);
    }
    lower_static_method_body(module, builder, &call, span)?;
    Ok(true)
}

/// Fill a method partial's omitted slots from the frame its owner retains for that name and target, receiver excluded.
///
/// A method partial's call carries its target's identity under the partial's own name; its presets are defaults of the
/// owner's retained frame for that name and target, whose parameters must be the target body's own. One generic owner
/// contributes a frame per closed layout; the retained receiver selects this instance. `None` means no such frame.
fn method_partial_arguments(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &MethodTarget,
    arguments: &[ArgumentElement],
    body: &Body,
    types: &[PlanType],
    span: &SourceSpan,
) -> Result<Option<Vec<Operand>>, String> {
    for owner in &module.nominal_declarations {
        for preset in &owner.method_partial_defaults {
            if preset.name != target.name || Some(&preset.target) != target.canonical.as_ref() {
                continue;
            }
            let frame: &Body = &preset.frame;
            if frame.canonical != body.canonical
                || frame.params.len() != body.params.len()
                || frame.params.len() != types.len() + 1
            {
                return Err("invalid Body IR method-partial frame".into());
            }
            if frame.params[0].ty != body.params[0].ty {
                continue;
            }
            for (declared, actual) in frame.params.iter().zip(&body.params) {
                if declared.name != actual.name
                    || declared.ty != actual.ty
                    || declared.input_access != actual.input_access
                {
                    return Err("invalid Body IR method-partial parameter".into());
                }
            }
            let call = DefaultCall {
                body: frame,
                parameter_offset: 1,
                owned: false,
            };
            let values = default_arguments(module, builder, &target.binding, &arguments[1..], types, &call, span)?;
            return Ok(Some(values));
        }
    }
    Ok(None)
}
