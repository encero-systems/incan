//! Closed native callable instances selected exclusively by checked call-site arguments.

use std::collections::{BTreeMap, VecDeque};

use incan_lang::lang::types::collections::{CollectionTypeId, from_str as collection_type};
use incan_semantics_core::body_ir::{
    Body, BodyIrModule, CallableParam, LocalDecl, LocalId, NOMINAL_CARRIER_KEY_PREFIX, NamedCallableTarget,
    TraitDefaultInstantiation, nominal_carrier_key,
};
use incan_semantics_core::{
    CanonicalSymbolId, HirSourceSpan, IncanType, SemanticSourceTargetKind, SymbolOrigin, function_type, generic,
    native_array_type,
};

use crate::native_type_identity::{ConstructorIndex, OWNED_CALLABLE_STORAGE};
use crate::symbols::type_component;

/// The refusal for a placeholder no binder of the instantiated frame closes; callers that may leave owner
/// placeholders open recognize their skip by this prefix.
pub const UNBOUND_TYPE_PARAMETER: &str = "unsupported Body IR unbound type parameter";

/// Canonical declaration, closed arguments, owned callable slots and execution role form the exact queue key.
type InstanceKey = (Option<CanonicalSymbolId>, Vec<IncanType>, Vec<i64>, bool);

/// One retained declaration and checked arguments; original-callable requests select the retained source role.
#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    pub body: Body,
    pub arguments: Vec<IncanType>,
    pub owned_callable_slots: Vec<i64>,
    /// Ordinary recursive calls still request the visible wrapper, even from a retained original.
    pub original_callable: bool,
}

/// Drain executable requests while retaining only their exact checked equivalence shapes after processing.
#[derive(Default)]
pub struct InstanceQueue {
    pub pending: VecDeque<Instance>,
    pub seen: BTreeMap<InstanceKey, Vec<Vec<LocalDecl>>>,
}

impl InstanceQueue {
    /// Preserve the first request with the same canonical arguments, local declarations, storage and role.
    pub fn add(&mut self, request: Instance) {
        let key = (
            request.body.canonical.clone(),
            request.arguments.clone(),
            request.owned_callable_slots.clone(),
            request.original_callable,
        );
        if let Some(shapes) = self.seen.get(&key)
            && shapes.iter().any(|shape| *shape == request.body.locals)
        {
            return;
        }
        self.seen.entry(key).or_default().push(request.body.locals.clone());
        self.pending.push_back(request);
    }
}

/// An empty executable queue and its structured, body-free seen inventory.
pub fn instance_queue() -> InstanceQueue {
    InstanceQueue::default()
}

/// Move a producer's requests into the shared queue in source order, leaving its body-owning list empty.
pub fn enqueue_instances(queue: &mut InstanceQueue, additions: &mut Vec<Instance>) {
    for request in additions.drain(..) {
        queue.add(request);
    }
}

/// Queue a checked frame: compare its retained identity and storage, and copy its body only for a new request.
pub fn request_instance(
    instances: &mut Vec<Instance>,
    body: &Body,
    arguments: Vec<IncanType>,
    owned_callable_slots: Vec<i64>,
    original_callable: bool,
) {
    if instances
        .iter()
        .any(|retained| instance_matches(retained, body, &arguments, &owned_callable_slots, original_callable))
    {
        return;
    }
    instances.push(Instance {
        body: body.clone(),
        arguments,
        owned_callable_slots,
        original_callable,
    });
}

/// Merge independent request lists in first-occurrence order, retaining each checked frame once.
pub fn merge_instance_requests(instances: &mut Vec<Instance>, additions: &[Instance]) {
    for added in additions {
        request_instance(
            instances,
            &added.body,
            added.arguments.clone(),
            added.owned_callable_slots.clone(),
            added.original_callable,
        );
    }
}

/// Use the completed-frame equivalence before allocation; emitted names never establish identity.
fn instance_matches(
    retained: &Instance,
    body: &Body,
    arguments: &[IncanType],
    owned_callable_slots: &[i64],
    original_callable: bool,
) -> bool {
    retained.body.canonical == body.canonical
        && retained.arguments == arguments
        && retained.body.locals == body.locals
        && retained.owned_callable_slots == owned_callable_slots
        && retained.original_callable == original_callable
}

