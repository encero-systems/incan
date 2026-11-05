//! Imports from compiled Loaves (DD-0007): `pub::<key>` dependencies and the `std.*` namespaces of the standard
//! packages, both read from the Loaf's checked manifest and bound under the Loaf's binding root.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use crate::api_metadata::{
    ApiDeclaration, CheckedApiMetadataPackage, DecoratorArgMetadata, DecoratorValue, SafeMetadataValue,
    checked_api_declaration_is_public_namespace_member, checked_api_modules_for_public_namespace,
    checked_api_public_module_paths, checked_api_public_namespace, class_export_from_api, enum_export_from_api,
    function_export_from_api, function_export_from_api_projected, manifest_partial_with_target_defaults,
    model_export_from_api, newtype_export_from_api, partial_export_from_api, partial_export_with_target_defaults,
    trait_export_from_api,
};
use crate::ast::*;
use crate::diagnostics::errors;
use crate::library_exports::{CheckedParamDefault, CheckedParamDefaultArg, CheckedParamDefaultCallSignature};
use crate::library_manifest::{
    AliasExport, ClassExport, ConstExport, EnumExport, EnumValueExport, EnumValueTypeExport, FieldExport,
    FunctionExport, ImplementationTraitBoundOriginExport, ImplementationTypeParamExport, LibraryManifest, MethodExport,
    ModelExport, NewtypeExport, NominalTypeOriginExport, ParamDefaultExport, ParamExport, ParamKindExport,
    PartialExport, PartialTargetKindExport, PresetValueExport, PropertyExport, ProviderFactKind, ReceiverExport,
    StaticExport, TraitExport, TypeAliasExport, TypeBoundExport, TypeParamExport, resolved_type_from_manifest_type_ref,
};
use crate::loaf_index::LoafEntry;
use crate::symbols::*;
use crate::testing_markers::testing_marker_semantics_from_manifest;
use crate::typechecker::{
    PartialProjectionInfo, PartialProjectionPreset, PartialProjectionTargetKind, PublicLibraryTypeIdentity,
    TypeChecker, canonical_public_library_type_name,
};
use incan_lang::interop::is_rust_capability_bound;
use incan_lang::lang::decorators::{self as core_decorators, DecoratorId};
use incan_lang::lang::stdlib;
use incan_lang::lang::surface::functions as surface_functions;
use incan_lang::lang::surface::types as surface_types;
use incan_semantics_core::{
    CanonicalSymbolId, DecoratorFeature, SemanticSourceTargetKind, SurfaceFeatureKey, SymbolOrigin,
};

mod import_aliases;
mod manifest_callables;
mod manifest_exports;
mod manifest_types;
mod module_imports;
mod pub_library_symbols;
mod public_modules;
mod standard_loaves;
mod type_remapping;

/// One import that reaches a compiled Loaf: the Loaf's index key and the public module path inside its manifest.
pub(in crate::typechecker) struct LoafImport {
    /// The Loaf's index key: the dependency key, or the standard package's name.
    pub key: String,
    /// The public module path inside the Loaf's manifest; empty for the Loaf's root.
    pub module: Vec<Ident>,
}

enum ManifestExportRef<'a> {
    Alias(&'a AliasExport),
    Model(&'a ModelExport),
    Class(&'a ClassExport),
    Function(&'a FunctionExport),
    Partial(&'a PartialExport),
    Trait(&'a TraitExport),
    Enum(&'a EnumExport),
    EnumVariant {
        enum_name: &'a str,
        fields: &'a [crate::library_manifest::TypeRef],
        canonical: Option<&'a crate::library_manifest::CanonicalIdentityExport>,
    },
    TypeAlias(&'a TypeAliasExport),
    Newtype(&'a NewtypeExport),
    Const(&'a ConstExport),
    Static(&'a StaticExport),
}

struct PublicModuleMember {
    kind: SymbolKind,
    canonical: Option<CanonicalSymbolId>,
    source_module_path: Vec<String>,
    source_name: String,
    type_alias: Option<(Vec<TypeParamExport>, ResolvedType)>,
    partial_projection: Option<PartialProjectionInfo>,
}

/// Shared package context while following a public alias chain to partial projection metadata.
struct PartialProjectionAliasContext<'a> {
    manifest: &'a LibraryManifest,
    local_name: &'a str,
    imported_type_aliases: &'a HashMap<String, String>,
    span: Span,
}

/// Exact checked identity returned when resolving a declaration through a public package namespace.
pub(in crate::typechecker) struct ResolvedPublicModuleSymbol {
    pub kind: SymbolKind,
    pub canonical: Option<CanonicalSymbolId>,
    pub source_module_path: Vec<String>,
    pub source_name: String,
}

/// Convert a manifest parameter kind into a checked parameter kind.
fn param_kind_from_manifest(kind: ParamKindExport) -> ParamKind {
    match kind {
        ParamKindExport::Normal => ParamKind::Normal,
        ParamKindExport::RestPositional => ParamKind::RestPositional,
        ParamKindExport::RestKeyword => ParamKind::RestKeyword,
    }
}

/// Collect every nominal type origin a manifest references, keyed by binding key.
///
/// Used both to seed the closure and to extend it when a newly opened artifact contributes more origins, so the
/// two callers cannot drift on what counts as an origin. Checked trait projections retain declaration dependencies
/// but never enter nominal layout reconstruction; unresolved or malformed origins still reach normal validation.
fn collect_nominal_type_origins(
    manifest: &LibraryManifest,
) -> BTreeMap<String, crate::library_manifest::NominalTypeOriginExport> {
    use crate::library_manifest::VisitTypeRefs;
    let mut origins = BTreeMap::new();
    let mut projection = manifest.clone();
    projection.visit_type_refs(&mut |ty| {
        if let Some(origin) = TypeChecker::nominal_type_origin(ty)
            && !origin
                .canonical
                .hydrate()
                .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Trait)
        {
            origins.insert(origin.binding_key(), origin.clone());
        }
    });
    origins
}

/// Render an export's public name without its leading package segment.
fn export_public_name(export: &crate::library_manifest::ExportIdentity) -> String {
    export
        .public_path
        .iter()
        .skip(1)
        .cloned()
        .collect::<Vec<_>>()
        .join("::")
}

#[cfg(test)]
mod nominal_origin_tests;
