//! Members of imported nominals that a body record may name.

use super::*;
use crate::symbols::{SymbolKind, TraitInfo, TypeInfo};
use incan_semantics_core::SymbolOrigin;

/// Resolve recorded member references to the current declaration anchors carried by their imported nominal.
/// Package methods and variants have their own executable anchors, distinct from the enclosing nominal.
pub(super) fn identities(checker: &TypeChecker) -> Vec<(String, CanonicalSymbolId)> {
    let mut identities = Vec::new();
    for (index, symbol) in checker.symbols.all_symbols().iter().enumerate() {
        let Some(owner) = checker.symbols.identity_of(index) else {
            continue;
        };
        if !matches!(owner.origin, SymbolOrigin::Module(_) | SymbolOrigin::Package { .. }) {
            continue;
        }
        let nominal = match &symbol.kind {
            SymbolKind::Type(info) => Nominal::Type(info),
            SymbolKind::Trait(info) => Nominal::Trait(info),
            _ => continue,
        };
        collect(owner, nominal, &mut identities);
    }
    identities
}

enum Nominal<'a> {
    Type(&'a TypeInfo),
    Trait(&'a TraitInfo),
}

/// Include only transitively imported nominals actually named by this owner's decoded references.
/// An imported callable's return type may carry a package member, or a sibling module's member, without introducing
/// a lexical name.
pub(super) fn bind_transitive(
    checker: &TypeChecker,
    positions: &mut Positions,
    fields: &Fields,
) -> Result<(), NodeBindingError> {
    let requested = fields
        .iter()
        .filter(|row| row.len() == 8 && row[0] == "identity-context")
        .filter_map(|row| positions.canonical_type(&row[4]).ok())
        .filter(|owner| matches!(owner.origin, SymbolOrigin::Package { .. } | SymbolOrigin::Module(_)))
        .collect::<std::collections::BTreeSet<_>>();
    if requested.is_empty() {
        return Ok(());
    }
    let mut members = Vec::new();
    bind_sibling_members(checker, &requested, &mut members);
    for (binding, export) in checker.public_library_type_identities.iter() {
        let Some(owner) = export.canonical.as_ref().filter(|owner| requested.contains(owner)) else {
            continue;
        };
        // The same exported nominal can have several visible aliases. Only its qualified provider binding
        // selects the semantic carrier; the bare-name cache may contain candidates from several packages.
        if crate::typechecker::split_canonical_public_library_type_name(binding).is_none() {
            continue;
        }
        if let Some(types) = checker.transitive_pub_types.get(binding) {
            for info in types.iter().filter_map(|layout| layout.get(checker)) {
                collect(owner, Nominal::Type(info), &mut members);
            }
        }
    }
    for (key, member) in members {
        if let Some(prior) = positions.identities.insert(key, member.clone())
            && prior != member
        {
            return Err(NodeBindingError::InvalidField(
                "conflicting package member declaration".into(),
            ));
        }
    }
    Ok(())
}

/// Bind the members of each requested sibling-module nominal that the checker reaches only transitively, as a
/// call's return type, to the declarations its current layout carries. A layout counts only when its members carry
/// the owner's own module origin, so a same-named nominal of another module never stands in for it.
fn bind_sibling_members(
    checker: &TypeChecker,
    requested: &std::collections::BTreeSet<CanonicalSymbolId>,
    members: &mut Vec<(String, CanonicalSymbolId)>,
) {
    let siblings = requested
        .iter()
        .filter(|owner| matches!(owner.origin, SymbolOrigin::Module(_)))
        .collect::<Vec<_>>();
    if siblings.is_empty() {
        return;
    }
    for (name, layouts) in checker.transitive_pub_types.iter() {
        let leaf = name.rsplit(['.', ':']).next().unwrap_or(name);
        for owner in siblings.iter().filter(|owner| owner.declaration_name == leaf) {
            for info in layouts.iter().filter_map(|layout| layout.get(checker)) {
                if declares_members_of(info, owner) {
                    collect(owner, Nominal::Type(info), members);
                }
            }
        }
    }
}

/// Whether a nominal layout's own members were declared in `owner`'s module.
fn declares_members_of(info: &TypeInfo, owner: &CanonicalSymbolId) -> bool {
    let same_origin = |identity: &CanonicalSymbolId| identity.origin == owner.origin;
    match info {
        TypeInfo::Model(model) => {
            model
                .fields
                .values()
                .filter_map(|field| field.identity.as_ref())
                .any(same_origin)
                || model
                    .methods
                    .values()
                    .filter_map(|method| method.identity.as_ref())
                    .any(same_origin)
        }
        TypeInfo::Class(class) => {
            class
                .fields
                .values()
                .filter_map(|field| field.identity.as_ref())
                .any(same_origin)
                || class
                    .methods
                    .values()
                    .filter_map(|method| method.identity.as_ref())
                    .any(same_origin)
        }
        TypeInfo::Enum(info) => info.variant_identities.values().any(same_origin),
        TypeInfo::Newtype(info) => info
            .methods
            .values()
            .filter_map(|method| method.identity.as_ref())
            .any(same_origin),
        _ => false,
    }
}

fn collect(owner: &CanonicalSymbolId, nominal: Nominal<'_>, identities: &mut Vec<(String, CanonicalSymbolId)>) {
    let mut record = |member: &CanonicalSymbolId, kind: SemanticSourceTargetKind| {
        if member.kind == kind
            && let Ok(identity) = stable_member_identity(member, owner, 0)
        {
            identities.push((identity, member.clone()));
        }
    };
    let (fields, properties, methods) = match nominal {
        Nominal::Type(TypeInfo::Model(info)) => (Some(&info.fields), Some(&info.properties), &info.methods),
        Nominal::Type(TypeInfo::Class(info)) => (Some(&info.fields), Some(&info.properties), &info.methods),
        Nominal::Type(TypeInfo::Newtype(info)) => (None, None, &info.methods),
        Nominal::Trait(info) => (None, Some(&info.properties), &info.methods),
        Nominal::Type(TypeInfo::Enum(info)) => {
            for variant in info.variant_identities.values() {
                record(variant, SemanticSourceTargetKind::Variant);
            }
            (None, None, &info.methods)
        }
        _ => return,
    };
    for field in fields
        .into_iter()
        .flat_map(|fields| fields.values())
        .filter_map(|field| field.identity.as_ref())
    {
        record(field, SemanticSourceTargetKind::Field);
    }
    for property in properties
        .into_iter()
        .flat_map(|properties| properties.values())
        .filter_map(|property| property.identity.as_ref())
    {
        record(property, SemanticSourceTargetKind::Property);
    }
    for method in methods.values().filter_map(|method| method.identity.as_ref()) {
        record(method, SemanticSourceTargetKind::Method);
    }
}
