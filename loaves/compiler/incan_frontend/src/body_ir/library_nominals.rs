//! Retain the checked layouts of the stdlib models and classes a module names, owned by compiled SDK packages.
//!
//! A consumer that links the SDK crates names each type by its compiled item and never declares its storage, so
//! only the layout the stdlib source declares crosses this boundary; its methods and defaults stay with the crate.

use std::collections::BTreeSet;

use incan_semantics_core::{CanonicalSymbolId, CompilerNodeId, SemanticSourceTargetKind, SymbolOrigin};

use super::{IncanType, TypeCheckInfo, bir, semantic_type_from_resolved};
use crate::ast;
use crate::symbols::{FieldInfo, TypeInfo};
use crate::typechecker::stdlib_loader::StdlibAstCache;

/// Every package-origin model, class, enum or newtype one module uses: named by its bindings, carried by its checked
/// expressions, or named in a called stdlib function's or method's own module.
///
/// A stdlib type reaches a body without an import as part of another type, such as the error side of a stdlib
/// method's `Result`, so the named leaves of checked expression types and of called stdlib signatures count as well as
/// the module's bindings. Whether an identity belongs to a linked SDK package is decided where it is resolved.
pub(super) fn type_identities(type_info: &TypeCheckInfo, cache: &mut StdlibAstCache) -> BTreeSet<CanonicalSymbolId> {
    let expression_leaves = type_info
        .expressions
        .expression_type_identities
        .values()
        .flat_map(|leaves| leaves.values().cloned());
    let callees = type_info
        .references
        .resolved_identities
        .values()
        .filter(|identity| {
            matches!(identity.origin, SymbolOrigin::Package { .. })
                && matches!(
                    identity.kind,
                    SemanticSourceTargetKind::Function | SemanticSourceTargetKind::Method
                )
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    let signature_leaves = callees
        .iter()
        .flat_map(|callee| super::library_calls::signature_type_identities(callee, cache))
        .collect::<Vec<_>>();
    type_info
        .declarations
        .named_type_identities
        .values()
        .cloned()
        .chain(expression_leaves)
        .chain(signature_leaves)
        .filter(|identity| {
            matches!(identity.origin, SymbolOrigin::Package { .. })
                && matches!(
                    identity.kind,
                    SemanticSourceTargetKind::Model
                        | SemanticSourceTargetKind::Class
                        | SemanticSourceTargetKind::Enum
                        | SemanticSourceTargetKind::Newtype
                )
        })
        .collect()
}

/// Read the layout of every stdlib model, class or plain newtype one module uses whose declaration the stdlib source
/// confirms.
pub(super) fn collect(type_info: &TypeCheckInfo) -> Vec<bir::NominalDeclaration> {
    let mut cache = StdlibAstCache::new();
    type_identities(type_info, &mut cache)
        .iter()
        .filter_map(|identity| match identity.kind {
            SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class => library_nominal(identity, &mut cache),
            SemanticSourceTargetKind::Newtype => library_newtype(identity, &mut cache),
            _ => None,
        })
        .collect()
}

/// Read one plain stdlib newtype as a single public slot holding its underlying value, the slot its own identity
/// authorizes, as a source-local plain newtype's layout is.
///
/// A newtype whose construction runs a checked constructor or carries primitive constraints yields nothing, because
/// building its slot directly would skip that check; so does a rusttype, whose storage is the Rust type it names.
fn library_newtype(identity: &CanonicalSymbolId, cache: &mut StdlibAstCache) -> Option<bir::NominalDeclaration> {
    if !matches!(identity.origin, SymbolOrigin::Package { .. }) {
        return None;
    }
    let source = cache.callable_source_identity(identity)?;
    let SymbolOrigin::Module(path) = &source.origin else {
        return None;
    };
    let TypeInfo::Newtype(newtype) = cache.lookup_type(path, &identity.declaration_name)? else {
        return None;
    };
    if newtype.is_rusttype
        || !newtype.type_params.is_empty()
        || !newtype.constraints.is_empty()
        || crate::typechecker::TypeChecker::validated_newtype_ctor_name(&identity.declaration_name, &newtype).is_some()
    {
        return None;
    }
    let module = incan_semantics_core::canonical_module_identity(identity)?;
    Some(bir::NominalDeclaration {
        direct_declaration_id: CompilerNodeId::declaration_span(
            &module,
            identity.declaration_span.start,
            identity.declaration_span.end,
        ),
        canonical: identity.clone(),
        name: identity.declaration_name.clone(),
        field_identities: vec![identity.clone()],
        field_types: vec![semantic_type_from_resolved(&newtype.underlying)],
        field_public: vec![true],
        field_aliases: Vec::new(),
        fields: vec!["0".to_owned()],
        public: true,
        has_field_defaults: false,
        field_default_body: None,
        method_partial_defaults: Vec::new(),
        derives: newtype.derives,
        named_type_identities: Default::default(),
        type_parameters: Vec::new(),
        type_parameter_count: 0,
    })
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
    let has_field_defaults = fields.iter().any(|(_, field)| field.has_default);
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
        field_aliases: Vec::new(),
        fields: fields.into_iter().map(|(name, _)| name).collect(),
        public: true,
        has_field_defaults,
        field_default_body: None,
        method_partial_defaults: Vec::new(),
        derives,
        named_type_identities: Default::default(),
        type_parameters: Vec::new(),
        type_parameter_count: 0,
    })
}
