//! Direct-only selection for the frozen source-owned string conversion bridge.

use super::{IncanPrimitiveType, IncanType, TypeCheckInfo, ast, bir, parameter_types};
use crate::{
    api_metadata::{ApiDeclaration, ApiTrait},
    library_manifest::{LibraryManifest, ParamKindExport, TypeRef},
    symbols::ResolvedType,
    typechecker::stdlib_loader::StdlibAstCache,
};
use incan_lang::lang::trait_capabilities;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};

/// Retain per-owner checked support and exclude source overrides; [`finalize_candidates`] closes the whole local chain.
pub(super) fn automatic_candidate(
    source: &ast::NewtypeDecl,
    owner: &CanonicalSymbolId,
    info: &TypeCheckInfo,
) -> bir::NewtypeStringConversion {
    if !matches!(owner.origin, SymbolOrigin::Module(_)) {
        return bir::NewtypeStringConversion::Unselected;
    }
    let Some(facts) = info.declarations.newtype_construction.get(&source.name) else {
        return bir::NewtypeStringConversion::Unselected;
    };
    let underlying = parameter_types::checked_parameter_type(&facts.underlying, info, &facts.type_params);
    let supported = matches!(
        underlying,
        IncanType::Primitive(IncanPrimitiveType::Int | IncanPrimitiveType::Str)
    ) || matches!(&underlying, IncanType::Named(name) if info.declarations.named_type_identities.get(name).is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Newtype && identity.declaration_name == *name));
    if source.is_rusttype
        || !source.type_params.is_empty()
        || !source.traits.is_empty()
        || !facts.type_params.is_empty()
        || facts.checked_constructor.is_some()
        || facts.checked_constructor_identity.is_some()
        || !facts.constraints.is_empty()
        || !facts.supports_string_conversion
        || !supported
    {
        return bir::NewtypeStringConversion::Unselected;
    }
    bir::NewtypeStringConversion::Automatic
}

/// Close per-owner attestations over only the original source registry before any SDK or dependency layouts enter it.
/// Ineligible chains keep their ordinary storage; transported Automatic facts remain subject to strict consumer checks.
pub(super) fn finalize_candidates(declarations: &mut [bir::NominalDeclaration]) {
    let eligible = (0..declarations.len())
        .map(|index| closed_candidate(declarations, index))
        .collect::<Vec<_>>();
    for (declaration, eligible) in declarations.iter_mut().zip(eligible) {
        if !eligible && declaration.newtype_string_conversion == bir::NewtypeStringConversion::Automatic {
            declaration.newtype_string_conversion = bir::NewtypeStringConversion::Unselected;
        }
    }
}

/// Follow exact same-module canonical newtype edges without allowing a foreign, open, missing or cyclic inner owner.
fn closed_candidate(declarations: &[bir::NominalDeclaration], mut index: usize) -> bool {
    let mut visited = std::collections::BTreeSet::new();
    loop {
        if !visited.insert(index) {
            return false;
        }
        let Some(owner) = declarations.get(index) else {
            return false;
        };
        if owner.newtype_string_conversion != bir::NewtypeStringConversion::Automatic
            || owner.canonical.kind != SemanticSourceTargetKind::Newtype
            || !matches!(owner.canonical.origin, SymbolOrigin::Module(_))
            || owner.name != owner.canonical.declaration_name
            || owner.type_parameter_count != 0
            || !owner.type_parameters.is_empty()
        {
            return false;
        }
        match owner.field_types.as_slice() {
            [IncanType::Primitive(IncanPrimitiveType::Int | IncanPrimitiveType::Str)] => return true,
            [IncanType::Named(name)] => {
                let Some(identity) = owner.named_type_identities.get(name) else {
                    return false;
                };
                if identity.origin != owner.canonical.origin
                    || identity.kind != SemanticSourceTargetKind::Newtype
                    || identity.declaration_name != *name
                {
                    return false;
                }
                let mut matches = declarations
                    .iter()
                    .enumerate()
                    .filter(|(_, declaration)| &declaration.canonical == identity);
                let Some((next, _)) = matches.next() else {
                    return false;
                };
                if matches.next().is_some() {
                    return false;
                }
                index = next;
            }
            _ => return false,
        }
    }
}

