//! Dependency roots for checked source-owned primitive capability implementations.

use std::collections::BTreeSet;

use incan_lang::lang::trait_capabilities::{self, PrimitiveSourceAdapterInfo};
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolOrigin};

use crate::loaf_index::LoafIndex;

/// Request registered source adapters only when the checked dependency graph reaches their real capability trait
/// or one of its registered activating declarations. Every returned identity comes from the selected public API.
pub fn required(loaves: &LoafIndex, requested: &BTreeSet<CanonicalSymbolId>) -> BTreeSet<CanonicalSymbolId> {
    let mut required = BTreeSet::new();
    for specification in trait_capabilities::PRIMITIVE_SOURCE_ADAPTERS {
        for entry in loaves.entries() {
            if entry.artifact.manifest.name != specification.package {
                continue;
            }
            let identities = entry
                .artifact
                .manifest
                .contract_metadata
                .identity_graph
                .exports
                .iter()
                .filter_map(|export| export.canonical.as_ref()?.hydrate())
                .collect::<Vec<_>>();
            if !activated(specification, requested, &identities) {
                continue;
            }
            let Some(owner) = identities
                .iter()
                .find(|identity| registered_owner(specification, identity))
            else {
                continue;
            };
            required.insert(owner.clone());
            required.extend(
                identities
                    .iter()
                    .filter(|identity| owned_member(owner, identity))
                    .cloned(),
            );
        }
    }
    required
}

/// Match a checked public model against the registry's declaring location; aliases preserve that same identity.
fn registered_owner(specification: &PrimitiveSourceAdapterInfo, identity: &CanonicalSymbolId) -> bool {
    identity.kind == SemanticSourceTargetKind::Model
        && matches!(&identity.origin, SymbolOrigin::Package { library, module_path }
            if specification.matches_owner(library, module_path, &identity.declaration_name))
}

/// Read activation from canonical dependency edges, including selected trait methods owned by the trait declaration.
fn activated(
    specification: &PrimitiveSourceAdapterInfo,
    requested: &BTreeSet<CanonicalSymbolId>,
    identities: &[CanonicalSymbolId],
) -> bool {
    let Some(capability) = trait_capabilities::TRAIT_CAPABILITIES
        .iter()
        .find(|capability| capability.id == specification.capability)
    else {
        return false;
    };
    identities.iter().any(|identity| {
        let SymbolOrigin::Package { library, module_path } = &identity.origin else {
            return false;
        };
        if specification.matches_trait(library, module_path, &identity.declaration_name) {
            return requested.contains(identity) || requested.iter().any(|selected| owned_member(identity, selected));
        }
        library == specification.package
            && module_path
                .iter()
                .map(String::as_str)
                .eq(capability.module_path.iter().skip(1).copied())
            && capability
                .import_trigger_items
                .contains(&identity.declaration_name.as_str())
            && requested.contains(identity)
    })
}

/// Preserve a public source member's physical owner boundary; a same-name member in another owner grants nothing.
fn owned_member(owner: &CanonicalSymbolId, member: &CanonicalSymbolId) -> bool {
    member.kind == SemanticSourceTargetKind::Method
        && member.origin == owner.origin
        && owner.declaration_span.start <= member.declaration_span.start
        && member.declaration_span.end <= owner.declaration_span.end
}
