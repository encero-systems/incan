//! OrdinalKey bridge planning for generated IR emission.

use std::collections::HashMap;

use crate::frontend::ast::Program;
use crate::frontend::library_manifest_index::{LibraryManifestIndex, LibraryManifestIndexEntry};
use crate::library_manifest::{EnumValueExport, EnumValueTypeExport};
use incan_core::lang::trait_capabilities;

use super::capability_bridge;
use crate::backend::ir::decl::{IrEnumValue, IrEnumValueType};
use crate::backend::ir::emit::{ExternalOrdinalCustomKey, ExternalOrdinalValueEnum};

/// Return whether a program imports the stdlib ordinal-map contract.
pub(super) fn imports_std_ordinal_contract(program: &Program) -> bool {
    capability_bridge::imports_contract(program, trait_capabilities::stable_ordinal_key())
}

/// Return whether any module in the current compilation needs value-enum `OrdinalKey` impls.
pub(super) fn compilation_imports_std_ordinal_contract(
    main: &Program,
    deps: &[(&str, &Program, Option<Vec<String>>)],
) -> bool {
    capability_bridge::compilation_imports_contract(main, deps, trait_capabilities::stable_ordinal_key())
}

/// Collect public scalar value enums from loaded `.incnlib` dependencies.
fn external_ordinal_value_enums(index: Option<&LibraryManifestIndex>) -> Vec<ExternalOrdinalValueEnum> {
    let Some(index) = index else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for dependency_key in index.known_libraries() {
        let Some(LibraryManifestIndexEntry::Loaded { manifest, metadata }) = index.get(&dependency_key) else {
            continue;
        };
        for enum_export in &manifest.exports.enums {
            let Some(value_type) = enum_export.value_type else {
                continue;
            };
            let value_type = match value_type {
                EnumValueTypeExport::Str => IrEnumValueType::String,
                EnumValueTypeExport::Int => IrEnumValueType::Int,
            };
            let mut values = Vec::new();
            let mut complete = true;
            for variant in &enum_export.variants {
                let Some(value) = &variant.value else {
                    complete = false;
                    break;
                };
                values.push(match value {
                    EnumValueExport::Str(value) => IrEnumValue::String(value.clone()),
                    EnumValueExport::Int(value) => IrEnumValue::Int(*value),
                });
            }
            if !complete {
                continue;
            }
            out.push(ExternalOrdinalValueEnum {
                dependency_key: dependency_key.clone(),
                name: enum_export.name.clone(),
                type_identity: enum_export
                    .ordinal_type_identity
                    .clone()
                    .unwrap_or_else(|| format!("{}.{}", metadata.manifest_name, enum_export.name)),
                value_type,
                values,
            });
        }
    }
    out
}

/// Return whether any exported trait adoption satisfies the std `OrdinalKey` contract.
fn export_adopts_ordinal_key(
    trait_adoptions: &[crate::library_manifest::TypeBoundExport],
    traits: &HashMap<String, &crate::library_manifest::TraitExport>,
) -> bool {
    capability_bridge::export_adopts_capability(trait_adoptions, traits, trait_capabilities::stable_ordinal_key())
}

/// Return whether a manifest method set exposes a source method or its generated alias.
fn export_methods_include(methods: &[crate::library_manifest::MethodExport], name: &str) -> bool {
    methods
        .iter()
        .any(|method| method.name == name || method.alias_of.as_deref() == Some(name))
}

/// Build custom-key bridge metadata for one exported concrete type when it adopts `OrdinalKey`.
fn external_ordinal_custom_key(
    dependency_key: &str,
    name: &str,
    type_params: &[crate::library_manifest::TypeParamExport],
    trait_adoptions: &[crate::library_manifest::TypeBoundExport],
    methods: &[crate::library_manifest::MethodExport],
    traits: &HashMap<String, &crate::library_manifest::TraitExport>,
) -> Option<ExternalOrdinalCustomKey> {
    if !type_params.is_empty() || !export_adopts_ordinal_key(trait_adoptions, traits) {
        return None;
    }
    let hooks = trait_capabilities::stable_ordinal_key().bridge_hooks?;
    Some(ExternalOrdinalCustomKey {
        dependency_key: dependency_key.to_string(),
        name: name.to_string(),
        has_ordinal_hash: export_methods_include(methods, hooks.hash_method),
        has_ordinal_bytes_equal: export_methods_include(methods, hooks.bytes_equal_method),
    })
}

/// Collect public user-authored `OrdinalKey` adopters from loaded `.incnlib` dependencies.
fn external_ordinal_custom_keys(index: Option<&LibraryManifestIndex>) -> Vec<ExternalOrdinalCustomKey> {
    let Some(index) = index else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for dependency_key in index.known_libraries() {
        let Some(LibraryManifestIndexEntry::Loaded { manifest, .. }) = index.get(&dependency_key) else {
            continue;
        };
        let traits = manifest
            .exports
            .traits
            .iter()
            .flat_map(|trait_export| {
                capability_bridge::trait_export_lookup_keys(trait_export).map(move |key| (key, trait_export))
            })
            .collect::<HashMap<_, _>>();
        for model in &manifest.exports.models {
            if let Some(key) = external_ordinal_custom_key(
                &dependency_key,
                &model.name,
                &model.type_params,
                &model.trait_adoptions,
                &model.methods,
                &traits,
            ) {
                out.push(key);
            }
        }
        for class in &manifest.exports.classes {
            if let Some(key) = external_ordinal_custom_key(
                &dependency_key,
                &class.name,
                &class.type_params,
                &class.trait_adoptions,
                &class.methods,
                &traits,
            ) {
                out.push(key);
            }
        }
        for newtype in &manifest.exports.newtypes {
            if let Some(key) = external_ordinal_custom_key(
                &dependency_key,
                &newtype.name,
                &newtype.type_params,
                &newtype.trait_adoptions,
                &newtype.methods,
                &traits,
            ) {
                out.push(key);
            }
        }
        for enum_export in &manifest.exports.enums {
            if enum_export.value_type.is_some() {
                continue;
            }
            if let Some(key) = external_ordinal_custom_key(
                &dependency_key,
                &enum_export.name,
                &enum_export.type_params,
                &enum_export.trait_adoptions,
                &enum_export.methods,
                &traits,
            ) {
                out.push(key);
            }
        }
    }
    out
}

#[derive(Debug, Clone)]
pub(super) struct OrdinalBridgeConfig {
    pub(super) emit_std_ordinal_value_enum_impls: bool,
    pub(super) external_value_enums: Vec<ExternalOrdinalValueEnum>,
    pub(super) external_custom_keys: Vec<ExternalOrdinalCustomKey>,
}

impl OrdinalBridgeConfig {
    /// Build a bridge configuration for generated internal modules.
    pub(super) fn for_internal_module(uses_std_ordinal_contract: bool) -> Self {
        Self {
            emit_std_ordinal_value_enum_impls: uses_std_ordinal_contract,
            external_value_enums: Vec::new(),
            external_custom_keys: Vec::new(),
        }
    }

    /// Build a bridge configuration for crate-root emission where dependency adapters live.
    pub(super) fn for_crate_root(uses_std_ordinal_contract: bool, index: Option<&LibraryManifestIndex>) -> Self {
        if !uses_std_ordinal_contract {
            return Self::for_internal_module(false);
        }
        Self {
            emit_std_ordinal_value_enum_impls: true,
            external_value_enums: external_ordinal_value_enums(index),
            external_custom_keys: external_ordinal_custom_keys(index),
        }
    }
}
