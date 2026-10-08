//! Standard-library nominals and callables compiled into active SDK components, retained as checked native facts.
//!
//! An SDK component is compiled into its provider crate, so a native consumer reaches its items by crate path rather
//! than lowering their bodies. These records carry what the checker proved about each referenced item: the provider
//! path, a nominal's field layout, and a callable's declared signature. Generic or inheriting owners, async and
//! trait-implementing callables, and anything else a record cannot describe exactly are omitted, so consumers refuse
//! them by name.

use super::{CanonicalSymbolId, TypeCheckInfo, ast, bir, semantic_type_from_resolved};
use crate::symbols::{CallableParam, FunctionInfo, MethodInfo, ResolvedType, SymbolKind, TypeInfo};
use crate::typechecker::stdlib_loader::StdlibAstCache;
use incan_semantics_core::{CompilerNodeId, SemanticSourceTargetKind, SymbolOrigin, encode_incan_symbol_identity};
use std::collections::BTreeSet;

/// One published SDK package location: the provider library, its module path, and the stdlib source module.
struct SdkPackage {
    library: String,
    module_path: Vec<String>,
    source_path: Vec<String>,
}

impl SdkPackage {
    /// Resolve a consumer identity to the SDK package that compiled it, refusing every non-SDK origin.
    fn of(identity: &CanonicalSymbolId) -> Option<Self> {
        let SymbolOrigin::Package { library, module_path } = &identity.origin else {
            return None;
        };
        Some(Self {
            library: library.clone(),
            module_path: module_path.clone(),
            source_path: StdlibAstCache::sdk_source_module(library, module_path)?,
        })
    }

    /// Present a stdlib source identity as the package identity a consumer of the compiled component references.
    fn published(&self, source: &CanonicalSymbolId) -> CanonicalSymbolId {
        let mut identity = source.clone();
        identity.origin = SymbolOrigin::Package {
            library: self.library.clone(),
            module_path: self.module_path.clone(),
        };
        identity
    }

    /// Name an item of this package's provider-crate module.
    fn item_path(&self, item: &str) -> String {
        format!("{}::{}::{item}", self.library, self.module_path.join("::"))
    }
}

/// Collect the SDK nominals named by checked types and the SDK callables selected by checked references.
pub(super) fn collect(
    type_info: &TypeCheckInfo,
    bodies: &[bir::Body],
) -> (Vec<bir::NominalDeclaration>, Vec<bir::SdkCallable>) {
    let mut cache = StdlibAstCache::new();
    let named = bodies
        .iter()
        .flat_map(|body| body.named_type_identities.values())
        .chain(type_info.declarations.named_type_identities.values())
        .cloned()
        .collect::<BTreeSet<_>>();
    let nominals = named
        .iter()
        .filter_map(|identity| nominal(identity, &mut cache))
        .collect();
    let referenced = type_info
        .references
        .resolved_identities
        .values()
        .cloned()
        .collect::<BTreeSet<_>>();
    let callables = referenced
        .iter()
        .filter_map(|identity| callable(identity, &mut cache))
        .collect();
    (nominals, callables)
}

/// Retain a nongeneric SDK model or class with its complete checked field layout in declaration order.
fn nominal(identity: &CanonicalSymbolId, cache: &mut StdlibAstCache) -> Option<bir::NominalDeclaration> {
    if !matches!(
        identity.kind,
        SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
    ) {
        return None;
    }
    let package = SdkPackage::of(identity)?;
    if package.published(&cache.lookup_identity(&package.source_path, &identity.declaration_name)?) != *identity {
        return None;
    }
    let (fields, order, derives) = match cache.lookup_type(&package.source_path, &identity.declaration_name)? {
        TypeInfo::Model(model) if model.type_params.is_empty() => (model.fields, model.field_order, model.derives),
        TypeInfo::Class(class) if class.type_params.is_empty() && class.extends.is_none() => {
            (class.fields, class.field_order, class.derives)
        }
        _ => return None,
    };
    let mut field_identities = Vec::new();
    let mut field_types = Vec::new();
    let mut field_public = Vec::new();
    for name in &order {
        let field = fields.get(name)?;
        field_identities.push(package.published(field.identity.as_ref()?));
        field_types.push(semantic_type_from_resolved(&field.ty));
        field_public.push(field.visibility == ast::Visibility::Public);
    }
    Some(bir::NominalDeclaration {
        direct_declaration_id: CompilerNodeId::declaration_span(
            &package.source_path.join("."),
            identity.declaration_span.start,
            identity.declaration_span.end,
        ),
        canonical: identity.clone(),
        name: identity.declaration_name.clone(),
        fields: order,
        field_identities,
        field_types,
        field_public,
        public: true,
        has_field_defaults: fields.values().any(|field| field.has_default),
        field_default_body: None,
        derives,
        named_type_identities: Default::default(),
        type_parameter_count: 0,
        native_path: Some(package.item_path(&identity.declaration_name)),
    })
}

