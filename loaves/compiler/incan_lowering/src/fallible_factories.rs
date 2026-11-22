//! Concrete native factory ABIs for authenticated published fallible-iterator defaults.
//!
//! The standard primitives own the fallible-iterator defaults beside the carriers they build, as
//! `incan_stdlib_primitives::fallible_*`. A call of a published default method that returns its concrete carrier
//! calls that function with the carrier's checked type arguments.

use incan_mir_plan_native::{EnumDeclaration, PlanType};
use incan_semantics_core::body_ir::{BodyIrModule, MethodTarget, NominalDeclaration};
use incan_semantics_core::{CanonicalSymbolId, IncanType, SemanticSourceTargetKind, SymbolNamespace};

use crate::decls::library_item;
use crate::fallible_methods::standard_protocol;
use crate::layout_types::native_source_type;
use crate::native_type_identity::ConstructorIndex;
use crate::nominal_instances::instance_arguments;
use crate::trait_links::contains;
use crate::unions::collect_unions;

/// One checked default and the standard helper's explicit concrete native arguments.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeFactory {
    pub path: String,
    pub arguments: Vec<PlanType>,
}

/// Select only a published standard factory with its retained concrete return and actual SDK storage; argument refusals
/// retain the checked type and slot position.
pub fn fallible_factory(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    target: &MethodTarget,
    result: &PlanType,
    intrinsics: &[EnumDeclaration],
) -> Result<Option<NativeFactory>, String> {
    let Some(slot) = &target.canonical else {
        return Ok(None);
    };
    if !published_factory(module, slot) {
        return Ok(None);
    }
    let Some((carrier, arity)) = factory_carrier(&slot.declaration_name) else {
        return Ok(None);
    };
    let PlanType::Model(owner, _name) = result else {
        return Ok(None);
    };
    let declaration = usize::try_from(*owner)
        .ok()
        .and_then(|position| module.nominal_declarations.get(position))
        .filter(|declaration| {
            !library_item(declaration).is_empty()
                && declaration.canonical.origin == slot.origin
                && declaration.canonical.declaration_name == carrier
        });
    let Some(declaration) = declaration else {
        return Err("the standard library factory has no checked canonical result type".into());
    };
    if !concrete_return(module, slot, declaration) {
        return Ok(None);
    }
    let unions = collect_unions(module);
    let mut arguments = Vec::with_capacity(arity);
    for argument in instance_arguments(declaration)? {
        match native_source_type(
            constructors,
            &module.nominal_declarations,
            &module.enum_declarations,
            &unions,
            &argument,
            intrinsics,
            "",
        ) {
            Ok(value) => arguments.push(value),
            Err(error) => {
                return Err(format!(
                    "{error}; standard {} factory argument {} ({argument})",
                    slot.declaration_name,
                    arguments.len() + 1
                ));
            }
        }
    }
    if arguments.len() != arity {
        return Err("unsupported Body IR standard factory native argument count".into());
    }
    Ok(Some(NativeFactory {
        path: format!("incan_stdlib_primitives::fallible_{}", slot.declaration_name),
        arguments,
    }))
}

/// A spelling grants no factory: the actual standard trait must enclose the checked member identity.
fn published_factory(module: &BodyIrModule, slot: &CanonicalSymbolId) -> bool {
    if slot.kind != SemanticSourceTargetKind::Method || slot.namespace != SymbolNamespace::Member {
        return false;
    }
    module
        .trait_declarations
        .iter()
        .any(|root| root.origin == slot.origin && standard_protocol(root) && contains(root, slot))
}

/// Require the exact published body and its return binding's canonical owner before substituting a runtime ABI.
fn concrete_return(module: &BodyIrModule, slot: &CanonicalSymbolId, declaration: &NominalDeclaration) -> bool {
    module.bodies.iter().any(|body| {
        if body.canonical.as_ref() != Some(slot) || body.first_representation_gap().is_some() {
            return false;
        }
        let name = match &body.return_type {
            IncanType::Generic { base, .. } => base,
            IncanType::Named(name) => name,
            _ => return false,
        };
        body.named_type_identities.get(name) == Some(&declaration.canonical)
    })
}

/// The concrete public factory signatures mirrored by the standard library's fallible-default helpers: each default's
/// carrier and its number of type arguments.
fn factory_carrier(name: &str) -> Option<(&'static str, usize)> {
    match name {
        "map" => Some(("MapFallibleIterator", 5)),
        "filter" => Some(("FilterFallibleIterator", 4)),
        "flat_map" => Some(("FlatMapFallibleIterator", 5)),
        "take" => Some(("TakeFallibleIterator", 3)),
        "inspect" => Some(("InspectFallibleIterator", 4)),
        "map_err" => Some(("MapErrFallibleIterator", 5)),
        "inspect_err" => Some(("InspectErrFallibleIterator", 4)),
        _ => None,
    }
}
