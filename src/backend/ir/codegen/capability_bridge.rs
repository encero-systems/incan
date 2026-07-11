//! Shared planning helpers for temporary source-owned capability bridges.

use std::collections::{HashMap, HashSet};

use crate::frontend::ast::{Declaration, ImportKind, Program};
use crate::library_manifest::{TraitExport, TypeBoundExport, TypeRef};
use incan_core::lang::trait_capabilities::{self, TraitCapabilityInfo, TraitCapabilityTypeArg};

/// Return whether one source program imports a capability contract or one of its trigger items.
pub(super) fn imports_contract(program: &Program, capability: &TraitCapabilityInfo) -> bool {
    program.declarations.iter().any(|decl| {
        let Declaration::Import(import) = &decl.node else {
            return false;
        };
        match &import.kind {
            ImportKind::Module(path) => {
                trait_capabilities::module_path_matches(capability, &path.segments)
                    || path.segments.split_last().is_some_and(|(item, module_path)| {
                        trait_capabilities::module_path_matches(capability, module_path)
                            && trait_capabilities::import_triggers_capability(capability, item)
                    })
            }
            ImportKind::From { module, items }
                if trait_capabilities::module_path_matches(capability, &module.segments) =>
            {
                items
                    .iter()
                    .any(|item| trait_capabilities::import_triggers_capability(capability, item.name.as_str()))
            }
            _ => false,
        }
    })
}

/// Return whether any source module in one compilation imports a capability contract.
pub(super) fn compilation_imports_contract(
    main: &Program,
    deps: &[(&str, &Program, Option<Vec<String>>)],
    capability: &TraitCapabilityInfo,
) -> bool {
    imports_contract(main, capability) || deps.iter().any(|(_, program, _)| imports_contract(program, capability))
}

/// Return lookup keys for a manifest trait, including its original source name after re-export aliasing.
pub(super) fn trait_export_lookup_keys(trait_export: &TraitExport) -> impl Iterator<Item = String> + '_ {
    std::iter::once(trait_export.name.clone()).chain(
        trait_export
            .source_name
            .iter()
            .filter(|source_name| *source_name != &trait_export.name)
            .cloned(),
    )
}

/// Return whether a serialized bound identifies one exact capability contract and argument list.
fn type_bound_matches_capability(bound: &TypeBoundExport, capability: &TraitCapabilityInfo) -> bool {
    let source_name = bound
        .source_name
        .as_deref()
        .unwrap_or_else(|| bound.name.rsplit('.').next().unwrap_or(bound.name.as_str()));
    source_name == capability.trait_name
        && bound
            .module_path
            .as_deref()
            .is_some_and(|path| trait_capabilities::module_path_matches(capability, path))
        && bound.type_args.len() == capability.required_type_args.len()
        && bound
            .type_args
            .iter()
            .zip(capability.required_type_args)
            .all(|(actual, required)| {
                matches!((actual, required), (TypeRef::Named { name }, TraitCapabilityTypeArg::Str) if name == "str")
            })
}

/// Return whether a serialized bound reaches one capability through exported source supertraits.
fn trait_bound_extends_capability(
    bound: &TypeBoundExport,
    traits: &HashMap<String, &TraitExport>,
    capability: &TraitCapabilityInfo,
) -> bool {
    let mut seen = HashSet::new();
    let mut work = vec![bound.clone()];
    while let Some(current) = work.pop() {
        let key = format!("{}<{:?}>", current.name, current.type_args);
        if !seen.insert(key) {
            continue;
        }
        let Some(trait_export) = traits.get(&current.name) else {
            continue;
        };
        let substitutions = trait_export
            .type_params
            .iter()
            .zip(&current.type_args)
            .map(|(param, arg)| (param.name.clone(), arg.clone()))
            .collect::<HashMap<_, _>>();
        for supertrait in &trait_export.supertraits {
            let mut instantiated = supertrait.clone();
            instantiated.type_args = instantiated
                .type_args
                .iter()
                .map(|arg| substitute_type_ref_params(arg, &substitutions))
                .collect();
            if type_bound_matches_capability(&instantiated, capability) {
                return true;
            }
            work.push(instantiated);
        }
    }
    false
}

/// Substitute exported type parameters while traversing a generic supertrait closure.
fn substitute_type_ref_params(ty: &TypeRef, substitutions: &HashMap<String, TypeRef>) -> TypeRef {
    match ty {
        TypeRef::TypeParam { name } => substitutions.get(name).cloned().unwrap_or_else(|| ty.clone()),
        TypeRef::Applied { name, args } => TypeRef::Applied {
            name: name.clone(),
            args: args
                .iter()
                .map(|arg| substitute_type_ref_params(arg, substitutions))
                .collect(),
        },
        TypeRef::Function { params, return_type } => TypeRef::Function {
            params: params
                .iter()
                .map(|param| substitute_type_ref_params(param, substitutions))
                .collect(),
            return_type: Box::new(substitute_type_ref_params(return_type, substitutions)),
        },
        TypeRef::TypeToken { inner } => TypeRef::TypeToken {
            inner: Box::new(substitute_type_ref_params(inner, substitutions)),
        },
        TypeRef::Tuple { elements } => TypeRef::Tuple {
            elements: elements
                .iter()
                .map(|element| substitute_type_ref_params(element, substitutions))
                .collect(),
        },
        TypeRef::Ref { inner } => TypeRef::Ref {
            inner: Box::new(substitute_type_ref_params(inner, substitutions)),
        },
        TypeRef::Named { .. } | TypeRef::SelfType | TypeRef::RustPath { .. } | TypeRef::Unknown => ty.clone(),
    }
}

/// Return whether a checked exported type explicitly adopts one capability contract.
pub(super) fn export_adopts_capability(
    trait_adoptions: &[TypeBoundExport],
    traits: &HashMap<String, &TraitExport>,
    capability: &TraitCapabilityInfo,
) -> bool {
    trait_adoptions.iter().any(|bound| {
        type_bound_matches_capability(bound, capability) || trait_bound_extends_capability(bound, traits, capability)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> Result<Program, String> {
        let tokens = crate::frontend::lexer::lex(source)
            .map_err(|errors| format!("capability import fixture should lex: {errors:?}"))?;
        crate::frontend::parser::parse(&tokens)
            .map_err(|errors| format!("capability import fixture should parse: {errors:?}"))
    }

    #[test]
    fn module_qualified_import_activates_string_conversion_capability() -> Result<(), String> {
        let program = parse("import std.traits.convert as convert\n")?;
        assert!(imports_contract(&program, trait_capabilities::string_try_from()));
        Ok(())
    }
}
