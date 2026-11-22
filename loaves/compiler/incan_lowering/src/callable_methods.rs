//! Native callable forwarding selected from complete checked standard adoptions.

use incan_mir_plan_native::{EnumDeclaration, PlanType};
use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::{CanonicalSymbolId, IncanType, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};

use crate::classes::{frame_type, function_name, parameter_local_index};
use crate::decls::library_item;
use crate::layout_types::native_source_type;
use crate::native_type_identity::ConstructorIndex;
use crate::trait_links::contains;
use crate::unions::collect_unions;

/// Require one actual Callable1 adoption, one complete checked slot, and its exact immutable source ABI.
pub fn callable_methods(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    owner: usize,
    native_names: &[String],
    intrinsics: &[EnumDeclaration],
) -> Result<(String, Vec<PlanType>), String> {
    let declaration = &module.nominal_declarations[owner];
    if !native_names.contains(&declaration.name) || !library_item(declaration).is_empty() {
        return Ok((String::new(), Vec::new()));
    }
    let mut selected: Option<CanonicalSymbolId> = None;
    let mut arguments: Vec<IncanType> = Vec::new();
    for adoption in &declaration.trait_adoptions {
        if !standard_callable(&adoption.trait_identity) {
            continue;
        }
        if selected.is_some() || adoption.arguments.len() != 2 {
            return Err("unsupported Body IR ambiguous or nonbinary native Callable1 adoption".into());
        }
        let Some(methods) = &adoption.methods else {
            return Err("unsupported Body IR incomplete native Callable1 slots".into());
        };
        if methods.len() != 1 {
            return Err("unsupported Body IR native Callable1 slot count".into());
        }
        let slot = &methods[0];
        if slot.kind != SemanticSourceTargetKind::Method
            || slot.namespace != SymbolNamespace::Member
            || slot.declaration_name != "__call__"
            || slot.origin != adoption.trait_identity.origin
            || !contains(&adoption.trait_identity, slot)
        {
            return Err("unsupported Body IR native Callable1 slot identity".into());
        }
        selected = Some(slot.clone());
        for argument in &adoption.arguments {
            if !owned_type(argument) {
                return Err("unsupported Body IR borrowed or open native Callable1 argument".into());
            }
            arguments.push(argument.clone());
        }
    }
    let Some(slot) = selected else {
        return Ok((String::new(), Vec::new()));
    };
    let nominals = &module.nominal_declarations;
    let enums = &module.enum_declarations;
    let mut signature = Vec::with_capacity(arguments.len());
    for argument in &arguments {
        let unions = collect_unions(module);
        signature.push(native_source_type(
            constructors,
            nominals,
            enums,
            &unions,
            argument,
            intrinsics,
            "",
        )?);
    }
    for implementation in &module.trait_implementations {
        if implementation.owner != declaration.canonical
            || implementation.method != slot
            || implementation.implementation == slot
        {
            continue;
        }
        for body in &module.bodies {
            if body.canonical.as_ref() != Some(&implementation.implementation)
                || !body.type_parameters.is_empty()
                || body.params.len() != 2
            {
                continue;
            }
            let receiver = &body.locals[parameter_local_index(body, 0)];
            let value = &body.locals[parameter_local_index(body, 1)];
            if frame_type(constructors, nominals, enums, receiver, intrinsics, "")?
                != PlanType::ModelRef(owner as i64, declaration.name.clone())
            {
                continue;
            }
            let unions = collect_unions(module);
            let result = native_source_type(
                constructors,
                nominals,
                enums,
                &unions,
                &body.return_type,
                intrinsics,
                "",
            )?;
            if frame_type(constructors, nominals, enums, value, intrinsics, "")? != signature[0]
                || result != signature[1]
            {
                return Err("unsupported Body IR native Callable1 source signature".into());
            }
            return Ok((function_name(module, body), signature));
        }
    }
    Err("unsupported Body IR unavailable native Callable1 source body".into())
}

/// Only the artifact-backed standard declaration grants this native protocol, including through aliases.
fn standard_callable(identity: &CanonicalSymbolId) -> bool {
    if identity.kind != SemanticSourceTargetKind::Trait
        || identity.namespace != SymbolNamespace::OrdinaryLexical
        || identity.declaration_name != "Callable1"
    {
        return false;
    }
    match &identity.origin {
        SymbolOrigin::Package { library, module_path } => {
            library == "incan_stdlib_primitives" && module_path == &["traits", "callable"]
        }
        _ => false,
    }
}

/// Borrowed or open callback arguments need a separately represented lifetime contract and remain refused.
fn owned_type(ty: &IncanType) -> bool {
    match ty {
        IncanType::Ref(_) | IncanType::RefMut(_) | IncanType::TypeVar(_) | IncanType::SelfType | IncanType::Unknown => {
            false
        }
        IncanType::Generic { args, .. } => args.iter().all(owned_type),
        IncanType::Tuple(elements) => elements.iter().all(owned_type),
        _ => true,
    }
}
