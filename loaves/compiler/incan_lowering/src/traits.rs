//! Concrete source-trait specialization and checker-selected static dispatch.

use incan_semantics_core::body_ir::{
    Body, BodyIrModule, EnumDeclaration, NominalDeclaration, TraitDefaultInstantiation, TraitImplementation,
    TraitTypeArgument,
};
use incan_semantics_core::{
    CanonicalSymbolId, HirSourceSpan, IncanType, SemanticSourceTargetKind, SymbolOrigin, function_type, generic,
    native_array_type,
};

use incan_mir_plan_native::PlanType;

use crate::classes::receiver_mode;
use crate::type_parameters::substitute;

/// Instantiate this module's open defaults for every adopter of the project the frontend retained.
///
/// An imported trait's default lives in the module that declares the trait, while its adopter's implementation
/// records live in the adopter's module, so the records of every module are consulted. Each instance keeps the
/// default's canonical identity, and with it the declaring module's source provenance.
pub fn append_specialized_project_trait_bodies(
    module: &BodyIrModule,
    modules: &[BodyIrModule],
    bodies: &mut Vec<Body>,
) -> Result<(), String> {
    if !module.bodies.iter().any(|body| is_default_body(module, body)) {
        bodies.extend(module.bodies.iter().cloned());
        return Ok(());
    }
    let mut implementations: Vec<TraitImplementation> = Vec::new();
    let mut owners: Vec<NominalDeclaration> = Vec::new();
    let mut enum_owners: Vec<EnumDeclaration> = Vec::new();
    for adopter in modules {
        implementations.extend(adopter.trait_implementations.iter().cloned());
        owners.extend(adopter.nominal_declarations.iter().cloned());
        enum_owners.extend(adopter.enum_declarations.iter().cloned());
    }
    append_specialized_bodies(module, &implementations, &owners, &enum_owners, bodies)
}

/// A retained trait method body, including receiver-free associated defaults.
pub fn is_default_body(module: &BodyIrModule, body: &Body) -> bool {
    let Some(identity) = &body.canonical else {
        return false;
    };
    module.trait_declarations.iter().any(|owner| {
        owner.origin == identity.origin
            && owner.declaration_span.start <= identity.declaration_span.start
            && identity.declaration_span.end <= owner.declaration_span.end
    })
}

/// Keep dependency generic-owner methods governed until a call or native bridge selects them.
pub fn is_lazy_method_body(module: &BodyIrModule, body: &Body) -> bool {
    if is_default_body(module, body) {
        return true;
    }
    let Some(first) = body.locals.first() else {
        return false;
    };
    if receiver_mode(&first.origin).is_none() {
        return false;
    }
    let Some(identity) = &body.canonical else {
        return false;
    };
    if identity.kind != SemanticSourceTargetKind::Method || !matches!(identity.origin, SymbolOrigin::Package { .. }) {
        return false;
    }
    module.nominal_declarations.iter().any(|owner| {
        !owner.type_parameters.is_empty()
            && owner.canonical.origin == identity.origin
            && owner.canonical.declaration_span.start <= identity.declaration_span.start
            && identity.declaration_span.end <= owner.canonical.declaration_span.end
    })
}

/// Replace each open default of the module by one instance per adopter whose implementation it is.
fn append_specialized_bodies(
    module: &BodyIrModule,
    implementations: &[TraitImplementation],
    owners: &[NominalDeclaration],
    enums: &[EnumDeclaration],
    result: &mut Vec<Body>,
) -> Result<(), String> {
    for body in &module.bodies {
        if !is_default_body(module, body) {
            result.push(body.clone());
            continue;
        }
        // One default can fill several slots of one adopter; the adopter still receives a single instantiation.
        let mut instantiated: Vec<&TraitImplementation> = Vec::new();
        for implementation in implementations {
            if body.canonical.as_ref() != Some(&implementation.implementation) {
                continue;
            }
            if !first_instantiation(&instantiated, implementation)? {
                continue;
            }
            instantiated.push(implementation);
            let concrete = concrete_self_type(&implementation.owner, owners, enums)?;
            // A default with overlapping owner and method scopes is not representable by the flat binding
            // environment; never substitute the adopter's U for the default's independent method U.
            if deferred_shadowed_default(body, &concrete, &implementation.type_arguments) {
                continue;
            }
            result.push(specialize_body(body, &concrete, &implementation.type_arguments));
        }
    }
    Ok(())
}