/// Project exactly the frozen emitter's source import grammar using the common capability registry.
/// This direct-only reader does not update parser activation, checker support, or legacy bridge configuration.
fn imports_string_conversion_contract(program: &ast::Program) -> bool {
    let capability = trait_capabilities::string_try_from();
    program.declarations.iter().any(|declaration| {
        let ast::Declaration::Import(import) = &declaration.node else {
            return false;
        };
        match &import.kind {
            ast::ImportKind::Module(path) => {
                trait_capabilities::module_path_matches(capability, &path.segments)
                    || path.segments.split_last().is_some_and(|(item, parent)| {
                        trait_capabilities::module_path_matches(capability, parent)
                            && trait_capabilities::import_triggers_capability(capability, item)
                    })
                    || path.segments == ["std", "environ"]
            }
            ast::ImportKind::From { module, items } => {
                (trait_capabilities::module_path_matches(capability, &module.segments)
                    && items
                        .iter()
                        .any(|item| trait_capabilities::import_triggers_capability(capability, &item.name)))
                    || (module.segments == ["std", "environ"] && items.iter().any(|item| item.name == "get_as"))
            }
            _ => false,
        }
    })
}

/// Authenticate the selected publication against the analysis source cache before retaining its exact trait slot.
fn selected_string_conversion_contract(
    identity: &CanonicalSymbolId,
    manifest: &LibraryManifest,
    cache: &mut StdlibAstCache,
) -> Option<bir::StringConversionContract> {
    let capability = trait_capabilities::string_try_from();
    let source = cache.callable_source_identity(identity)?;
    let SymbolOrigin::Module(path) = &source.origin else {
        return None;
    };
    if identity.kind != SemanticSourceTargetKind::Trait
        || identity.namespace != SymbolNamespace::OrdinaryLexical
        || identity.scope_discriminant.is_some()
        || identity.declaration_name != capability.trait_name
        || !trait_capabilities::module_path_matches(capability, path)
        || !matches!(&identity.origin, SymbolOrigin::Package { .. })
    {
        return None;
    }
    let declaration = cache.lookup_trait(path, capability.trait_name)?;
    let syntax = cache.lookup_trait_decl(path, capability.trait_name)?;
    if !exact_conversion_source(&syntax) {
        return None;
    }
    let [binder] = declaration.type_params.as_slice() else {
        return None;
    };
    let method = declaration.methods.get("try_from")?;
    let [parameter] = method.params.as_slice() else {
        return None;
    };
    let mut method_identity = method.identity.clone()?;
    if declaration.methods.len() != 1
        || !declaration.supertraits.is_empty()
        || !declaration.requires.is_empty()
        || !declaration.properties.is_empty()
        || !declaration.method_aliases.is_empty()
        || !method.type_params.is_empty()
        || !method.type_param_bounds.is_empty()
        || !method.type_param_bound_details.is_empty()
        || method.trait_target.is_some()
        || method.receiver.is_some()
        || method.is_async
        || method.has_body
        || method.alias_of.is_some()
        || parameter.kind != ast::ParamKind::Normal
        || parameter.has_default
        || parameter.is_mut
        || parameter.ty != ResolvedType::TypeVar(binder.clone())
        || method.return_type
            != ResolvedType::Generic("Result".to_owned(), vec![ResolvedType::SelfType, ResolvedType::Str])
        || method_identity.kind != SemanticSourceTargetKind::Method
        || method_identity.namespace != SymbolNamespace::Member
        || method_identity.origin != source.origin
        || method_identity.scope_discriminant.is_some()
        || method_identity.declaration_name != "try_from"
        || method_identity.declaration_span.start < source.declaration_span.start
        || method_identity.declaration_span.end > source.declaration_span.end
    {
        return None;
    }
    method_identity.origin = identity.origin.clone();
    let published = published_conversion_trait(identity, manifest)?;
    let [published_method] = published.methods.as_slice() else {
        return None;
    };
    let actual_method = published_method
        .canonical
        .as_ref()
        .and_then(|canonical| canonical.hydrate())?;
    if actual_method != method_identity || published_method.name != "try_from" {
        return None;
    }
    Some(bir::StringConversionContract::Selected(Box::new(
        bir::StringConversionSelection {
            trait_identity: identity.clone(),
            method_identity: actual_method,
        },
    )))
}

