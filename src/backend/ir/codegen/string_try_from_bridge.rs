//! `TryFrom[str]` bridge planning for compiler-provided primitive and newtype conversions.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::frontend::ast::{Declaration, ImportKind, Program};
use crate::frontend::library_manifest_index::{LibraryManifestIndex, LibraryManifestIndexEntry};
use crate::library_manifest::{LibraryManifest, NewtypeExport, TypeRef};
use incan_core::lang::trait_capabilities;
use incan_core::lang::trait_capabilities::TraitCapabilityType;

use super::capability_bridge;
use crate::backend::ir::emit::{ExternalStringTryFromKind, ExternalStringTryFromType};

/// Return whether a program imports the source-owned `TryFrom` contract that carries string conversion support.
pub(super) fn imports_std_string_try_from_contract(program: &Program) -> bool {
    capability_bridge::imports_contract(program, trait_capabilities::string_try_from())
        || imports_std_environ_typed_accessors(program)
}

/// Return whether any source module in one compilation imports the string conversion contract.
pub(super) fn compilation_imports_std_string_try_from_contract(
    main: &Program,
    deps: &[(&str, &Program, Option<Vec<String>>)],
) -> bool {
    imports_std_string_try_from_contract(main)
        || deps
            .iter()
            .any(|(_, program, _)| imports_std_string_try_from_contract(program))
}

/// Return whether a program imports the stdlib surface that consumes `TryFrom[str]` bounds on caller-defined types.
///
/// `std.environ.get_as()` owns that bound inside the compiled artifact. A caller that imports the module may therefore
/// need a compiler-provided local-newtype implementation even when it does not spell `TryFrom` in its own source.
fn imports_std_environ_typed_accessors(program: &Program) -> bool {
    program.declarations.iter().any(|decl| {
        let Declaration::Import(import) = &decl.node else {
            return false;
        };
        match &import.kind {
            ImportKind::Module(path) => path.segments == ["std", "environ"],
            ImportKind::From { module, items } if module.segments == ["std", "environ"] => {
                items.iter().any(|item| item.name == "get_as")
            }
            _ => false,
        }
    })
}

/// Convert a manifest primitive into a registry-supported string conversion target.
fn supported_primitive(ty: &TypeRef) -> bool {
    let resolved = crate::library_manifest::resolved_type_from_manifest_type_ref(ty);
    let capability_type = match resolved {
        crate::frontend::symbols::ResolvedType::Int => TraitCapabilityType::Int,
        crate::frontend::symbols::ResolvedType::Float => TraitCapabilityType::Float,
        crate::frontend::symbols::ResolvedType::Bool => TraitCapabilityType::Bool,
        crate::frontend::symbols::ResolvedType::Str => TraitCapabilityType::Str,
        crate::frontend::symbols::ResolvedType::Numeric(id) => TraitCapabilityType::Numeric(id),
        _ => return false,
    };
    trait_capabilities::supports_type(trait_capabilities::string_try_from(), capability_type)
}

/// Return whether one exported type reference has a package-reconstructable string conversion path.
fn external_type_ref_supports_string_conversion(
    ty: &TypeRef,
    newtypes: &HashMap<String, &NewtypeExport>,
    explicit_adopters: &HashSet<String>,
    visiting: &mut HashSet<String>,
) -> bool {
    if supported_primitive(ty) {
        return true;
    }
    if matches!(ty, TypeRef::TypeParam { .. }) {
        return true;
    }
    let name = match ty {
        TypeRef::Named { name } | TypeRef::Applied { name, .. } => name,
        _ => return false,
    };
    if explicit_adopters.contains(name) {
        return true;
    }
    let Some(underlying) = newtypes.get(name) else {
        return false;
    };
    if underlying.is_rusttype || !visiting.insert(name.clone()) {
        return false;
    }
    let supported =
        external_type_ref_supports_string_conversion(&underlying.underlying, newtypes, explicit_adopters, visiting);
    visiting.remove(name);
    supported
}

/// Collect consumer-side adapters for public types in loaded `.incnlib` dependencies.
fn external_string_try_from_types(index: Option<&Arc<LibraryManifestIndex>>) -> Vec<ExternalStringTryFromType> {
    let Some(index) = index else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for dependency_key in index.known_libraries() {
        let Some(LibraryManifestIndexEntry::Loaded { manifest, .. }) = index.get(&dependency_key) else {
            continue;
        };
        out.extend(external_string_try_from_types_for_manifest(&dependency_key, manifest));
    }
    out
}

