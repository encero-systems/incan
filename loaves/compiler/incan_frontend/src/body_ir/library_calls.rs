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

use super::{IncanType, TypeCheckInfo, bir, semantic_type_from_resolved};
use crate::ast;
use crate::symbols::{CallableParam, ResolvedType, SymbolKind, TypeInfo};
use crate::typechecker::stdlib_loader::StdlibAstCache;
use incan_lang::lang::stdlib;

/// Collect a crate-call record for every stdlib function, generic ones included, and every nongeneric model, class or
/// newtype method one module resolves.
pub(super) fn collect(type_info: &TypeCheckInfo) -> Vec<bir::StdlibDelegation> {
    let identities = type_info
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
    let mut cache = StdlibAstCache::new();
    identities
        .iter()
        .filter_map(|identity| library_call(identity, &mut cache))
        .collect()
}

/// Build one record after confirming the stdlib source declares exactly this identity.
fn library_call(identity: &CanonicalSymbolId, cache: &mut StdlibAstCache) -> Option<bir::StdlibDelegation> {
    let SymbolOrigin::Package { library, module_path } = &identity.origin else {
        return None;
    };
    let path = std::iter::once(stdlib::STDLIB_ROOT.to_owned())
        .chain(module_path.iter().cloned())
        .collect::<Vec<_>>();
    let mut item = std::iter::once(library.clone())
        .chain(module_path.iter().cloned())
        .collect::<Vec<_>>();
    let (parameters, return_type, type_parameters) = match identity.kind {
        SemanticSourceTargetKind::Method => {
            let (owner, receiver, params, result) = method_signature(identity, &path, cache)?;
            item.push(owner.clone());
            item.push(identity.declaration_name.clone());
            let mut parameters = receiver.into_iter().collect::<Vec<_>>();
            parameters.extend(owned_parameters(&params)?);
            (
                parameters,
                self_as(&semantic_type_from_resolved(&result), &owner),
                Vec::new(),
            )
        }
        _ => {
            cache.callable_source_identity(identity)?;
            let SymbolKind::Function(function) = cache.lookup_function_symbol(&path, &identity.declaration_name)?
            else {
                return None;
            };
            if function.is_async || function.emitted_name.is_some() {
                return None;
            }
            // A free function compiles under its full canonical identity, which the consumer's checked identity is.
            // A generic one keeps its type parameters, which each call instantiates with its checked type arguments.
            item.push(encode_incan_symbol_identity(identity));
            (
                owned_parameters(&function.params)?,
                semantic_type_from_resolved(&function.return_type),
                function.type_params.clone(),
            )
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
    })
}

/// The stdlib models, classes, enums and newtypes a called stdlib function or method names in its signature, declared
/// in its own module, under the callee's package origin.
///
/// A name in a stdlib signature is resolved where the stdlib source wrote it, never in the consumer's scope, so a
/// type that reaches the consumer only through a call, such as a method's `Result` error, keeps its identity.
pub(super) fn signature_type_identities(
    callee: &CanonicalSymbolId,
    cache: &mut StdlibAstCache,
) -> Vec<CanonicalSymbolId> {
    let SymbolOrigin::Package { module_path, .. } = &callee.origin else {
        return Vec::new();
    };
    let Some(call) = library_call(callee, cache) else {
        return Vec::new();
    };
    let path = std::iter::once(stdlib::STDLIB_ROOT.to_owned())
        .chain(module_path.iter().cloned())
        .collect::<Vec<_>>();
    let mut names = BTreeSet::new();
    for ty in call.parameters.iter().chain(std::iter::once(&call.return_type)) {
        named_leaves(ty, &mut names);
    }
    names
        .into_iter()
        .filter_map(|name| {
            let mut identity = cache.lookup_identity(&path, &name)?;
            if identity.origin != SymbolOrigin::Module(path.clone())
                || !matches!(
                    identity.kind,
                    SemanticSourceTargetKind::Model
                        | SemanticSourceTargetKind::Class
                        | SemanticSourceTargetKind::Enum
                        | SemanticSourceTargetKind::Newtype
                )
            {
                return None;
            }
            identity.origin = callee.origin.clone();
            Some(identity)
        })
        .collect()
}

/// Collect the nominal names a type mentions, through generic arguments, tuples and references.
fn named_leaves(ty: &IncanType, names: &mut BTreeSet<String>) {
    match ty {
        IncanType::Named(name) => {
            names.insert(name.clone());
        }
        IncanType::Generic { args, .. } | IncanType::Tuple(args) => {
            for arg in args {
                named_leaves(arg, names);
            }
        }
        IncanType::Ref(inner) | IncanType::RefMut(inner) => named_leaves(inner, names),
        _ => {}
    }
}

/// Find the nongeneric model, class or newtype declaring this method and return its owner name, receiver parameter,
/// ordinary parameters and result. A static method has no receiver parameter.
///
/// The stdlib source records each method under its source module; rebased onto the consumer's package origin it must
/// equal the checked identity exactly, and the owner type must itself be a declaration that SDK package owns.
fn method_signature(
    identity: &CanonicalSymbolId,
    path: &[String],
    cache: &mut StdlibAstCache,
) -> Option<(String, Option<IncanType>, Vec<CallableParam>, ResolvedType)> {
    let source_origin = SymbolOrigin::Module(path.to_vec());
    let rebased = |declared: &CanonicalSymbolId| {
        let mut rebased = declared.clone();
        if rebased.origin == source_origin {
            rebased.origin = identity.origin.clone();
        }
        rebased
    };
    for (owner, info) in cache.list_types(path) {
        let (methods, generic) = match &info {
            TypeInfo::Model(model) => (&model.methods, !model.type_params.is_empty()),
            TypeInfo::Class(class) => (&class.methods, !class.type_params.is_empty()),
            // A rusttype's methods belong to the Rust type it names, not to an item the SDK crate compiled.
            TypeInfo::Newtype(newtype) if !newtype.is_rusttype => (&newtype.methods, !newtype.type_params.is_empty()),
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
        if generic || method.is_async || !method.has_body || !method.type_params.is_empty() {
            return None;
        }
        let receiver = method.receiver.as_ref().map(|receiver| {
            let owner_type = Box::new(IncanType::Named(owner.clone()));
            match receiver {
                ast::Receiver::Immutable => IncanType::Ref(owner_type),
                ast::Receiver::Mutable => IncanType::RefMut(owner_type),
            }
        });
        return Some((owner, receiver, method.params.clone(), method.return_type.clone()));
    }
    None
}

/// Keep only ordinary parameters, each passed owned as the SDK crate declares it.
fn owned_parameters(params: &[CallableParam]) -> Option<Vec<IncanType>> {
    params
        .iter()
        .map(|param| (param.kind == ast::ParamKind::Normal).then(|| semantic_type_from_resolved(&param.ty)))
        .collect()
}

/// Name `Self` in a method result by its owner, as the compiled item's signature does.
fn self_as(ty: &IncanType, owner: &str) -> IncanType {
    match ty {
        IncanType::SelfType => IncanType::Named(owner.to_owned()),
        IncanType::Generic { base, args } => IncanType::Generic {
            base: base.clone(),
            args: args.iter().map(|arg| self_as(arg, owner)).collect(),
        },
        other => other.clone(),
    }
}
