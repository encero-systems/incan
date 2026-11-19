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
/// An imported callable's return type may carry a package member without introducing a lexical name.
pub(super) fn bind_transitive(
    checker: &TypeChecker,
    positions: &mut Positions,
    fields: &Fields,
) -> Result<(), NodeBindingError> {
    let requested = fields
        .iter()
        .filter(|row| row.len() == 8 && row[0] == "identity-context")
        .filter_map(|row| positions.canonical_type(&row[4]).ok())
        .filter(|owner| matches!(owner.origin, SymbolOrigin::Package { .. }))
        .collect::<std::collections::BTreeSet<_>>();
    if requested.is_empty() {
        return Ok(());
    }
    let mut members = Vec::new();
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
            for info in types {
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

/// Resolve a missing payload only through the exact direct callee selected by the expression.
pub(super) fn bind_result_types(
    checker: &TypeChecker,
    positions: &mut Positions,
    fields: &Fields,
) -> Result<(), NodeBindingError> {
    let mut signatures = BTreeMap::new();
    for row in fields {
        if row.len() != 4 || row[0] != "type-identity" || positions.canonical_type(&row[3]).is_ok() {
            continue;
        }
        let span = positions.span(&row[1])?;
        let Some(callee) = positions.nodes.direct_callee(&row[1]) else {
            continue;
        };
        let anchor = positions.nodes.anchor(callee)?.path;
        let key = ["reference", anchor.as_str(), "read"];
        let Ok(slot) = fields.binary_search_by(|candidate| candidate.iter().take(3).map(String::as_str).cmp(key))
        else {
            continue;
        };
        let Some(canonical) = fields[slot].get(3).and_then(|stable| positions.identities.get(stable)) else {
            continue;
        };
        let SymbolOrigin::Module(module) = &canonical.origin else {
            continue;
        };
        let name = &canonical.declaration_name;
        if canonical.kind != SemanticSourceTargetKind::Function {
            continue;
        }
        let module = module.join("_");
        if checker
            .dependency_direct_member_identities
            .get(&module)
            .and_then(|members| members.get(name))
            != Some(canonical)
        {
            continue;
        }
        let Some(SymbolKind::Function(signature)) = checker
            .dependency_direct_member_symbols
            .get(&module)
            .and_then(|members| members.get(name))
        else {
            continue;
        };
        let path = read_type_identity_path(&row[2])?;
        let native = local_restore::native_component_item(&signature.return_type, &path)
            .and_then(incan_semantics_core::body_ir::rust_item_identity);
        let identity = native.or_else(|| {
            let leaves = signatures.entry(canonical).or_insert_with(|| {
                let mut identities = BTreeMap::new();
                checker.collect_expression_type_identities(&signature.return_type, &mut Vec::new(), &mut identities);
                identities
            });
            leaves.get(&path).cloned()
        });
        if let Some(identity) = identity
            && identity.kind == SemanticSourceTargetKind::RustItem
            && StableDeclarationId::from_canonical(&identity, None, None)
                .is_some_and(|id| id.render_compact() == row[3])
        {
            positions.source_results.insert((span, path), identity);
        }
    }
    Ok(())
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
