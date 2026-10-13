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
use crate::symbols::{CallableParam, MethodInfo, SymbolKind, TypeInfo};
use crate::typechecker::stdlib_loader::StdlibAstCache;
use incan_lang::lang::stdlib;

/// One source-owned SDK method signature and the native instantiation its explicit trait target selects.
struct MethodSignature {
    /// Source nominal or trait owning the selected declaration.
    owner: String,
    /// Declared shared or exclusive receiver, absent for a static method.
    receiver: Option<IncanType>,
    /// Source ordinary parameters, before closing owner placeholders.
    parameters: Vec<CallableParam>,
    /// Declared result, before substituting Self or owner placeholders.
    return_type: IncanType,
    /// Owner binders that the concrete receiver closes.
    type_parameters: Vec<String>,
    /// Exact compiled trait item; empty uses the source owner's ordinary native method.
    native_item: Vec<String>,
    /// Explicit trait instantiation, Self first, independent of the source owner's binder count.
    native_type_arguments: Vec<IncanType>,
}

/// Collect a crate-call record for every stdlib function and model or class method one module resolves, including
/// declared type parameters. Nongeneric newtype methods keep their existing admission.
/// Checked iteration hooks contribute their own identities after written call-site facts have been restored.
pub(super) fn collect(type_info: &TypeCheckInfo) -> Vec<bir::StdlibDelegation> {
    let identities = type_info
        .references
        .resolved_identities
        .values()
        .chain(type_info.traits.provider_slot_identities.values())
        .chain(
            type_info
                .protocols
                .body_ir_iterations
                .values()
                .flat_map(|hooks| [&hooks.iter, &hooks.next])
                .filter_map(|hook| hook.canonical.as_ref()),
        )
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
    let mut calls: Vec<_> = identities
        .iter()
        .filter_map(|identity| library_call(identity, &mut cache))
        .collect();
    calls.extend(super::runtime_nominals::members(type_info));
    calls
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
    let mut native_type_arguments = Vec::new();
    let (parameters, return_type, type_parameters, parameter_defaults) = match identity.kind {
        SemanticSourceTargetKind::Method => {
            let MethodSignature {
                owner,
                receiver,
                parameters: params,
                return_type: result,
                type_parameters: names,
                native_item,
                native_type_arguments: native_args,
            } = method_signature(&published, &path, cache)?;
            if native_item.is_empty() {
                item.push(owner.clone());
                item.push(name.clone());
            } else {
                item = native_item;
            }
            native_type_arguments = native_args;
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
        native_type_arguments,
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
            .chain(call.native_type_arguments)
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
/// equal the checked identity exactly, and the owner type must itself be a declaration that SDK package owns. A
/// same-name overload is selected by that complete identity, never by the cache's single-method projection.
fn method_signature(
    identity: &CanonicalSymbolId,
    path: &[String],
    cache: &mut StdlibAstCache,
) -> Option<MethodSignature> {
    let source_origin = SymbolOrigin::Module(path.to_vec());
    let rebased = |declared: &CanonicalSymbolId| {
        let mut rebased = declared.clone();
        if rebased.origin == source_origin {
            rebased.origin = identity.origin.clone();
        }
        rebased
    };
    for (owner, info) in cache.list_types(path) {
        let (methods, overloads, names) = match &info {
            TypeInfo::Model(model) => (&model.methods, &model.method_overloads, &model.type_params),
            TypeInfo::Class(class) => (&class.methods, &class.method_overloads, &class.type_params),
            // A rusttype's methods belong to the Rust type it names, not to an item the SDK crate compiled.
            TypeInfo::Newtype(newtype) if !newtype.is_rusttype && newtype.type_params.is_empty() => {
                (&newtype.methods, &newtype.method_overloads, &newtype.type_params)
            }
            _ => continue,
        };
        let selected =
            |method: &&crate::symbols::MethodInfo| method.identity.as_ref().map(rebased).as_ref() == Some(identity);
        let Some(method) = overloads
            .get(&identity.declaration_name)
            .into_iter()
            .flat_map(|methods| methods.iter())
            .find(selected)
            .or_else(|| methods.get(&identity.declaration_name).filter(selected))
        else {
            continue;
        };
        let owner_identity = rebased(&cache.lookup_identity(path, &owner)?);
        cache.callable_source_identity(&owner_identity)?;
        if method.is_async || !method.has_body || !method.type_params.is_empty() {
            return None;
        }
        let concrete_owner = owner_type(&owner, names);
        let (native_item, native_type_arguments) = if method.trait_target.is_some() {
            native_trait_item(method, &concrete_owner, cache)?
        } else {
            (Vec::new(), Vec::new())
        };
        let receiver = method.receiver.as_ref().map(|receiver| {
            let owner_type = Box::new(owner_type(&owner, names));
            match receiver {
                ast::Receiver::Immutable => IncanType::Ref(owner_type),
                ast::Receiver::Mutable => IncanType::RefMut(owner_type),
            }
        });
        return Some(MethodSignature {
            owner,
            receiver,
            parameters: method.params.clone(),
            return_type: semantic_type_from_resolved(&method.return_type),
            type_parameters: names.clone(),
            native_item,
            native_type_arguments,
        });
    }
    trait_method_signature(identity, path, cache)
}

/// Name an SDK trait method by its exact declaring identity, retaining Self, trait, and method binders in native order.
/// Metadata validates the concrete receiver's implementation at the native call.
fn trait_method_signature(
    identity: &CanonicalSymbolId,
    path: &[String],
    cache: &mut StdlibAstCache,
) -> Option<MethodSignature> {
    for (owner, declaration) in cache.list_traits(path) {
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
        if declared != *identity || method.is_async {
            continue;
        }
        let mut owner_identity = cache.lookup_identity(path, &owner)?;
        owner_identity.origin = identity.origin.clone();
        cache.callable_source_identity(&owner_identity)?;
        let self_type = IncanType::TypeVar("Self".to_owned());
        let names = std::iter::once("Self".to_owned())
            .chain(declaration.type_params.iter().cloned())
            .chain(method.type_params.iter().cloned())
            .collect::<Vec<_>>();
        let receiver = method.receiver.as_ref().map(|receiver| match receiver {
            ast::Receiver::Immutable => IncanType::Ref(Box::new(self_type.clone())),
            ast::Receiver::Mutable => IncanType::RefMut(Box::new(self_type.clone())),
        });
        return Some(MethodSignature {
            owner,
            receiver,
            parameters: method.params.clone(),
            return_type: super::parameter_types::retain_parameter_type(
                self_as(&semantic_type_from_resolved(&method.return_type), &self_type),
                &names,
            ),
            type_parameters: names,
            native_item: Vec::new(),
            native_type_arguments: Vec::new(),
        });
    }
    None
}

/// Authenticate a checked trait dispatch against the SDK's declaring slot and retain its ordered trait arguments.
/// Explicit receivers retain their binder-aware checked Self type; synthetic nominal hooks keep their physical frame.
/// This is a Body IR-only projection: neither legacy resolution tables nor receiver types are changed.
pub(super) fn trait_dispatch(
    identity: &CanonicalSymbolId,
    dispatch: Option<&crate::typechecker::ResolvedMethodDispatch>,
    receiver_type: Option<IncanType>,
) -> Option<bir::StdlibTraitCall> {
    let crate::typechecker::ResolvedMethodDispatch::Trait { type_args, .. } = dispatch?;
    let published = super::library_nominals::sdk_identity(identity)?;
    let SymbolOrigin::Package { library, module_path } = &published.origin else {
        return None;
    };
    let path = std::iter::once(stdlib::STDLIB_ROOT.to_owned())
        .chain(module_path.iter().cloned())
        .collect::<Vec<_>>();
    let mut cache = StdlibAstCache::new();
    let signature = trait_method_signature(&published, &path, &mut cache)?;
    let declaration = cache.lookup_trait(&path, &signature.owner)?;
    if declaration.type_params.len() != type_args.len() {
        return None;
    }
    let mut trait_identity = cache.lookup_identity(&path, &signature.owner)?;
    trait_identity.origin = published.origin.clone();
    cache.callable_source_identity(&trait_identity)?;
    let native_item = std::iter::once(library.as_str())
        .chain(module_path.iter().map(String::as_str))
        .chain([signature.owner.as_str(), published.declaration_name.as_str()])
        .collect::<Vec<_>>()
        .join("::");
    Some(bir::StdlibTraitCall {
        trait_identity,
        native_item,
        type_arguments: type_args.iter().map(semantic_type_from_resolved).collect(),
        receiver_type,
    })
}

/// Resolve an explicit SDK implementation's native trait declaration and preserve its checked Self/trait arguments.
/// The catalog-owned trait and exact source slot must exist; unresolved aliases, missing binders, or a different
/// receiver contract cannot authorize an inherent-method approximation.
fn native_trait_item(
    method: &MethodInfo,
    owner: &IncanType,
    cache: &mut StdlibAstCache,
) -> Option<(Vec<String>, Vec<IncanType>)> {
    let bound = method.trait_target.as_ref()?;
    let path = bound.module_path.as_deref()?;
    let declaration = cache.lookup_trait(path, &bound.name)?;
    if declaration.type_params.len() != bound.type_args.len() {
        return None;
    }
    let name = &method.identity.as_ref()?.declaration_name;
    let slot = declaration.methods.get(name)?;
    if slot.receiver != method.receiver || !slot.type_params.is_empty() || slot.is_async {
        return None;
    }
    let identity = cache.lookup_identity(path, &bound.name)?;
    let published = super::library_nominals::sdk_identity(&identity)?;
    cache.callable_source_identity(&published)?;
    let SymbolOrigin::Package { library, module_path } = published.origin else {
        return None;
    };
    let item = std::iter::once(library)
        .chain(module_path)
        .chain([published.declaration_name, name.clone()])
        .collect();
    let arguments = std::iter::once(owner.clone())
        .chain(
            bound
                .type_args
                .iter()
                .map(|argument| self_as(&semantic_type_from_resolved(argument), owner)),
        )
        .collect();
    Some((item, arguments))
}

/// Retain owned SDK parameters and positional rest slots, whose compiled ABI is a single list of the element type.
/// Keyword rest remains refused; argument packing is owned by the existing checked ArgumentBinding.
fn owned_parameters(params: &[CallableParam], binders: &[String]) -> Option<Vec<IncanType>> {
    params
        .iter()
        .map(|param| {
            let ty = sdk_type(&param.ty, binders);
            match param.kind {
                ast::ParamKind::Normal => Some(ty),
                ast::ParamKind::RestPositional => Some(IncanType::Generic {
                    base: "List".to_owned(),
                    args: vec![ty],
                }),
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
