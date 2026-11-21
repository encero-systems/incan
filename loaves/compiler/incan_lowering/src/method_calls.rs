//! Method calls: checked receivers, selected implementations and their ordinary arguments.

use incan_lang::lang::surface::iterator_methods::IteratorMethodId;
use incan_lang::lang::surface::result_methods::ResultMethodId;
use incan_mir_plan_native::{CalleeKind, Operand, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign};
use incan_semantics_core::body_ir::{
    ArgumentBinding, ArgumentElement, Body, BodyIrModule, MethodTarget, Operand as SourceOperand, OwnershipFact,
    Place as SourcePlace, ReflectionOperation,
};
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};

use crate::builtin_methods::lower_builtin_method;
use crate::call_arguments::{SlotPolicy, call_parameter_type, has_defaulted_slots, ordered_slot_operands};
use crate::callable_calls::{checked_type_arguments, emit_selected_body_call};
use crate::classes::{frame_type, function_name, receiver_mode};
use crate::decls::{library_enum_item, library_item};
use crate::defaults::{DefaultCall, default_arguments};
use crate::element_places::{element_place_type, is_element_place};
use crate::explicit_arguments::parameter_accesses;
use crate::frame::{Builder, dereferenced_place, place_type, read_place, temporary};
use crate::library_methods::lower_library_method;
use crate::models::model_place;
use crate::pending::untranslated;
use crate::sequence_builtins::argument_type;
use crate::traits::{is_lazy_method_body, receiver_selects_slot_arguments};
use crate::type_parameters::request_instance;

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
        return Err(untranslated("result_carriers::lower_carrier_method"));
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
    let Some(receiver) = arguments.first() else {
        return Err("invalid Body IR Method missing receiver".into());
    };
    match special_method_route(module, builder, target, receiver, span)? {
        MethodRoute::Refused(step) => return Err(untranslated(step)),
        MethodRoute::Library(index, selected) => {
            let callable = &module.loaf_callables[index];
            return lower_library_method(module, builder, callable, &selected, arguments, destination, span);
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
    receiver: &ArgumentElement,
    span: &SourceSpan,
) -> Result<MethodRoute, String> {
    use MethodRoute::Refused;
    if let Some(SourceOperand::Place(read)) = receiver.as_one()
        && read.place.global().is_some()
    {
        return Ok(Refused("method_calls::lower_global_method"));
    }
    match &target.operation {
        None | Some(ReflectionOperation::Builtin(_)) => {}
        Some(_) => return Ok(Refused("reflection::lower_reflection")),
    }
    if standard_slot(target) || is_std_primitives_method(target, "traits", "callable", "__call__") {
        return Ok(Refused("library_methods::lower_standard_slot"));
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
    if matches!(
        target.builtin_iterator(),
        Some(IteratorMethodId::Iter | IteratorMethodId::Collect)
    ) {
        return Ok(Refused("method_calls::retained_iterator_default"));
    }
    if !target.has_valid_native_trait_selection() {
        return Err("the selected standard library trait signature is invalid".into());
    }
    if target.native_trait_delegation.is_some() {
        return Ok(Refused("method_calls::lower_selected_library_method"));
    }
    if let Some(SourceOperand::Place(read)) = receiver.as_one()
        && is_element_place(builder, &read.place)?
    {
        return Ok(Refused("element_borrows::element_method_owner"));
    }
    if let Some(enum_index) = receiver_enum_layout(builder, receiver, span)? {
        if generated_enum_member(module, target, enum_index, "message") {
            return Ok(Refused("value_enums::lower_enum_message"));
        }
        if target.loaf_trait.is_none() && value_enum_getter(module, target, enum_index) {
            return Ok(Refused("value_enum_methods::lower_value_enum_method"));
        }
    }
    if !receiver_fills_source_slot(module, builder, target, receiver, span)?
        && let Some(identity) = implementation
        && let Some(index) = compiled_callable_index(module, &identity)
    {
        return Ok(MethodRoute::Library(index, implementation_target(target, identity)));
    }
    Ok(MethodRoute::Retained)
}

/// Whether the checker selected a standard derive or comparison slot, which a derive or JSON protocol implements.
fn standard_slot(target: &MethodTarget) -> bool {
    let json = target.canonical.as_ref().is_some_and(|identity| {
        identity.kind == SemanticSourceTargetKind::Method
            && identity.declaration_name == "to_json"
            && match &identity.origin {
                SymbolOrigin::Module(path) => {
                    path.len() == 3 && path[0] == "std" && path[1] == "serde" && path[2] == "json"
                }
                SymbolOrigin::Package { library, module_path } => {
                    library == "incan_stdlib_data"
                        && module_path.len() == 2
                        && module_path[0] == "serde"
                        && module_path[1] == "json"
                }
                _ => false,
            }
    });
    json || is_std_primitives_method(target, "derives", "copying", "clone")
        || is_std_primitives_method(target, "derives", "string", "__str__")
        || ["__eq__", "__ne__", "__lt__", "__le__", "__gt__", "__ge__"]
            .iter()
            .any(|name| is_std_primitives_method(target, "derives", "comparison", name))
}

/// Whether the checker selected `name` of `std.<section>.<module>`, in source or compiled SDK identities.
fn is_std_primitives_method(target: &MethodTarget, section: &str, module: &str, name: &str) -> bool {
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
    Refused(&'static str),
    Library(usize, MethodTarget),
    Retained,
}

/// The source enum layout a receiver place holds, if any.
fn receiver_enum_layout(
    builder: &Builder,
    receiver: &ArgumentElement,
    span: &SourceSpan,
) -> Result<Option<usize>, String> {
    let Some(SourceOperand::Place(read)) = receiver.as_one() else {
        return Ok(None);
    };
    let mut source = model_place(
        builder.tables(),
        &builder.locals,
        &read.place,
        span,
        builder.local_offset,
    )?;
    source.local += builder.local_offset;
    match place_type(&builder.locals, &source) {
        PlanType::Enum(index, _) if index >= 0 && (index as usize) < builder.enums().len() => Ok(Some(index as usize)),
        _ => Ok(None),
    }
}

/// Whether the target is a checker-generated member of the receiver's own source enum.
fn generated_enum_member(module: &BodyIrModule, target: &MethodTarget, index: usize, name: &str) -> bool {
    let Some(identity) = &target.canonical else {
        return false;
    };
    let Some(owner) = module.enum_declarations.get(index) else {
        return false;
    };
    identity.declaration_name == name
        && identity.namespace == SymbolNamespace::Member
        && identity.kind == SemanticSourceTargetKind::Method
        && identity.origin == owner.canonical.origin
        && identity.scope_discriminant == owner.canonical.scope_discriminant
        && identity.declaration_span == owner.canonical.declaration_span
}

/// Whether the target is a value enum's generated getter on the receiver's layout.
fn value_enum_getter(module: &BodyIrModule, target: &MethodTarget, index: usize) -> bool {
    let Some(layout) = module.enum_declarations.get(index) else {
        return false;
    };
    module.value_enum_declarations.iter().any(|declaration| {
        layout.canonical == declaration.canonical
            && target
                .canonical
                .as_ref()
                .is_none_or(|identity| identity.declaration_span == declaration.canonical.declaration_span)
    })
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
            if read.place.global().is_some() {
                return Err(untranslated("static_storage::computed_static_type"));
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
fn owner_method_index(
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
    let body = &module.bodies[index];
    if !body.type_parameters.is_empty() {
        return Err(untranslated("method_calls::sdk_forwarded_type_arguments"));
    }
    if !target.type_args.is_empty() {
        return Err("unsupported Body IR type arguments on a method without type parameters".into());
    }
    if is_lazy_method_body(module, body) {
        request_instance(&mut builder.instances, body, Vec::new(), Vec::new(), false);
    }
    if target.loaf_trait.is_some()
        && body.canonical == target.canonical
        && target.has_valid_native_trait_selection()
        && target.native_trait_delegation.is_some()
    {
        return Err(untranslated("method_calls::retain_sdk_factory_result"));
    }
    if source_equality_target(module, target, body) {
        return Err(untranslated("comparison_methods::source_equality_value"));
    }
    let ordinary = method_ordinary_operands(module, builder, target, arguments, body, span)?;
    let mut operands = vec![read_place(reference, true, span)];
    operands.extend(ordinary);
    if body.is_async {
        return Err(untranslated("async_::construct_future"));
    }
    let callee = CalleeKind::Planned(function_name(module, body));
    emit_selected_body_call(module, builder, body, callee, operands, destination, span)
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
fn method_ordinary_operands(
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
    let partial = module.nominal_declarations.iter().any(|owner| {
        owner
            .method_partial_defaults
            .iter()
            .any(|preset| preset.name == target.name && Some(&preset.target) == target.canonical.as_ref())
    });
    if partial {
        return Err(untranslated("defaults::method_default_arguments"));
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
    let _ = module;
    if target.builtin_iterator() == Some(IteratorMethodId::Collect)
        && let PlanType::Model(..) = argument_type(builder, arguments, 0, span)?
    {
        return Err(untranslated("method_calls::concrete_iterator_collect"));
    }
    if let Some(method) = target.builtin_result()
        && matches!(
            method,
            ResultMethodId::Map
                | ResultMethodId::MapErr
                | ResultMethodId::AndThen
                | ResultMethodId::OrElse
                | ResultMethodId::Inspect
                | ResultMethodId::InspectErr
        )
    {
        return Err(untranslated("result_combinators::lower_result_combinator"));
    }
    lower_builtin_method(builder, target, arguments, destination, span)
}