/// An associated call's checked owner arguments followed by its method arguments.
pub fn static_type_arguments(module: &BodyIrModule, target: &NamedCallableTarget) -> Result<Vec<IncanType>, String> {
    let mut arguments = static_owner_arguments(module, target)?;
    arguments.extend(target.type_args.iter().cloned());
    Ok(arguments)
}

/// Close an associated call from compact template arity authority after source declarations leave execution context.
pub fn indexed_static_type_arguments(
    constructors: &ConstructorIndex,
    target: &NamedCallableTarget,
) -> Result<Vec<IncanType>, String> {
    let mut arguments = Vec::new();
    if let Some(IncanType::Generic { base, args }) = &target.receiver_type {
        match constructors.generic_arities.get(base) {
            Some(arity) => {
                if *arity != args.len() {
                    return Err(format!(
                        "unsupported Body IR static owner type argument arity on {base}"
                    ));
                }
            }
            None => return Err(format!("unsupported Body IR static receiver {base}")),
        }
        arguments.extend(args.iter().cloned());
    }
    arguments.extend(target.type_args.iter().cloned());
    Ok(arguments)
}

/// Read an applied receiver only when its retained canonical template confirms the owner and arity.
fn static_owner_arguments(module: &BodyIrModule, target: &NamedCallableTarget) -> Result<Vec<IncanType>, String> {
    let Some(IncanType::Generic { base, args }) = &target.receiver_type else {
        return Ok(Vec::new());
    };
    for declaration in &module.nominal_declarations {
        if declaration.template_carrier_name() != *base {
            continue;
        }
        if declaration.type_parameters.len() != args.len() {
            return Err(format!(
                "unsupported Body IR static owner type argument arity on {base}"
            ));
        }
        return Ok(args.clone());
    }
    Err(format!("unsupported Body IR static receiver {base}"))
}

/// Incan's list text, `["a", "b"]`, as diagnostics name a binding scope.
pub fn list_text(names: &[String]) -> String {
    let quoted: Vec<String> = names.iter().map(|name| format!("{name:?}")).collect();
    format!("[{}]", quoted.join(", "))
}

/// Close structural types and references with declared arguments; names never infer new type bindings.
pub fn substitute(ty: &IncanType, names: &[String], arguments: &[IncanType]) -> Result<IncanType, String> {
    let close = |inner: &IncanType| substitute(inner, names, arguments);
    match ty {
        IncanType::TypeVar(name) => {
            for (index, candidate) in names.iter().enumerate() {
                if candidate == name {
                    return Ok(arguments[index].clone());
                }
            }
            Err(format!(
                "{UNBOUND_TYPE_PARAMETER} {name} in binding scope {}",
                list_text(names)
            ))
        }
        IncanType::Named(_) => Ok(ty.clone()),
        IncanType::Ref(inner) => Ok(IncanType::Ref(Box::new(close(inner)?))),
        IncanType::RefMut(inner) => Ok(IncanType::RefMut(Box::new(close(inner)?))),
        IncanType::NativeArray { element, length } => Ok(native_array_type(close(element)?, *length)),
        IncanType::Generic { base, args } => {
            let concrete = args.iter().map(close).collect::<Result<Vec<_>, _>>()?;
            Ok(generic(base.clone(), concrete))
        }
        IncanType::Tuple(elements) => Ok(IncanType::Tuple(elements.iter().map(close).collect::<Result<_, _>>()?)),
        IncanType::Function { params, return_type } => {
            let mut closed = Vec::with_capacity(params.len());
            for parameter in params {
                let mut parameter = parameter.clone();
                parameter.ty = close(&parameter.ty)?;
                closed.push(parameter);
            }
            Ok(function_type(closed, close(return_type)?))
        }
        _ => Ok(ty.clone()),
    }
}

