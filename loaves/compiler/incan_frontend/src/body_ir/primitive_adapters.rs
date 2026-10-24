//! Retain source-owned primitive capability implementations from checked standard-provider declarations.

use incan_lang::lang::trait_capabilities::{self, PrimitiveSourceAdapterInfo};
use incan_semantics_core::{IncanPrimitiveType, SemanticSourceTargetKind, SymbolOrigin};

use super::{CanonicalSymbolId, IncanType, bir};

/// Attach an adapter only to its registered source declaration after its field, adoption and decoder signatures
/// have been checked. Published consumers use these identities; they never recover an adapter from a name.
pub(super) fn retain(module: &mut bir::BodyIrModule) {
    for owner in &mut module.nominal_declarations {
        let Some(specification) = trait_capabilities::PRIMITIVE_SOURCE_ADAPTERS
            .iter()
            .find(|specification| registered_owner(&owner.canonical, specification))
        else {
            continue;
        };
        let Some(receiver) = bir::PrimitiveTraitAdapter::registered_receiver_type(specification.receiver) else {
            continue;
        };
        if !owner.type_parameters.is_empty()
            || owner.fields.len() != 1
            || owner.fields.first().map(String::as_str) != Some(specification.payload_field)
            || owner.field_types.as_slice() != [receiver.clone()]
            || owner.field_public.as_slice() != [true]
        {
            continue;
        }
        let Some(field) = owner.field_identities.first() else {
            continue;
        };
        let Some(decoder) = module
            .bodies
            .iter()
            .find(|body| checked_decoder(body, owner, specification, &receiver))
        else {
            continue;
        };
        let Some(decoder) = decoder.canonical.as_ref() else {
            continue;
        };
        for adoption in &mut owner.trait_adoptions {
            if !registered_adoption(adoption, specification) {
                continue;
            }
            let Some(decode_slot) = adoption
                .methods
                .iter()
                .flatten()
                .find(|method| method.declaration_name == specification.decode_slot)
            else {
                continue;
            };
            adoption.primitive_adapter = Some(bir::PrimitiveTraitAdapter {
                receiver_type: receiver.clone(),
                payload_field: field.clone(),
                decode_slot: decode_slot.clone(),
                decoded_value: decoder.clone(),
            });
        }
    }
}

/// Match the source provider's declaration identity before reading any adapter-specific fields.
fn registered_owner(identity: &CanonicalSymbolId, specification: &PrimitiveSourceAdapterInfo) -> bool {
    identity.kind == SemanticSourceTargetKind::Model
        && matches!(&identity.origin, SymbolOrigin::Package { library, module_path }
            if specification.matches_owner(library, module_path, &identity.declaration_name))
}

/// Authenticate the exact source trait and its complete required-slot surface, independently of import aliases.
fn registered_adoption(adoption: &bir::TraitAdoption, specification: &PrimitiveSourceAdapterInfo) -> bool {
    let Some(capability) = trait_capabilities::TRAIT_CAPABILITIES
        .iter()
        .find(|capability| capability.id == specification.capability)
    else {
        return false;
    };
    adoption.trait_identity.kind == SemanticSourceTargetKind::Trait
        && matches!(&adoption.trait_identity.origin, SymbolOrigin::Package { library, module_path }
        if specification.matches_trait(library, module_path, &adoption.trait_identity.declaration_name))
        && adoption.arguments.is_empty()
        && adoption.methods.as_ref().is_some_and(|methods| {
            capability
                .required_methods
                .iter()
                .all(|name| methods.iter().any(|method| method.declaration_name == *name))
        })
}

/// Require the adapter's checked static decoder to preserve bytes input, primitive output and structured key errors.
fn checked_decoder(
    body: &bir::Body,
    owner: &bir::NominalDeclaration,
    specification: &PrimitiveSourceAdapterInfo,
    receiver: &IncanType,
) -> bool {
    let Some(identity) = &body.canonical else {
        return false;
    };
    let IncanType::Generic { base, args } = &body.return_type else {
        return false;
    };
    let [value, IncanType::Named(error)] = args.as_slice() else {
        return false;
    };
    identity.origin == owner.canonical.origin
        && identity.declaration_name == specification.decoded_value
        && owner.canonical.declaration_span.start <= identity.declaration_span.start
        && identity.declaration_span.end <= owner.canonical.declaration_span.end
        && body.type_parameters.is_empty()
        && !body.is_async
        && matches!(body.params.as_slice(), [parameter] if parameter.ty == IncanType::Primitive(IncanPrimitiveType::Bytes))
        && base == "Result"
        && value == receiver
        && body.named_type_identities.get(error).is_some_and(|identity| {
            identity.declaration_name == "OrdinalMapError"
                && matches!(&identity.origin, SymbolOrigin::Package { library, module_path }
                    if library == specification.package && module_path.iter().map(String::as_str).eq(["collections"]))
        })
}
