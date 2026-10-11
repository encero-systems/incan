//! Retain the checked layouts of the stdlib models and classes a module names, owned by compiled SDK packages.
//!
//! A consumer that links the SDK crates names each type by its compiled item and never declares its storage, so
//! only the layout the stdlib source declares crosses this boundary; its methods and defaults stay with the crate.

use incan_semantics_core::{CanonicalSymbolId, CompilerNodeId, SemanticSourceTargetKind, SymbolOrigin};

use super::{IncanType, TypeCheckInfo, bir, semantic_type_from_resolved};
use crate::ast;
use crate::symbols::{FieldInfo, TypeInfo};
use crate::typechecker::stdlib_loader::StdlibAstCache;

/// Collect every named SDK-package model or class whose declaration the stdlib source confirms, once per identity.
pub(super) fn collect(type_info: &TypeCheckInfo) -> Vec<bir::NominalDeclaration> {
    let identities = type_info
        .declarations
        .named_type_identities
        .values()
        .filter(|identity| {
            matches!(
                identity.kind,
                SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
            )
        })
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let mut cache = StdlibAstCache::new();
    identities
        .iter()
        .filter_map(|identity| library_nominal(identity, &mut cache))
        .collect()
}

/// Read one nongeneric stdlib layout in declaration order, keeping the consumer's package identity for the owner
/// and rebasing each field identity onto it. A declaration the stdlib catalog does not own yields nothing.
fn library_nominal(identity: &CanonicalSymbolId, cache: &mut StdlibAstCache) -> Option<bir::NominalDeclaration> {
    if !matches!(identity.origin, SymbolOrigin::Package { .. }) {
        return None;
    }
    let source = cache.callable_source_identity(identity)?;
    let SymbolOrigin::Module(path) = &source.origin else {
        return None;
    };
    let (order, fields, derives) = match cache.lookup_type(path, &identity.declaration_name)? {
        TypeInfo::Model(model) if model.type_params.is_empty() => (model.field_order, model.fields, model.derives),
        TypeInfo::Class(class) if class.type_params.is_empty() && class.extends.is_none() => {
            (class.field_order, class.fields, class.derives)
        }
        _ => return None,
    };
    let fields = order
        .into_iter()
        .map(|name| fields.get(&name).cloned().map(|field| (name, field)))
        .collect::<Option<Vec<(String, FieldInfo)>>>()?;
    // Each field keeps the identity the stdlib declared, moved onto the package origin the consumer checked against.
    let rebase = |field: &FieldInfo| {
        let mut rebased = field.identity.clone()?;
        if rebased.origin != source.origin {
            return None;
        }
        rebased.origin = identity.origin.clone();
        Some(rebased)
    };
    let field_identities = fields
        .iter()
        .map(|(_, field)| rebase(field))
        .collect::<Option<Vec<_>>>()?;
    let module = incan_semantics_core::canonical_module_identity(identity)?;
    Some(bir::NominalDeclaration {
        direct_declaration_id: CompilerNodeId::declaration_span(
            &module,
            identity.declaration_span.start,
            identity.declaration_span.end,
        ),
        canonical: identity.clone(),
        name: identity.declaration_name.clone(),
        field_identities,
        field_types: fields
            .iter()
            .map(|(_, field)| semantic_type_from_resolved(&field.ty))
            .collect::<Vec<IncanType>>(),
        field_public: fields
            .iter()
            .map(|(_, field)| field.visibility == ast::Visibility::Public)
            .collect(),
        fields: fields.into_iter().map(|(name, _)| name).collect(),
        public: true,
        has_field_defaults: false,
        field_default_body: None,
        derives,
        named_type_identities: Default::default(),
        type_parameters: Vec::new(),
        type_parameter_count: 0,
    })
}
