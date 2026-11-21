//! Class receiver frames and canonical source method admission.

use incan_mir_plan_native::{EnumDeclaration, PlanType};
use incan_semantics_core::body_ir::{
    Body, BodyIrModule, EnumDeclaration as SourceEnumDeclaration, LocalDecl, LocalOrigin, NominalDeclaration,
};
use incan_semantics_core::{
    CanonicalSymbolId, IncanType, SemanticSourceTargetKind, SymbolNamespace, canonical_module_identity,
};

use crate::layout_types::native_source_type;
use crate::native_type_identity::ConstructorIndex;
use crate::ranges::range_value_local;
use crate::symbols::{symbol_component, type_component};

/// A parameter's retained local identity.
pub fn parameter_local_index(body: &Body, index: usize) -> usize {
    body.params[index].local.index()
}

/// Map checked receiver origins and retained types to native slots through borrowed declaration tables.
///
/// A source enum's receiver is a shared reference; a `mut self` enum method would replace the value its caller
/// holds, which no enum receiver frame retains, so it refuses. Unions resolve from retained intrinsic layouts.
pub fn frame_type(
    constructors: &ConstructorIndex,
    nominals: &[NominalDeclaration],
    enums: &[SourceEnumDeclaration],
    local: &LocalDecl,
    intrinsics: &[EnumDeclaration],
    source_file: &str,
) -> Result<PlanType, String> {
    if range_value_local(&local.ty, &local.origin) {
        return Ok(PlanType::RangeValue);
    }
    let ty = native_source_type(constructors, nominals, enums, &[], &local.ty, intrinsics, source_file)?;
    match receiver_mode(&local.origin) {
        Some(mutable) => {
            match &ty {
                PlanType::Model(index, name) => {
                    if mutable {
                        return Ok(PlanType::ModelMutRef(*index, name.clone()));
                    }
                    return Ok(PlanType::ModelRef(*index, name.clone()));
                }
                PlanType::Enum(index, name) => {
                    if mutable {
                        return Err("unsupported Body IR mutable enum receiver".into());
                    }
                    if *index < enums.len() as i64 {
                        return Ok(PlanType::EnumRef(*index, name.clone()));
                    }
                }
                _ => {}
            }
            Err("unsupported Body IR nonnominal receiver".into())
        }
        None => Ok(ty),
    }
}

/// Keep entry symbols and encode dependency module identities so equal spellings and spans remain distinct.
pub fn function_name(module: &BodyIrModule, body: &Body) -> String {
    if let Some(identity) = &body.canonical {
        if let Some(owner) = canonical_module_identity(identity)
            && owner != module.module_id.path()
        {
            return module_function_symbol(module, &owner, body);
        }
        if identity.kind == SemanticSourceTargetKind::Function {
            // Overloads share their source name, so each takes its own declaration span as well.
            if has_overloaded_name(module, body, identity) {
                return format!(
                    "__incan_overload_{}{}",
                    body_span_symbol(body),
                    symbol_component(&body.name)
                );
            }
            return body.name.clone();
        }
    }
    if let Some(first) = body.locals.first()
        && receiver_mode(&first.origin).is_some()
    {
        let mut symbol = format!("__incan_body_{}{}", body_span_symbol(body), type_component(&first.ty));
        if !body.type_parameters.is_empty() {
            symbol.push_str(&symbol_component(&body.name));
        }
        return symbol;
    }
    // Instances of one associated method share its span; the instantiated name tells their frames apart.
    if !body.type_parameters.is_empty() {
        return format!(
            "__incan_body_{}{}",
            body_span_symbol(body),
            symbol_component(&body.name)
        );
    }
    format!("__incan_body_{}", body_span_symbol(body))
}

/// Retain authored symbols while distinguishing generated frames by their complete checked declaration identity.
fn body_span_symbol(body: &Body) -> String {
    let mut symbol = body.span.start.to_string();
    if body.span.is_generated() {
        symbol.push_str(&symbol_component(body.direct_call_id.path()));
    }
    symbol
}

/// Read explicit checked entry identities; an ordinary imported main keeps ordinary function semantics.
pub fn is_program_entry(module: &BodyIrModule, body: &Body) -> bool {
    if body.name != "main" {
        return false;
    }
    if function_name(module, body) == "main" {
        return true;
    }
    match &body.canonical {
        Some(identity) => module.program_entrypoints.iter().any(|selected| selected == identity),
        None => false,
    }
}