/// Leave an unsupported default unavailable without rejecting unrelated used methods of its adopter (#1698).
fn deferred_shadowed_default(body: &Body, concrete: &IncanType, arguments: &[TraitTypeArgument]) -> bool {
    let IncanType::Generic {
        args: owner_arguments, ..
    } = concrete
    else {
        return false;
    };
    for parameter in &body.type_parameters {
        if is_adoption_parameter(parameter, arguments) {
            continue;
        }
        if owner_arguments
            .iter()
            .any(|argument| matches!(argument, IncanType::TypeVar(name) if name == parameter))
        {
            return true;
        }
    }
    false
}

/// Whether this adopter has no instantiation of the default yet; a conflicting second one refuses.
fn first_instantiation(retained: &[&TraitImplementation], candidate: &TraitImplementation) -> Result<bool, String> {
    for implementation in retained {
        if implementation.owner != candidate.owner {
            continue;
        }
        if implementation.type_arguments != candidate.type_arguments {
            return Err("unsupported Body IR trait default at several instantiations for one adopter".into());
        }
        return Ok(false);
    }
    Ok(true)
}

/// Instantiate a trait default for one adopter: same statements and identities, Self and type parameters closed.
fn specialize_body(body: &Body, concrete: &IncanType, arguments: &[TraitTypeArgument]) -> Body {
    let mut result = body.clone();
    result.set_trait_default_instantiation(concrete.clone(), arguments.to_vec());
    result.type_parameters = body
        .type_parameters
        .iter()
        .filter(|parameter| !is_adoption_parameter(parameter, arguments))
        .cloned()
        .collect();
    for local in &mut result.locals {
        local.ty = instantiate_type(&local.ty, concrete, arguments);
    }
    for parameter in &mut result.params {
        parameter.ty = instantiate_type(&parameter.ty, concrete, arguments);
    }
    result.return_type = instantiate_type(&result.return_type, concrete, arguments);
    // Adoption closes trait binders before the default's own method binders are instantiated.
    if let Some(profile) = &result.checked_input_profile {
        let mut closed_profile = profile.clone();
        let requirements: Vec<(HirSourceSpan, IncanType)> = profile
            .result_requirements
            .iter()
            .map(|(anchor, required)| (anchor.clone(), instantiate_type(required, concrete, arguments)))
            .collect();
        closed_profile.result_requirements = requirements;
        result.checked_input_profile = Some(closed_profile);
    }
    result
}

/// Close trait-scoped dispatch facts while preserving independent method parameters.
pub fn close_trait_dispatch_arguments(
    environment: &Option<TraitDefaultInstantiation>,
    arguments: Vec<IncanType>,
) -> Vec<IncanType> {
    match environment {
        None => arguments,
        Some(retained) => arguments
            .iter()
            .map(|argument| instantiate_type(argument, &retained.receiver_type, &retained.arguments))
            .collect(),
    }
}

/// Whether the checked adoption binds one declared trait parameter, leaving method parameters open.
fn is_adoption_parameter(name: &str, arguments: &[TraitTypeArgument]) -> bool {
    arguments.iter().any(|argument| argument.parameter == name)
}

/// Close `Self` and the checker-recorded trait type arguments inside one checked type.
pub fn instantiate_type(ty: &IncanType, concrete: &IncanType, arguments: &[TraitTypeArgument]) -> IncanType {
    let close = |inner: &IncanType| instantiate_type(inner, concrete, arguments);
    match ty {
        IncanType::SelfType => concrete.clone(),
        IncanType::TypeVar(name) => arguments
            .iter()
            .find(|argument| argument.parameter == *name)
            .map(|argument| argument.argument.clone())
            .unwrap_or_else(|| ty.clone()),
        IncanType::Ref(inner) => IncanType::Ref(Box::new(close(inner))),
        IncanType::RefMut(inner) => IncanType::RefMut(Box::new(close(inner))),
        IncanType::NativeArray { element, length } => native_array_type(close(element), *length),
        IncanType::Generic { base, args } => generic(base.clone(), args.iter().map(close).collect()),
        IncanType::Tuple(elements) => IncanType::Tuple(elements.iter().map(close).collect()),
        IncanType::Function { params, return_type } => {
            let mut closed = params.clone();
            for parameter in &mut closed {
                parameter.ty = close(&parameter.ty);
            }
            function_type(closed, close(return_type))
        }
        _ => ty.clone(),
    }
}

