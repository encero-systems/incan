//! Constructor and comparison arguments retain their checked declaration-owned field context.

use super::{CanonicalSymbolId, Identities, IncanType, bir, retain_declared_return_type};
use incan_semantics_core::CompilerNodeId;
use std::collections::BTreeMap;

/// Immutable declaration facts shared while individual frames retain their use-site argument carriers.
pub(super) struct ConstructorCarrier {
    /// The physical declaration must agree with the checked canonical target before its fields grant storage.
    declaration: CompilerNodeId,
    /// Original field order authenticates each resolved slot independently of the caller's argument names.
    fields: Vec<String>,
    /// Member identities authenticate projected field context independently of source spellings.
    field_identities: Vec<CanonicalSymbolId>,
    /// Declaration-owned nested types retain their checked named-leaf identities.
    types: Vec<IncanType>,
    /// A caller's same-spelled type cannot replace this declaring context.
    identities: Identities,
}

/// Constructor facts indexed only by their checked canonical declaration.
pub(super) type ConstructorCarriers = BTreeMap<CanonicalSymbolId, ConstructorCarrier>;

/// Snapshot selected declaration facts before frame qualification mutates the current module's carrier names.
pub(super) fn collect(module: &bir::BodyIrModule, context: &[bir::BodyIrModule]) -> ConstructorCarriers {
    context
        .iter()
        .flat_map(|module| &module.nominal_declarations)
        .chain(&module.nominal_declarations)
        .map(|declaration| {
            (
                declaration.canonical.clone(),
                ConstructorCarrier {
                    declaration: declaration.direct_declaration_id.clone(),
                    fields: declaration.fields.clone(),
                    field_identities: declaration.field_identities.clone(),
                    types: declaration.field_types.clone(),
                    identities: declaration.named_type_identities.clone(),
                },
            )
        })
        .collect()
}

/// Retain an equality operand's nominal context from the other operand's exact checked field projection.
///
/// Only a direct canonical field on a retained nominal local supplies this context. Generic, structural, unbound, or
/// deeper projections remain unchanged; no field name or caller namespace can select a declaration by itself.
pub(super) fn retain_comparison(
    left: &bir::Operand,
    right: &bir::Operand,
    locals: &mut [bir::LocalDecl],
    identities: &mut Identities,
    declarations: &ConstructorCarriers,
) {
    for (field, argument) in [(left, right), (right, left)] {
        let Some((declared, source)) = field_context(field, locals, identities, declarations) else {
            continue;
        };
        let bir::Operand::Place(read) = argument else { continue };
        if !read.place.projection.is_empty() {
            continue;
        }
        let Some(local) = read.place.local_id().and_then(|id| locals.get_mut(id.index())) else {
            continue;
        };
        retain_declared_return_type(&mut local.ty, declared, source, identities);
    }
}

/// Return only the declaring layout's type and named-leaf identities for one checker-selected source field.
fn field_context<'declaration>(
    operand: &bir::Operand,
    locals: &[bir::LocalDecl],
    identities: &Identities,
    declarations: &'declaration ConstructorCarriers,
) -> Option<(&'declaration IncanType, &'declaration Identities)> {
    let bir::Operand::Place(read) = operand else {
        return None;
    };
    let [
        bir::PlaceElem::Field {
            name,
            canonical: Some(member),
            structural: false,
        },
    ] = read.place.projection.as_slice()
    else {
        return None;
    };
    let local = read.place.local_id().and_then(|id| locals.get(id.index()))?;
    let IncanType::Named(owner) = &local.ty else {
        return None;
    };
    let declaration = declarations.get(identities.get(owner)?)?;
    let slot = declaration
        .field_identities
        .iter()
        .position(|identity| identity == member)?;
    if declaration.fields.get(slot) != Some(name) {
        return None;
    }
    Some((declaration.types.get(slot)?, &declaration.identities))
}

/// Retain only whole argument temporaries through the exact checked field slots of an authenticated constructor.
///
/// This fills nominal provenance in existing nested types, including empty collections and enclosing Options. It
/// neither changes argument order or ownership nor introduces an ambient caller import. Missing declaration, field
/// layout, or bound slots leave the original storage unqualified and therefore refused by the executor.
pub(super) fn retain(
    target: &bir::ConstructorTarget,
    operands: &[bir::ArgumentElement],
    locals: &mut [bir::LocalDecl],
    identities: &mut Identities,
    constructors: &ConstructorCarriers,
) {
    let Some(declaration) = target
        .canonical
        .as_ref()
        .and_then(|identity| constructors.get(identity))
    else {
        return;
    };
    if target.direct_declaration_id.as_ref() != Some(&declaration.declaration)
        || target.canonical_field_layout.as_ref() != Some(&declaration.fields)
    {
        return;
    }
    let bir::ArgumentBinding::Resolved { arguments, .. } = &target.binding else {
        return;
    };
    if arguments.len() != operands.len() {
        return;
    }
    for (argument, operand) in arguments.iter().zip(operands) {
        let Some(declared) = declaration.types.get(argument.slot) else {
            continue;
        };
        let Some(bir::Operand::Place(read)) = operand.as_one() else {
            continue;
        };
        if !read.place.projection.is_empty() {
            continue;
        }
        let Some(local) = read.place.local_id().and_then(|id| locals.get_mut(id.index())) else {
            continue;
        };
        retain_declared_return_type(&mut local.ty, declared, &declaration.identities, identities);
    }
}