/// Whether another function of the same module declares this body's source name, as overloads do.
fn has_overloaded_name(module: &BodyIrModule, body: &Body, identity: &CanonicalSymbolId) -> bool {
    let owner = canonical_module_identity(identity);
    module.bodies.iter().any(|other| {
        if other.name != body.name || other.span == body.span {
            return false;
        }
        match &other.canonical {
            Some(other_identity) => {
                other_identity.kind == SemanticSourceTargetKind::Function
                    && canonical_module_identity(other_identity) == owner
            }
            None => false,
        }
    })
}

/// The validated body's declaring module, independently of the project entry module.
pub fn body_module_identity(body: &Body) -> Result<String, String> {
    match &body.canonical {
        Some(identity) => canonical_module_identity(identity)
            .ok_or_else(|| "invalid Body IR body without source module owner".to_owned()),
        None => Err("invalid Body IR body without canonical identity".into()),
    }
}

/// Retain the scheduled method's checked class bindings alongside its own ordered call arguments.
pub fn body_type_bindings(
    module: &BodyIrModule,
    body: &Body,
    arguments: &[IncanType],
) -> Result<(Vec<String>, Vec<IncanType>), String> {
    let names = body.type_parameters.clone();
    if names.len() != arguments.len() {
        return Err(format!(
            "unsupported Body IR active type argument arity on {}",
            body.name
        ));
    }
    if !body.nominal_instantiation.is_empty() {
        let mut owner_names = Vec::new();
        let mut owner_arguments = Vec::new();
        for binding in &body.nominal_instantiation {
            if names.contains(&binding.parameter) {
                return Err(format!(
                    "unsupported Body IR shadowed class and method type parameter {}",
                    binding.parameter
                ));
            }
            owner_names.push(binding.parameter.clone());
            owner_arguments.push(binding.argument.clone());
        }
        owner_arguments.extend(arguments.iter().cloned());
        owner_names.extend(names);
        return Ok((owner_names, owner_arguments));
    }
    let Some(first) = body.locals.first() else {
        return Ok((names, arguments.to_vec()));
    };
    if receiver_mode(&first.origin).is_none() {
        return Ok((names, arguments.to_vec()));
    }
    if let IncanType::Generic {
        base,
        args: owner_arguments,
    } = &first.ty
    {
        for declaration in &module.nominal_declarations {
            if declaration.template_carrier_name() != *base {
                continue;
            }
            if declaration.type_parameters.len() != owner_arguments.len() {
                return Err(format!(
                    "unsupported Body IR active class type argument arity on {base}"
                ));
            }
            for name in &declaration.type_parameters {
                if names.contains(name) {
                    return Err(format!(
                        "unsupported Body IR shadowed class and method type parameter {name}"
                    ));
                }
            }
            let mut bindings = owner_arguments.clone();
            bindings.extend(arguments.iter().cloned());
            let mut all = declaration.type_parameters.clone();
            all.extend(names);
            return Ok((all, bindings));
        }
    }
    Ok((names, arguments.to_vec()))
}

/// Name a dependency module's frame by its owner module and declaration, using identifier-safe bytes.
fn module_function_symbol(module: &BodyIrModule, owner: &str, body: &Body) -> String {
    let mut symbol = "__incan_module".to_owned();
    for segment in owner.split("::") {
        symbol.push_str(&symbol_component(segment));
    }
    if declaration_named_frame(module, body) {
        return format!("{symbol}_named{}", symbol_component(&body.name));
    }
    // A trait default can have the same canonical declaration and span in several concrete receiver frames.
    if let Some(first) = body.locals.first()
        && receiver_mode(&first.origin).is_some()
    {
        symbol.push_str("_receiver");
        symbol.push_str(&type_component(&first.ty));
    }
    if !body.type_parameters.is_empty() {
        symbol.push_str(&symbol_component(&body.name));
    }
    format!("{symbol}_{}_{}", body_span_symbol(body), body.span.end)
}

/// Whether a frame's owner module and source name alone identify it among the unit's frames.
fn declaration_named_frame(module: &BodyIrModule, body: &Body) -> bool {
    if body.span.is_generated() || !body.type_parameters.is_empty() {
        return false;
    }
    if let Some(first) = body.locals.first()
        && receiver_mode(&first.origin).is_some()
    {
        return false;
    }
    let Some(identity) = &body.canonical else {
        return false;
    };
    if identity.namespace != SymbolNamespace::OrdinaryLexical || identity.scope_discriminant.is_some() {
        return false;
    }
    if identity.kind != SemanticSourceTargetKind::Function {
        return false;
    }
    !has_overloaded_name(module, body, identity)
}

/// Read receiver origins through the typed enum boundary.
pub fn receiver_mode(value: &LocalOrigin) -> Option<bool> {
    match value {
        LocalOrigin::Receiver { mutable } => Some(*mutable),
        _ => None,
    }
}