/// Retain a synchronous SDK function or inherent method with its declared signature and projected provider symbol.
fn callable(identity: &CanonicalSymbolId, cache: &mut StdlibAstCache) -> Option<bir::SdkCallable> {
    let package = SdkPackage::of(identity)?;
    let symbol = encode_incan_symbol_identity(identity);
    match identity.kind {
        SemanticSourceTargetKind::Function => {
            let info = function_signature(cache, &package, identity)?;
            record(
                identity,
                package.item_path(&symbol),
                None,
                (info.type_params.as_slice(), info.params.as_slice(), &info.return_type),
                info.is_async,
            )
        }
        SemanticSourceTargetKind::Method => {
            let (owner, method) = method_signature(cache, &package, identity)?;
            if !method.has_body || method.trait_target.is_some() {
                return None;
            }
            let receiver = method.receiver.map(|receiver| match receiver {
                ast::Receiver::Immutable => bir::SdkReceiver::Shared,
                ast::Receiver::Mutable => bir::SdkReceiver::Mutable,
            });
            record(
                identity,
                package.item_path(&format!("{owner}::{symbol}")),
                receiver,
                (
                    method.type_params.as_slice(),
                    method.params.as_slice(),
                    &method.return_type,
                ),
                method.is_async,
            )
        }
        _ => None,
    }
}

/// Select the checked signature of exactly this function declaration, including one candidate of an overload set.
fn function_signature(
    cache: &mut StdlibAstCache,
    package: &SdkPackage,
    identity: &CanonicalSymbolId,
) -> Option<FunctionInfo> {
    match cache.lookup_function_symbol(&package.source_path, &identity.declaration_name)? {
        SymbolKind::Function(info) => {
            let source = cache.lookup_identity(&package.source_path, &identity.declaration_name)?;
            (package.published(&source) == *identity).then_some(info)
        }
        SymbolKind::FunctionOverloads(candidates) => candidates
            .into_iter()
            .find(|candidate| {
                candidate
                    .identity
                    .as_ref()
                    .is_some_and(|source| package.published(source) == *identity)
            })
            .map(|candidate| candidate.info),
        _ => None,
    }
}

/// Find the nongeneric, non-inheriting owner declaring exactly this method, with the method's checked signature.
fn method_signature(
    cache: &mut StdlibAstCache,
    package: &SdkPackage,
    identity: &CanonicalSymbolId,
) -> Option<(String, MethodInfo)> {
    cache
        .list_types(&package.source_path)
        .into_iter()
        .find_map(|(owner, info)| {
            let (methods, overloads) = match info {
                TypeInfo::Model(model) if model.type_params.is_empty() => (model.methods, model.method_overloads),
                TypeInfo::Class(class) if class.type_params.is_empty() && class.extends.is_none() => {
                    (class.methods, class.method_overloads)
                }
                _ => return None,
            };
            methods
                .into_values()
                .chain(overloads.into_values().flatten())
                .find(|method| {
                    method
                        .identity
                        .as_ref()
                        .is_some_and(|source| package.published(source) == *identity)
                })
                .map(|method| (owner, method))
        })
}

/// Build one callable record; async callables need a native future protocol and are omitted.
fn record(
    identity: &CanonicalSymbolId,
    native_path: String,
    receiver: Option<bir::SdkReceiver>,
    (type_params, params, return_type): (&[String], &[CallableParam], &ResolvedType),
    is_async: bool,
) -> Option<bir::SdkCallable> {
    if is_async {
        return None;
    }
    Some(bir::SdkCallable {
        canonical: identity.clone(),
        native_path,
        receiver,
        type_parameters: type_params.to_vec(),
        parameters: params
            .iter()
            .map(|parameter| semantic_type_from_resolved(&parameter.ty))
            .collect(),
        return_type: semantic_type_from_resolved(return_type),
    })
}