/// Collect adapters from one checked library manifest.
fn external_string_try_from_types_for_manifest(
    dependency_key: &str,
    manifest: &LibraryManifest,
) -> Vec<ExternalStringTryFromType> {
    let traits = manifest
        .exports
        .traits
        .iter()
        .flat_map(|trait_export| {
            capability_bridge::trait_export_lookup_keys(trait_export).map(move |key| (key, trait_export))
        })
        .collect::<HashMap<_, _>>();
    let mut explicit_adopters = HashSet::new();
    let mut out = Vec::new();
    let preferred_public_names = manifest
        .contract_metadata
        .identity_graph
        .exports
        .iter()
        .fold(HashMap::<Vec<String>, Vec<String>>::new(), |mut grouped, identity| {
            let source_identity = identity.target_path().unwrap_or(identity.source_path.as_slice());
            grouped
                .entry(source_identity.to_vec())
                .or_default()
                .push(identity.public_name.clone());
            grouped
        })
        .into_iter()
        .map(|(source_identity, mut public_names)| {
            public_names.sort();
            public_names.dedup();
            let source_name = source_identity.last();
            let preferred = source_name
                .and_then(|source_name| public_names.iter().find(|name| *name == source_name))
                .cloned()
                .unwrap_or_else(|| public_names[0].clone());
            (source_identity, preferred)
        })
        .collect::<HashMap<_, _>>();
    let bridge_is_preferred = |public_name: &str| {
        let Some(identity) = manifest
            .contract_metadata
            .identity_graph
            .entry_for_public_name(public_name)
        else {
            return true;
        };
        let source_identity = identity.target_path().unwrap_or(identity.source_path.as_slice());
        preferred_public_names
            .get(source_identity)
            .is_none_or(|preferred| preferred == public_name)
    };

    macro_rules! collect_explicit {
        ($exports:expr) => {
            for export in $exports {
                if capability_bridge::export_adopts_capability(
                    &export.trait_adoptions,
                    &traits,
                    trait_capabilities::string_try_from(),
                ) {
                    explicit_adopters.insert(export.name.clone());
                    if !bridge_is_preferred(&export.name) {
                        continue;
                    }
                    out.push(ExternalStringTryFromType {
                        dependency_key: dependency_key.to_string(),
                        name: export.name.clone(),
                        type_params: export.type_params.clone(),
                        kind: ExternalStringTryFromKind::Explicit,
                    });
                }
            }
        };
    }

    collect_explicit!(&manifest.exports.models);
    collect_explicit!(&manifest.exports.classes);
    collect_explicit!(&manifest.exports.enums);
    collect_explicit!(&manifest.exports.newtypes);

    let newtypes = manifest
        .exports
        .newtypes
        .iter()
        .map(|newtype| (newtype.name.clone(), newtype))
        .collect::<HashMap<_, _>>();
    for newtype in &manifest.exports.newtypes {
        if newtype.is_rusttype || explicit_adopters.contains(&newtype.name) || !bridge_is_preferred(&newtype.name) {
            continue;
        }
        if !external_type_ref_supports_string_conversion(
            &newtype.underlying,
            &newtypes,
            &explicit_adopters,
            &mut HashSet::from([newtype.name.clone()]),
        ) {
            continue;
        }
        out.push(ExternalStringTryFromType {
            dependency_key: dependency_key.to_string(),
            name: newtype.name.clone(),
            type_params: newtype.type_params.clone(),
            kind: ExternalStringTryFromKind::Newtype {
                underlying: newtype.underlying.clone(),
                checked_constructor: newtype.checked_constructor.clone(),
                constraints: newtype
                    .constraints
                    .iter()
                    .map(|constraint| constraint.to_checked())
                    .collect(),
            },
        });
    }
    out
}

/// Generated bridge configuration shared by every emitter in one compilation.
#[derive(Debug, Clone)]
pub(super) struct StringTryFromBridgeConfig {
    pub(super) emit_local_newtype_impls: bool,
    pub(super) external_types: Vec<ExternalStringTryFromType>,
}

impl StringTryFromBridgeConfig {
    /// Build a bridge configuration for generated internal modules.
    pub(super) fn for_internal_module(uses_contract: bool) -> Self {
        Self {
            emit_local_newtype_impls: uses_contract,
            external_types: Vec::new(),
        }
    }

    /// Build the crate-root configuration where consumer-side dependency adapters live.
    pub(super) fn for_crate_root(uses_contract: bool, index: Option<&Arc<LibraryManifestIndex>>) -> Self {
        if !uses_contract {
            return Self::for_internal_module(false);
        }
        Self {
            emit_local_newtype_impls: true,
            external_types: external_string_try_from_types(index),
        }
    }
}