/// Retain the consumer's complete source import union once, authenticating only its active SDK publication.
/// Missing active publication/API authority returns None; a freshly computed inactive union remains distinct.
pub fn compilation_string_conversion_contract<'program>(
    programs: impl IntoIterator<Item = &'program ast::Program>,
    providers: &crate::provider::ProviderPlan,
    analysis_cache: &StdlibAstCache,
) -> Option<bir::StringConversionContract> {
    if !programs.into_iter().any(imports_string_conversion_contract) {
        return Some(bir::StringConversionContract::Inactive);
    }
    let capability = trait_capabilities::string_try_from();
    let module = capability
        .module_path
        .iter()
        .map(|segment| (*segment).to_owned())
        .collect::<Vec<_>>();
    let provider = providers.active_sdk_provider_for_module(&module)?;
    let manifest = provider.manifest.as_deref()?;
    let public_path = std::iter::once(provider.identity.name.clone())
        .chain(module.into_iter().skip(1))
        .chain(std::iter::once(capability.trait_name.to_owned()))
        .collect::<Vec<_>>();
    let identity = manifest
        .contract_metadata
        .identity_graph
        .canonical_for_public_path(&public_path)?;
    selected_string_conversion_contract(&identity, manifest, &mut analysis_cache.clone())
}

/// Check source-only structure before using the cache's deliberately lightweight, name-indexed method summary.
fn exact_conversion_source(declaration: &ast::TraitDecl) -> bool {
    let [method] = declaration.methods.as_slice() else {
        return false;
    };
    let [parameter] = method.node.params.as_slice() else {
        return false;
    };
    let [decorator] = method.node.decorators.as_slice() else {
        return false;
    };
    declaration.visibility == ast::Visibility::Public
        && declaration.name == "TryFrom"
        && declaration.decorators.is_empty()
        && declaration.traits.is_empty()
        && declaration.properties.is_empty()
        && declaration.method_aliases.is_empty()
        && declaration.method_partials.is_empty()
        && method.node.name == "try_from"
        && method.node.receiver.is_none()
        && method
            .node
            .receiver_binding
            .as_ref()
            .is_some_and(|binding| binding.node == "cls")
        && method.node.trait_target.is_none()
        && method.node.type_params.is_empty()
        && !method.node.is_async()
        && method.node.body.is_none()
        && parameter.node.kind == ast::ParamKind::Normal
        && !parameter.node.is_mut
        && parameter.node.default.is_none()
        && decorator.node.path.segments == ["classmethod"]
        && decorator.node.name == "classmethod"
        && !decorator.node.is_call
        && decorator.node.type_args.is_empty()
        && decorator.node.args.is_empty()
}

/// Join the selected canonical trait's direct source path to its unique checked API and require its owned ABI.
fn published_conversion_trait<'manifest>(
    identity: &CanonicalSymbolId,
    manifest: &'manifest LibraryManifest,
) -> Option<&'manifest ApiTrait> {
    let path = manifest
        .contract_metadata
        .identity_graph
        .declaration_source_path(identity)?;
    let (name, module_path) = path.split_last()?;
    let mut candidates = manifest
        .contract_metadata
        .api
        .as_ref()?
        .modules
        .iter()
        .filter(|module| module.module_path == module_path)
        .flat_map(|module| &module.declarations)
        .filter_map(|declaration| match declaration {
            ApiDeclaration::Trait(value)
                if value.name == *name
                    && value.anchor.span.start == identity.declaration_span.start
                    && value.anchor.span.end == identity.declaration_span.end =>
            {
                Some(value)
            }
            _ => None,
        });
    let declaration = candidates.next()?;
    if candidates.next().is_some() || !declaration.requires.is_empty() || !declaration.supertraits.is_empty() {
        return None;
    }
    let [binder] = declaration.type_params.as_slice() else {
        return None;
    };
    let [method] = declaration.methods.as_slice() else {
        return None;
    };
    let [parameter] = method.params.as_slice() else {
        return None;
    };
    let expected = TypeRef::Applied {
        name: "Result".to_owned(),
        args: vec![
            TypeRef::SelfType,
            TypeRef::Named {
                name: "str".to_owned(),
                origin: None,
            },
        ],
        origin: None,
    };
    (!binder.name.is_empty()
        && binder.bounds.is_empty()
        && method.type_params.is_empty()
        && method.receiver.is_none()
        && !method.is_async
        && !method.has_body
        && method.alias_of.is_none()
        && parameter.kind == ParamKindExport::Normal
        && !parameter.has_default
        && !parameter.is_mut
        && parameter.ty
            == TypeRef::TypeParam {
                name: binder.name.clone(),
            }
        && method.return_type == expected)
        .then_some(declaration)
}
