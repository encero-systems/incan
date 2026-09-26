//! Typechecker unit tests, one module per subject; `helpers` holds the shared helpers.

use super::type_info::{
    CBindingDescriptor, CBindingParameter, CBindingResource, CBindingSymbol, CBindingType, CResourceAccess,
    c_binding_descriptor_identity,
};
use super::*;
use crate::api_metadata::{
    ApiDeclaration, ApiFunction, CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadata, CheckedApiMetadataPackage,
    SourceAnchor, SourceSpan, collect_checked_api_alias_metadata, collect_checked_api_metadata,
    materialize_api_alias_projections, materialize_checked_api_public_namespaces,
};
use crate::ast::TypeConstraintKey;
use crate::library_exports::{
    CheckedAliasExport, CheckedExportIdentity, CheckedExportKind, CheckedNamedExport, CheckedPartialTargetKind,
    CheckedPresetValue, collect_checked_public_exports,
};
use crate::library_manifest::{
    AliasExport, ClassExport, ConstExport, EnumExport, EnumValueExport, EnumValueTypeExport, EnumVariantExport,
    ExportIdentity, ExportIdentityKind, ExportIdentityProjection, FieldExport, FieldVisibilityExport, FunctionExport,
    LEGACY_LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION, LibraryContractMetadata, LibraryExports, LibraryIdentityGraph,
    LibraryManifest, LibraryRustAbi, MethodExport, ModelExport, ParamDefaultCallArgExport,
    ParamDefaultCallSignatureExport, ParamDefaultExport, ParamExport, ParamKindExport, PartialExport,
    PartialPresetExport, PartialTargetKindExport, PresetValueExport, ReceiverExport, StaticExport, TraitExport,
    TypeAliasExport, TypeBoundExport, TypeParamExport, TypeRef,
};
use crate::library_manifest_index::{
    LibraryArtifactMetadata, LibraryManifestFailureKind, LibraryManifestIndex, LibraryManifestIndexEntry,
    LibraryManifestLoadFailure,
};
use crate::provider::{
    NamespaceAuthority, ProviderIdentity, ProviderPlan, ProviderPlanError, ProviderProvenance, ProviderRecord,
};
use crate::test_support::seeded_rust_inspect_workspace;
use crate::testing_markers::TestingFixtureScope;
use crate::{lexer, parser};
use incan_lang::interop::{
    CoercionPolicy, RustFieldInfo, RustFunctionSig, RustImplementedTrait, RustItemKind, RustItemMetadata,
    RustMethodSig, RustParam, RustTraitAssoc, RustTraitInfo, RustTypeInfo, RustTypeShape, RustVariantInfo,
    RustVisibility,
};
use incan_lang::lang::c_abi::ScalarTypeId;
use incan_lang::lang::surface::constructors::{self as surface_constructors, ConstructorId};
use incan_lang::lang::traits::{self as builtin_traits, TraitId};
use incan_lang::lang::types::collections::{self as collection_types, CollectionTypeId};
use incan_lang::lang::types::numerics::NumericTypeId;
use rust_inspect::test_fixtures::{write_borrowed_param_probe_crate, write_substrait_probe_crate};
#[cfg(feature = "rust_inspect")]
use rust_inspect::{Inspector, InspectorConfig};
use std::collections::{BTreeSet, HashMap};
#[cfg(feature = "rust_inspect")]
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

mod helpers;

mod async_and_iteration;
mod bounds_and_derives;
mod calls_decorators_and_builtins;
mod canonical_identity;
mod capabilities;
mod checked_facts_and_registries;
mod collections_strings_and_bytes;
mod extern_and_c_bindings;
mod fields_and_members;
mod forward_declared_types;
mod generic_model_bounds;
mod generics_and_type_tokens;
mod imports_and_stdlib_modules;
mod list_method_forms;
mod match_literals_and_payload_coverage;
mod method_decorator_receivers;
mod models_enums_and_newtypes;
mod narrowing_and_matching;
mod numerics_const_and_static;
mod partials_and_callable_aliases;
mod pub_imports_namespaces_and_fields;
mod pub_imports_symbols_and_identity;
mod pub_imports_trait_adoptions;
mod rust_constructors_and_fields;
mod rust_generics_and_traits;
mod rust_imports_and_types;
mod rust_metadata_and_methods;
mod rust_supertraits;
mod rust_trait_import_candidates;
mod rust_trait_qualified_calls;
mod statements_and_bindings;
mod stdlib_surfaces;
mod trait_instantiation_and_operators;
mod traits;

use helpers::{
    assert_check_ok, check_str, check_str_err, check_str_warnings, check_str_with_library_index,
    check_str_with_library_index_err, has_private_field_error, has_unknown_symbol_error,
    library_index_with_mylib_exports, library_index_with_rust_abi_item, parse_program, shadowed_trait_name,
    synthetic_artifact_root, typecheck_info_for_module,
};