/// Close a derived default scope with the same owner or method instance as its frame.
pub fn close_trait_environment(
    environment: &Option<TraitDefaultInstantiation>,
    names: &[String],
    arguments: &[IncanType],
) -> Result<Option<TraitDefaultInstantiation>, String> {
    let Some(retained) = environment else {
        return Ok(None);
    };
    let mut closed = retained.clone();
    closed.receiver_type = substitute(&retained.receiver_type, names, arguments)
        .map_err(|reason| format!("{reason} in trait-default receiver"))?;
    for argument in &mut closed.arguments {
        argument.argument = substitute(&argument.argument, names, arguments)
            .map_err(|reason| format!("{reason} in trait-default argument"))?;
    }
    Ok(Some(closed))
}

/// Close checked types and preserve the value ABI of captured callables passed to original generic parameters.
pub fn instantiate(body: &Body, arguments: &[IncanType], owned_callable_slots: &[i64]) -> Result<Body, String> {
    let names = body.type_parameters.clone();
    match &body.canonical {
        Some(identity) => {
            if identity.kind != SemanticSourceTargetKind::Function
                && identity.kind != SemanticSourceTargetKind::Method
                && identity.kind != SemanticSourceTargetKind::Property
            {
                return Err("unsupported Body IR declaration with type parameters".into());
            }
        }
        None => return Err("unsupported Body IR unproven declaration with type parameters".into()),
    }
    if names.len() != arguments.len() {
        return Err(format!("unsupported Body IR type argument arity on {}", body.name));
    }
    let storage = callable_storage_arguments(body, arguments, owned_callable_slots)?;
    let mut result = retain_owned_callable_storage(body, owned_callable_slots, &storage)?;
    result.trait_default_instantiation = close_trait_environment(&body.trait_default_instantiation, &names, arguments)?;
    let body_name = body.name.clone();
    let source_owners = body.named_type_identities.clone();
    for local in &mut result.locals {
        let concrete = substitute(&local.ty, &names, &storage)
            .map_err(|reason| format!("{reason} in instantiated local of {body_name}"))?;
        local.ty = source_nominal_storage(&source_owners, &concrete)?;
    }
    for index in 0..result.params.len() {
        let parameter = instantiate_parameter(&result.params[index], &names, &storage)?;
        let concrete = source_nominal_storage(&source_owners, &parameter.ty)?;
        result.params[index] = parameter;
        result.params[index].ty = concrete;
    }
    let concrete = substitute(&result.return_type, &names, &storage)?;
    result.return_type = source_nominal_storage(&source_owners, &concrete)?;
    close_result_requirements(&mut result, &names, &storage)?;
    let mut name = body_name;
    for argument in arguments {
        name.push_str(&type_component(argument));
    }
    for slot in owned_callable_slots {
        name.push_str(&format!("__owned_callable_{slot}"));
    }
    result.name = name;
    Ok(result)
}

/// Close result obligations with the same declared binders as the frame, retaining their original call anchors.
pub fn close_result_requirements(body: &mut Body, names: &[String], arguments: &[IncanType]) -> Result<(), String> {
    if let Some(profile) = &body.checked_input_profile {
        let mut closed_profile = profile.clone();
        let mut requirements: Vec<(HirSourceSpan, IncanType)> = Vec::new();
        for (anchor, required_type) in &profile.result_requirements {
            requirements.push((anchor.clone(), substitute(required_type, names, arguments)?));
        }
        closed_profile.result_requirements = requirements;
        body.checked_input_profile = Some(closed_profile);
    }
    Ok(())
}

/// Close a callable parameter with its frame's checked arguments and preserve its generic value ABI.
pub fn instantiate_parameter(
    parameter: &CallableParam,
    names: &[String],
    arguments: &[IncanType],
) -> Result<CallableParam, String> {
    let concrete = substitute(&parameter.ty, names, arguments)?;
    let mut result = parameter.clone();
    result.ty = concrete.clone();
    if owns_instantiated_collection(&parameter.ty, parameter.mutable, &concrete) {
        result.native_owned = true;
    }
    Ok(result)
}

