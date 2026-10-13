//! Describe each called stdlib function and method by the compiled SDK crate item that implements it.
//!
//! A consumer that links the SDK crates calls these items directly. The signature recorded here is the one the SDK
//! crate compiled: owned parameters, a shared receiver for `self` and an exclusive one for `mut self`. The record is a
//! claim the consumer must still check against native metadata, so a convention this collector gets wrong refuses
//! instead of miscompiling.

use std::collections::BTreeSet;

use incan_semantics_core::{
    CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin,
    encode_incan_symbol_identity,
};

use super::{IncanType, TypeCheckInfo, bir, library_defaults, semantic_type_from_resolved};
use crate::ast;
use crate::symbols::{CallableParam, SymbolKind, TypeInfo};
use crate::typechecker::stdlib_loader::StdlibAstCache;
use incan_lang::lang::stdlib;

/// Collect a crate-call record for every stdlib function and model or class method one module resolves, including
/// declared type parameters. Nongeneric newtype methods keep their existing admission.
pub(super) fn collect(type_info: &TypeCheckInfo) -> Vec<bir::StdlibDelegation> {
    let identities = type_info
        .references
        .resolved_identities
        .values()
        .filter(|identity| {
            super::library_nominals::sdk_identity(identity).is_some()
                && matches!(
                    identity.kind,
                    SemanticSourceTargetKind::Function | SemanticSourceTargetKind::Method
                )
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut cache = StdlibAstCache::new();
    identities
        .iter()
        .filter_map(|identity| library_call(identity, &mut cache))
        .collect()
}

/// Build one record after confirming the stdlib source declares exactly this identity, with what each of its parameters
/// takes when a call omits it.
fn library_call(identity: &CanonicalSymbolId, cache: &mut StdlibAstCache) -> Option<bir::StdlibDelegation> {
    let published = super::library_nominals::sdk_identity(identity)?;
    let SymbolOrigin::Package { library, module_path } = &published.origin else {
        return None;
    };
    let path = std::iter::once(stdlib::STDLIB_ROOT.to_owned())
        .chain(module_path.iter().cloned())
        .collect::<Vec<_>>();
    let mut item = std::iter::once(library.clone())
        .chain(module_path.iter().cloned())
        .collect::<Vec<_>>();
    let name = &identity.declaration_name;
    let (parameters, return_type, type_parameters, parameter_defaults) = match identity.kind {
        SemanticSourceTargetKind::Method => {
            let (owner, receiver, params, result, names) = method_signature(&published, &path, cache)?;
            item.push(owner.clone());
            item.push(name.clone());
            let ordinary = owned_parameters(&params, &names)?;
            let defaults = library_defaults::method_defaults(&path, &owner, name, receiver.is_some(), &ordinary, cache);
            let mut parameters = receiver.into_iter().collect::<Vec<_>>();
            parameters.extend(ordinary);
            (
                parameters,
                self_as(&result, &owner_type(&owner, &names)),
                names,
                defaults,
            )
        }
        _ => {
            // One overload of a same-name set is selected by its own declaration span and records no defaults; any
            // other function has the module's single declaration under its name.
            let (function, single) = match cache.callable_source_identity(&published) {
                Some(_) => match cache.lookup_function_symbol(&path, name)? {
                    SymbolKind::Function(function) => (function, true),
                    _ => return None,
                },
                None => (cache.overload_function_info(&published)?, false),
            };
            // A free function compiles under its full canonical identity, which the consumer's checked identity is:
            // an overload's names its own declaration span, so each overload is its own crate item. A generic one
            // keeps its type parameters, which each call instantiates with its checked type arguments.
            item.push(encode_incan_symbol_identity(&published));
            let parameters = owned_parameters(&function.params, &function.type_params)?;
            let defaults = if single {
                library_defaults::function_defaults(&path, name, &parameters, cache)
            } else {
                Vec::new()
            };
            let output = sdk_type(&function.return_type, &function.type_params);
            let output = if function.is_async {
                IncanType::Generic {
                    base: "Awaitable".into(),
                    args: vec![output],
                }
            } else {
                output
            };
            (parameters, output, function.type_params.clone(), defaults)
        }
    };
    Some(bir::StdlibDelegation {
        canonical: identity.clone(),
        target: CanonicalSymbolId {
            namespace: SymbolNamespace::OrdinaryLexical,
            declaration_name: item.last()?.clone(),
            origin: SymbolOrigin::RustCrate(item),
            kind: SemanticSourceTargetKind::RustItem,
            scope_discriminant: None,
            declaration_span: HirSourceSpan::new(0, 0),
        },
        parameters,
        return_type,
        type_parameters,
        parameter_defaults,
    })
}

/// The stdlib models, classes, enums and newtypes a called stdlib function or method names in its signature, declared
/// in its own module or imported there from another module of the same package, under that package's origin.
///
/// A name in a stdlib signature is resolved where the stdlib source wrote it, never in the consumer's scope, so a
/// type that reaches the consumer only through a call, such as a method's `Result` error, keeps its identity. An
/// imported name is the declaration its import names, and must be one the callee's package owns.
pub(super) fn signature_type_identities(
    callee: &CanonicalSymbolId,
    cache: &mut StdlibAstCache,
) -> Vec<CanonicalSymbolId> {
    let Some(published) = super::library_nominals::sdk_identity(callee) else {
        return Vec::new();
    };
    let SymbolOrigin::Package { library, module_path } = &published.origin else {
        return Vec::new();
    };
    let path = std::iter::once(stdlib::STDLIB_ROOT.to_owned())
        .chain(module_path.iter().cloned())
        .collect::<Vec<_>>();
    let types = match library_call(callee, cache) {
        Some(call) => call
            .parameters
            .into_iter()
            .chain(std::iter::once(call.return_type))
            .collect(),
        None => trait_signature_types(&published, &path, cache),
    };
    let mut names = BTreeSet::new();
    for ty in &types {
        named_leaves(ty, &mut names);
    }
    names
        .into_iter()
        .filter_map(|name| {
            let mut identity = cache
                .lookup_identity(&path, &name)
                .or_else(|| cache.imported_type_identity(&path, &name))?;
            if !matches!(
                identity.kind,
                SemanticSourceTargetKind::Model
                    | SemanticSourceTargetKind::Class
                    | SemanticSourceTargetKind::Enum
                    | SemanticSourceTargetKind::Newtype
            ) {
                return None;
            }
            let SymbolOrigin::Module(declaring) = &identity.origin else {
                return None;
            };
            let (root, declaring_path) = declaring.split_first()?;
            if root != stdlib::STDLIB_ROOT {
                return None;
            }
            identity.origin = SymbolOrigin::Package {
                library: library.clone(),
                module_path: declaring_path.to_vec(),
            };
            cache.callable_source_identity(&identity)?;
            Some(identity)
        })
        .collect()
}

/// Retain a trait method's signature types even when its call needs concrete-adopter dispatch rather than a crate
/// delegation. Matching the complete checked method identity keeps same-named methods on other traits out.
fn trait_signature_types(identity: &CanonicalSymbolId, path: &[String], cache: &mut StdlibAstCache) -> Vec<IncanType> {
    if identity.kind != SemanticSourceTargetKind::Method {
        return Vec::new();
    }
    for (_, declaration) in cache.list_traits(path) {
        let Some(method) = declaration.methods.get(&identity.declaration_name) else {
            continue;
        };
        let Some(mut declared) = method.identity.clone() else {
            continue;
        };
        if declared.origin != SymbolOrigin::Module(path.to_vec()) {
            continue;
        }
        declared.origin = identity.origin.clone();
        if declared != *identity {
            continue;
        }
        return method
            .params
            .iter()
            .map(|param| semantic_type_from_resolved(&param.ty))
            .chain(std::iter::once(semantic_type_from_resolved(&method.return_type)))
            .collect();
    }
    Vec::new()
}

/// Collect the nominal names a type mentions, through generic arguments, tuples and references.
pub(super) fn named_leaves(ty: &IncanType, names: &mut BTreeSet<String>) {
    match ty {
        IncanType::Named(name) => {
            names.insert(name.clone());
        }
        IncanType::Generic { base, args } => {
            names.insert(base.clone());
            for arg in args {
                named_leaves(arg, names);
            }
        }
        IncanType::Tuple(args) => {
            for arg in args {
                named_leaves(arg, names);
            }
        }
        IncanType::Ref(inner) | IncanType::RefMut(inner) => named_leaves(inner, names),
        _ => {}
    }
}

/// Find the model, class, nongeneric newtype or nongeneric trait declaring this method and return its owner name,
/// receiver parameter, ordinary parameters, result and owner binders. A static method has no receiver parameter.
///
/// The stdlib source records each method under its source module; rebased onto the consumer's package origin it must
/// equal the checked identity exactly, and the owner type must itself be a declaration that SDK package owns.
fn method_signature(
    identity: &CanonicalSymbolId,
    path: &[String],
    cache: &mut StdlibAstCache,
) -> Option<(String, Option<IncanType>, Vec<CallableParam>, IncanType, Vec<String>)> {
    let source_origin = SymbolOrigin::Module(path.to_vec());
    let rebased = |declared: &CanonicalSymbolId| {
        let mut rebased = declared.clone();
        if rebased.origin == source_origin {
            rebased.origin = identity.origin.clone();
        }
        rebased
    };
    for (owner, info) in cache.list_types(path) {
        let (methods, names) = match &info {
            TypeInfo::Model(model) => (&model.methods, &model.type_params),
            TypeInfo::Class(class) => (&class.methods, &class.type_params),
            // A rusttype's methods belong to the Rust type it names, not to an item the SDK crate compiled.
            TypeInfo::Newtype(newtype) if !newtype.is_rusttype && newtype.type_params.is_empty() => {
                (&newtype.methods, &newtype.type_params)
            }
            _ => continue,
        };
        let Some(method) = methods.get(&identity.declaration_name) else {
            continue;
        };
        if method.identity.as_ref().map(rebased).as_ref() != Some(identity) {
            continue;
        }
        let owner_identity = rebased(&cache.lookup_identity(path, &owner)?);
        cache.callable_source_identity(&owner_identity)?;
        if method.is_async || !method.has_body || !method.type_params.is_empty() || method.trait_target.is_some() {
            return None;
        }
        let receiver = method.receiver.as_ref().map(|receiver| {
            let owner_type = Box::new(owner_type(&owner, names));
            match receiver {
                ast::Receiver::Immutable => IncanType::Ref(owner_type),
                ast::Receiver::Mutable => IncanType::RefMut(owner_type),
            }
        });
        return Some((
            owner,
            receiver,
            method.params.clone(),
            semantic_type_from_resolved(&method.return_type),
            names.clone(),
        ));
    }
    trait_method_signature(identity, path, cache)
}

/// Name a nongeneric SDK trait method by the trait that owns its checked identity, retaining Self as the trait
/// item's single type parameter. Metadata validates the concrete receiver's implementation at the native call.
fn trait_method_signature(
    identity: &CanonicalSymbolId,
    path: &[String],
    cache: &mut StdlibAstCache,
) -> Option<(String, Option<IncanType>, Vec<CallableParam>, IncanType, Vec<String>)> {
    for (owner, declaration) in cache.list_traits(path) {
        if !declaration.type_params.is_empty() {
            continue;
        }
        let Some(method) = declaration.methods.get(&identity.declaration_name) else {
            continue;
        };
        let Some(mut declared) = method.identity.clone() else {
            continue;
        };
        if declared.origin != SymbolOrigin::Module(path.to_vec()) {
            continue;
        }
        declared.origin = identity.origin.clone();
        if declared != *identity || method.is_async || !method.type_params.is_empty() {
            continue;
        }
        let mut owner_identity = cache.lookup_identity(path, &owner)?;
        owner_identity.origin = identity.origin.clone();
        cache.callable_source_identity(&owner_identity)?;
        let self_type = IncanType::TypeVar("Self".to_owned());
        let receiver = match method.receiver.as_ref()? {
            ast::Receiver::Immutable => IncanType::Ref(Box::new(self_type.clone())),
            ast::Receiver::Mutable => IncanType::RefMut(Box::new(self_type.clone())),
        };
        return Some((
            owner,
            Some(receiver),
            method.params.clone(),
            self_as(&semantic_type_from_resolved(&method.return_type), &self_type),
            vec!["Self".to_owned()],
        ));
    }
    None
}

/// Keep only ordinary parameters, each passed owned as the SDK declares it, with native carriers and active binders.
fn owned_parameters(params: &[CallableParam], binders: &[String]) -> Option<Vec<IncanType>> {
    params
        .iter()
        .map(|param| {
            let ty = sdk_type(&param.ty, binders);
            match param.kind {
                ast::ParamKind::Normal => Some(ty),
                _ => None,
            }
        })
        .collect()
}

/// Retain native SDK surface carriers and declaration-owned binders without consulting consumer source identities.
fn sdk_type(ty: &crate::symbols::ResolvedType, binders: &[String]) -> IncanType {
    let ty = super::runtime_nominals::project_type(semantic_type_from_resolved(ty), &TypeCheckInfo::default());
    super::parameter_types::retain_parameter_type(ty, binders)
}

/// Name `Self` in a method result by its owner, as the compiled item's signature does.
fn self_as(ty: &IncanType, owner: &IncanType) -> IncanType {
    match ty {
        IncanType::SelfType => owner.clone(),
        IncanType::Generic { base, args } => IncanType::Generic {
            base: base.clone(),
            args: args.iter().map(|arg| self_as(arg, owner)).collect(),
        },
        other => other.clone(),
    }
}

/// Keep a method owner's declared placeholders until its checked receiver or static call closes them.
fn owner_type(owner: &str, names: &[String]) -> IncanType {
    if names.is_empty() {
        IncanType::Named(owner.to_owned())
    } else {
        IncanType::Generic {
            base: owner.to_owned(),
            args: names.iter().cloned().map(IncanType::TypeVar).collect(),
        }
    }
}

#[cfg(test)]
mod tests;
