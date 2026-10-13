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
use incan_lang::lang::stdlib;

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
            sdk_identity(identity).is_some()
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
    let roots = type_info
        .declarations
        .named_type_identities
        .values()
        .cloned()
        .chain(type_info.declarations.body_ir_native_type_identities.values().cloned())
        .chain(expression_leaves)
        .chain(signature_leaves)
        .filter(|identity| {
            matches!(
                identity.kind,
                SemanticSourceTargetKind::Model
                    | SemanticSourceTargetKind::Class
                    | SemanticSourceTargetKind::Enum
                    | SemanticSourceTargetKind::Newtype
                    | SemanticSourceTargetKind::Rusttype
            )
        })
        .filter_map(|identity| declaring_identity(&identity, cache))
        .collect::<BTreeSet<_>>();
    let mut retained = roots;
    let mut pending = retained.iter().cloned().collect::<Vec<_>>();
    while let Some(identity) = pending.pop() {
        for dependency in layout_dependencies(&identity, cache) {
            if retained.insert(dependency.clone()) {
                pending.push(dependency);
            }
        }
    }
    retained
}

/// Follow a checked facade binding to the declaring module only when its complete declaration identity agrees.
fn declaring_identity(identity: &CanonicalSymbolId, cache: &mut StdlibAstCache) -> Option<CanonicalSymbolId> {
    let published = sdk_identity(identity)?;
    let SymbolOrigin::Package { library, module_path } = &published.origin else {
        return None;
    };
    let path = std::iter::once(stdlib::STDLIB_ROOT.to_owned())
        .chain(module_path.iter().cloned())
        .collect::<Vec<_>>();
    let mut declared = cache.lookup_identity(&path, &identity.declaration_name)?;
    let SymbolOrigin::Module(declaring_path) = &declared.origin else {
        return None;
    };
    let declaring_path = declaring_path.strip_prefix(&[stdlib::STDLIB_ROOT.to_owned()])?.to_vec();
    let mut comparable = declared.clone();
    comparable.origin = identity.origin.clone();
    if comparable != *identity {
        return None;
    }
    declared.origin = SymbolOrigin::Package {
        library: library.clone(),
        module_path: declaring_path,
    };
    cache.callable_source_identity(&declared)?;
    Some(declared)
}

/// Rebase a checked stdlib source identity onto the SDK package assigned by the source catalog.
///
/// Source bindings, including facade annotations, can retain module origins even when the consumer links an SDK.
/// Other modules and Rust imports yield nothing; callers still validate the full declaration against source.
pub(super) fn sdk_identity(identity: &CanonicalSymbolId) -> Option<CanonicalSymbolId> {
    let SymbolOrigin::Module(path) = &identity.origin else {
        return matches!(identity.origin, SymbolOrigin::Package { .. }).then(|| identity.clone());
    };
    if !stdlib::is_any_stdlib_path(path) {
        return None;
    }
    let sources = crate::provider::stdlib_sources::StdlibSources::discover()?;
    let owner = sources.owner_of(path.get(1)?)?;
    let manifest = oven_model::manifest::ProjectManifest::load(&owner.project_root.join("loaf.toml")).ok()?;
    let library = manifest.project?.name?;
    let mut published = identity.clone();
    published.origin = SymbolOrigin::Package {
        library,
        module_path: path[1..].to_vec(),
    };
    Some(published)
}

/// Collect each nominal carried by an SDK declaration's fields and checked trait arguments in its declaring module.
///
/// Trait arguments can supply an error or item type absent from storage, such as ReaderChunks' IoError. Retaining
/// their layouts does not select an implementation or grant a call target; those remain separate checked facts.
fn layout_dependencies(identity: &CanonicalSymbolId, cache: &mut StdlibAstCache) -> Vec<CanonicalSymbolId> {
    let Some(source) = cache.callable_source_identity(identity) else {
        return Vec::new();
    };
    let SymbolOrigin::Module(path) = source.origin else {
        return Vec::new();
    };
    let Some(info) = cache.lookup_type(&path, &identity.declaration_name) else {
        return Vec::new();
    };
    let (mut types, adoptions) = match info {
        TypeInfo::Model(model) => (
            model.fields.into_values().map(|field| field.ty).collect::<Vec<_>>(),
            model.trait_adoptions,
        ),
        TypeInfo::Class(class) => (
            class.fields.into_values().map(|field| field.ty).collect(),
            class.trait_adoptions,
        ),
        TypeInfo::Enum(declaration) => (
            declaration.variant_fields.into_values().flatten().collect(),
            declaration.trait_adoptions,
        ),
        TypeInfo::Newtype(newtype) => (vec![newtype.underlying], newtype.trait_adoptions),
        _ => (Vec::new(), Vec::new()),
    };
    types.extend(adoptions.into_iter().flat_map(|adoption| adoption.type_args));
    let mut names = BTreeSet::new();
    for ty in types {
        super::library_calls::named_leaves(&semantic_type_from_resolved(&ty), &mut names);
    }
    let SymbolOrigin::Package { library, .. } = &identity.origin else {
        return Vec::new();
    };
    names
        .into_iter()
        .filter_map(|name| {
            let mut dependency = cache
                .lookup_identity(&path, &name)
                .or_else(|| cache.imported_type_identity(&path, &name))?;
            let SymbolOrigin::Module(owner) = &dependency.origin else {
                return None;
            };
            let owner = owner.strip_prefix(&[stdlib::STDLIB_ROOT.to_owned()])?.to_vec();
            dependency.origin = SymbolOrigin::Package {
                library: library.clone(),
                module_path: owner,
            };
            cache.callable_source_identity(&dependency)?;
            Some(dependency)
        })
        .collect()
}