/// Retain the implementing owner's own parameters in Self until checked nominal instance scheduling closes them.
fn concrete_self_type(
    owner: &CanonicalSymbolId,
    owners: &[NominalDeclaration],
    enums: &[EnumDeclaration],
) -> Result<IncanType, String> {
    for declaration in owners {
        if declaration.canonical != *owner {
            continue;
        }
        if declaration.type_parameters.is_empty() {
            return Ok(IncanType::Named(declaration.carrier_name()));
        }
        let arguments = declaration
            .type_parameters
            .iter()
            .map(|parameter| IncanType::TypeVar(parameter.clone()))
            .collect();
        return Ok(generic(declaration.template_carrier_name(), arguments));
    }
    for declaration in enums {
        if declaration.canonical == *owner {
            return Ok(IncanType::Named(declaration.carrier_name()));
        }
    }
    Err("unsupported Body IR trait default without retained owner layout".into())
}

/// Retain a concrete default's checked declaring-trait bindings beside its class and method arguments.
///
/// 1. Select the receiver's canonical owner from its validated frame.
/// 2. Read only that owner's implementation of this exact default body.
/// 3. Close the bindings in the adopter scope before entering the default's declaring scope.
pub fn append_default_type_bindings(
    module: &BodyIrModule,
    body: &Body,
    receiver: &PlanType,
    names: &mut Vec<String>,
    arguments: &mut Vec<IncanType>,
) -> Result<(), String> {
    // 1. A non-receiver frame has no inherited trait environment.
    if body.locals.is_empty() || receiver_mode(&body.locals[0].origin).is_none() {
        return Ok(());
    }
    let owner = match receiver {
        PlanType::ModelRef(index, name) | PlanType::ModelMutRef(index, name) => PlanType::Model(*index, name.clone()),
        PlanType::EnumRef(index, name) => PlanType::Enum(*index, name.clone()),
        _ => receiver.clone(),
    };
    let identity = owner_identity(module, &owner)?;
    // 2. A default shared by several adopters keeps each adopter's own checked bindings.
    let mut selected: Option<&TraitImplementation> = None;
    for implementation in &module.trait_implementations {
        if implementation.owner != identity || Some(&implementation.implementation) != body.canonical.as_ref() {
            continue;
        }
        if let PlanType::Model(..) = owner
            && !receiver_selects_slot_arguments(module, implementation, &owner)
        {
            continue;
        }
        match selected {
            Some(previous) => {
                if previous.type_arguments != implementation.type_arguments {
                    return Err("unsupported Body IR conflicting default frame instantiations".into());
                }
            }
            None => selected = Some(implementation),
        }
    }
    // 3. Statement facts keep the declaring trait's scope, independently of the adopter's binder spellings.
    match selected {
        Some(implementation) => merge_default_type_bindings(body, &implementation.type_arguments, names, arguments),
        None => Ok(()),
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

/// Select the checked adoption of one concrete receiver even when several instances share its declaration.
pub fn receiver_selects_slot_arguments(
    module: &BodyIrModule,
    implementation: &TraitImplementation,
    owner: &PlanType,
) -> bool {
    if implementation.slot_arguments.is_empty() {
        return true;
    }
    let PlanType::Model(index, _) = owner else {
        return false;
    };
    let Some(declaration) = module.nominal_declarations.get(*index as usize) else {
        return false;
    };
    declaration.trait_adoptions.iter().any(|adoption| {
        adoption.arguments == implementation.slot_arguments
            && adoption
                .methods
                .as_ref()
                .is_some_and(|methods| methods.contains(&implementation.method))
    })
}

/// Close every adoption argument before a declaring-trait binder shadows an adopter binder of the same name.
fn merge_default_type_bindings(
    body: &Body,
    bindings: &[TraitTypeArgument],
    names: &mut Vec<String>,
    arguments: &mut Vec<IncanType>,
) -> Result<(), String> {
    let mut closed = Vec::with_capacity(bindings.len());
    for binding in bindings {
        closed.push(substitute(&binding.argument, names, arguments)?);
    }
    for (binding, concrete) in bindings.iter().zip(closed) {
        let parameter = &binding.parameter;
        let Some(existing) = names.iter().position(|name| name == parameter) else {
            names.push(parameter.clone());
            arguments.push(concrete);
            continue;
        };
        if arguments[existing] != concrete && body.type_parameters.iter().any(|name| name == parameter) {
            return Err(format!(
                "unsupported Body IR shadowed trait and method type parameter {parameter}"
            ));
        }
        arguments[existing] = concrete;
    }
    Ok(())
}