/// Keep a generic value's owned ABI after substitution; written collection parameters retain source borrowing.
fn owns_instantiated_collection(formal: &IncanType, mutable: bool, concrete: &IncanType) -> bool {
    if mutable || !matches!(formal, IncanType::TypeVar(_)) {
        return false;
    }
    if let IncanType::Generic { base, .. } = concrete {
        return matches!(
            collection_type(base),
            Some(CollectionTypeId::List | CollectionTypeId::Set | CollectionTypeId::Dict)
        );
    }
    false
}

/// Read selected generic value slots and preserve their owned ABI throughout nested native arguments.
pub fn callable_storage_arguments(
    body: &Body,
    arguments: &[IncanType],
    slots: &[i64],
) -> Result<Vec<IncanType>, String> {
    let names = owned_callable_parameters(body, slots)?;
    let mut storage = Vec::with_capacity(arguments.len());
    for (index, argument) in arguments.iter().enumerate() {
        if names.contains(&body.type_parameters[index]) {
            storage.push(owned_callable_argument(argument)?);
        } else {
            storage.push(argument.clone());
        }
    }
    Ok(storage)
}

/// Keep source nominal arguments semantic; their declared generic value fields already retain owning storage.
fn source_nominal_storage(owners: &BTreeMap<String, CanonicalSymbolId>, ty: &IncanType) -> Result<IncanType, String> {
    let close = |inner: &IncanType| source_nominal_storage(owners, inner);
    match ty {
        IncanType::Generic { base, args } => {
            let source = source_nominal_owner(owners, base);
            let mut retained = Vec::with_capacity(args.len());
            for argument in args {
                if let IncanType::Generic {
                    base: marker,
                    args: signature,
                } = argument
                    && source
                    && marker == OWNED_CALLABLE_STORAGE
                    && signature.len() == 1
                {
                    retained.push(close(&signature[0])?);
                    continue;
                }
                retained.push(close(argument)?);
            }
            Ok(generic(base.clone(), retained))
        }
        IncanType::Ref(inner) => Ok(IncanType::Ref(Box::new(close(inner)?))),
        IncanType::RefMut(inner) => Ok(IncanType::RefMut(Box::new(close(inner)?))),
        IncanType::TypeToken(inner) => Ok(IncanType::TypeToken(Box::new(close(inner)?))),
        IncanType::Tuple(elements) => Ok(IncanType::Tuple(elements.iter().map(close).collect::<Result<_, _>>()?)),
        IncanType::NativeArray { element, length } => Ok(native_array_type(close(element)?, *length)),
        IncanType::Function { params, return_type } => {
            let mut retained = params.clone();
            for (index, parameter) in params.iter().enumerate() {
                retained[index].ty = close(&parameter.ty)?;
            }
            Ok(function_type(retained, close(return_type)?))
        }
        _ => Ok(ty.clone()),
    }
}

/// Read the frame's checked scope, where a direct alias takes precedence over a computed carrier.
pub fn source_nominal_owner(owners: &BTreeMap<String, CanonicalSymbolId>, name: &str) -> bool {
    if let Some(identity) = owners.get(name) {
        return source_nominal_identity(identity);
    }
    if name.starts_with(NOMINAL_CARRIER_KEY_PREFIX) {
        for identity in owners.values() {
            if nominal_carrier_key(identity).is_some_and(|key| key == name) {
                return source_nominal_identity(identity);
            }
        }
    }
    false
}

/// Separate retained source declarations from native constructors by their canonical origin.
fn source_nominal_identity(identity: &CanonicalSymbolId) -> bool {
    if identity.kind != SemanticSourceTargetKind::Model
        && identity.kind != SemanticSourceTargetKind::Class
        && identity.kind != SemanticSourceTargetKind::Newtype
        && identity.kind != SemanticSourceTargetKind::Enum
    {
        return false;
    }
    matches!(identity.origin, SymbolOrigin::Module(_) | SymbolOrigin::Package { .. })
}