/// Read the layout of every stdlib model, class or plain newtype one module uses whose declaration the stdlib source
/// confirms, plus opaque runtime types whose storage the surface registry assigns to the SDK.
pub(super) fn collect(type_info: &TypeCheckInfo) -> Vec<bir::NominalDeclaration> {
    let mut cache = StdlibAstCache::new();
    let mut identities = type_identities(type_info, &mut cache);
    if uses_implicit_field_info(type_info) {
        let path = vec!["std".to_owned(), "reflection".to_owned()];
        if let Some(identity) = cache.lookup_identity(&path, "FieldInfo")
            && let Some(identity) = declaring_identity(&identity, &mut cache)
        {
            identities.insert(identity);
        }
    }
    let mut declarations = identities
        .iter()
        .filter_map(|identity| match identity.kind {
            SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class => library_nominal(identity, &mut cache),
            SemanticSourceTargetKind::Newtype => library_newtype(identity, &mut cache),
            _ => None,
        })
        .chain(super::runtime_nominals::collect(type_info))
        .collect::<Vec<_>>();
    let native_owners = type_info
        .declarations
        .body_ir_native_type_identities
        .values()
        .collect::<BTreeSet<_>>();
    for identity in native_owners {
        if uses_native_carrier(type_info, &identity.declaration_name)
            && let Some(declaration) = bir::native_carrier_nominal(identity)
        {
            declarations.push(declaration);
        }
    }
    declarations
}

/// A native ABI is needed by a checked expression or declaration signature, including unused function parameters.
/// Checking supplies the active owner separately; this leaf walk never selects a carrier from a spelling alone.
/// Scan the original checked names as well as their projected storage, since a runtime token hides its nominal leaf.
fn uses_native_carrier(type_info: &TypeCheckInfo, name: &str) -> bool {
    let signatures = type_info.declarations.function_bindings.values().flat_map(|binding| {
        binding
            .params
            .iter()
            .map(|parameter| &parameter.ty)
            .chain(std::iter::once(&binding.return_type))
    });
    type_info.expressions.expr_types.values().chain(signatures).any(|ty| {
        let mut names = BTreeSet::new();
        super::library_calls::named_leaves(
            &super::parameter_types::checked_semantic_type(ty, type_info),
            &mut names,
        );
        super::library_calls::named_leaves(&super::semantic_type_from_resolved(ty), &mut names);
        names.contains(name)
    })
}

/// Compiler-provided reflection results name FieldInfo without a lexical import. A checked project owner always
/// wins over that implicit result spelling, so a user declaration cannot acquire the native metadata layout.
fn uses_implicit_field_info(type_info: &TypeCheckInfo) -> bool {
    if type_info.declarations.named_type_identities.contains_key("FieldInfo") {
        return false;
    }
    type_info.expressions.expr_types.values().any(|ty| {
        let mut names = BTreeSet::new();
        super::library_calls::named_leaves(&semantic_type_from_resolved(ty), &mut names);
        names.contains("FieldInfo")
    })
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
        native_item: String::new(),
        class_inheritance: None,
        native_type_arguments: Vec::new(),
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
        reflection_fields: None,
        fields: vec!["0".to_owned()],
        public: true,
        has_field_defaults: false,
        field_default_body: None,
        method_partial_defaults: Vec::new(),
        display_method: None,
        derives: newtype.derives,
        named_type_identities: Default::default(),
        type_parameters: Vec::new(),
        type_parameter_count: 0,
    })
}

/// Read one stdlib layout and its declared type parameters in declaration order, keeping the package identity for the
/// owner and rebasing each field identity onto it. A declaration the stdlib catalog does not own yields nothing.
fn library_nominal(identity: &CanonicalSymbolId, cache: &mut StdlibAstCache) -> Option<bir::NominalDeclaration> {
    if !matches!(identity.origin, SymbolOrigin::Package { .. }) {
        return None;
    }
    let source = cache.callable_source_identity(identity)?;
    let SymbolOrigin::Module(path) = &source.origin else {
        return None;
    };
    let info = cache
        .lookup_type(path, &identity.declaration_name)
        .or_else(|| cache.native_nominal_type(identity))?;
    let (order, fields, derives, type_parameters) = match info {
        TypeInfo::Model(model) => (model.field_order, model.fields, model.derives, model.type_params),
        TypeInfo::Class(class) if class.extends.is_none() => {
            (class.field_order, class.fields, class.derives, class.type_params)
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
        native_item: bir::native_nominal_item(identity).unwrap_or_default(),
        class_inheritance: None,
        native_type_arguments: Vec::new(),
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
        reflection_fields: None,
        fields: fields.into_iter().map(|(name, _)| name).collect(),
        public: true,
        has_field_defaults,
        field_default_body: None,
        method_partial_defaults: Vec::new(),
        display_method: None,
        derives,
        named_type_identities: Default::default(),
        type_parameter_count: type_parameters.len(),
        type_parameters,
    })
}