/// Read generic binders only from the validated original value slots that own callable environments.
fn owned_callable_parameters(body: &Body, slots: &[i64]) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for slot in slots {
        if *slot < 0 || *slot as usize >= body.params.len() {
            return Err("invalid owned callable storage slot".into());
        }
        match &body.params[*slot as usize].ty {
            IncanType::TypeVar(name) => names.push(name.clone()),
            _ => return Err("invalid owned callable storage binder".into()),
        }
    }
    Ok(names)
}

/// Mark one function signature as owned, retaining an existing marker when a generic call forwards it.
fn owned_callable_argument(argument: &IncanType) -> Result<IncanType, String> {
    match argument {
        IncanType::Function { .. } => return Ok(generic(OWNED_CALLABLE_STORAGE.to_owned(), vec![argument.clone()])),
        IncanType::Generic { base, args } if base == OWNED_CALLABLE_STORAGE && args.len() == 1 => {
            return Ok(argument.clone());
        }
        _ => {}
    }
    Err("invalid owned callable storage argument".into())
}

/// Preserve selected pointer storage and retain environments only for verified generic owners and their aliases.
fn retain_owned_callable_storage(original: &Body, slots: &[i64], arguments: &[IncanType]) -> Result<Body, String> {
    let mut result = retain_pointer_callable_storage(original, arguments)?;
    if slots.is_empty() {
        return Ok(result);
    }
    let mut names = Vec::new();
    for slot in slots {
        if *slot < 0 || *slot as usize >= original.params.len() || original.params[*slot as usize].mutable {
            return Err("unsupported Body IR owned generic callable parameter".into());
        }
        match &original.params[*slot as usize].ty {
            IncanType::TypeVar(name) => names.push(name.clone()),
            _ => return Err("invalid Body IR owned callable slot outside a generic value parameter".into()),
        }
    }
    let Some(representation) = &result.callable_representation else {
        return Err("unsupported Body IR generic callable without representation facts".into());
    };
    let mut retained = representation.clone();
    for local in &original.locals {
        if let IncanType::TypeVar(name) = &local.ty
            && names.contains(name)
            && !has_local(&retained.closure_holding_locals, &local.id)
        {
            retained.closure_holding_locals.push(local.id);
        }
    }
    if let IncanType::TypeVar(name) = &original.return_type
        && names.contains(name)
    {
        retained.closure_holding_return = true;
    }
    result.callable_representation = Some(retained);
    Ok(result)
}

/// Keep a checked pointer instantiation concrete; environment-capable source parameters retain their shared ABI.
fn retain_pointer_callable_storage(original: &Body, arguments: &[IncanType]) -> Result<Body, String> {
    let mut result = original.clone();
    let pointers: Vec<LocalId> = original
        .locals
        .iter()
        .filter(|local| pointer_instantiation(&local.ty, &original.type_parameters, arguments))
        .map(|local| local.id)
        .collect();
    if pointers.is_empty() {
        return Ok(result);
    }
    let Some(representation) = &result.callable_representation else {
        return Err("unsupported Body IR generic callable without representation facts".into());
    };
    let mut retained = representation.clone();
    retained.closure_holding_locals = without_locals(&representation.closure_holding_locals, &pointers);
    retained.closure_holding_parameters = without_locals(&representation.closure_holding_parameters, &pointers);
    if pointer_instantiation(&original.return_type, &original.type_parameters, arguments) {
        retained.closure_holding_return = false;
    }
    result.callable_representation = Some(retained);
    Ok(result)
}

/// Recognize a generic value's checked pointer argument; an authored callable keeps its environment capability.
fn pointer_instantiation(ty: &IncanType, names: &[String], arguments: &[IncanType]) -> bool {
    if let IncanType::TypeVar(name) = ty {
        for (index, candidate) in names.iter().enumerate() {
            if candidate != name {
                continue;
            }
            return matches!(arguments[index], IncanType::Function { .. });
        }
    }
    false
}

fn without_locals(locals: &[LocalId], excluded: &[LocalId]) -> Vec<LocalId> {
    locals
        .iter()
        .filter(|local| !has_local(excluded, local))
        .copied()
        .collect()
}

fn has_local(locals: &[LocalId], sought: &LocalId) -> bool {
    locals.iter().any(|local| local.index() == sought.index())
}
