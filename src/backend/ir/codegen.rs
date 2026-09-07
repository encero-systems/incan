//! IR-based code generation facade
//!
//! This module provides `IrCodegen`, a unified API for generating Rust code from Incan AST using the IR pipeline:
//!
//! ```text
//! AST → AstLowering → IR → IrEmitter (quote!) → prettyplease → RustSource
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use incan::backend::IrCodegen;
//!
//! // Fallible API (recommended):
//! let codegen = IrCodegen::new();
//! let rust_code = codegen.try_generate(&ast)?;
//!
//! // Convenience API (returns error comments on failure):
//! let mut codegen = IrCodegen::new();
//! let rust_code = codegen.generate(&ast);
//! ```
//!
//! ## Error Handling
//!
//! The `try_generate*` family of methods return `Result<_, GenerationError>`,
//! allowing callers to handle lowering and emission errors explicitly.
//! The `generate*` methods are convenience wrappers that return error comments
//! on failure (useful for debugging but not recommended for production).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::env;
#[cfg(feature = "rust_inspect")]
use std::path::PathBuf;
use std::sync::Arc;

use crate::frontend::api_metadata::ApiDeclaration;
use crate::frontend::ast::{Declaration, ImportKind, Program};
use crate::frontend::diagnostics::CompileError;
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::module::canonicalize_source_module_segments;
use crate::frontend::typechecker::TypeCheckInfo;
use crate::frontend::typechecker::stdlib_loader::StdlibAstCache;
use crate::library_manifest::{
    ExportIdentityKind, ImplementationAssociatedTypeExport, ImplementationTraitBoundExport,
    ImplementationTraitBoundOriginExport, ImplementationTypeParamExport, LibraryManifest, TypeBoundExport, TypeRef,
};
use crate::oven::loaf::OVEN_LOAF_ENV;
use crate::provider::{ProviderPlan, SDK_PROVIDER_BUILD_ENV};
use incan_core::lang::{rust_keywords, stdlib};

use super::decl::{IrTraitBoundOrigin, IrTypeParam, Visibility};
use super::emit::CallableNameResolution;
use super::scanners::{
    check_for_this_import as scan_check_for_this_import, collect_rust_crates as scan_collect_rust_crates,
    detect_serde_usage,
};
use super::types::IrType;
use super::{AstLowering, EmitError, EmitService, FunctionRegistry, IrEmitter, IrProgram, LoweringErrors};

mod capability_bridge;
mod dependency_metadata;
mod ordinal_bridge;
mod serde_activation;
mod string_try_from_bridge;

use dependency_metadata::{
    DependencySymbolMetadata, collect_dependency_symbol_metadata, collect_externally_reachable_items_by_module,
    collect_model_field_aliases, record_direct_generated_path_support_items_from_ir,
    should_preserve_dependency_public_items,
};
use ordinal_bridge::{OrdinalBridgeConfig, compilation_imports_std_ordinal_contract, imports_std_ordinal_contract};
use serde_activation::{add_serde_to_newtypes, collect_serde_derives};
use string_try_from_bridge::{
    StringTryFromBridgeConfig, compilation_imports_std_string_try_from_contract, imports_std_string_try_from_contract,
};

/// Resolve and canonicalize the source module path used for emitted identity projection.
fn source_module_identity_path(
    program: &Program,
    explicit_path: Option<Vec<String>>,
    fallback_name: Option<&str>,
) -> Option<Vec<String>> {
    let path = explicit_path
        .or_else(|| {
            program
                .source_path
                .as_deref()
                .and_then(crate::frontend::module::logical_module_name_from_source_path)
                .map(|name| name.split('.').map(str::to_owned).collect())
        })
        .or_else(|| fallback_name.map(|name| vec![name.to_string()]))?;
    Some(canonicalize_source_module_segments(&path))
}

/// Error during Rust code generation.
///
/// This error type wraps all possible errors that can occur during code generation,
/// including AST lowering errors and IR emission errors.
///
/// ## Examples
///
/// ```rust,ignore
/// use incan::backend::{IrCodegen, GenerationError};
///
/// let codegen = IrCodegen::new();
/// match codegen.try_generate(&ast) {
///     Ok(code) => println!("{}", code),
///     Err(GenerationError::Lowering(errors)) => {
///         for err in errors.iter() {
///             eprintln!("Lowering error: {}", err);
///         }
///     }
///     Err(GenerationError::Emission(e)) => eprintln!("Emission failed: {}", e),
/// }
/// ```
#[derive(Debug)]
pub enum GenerationError {
    /// Errors during frontend typechecking.
    TypeCheck(Vec<CompileError>),
    /// Errors during AST to IR lowering (may contain multiple errors)
    Lowering(LoweringErrors),
    /// Error during IR to Rust emission
    Emission(EmitError),
}

impl std::fmt::Display for GenerationError {
    /// Format generation errors for CLI and integration-test diagnostics.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GenerationError::TypeCheck(errs) => {
                if errs.is_empty() {
                    write!(f, "typecheck failed")
                } else {
                    // We intentionally avoid rich source formatting here (no file/source context at this layer), but
                    // include every message so generated-project stdlib failures are actionable.
                    let messages = errs
                        .iter()
                        .map(|err| err.message.as_str())
                        .collect::<Vec<_>>()
                        .join("; ");
                    write!(f, "typecheck failed ({} errors): {}", errs.len(), messages)
                }
            }
            GenerationError::Lowering(e) => write!(f, "{}", e),
            GenerationError::Emission(e) => write!(f, "emission error: {}", e),
        }
    }
}

impl std::error::Error for GenerationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GenerationError::TypeCheck(_) => None,
            GenerationError::Lowering(e) => Some(e),
            GenerationError::Emission(e) => Some(e),
        }
    }
}

impl From<LoweringErrors> for GenerationError {
    fn from(e: LoweringErrors) -> Self {
        GenerationError::Lowering(e)
    }
}

impl From<EmitError> for GenerationError {
    fn from(e: EmitError) -> Self {
        GenerationError::Emission(e)
    }
}

/// Options for one IR-to-Rust generation pass that needs cross-module identity side channels.
struct IrGenerationOptions<'a> {
    /// Shared anonymous union definitions keyed by stable union shape.
    generated_union_types: HashMap<String, super::types::IrType>,
    /// Whether anonymous union references should be emitted from the crate root.
    qualify_union_types_from_crate: bool,
    /// Shared callable-name resolutions collected while emitting multi-module generated code.
    callable_name_resolutions: Option<&'a mut HashMap<String, CallableNameResolution>>,
    /// Callable signature keys that require `__IncanCallableName` support.
    callable_name_used_signature_keys: Option<&'a mut HashSet<String>>,
    /// Collect callable signatures from this program when an imported module uses the generic callable-name trait.
    ///
    /// An imported generic helper can receive a function declared by the root program.  The helper's module owns the
    /// trait declaration, so it must receive the root program's concrete function-pointer signature even when the
    /// root program does not itself read `F.__name__`.
    collect_function_arg_signatures_for_imported_generic_callable_name_trait: bool,
    /// Dependency support items required by generated paths observed in lowered IR.
    direct_generated_path_support_items: Option<&'a mut HashMap<Vec<String>, HashSet<String>>>,
}

/// Lowered metadata-only modules whose generated Rust identity belongs to compiled SDK providers.
type CompiledSdkMetadataPrograms = Vec<(Vec<String>, IrProgram)>;

/// Generated root/module Rust plus the implementation metadata inferred from the same IR.
type NestedLibraryGeneration = ((String, HashMap<Vec<String>, String>), IrGenerationMetadata);

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedImplementationBoundRequirement {
    module_path: Vec<String>,
    requirement: super::trait_bound_inference::ImplementationBoundRequirement,
    target_visibility: CapturedImplementationTargetVisibility,
}

/// Visibility of an implementation target resolved within the IR program that owns the implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapturedImplementationTargetVisibility {
    SameProgram(Visibility),
    Unknown,
}

/// Compiler-owned metadata discovered while lowering and inferring one generated library.
#[derive(Debug, Clone, Default)]
pub(crate) struct IrGenerationMetadata {
    implementation_bound_requirements: Vec<CapturedImplementationBoundRequirement>,
}

impl IrGenerationMetadata {
    /// Publish exact implementation headers into the checked trait adoptions that consumers already resolve.
    pub(crate) fn apply_to_library_manifest(&self, manifest: &mut LibraryManifest) -> Result<(), String> {
        for captured in &self.implementation_bound_requirements {
            let requirement = &captured.requirement;
            let implementation_type_params = requirement
                .type_params
                .iter()
                .map(implementation_type_param_export)
                .collect::<Result<Vec<_>, _>>()?;
            let trait_type_args = requirement
                .trait_type_args
                .iter()
                .map(manifest_type_ref_from_ir)
                .collect::<Result<Vec<_>, _>>()?;
            let mut matched = false;

            if let Some(api) = manifest.contract_metadata.api.as_mut() {
                for module in &mut api.modules {
                    if module.module_path != captured.module_path {
                        continue;
                    }
                    for declaration in &mut module.declarations {
                        let Some((name, adoptions)) = api_declaration_trait_adoptions_mut(declaration) else {
                            continue;
                        };
                        if name == requirement.target_type {
                            matched |= attach_implementation_type_params(
                                adoptions,
                                requirement,
                                &trait_type_args,
                                &implementation_type_params,
                            )?;
                        }
                    }
                }
            }

            let mut source_path = captured.module_path.clone();
            source_path.push(requirement.target_type.clone());
            let public_exports = manifest
                .contract_metadata
                .identity_graph
                .exports
                .iter()
                .filter(|identity| identity.source_path == source_path)
                .map(|identity| (identity.public_name.clone(), identity.kind))
                .collect::<Vec<_>>();
            for (public_name, kind) in public_exports {
                let adoptions = match kind {
                    ExportIdentityKind::Model => manifest
                        .exports
                        .models
                        .iter_mut()
                        .find(|export| export.name == public_name)
                        .map(|export| &mut export.trait_adoptions),
                    ExportIdentityKind::Class => manifest
                        .exports
                        .classes
                        .iter_mut()
                        .find(|export| export.name == public_name)
                        .map(|export| &mut export.trait_adoptions),
                    ExportIdentityKind::Enum => manifest
                        .exports
                        .enums
                        .iter_mut()
                        .find(|export| export.name == public_name)
                        .map(|export| &mut export.trait_adoptions),
                    ExportIdentityKind::Newtype => manifest
                        .exports
                        .newtypes
                        .iter_mut()
                        .find(|export| export.name == public_name)
                        .map(|export| &mut export.trait_adoptions),
                    _ => None,
                };
                if let Some(adoptions) = adoptions {
                    matched |= attach_implementation_type_params(
                        adoptions,
                        requirement,
                        &trait_type_args,
                        &implementation_type_params,
                    )?;
                }
            }

            // A private same-program target has no manifest surface by construction. Public aliases were tried above,
            // so suppress only a still-unmatched target whose private visibility was retained directly from this IR.
            if !matched
                && matches!(
                    captured.target_visibility,
                    CapturedImplementationTargetVisibility::SameProgram(Visibility::Private)
                )
            {
                continue;
            }

            if !matched {
                return Err(format!(
                    "inferred implementation requirement for `{}::{}` and trait `{}` had no checked manifest adoption",
                    captured.module_path.join("::"),
                    requirement.target_type,
                    requirement.trait_source_name,
                ));
            }
        }
        Ok(())
    }
}

/// Return the adopted-trait surface for a manifest declaration that can own implementations.
fn api_declaration_trait_adoptions_mut(declaration: &mut ApiDeclaration) -> Option<(&str, &mut Vec<TypeBoundExport>)> {
    match declaration {
        ApiDeclaration::Model(model) => Some((&model.name, &mut model.trait_adoptions)),
        ApiDeclaration::Class(class) => Some((&class.name, &mut class.trait_adoptions)),
        ApiDeclaration::Enum(enum_decl) => Some((&enum_decl.name, &mut enum_decl.trait_adoptions)),
        ApiDeclaration::Newtype(newtype) => Some((&newtype.name, &mut newtype.trait_adoptions)),
        _ => None,
    }
}

/// Attach one exact implementation header to its canonically matching checked trait adoption.
fn attach_implementation_type_params(
    adoptions: &mut [TypeBoundExport],
    requirement: &super::trait_bound_inference::ImplementationBoundRequirement,
    trait_type_args: &[TypeRef],
    implementation_type_params: &[ImplementationTypeParamExport],
) -> Result<bool, String> {
    let mut matched = false;
    for adoption in adoptions {
        let source_name = adoption.source_name.as_deref().unwrap_or(adoption.name.as_str());
        if source_name != requirement.trait_source_name
            || adoption.module_path != requirement.trait_module_path
            || adoption.type_args != trait_type_args
        {
            continue;
        }
        if !adoption.implementation_type_params.is_empty()
            && adoption.implementation_type_params != implementation_type_params
        {
            return Err(format!(
                "checked trait adoption `{}` carries conflicting implementation requirements",
                adoption.name
            ));
        }
        adoption.implementation_type_params = implementation_type_params.to_vec();
        matched = true;
    }
    Ok(matched)
}

/// Convert one inferred IR implementation parameter into stable manifest metadata.
fn implementation_type_param_export(type_param: &IrTypeParam) -> Result<ImplementationTypeParamExport, String> {
    Ok(ImplementationTypeParamExport {
        name: type_param.name.clone(),
        bounds: type_param
            .bounds
            .iter()
            .map(|bound| {
                Ok(ImplementationTraitBoundExport {
                    trait_path: bound.trait_path.clone(),
                    type_args: bound
                        .type_args
                        .iter()
                        .map(manifest_type_ref_from_ir)
                        .collect::<Result<Vec<_>, _>>()?,
                    associated_types: bound
                        .assoc_types
                        .iter()
                        .map(|(name, ty)| {
                            Ok(ImplementationAssociatedTypeExport {
                                name: name.clone(),
                                ty: manifest_type_ref_from_ir(ty)?,
                            })
                        })
                        .collect::<Result<Vec<_>, String>>()?,
                    origin: match bound.origin {
                        IrTraitBoundOrigin::Standard => ImplementationTraitBoundOriginExport::Standard,
                        IrTraitBoundOrigin::RustCapability => ImplementationTraitBoundOriginExport::RustCapability,
                        IrTraitBoundOrigin::SourceCallable => ImplementationTraitBoundOriginExport::SourceCallable,
                    },
                })
            })
            .collect::<Result<Vec<_>, String>>()?,
    })
}

/// Convert an IR type used by implementation metadata into its checked manifest representation.
fn manifest_type_ref_from_ir(ty: &IrType) -> Result<TypeRef, String> {
    let named = |name: &str| TypeRef::Named { name: name.to_string() };
    let applied = |name: &str, args: &[IrType]| -> Result<TypeRef, String> {
        Ok(TypeRef::Applied {
            name: name.to_string(),
            args: args
                .iter()
                .map(manifest_type_ref_from_ir)
                .collect::<Result<Vec<_>, _>>()?,
        })
    };
    match ty {
        IrType::Unit => Ok(named(incan_core::lang::conventions::UNIT_TYPE_NAME)),
        IrType::Bool => Ok(named("bool")),
        IrType::Int => Ok(named("int")),
        IrType::Float => Ok(named("float")),
        IrType::Numeric(id) => Ok(named(incan_core::lang::types::numerics::as_str(*id))),
        IrType::String | IrType::StaticStr | IrType::StrRef => Ok(named("str")),
        IrType::Bytes | IrType::StaticBytes => Ok(named("bytes")),
        IrType::FrozenStr => Ok(named("FrozenStr")),
        IrType::FrozenBytes => Ok(named("FrozenBytes")),
        IrType::List(inner) => applied("List", std::slice::from_ref(inner.as_ref())),
        IrType::Dict(key, value) => applied("Dict", &[key.as_ref().clone(), value.as_ref().clone()]),
        IrType::Set(inner) => applied("Set", std::slice::from_ref(inner.as_ref())),
        // Source tuple annotations are checked as the canonical `Tuple[...]` generic even though lowering gives
        // codegen the dedicated IR tuple shape. Preserve the checked manifest spelling so exact trait-adoption
        // matching does not mistake those two internal representations for different instantiations.
        IrType::Tuple(elements) => applied("Tuple", elements),
        IrType::Option(inner) => applied("Option", std::slice::from_ref(inner.as_ref())),
        IrType::Result(ok, err) => applied("Result", &[ok.as_ref().clone(), err.as_ref().clone()]),
        IrType::Struct(name) | IrType::Enum(name) | IrType::Trait(name) => Ok(named(name)),
        IrType::NamedGeneric(name, args) => applied(name, args),
        IrType::TypeToken(inner) => Ok(TypeRef::TypeToken {
            inner: Box::new(manifest_type_ref_from_ir(inner)?),
        }),
        IrType::RustDisplay(path) => Ok(TypeRef::RustPath { path: path.clone() }),
        IrType::ExternalUnion { union, .. } => manifest_type_ref_from_ir(union),
        IrType::ImplTrait(bound) => applied(&bound.trait_path, &bound.type_args),
        IrType::Function { params, ret } => Ok(TypeRef::Function {
            params: params
                .iter()
                .map(manifest_type_ref_from_ir)
                .collect::<Result<Vec<_>, _>>()?,
            return_type: Box::new(manifest_type_ref_from_ir(ret)?),
        }),
        IrType::Generic(name) => Ok(TypeRef::TypeParam { name: name.clone() }),
        IrType::SelfType => Ok(TypeRef::SelfType),
        IrType::Ref(inner) | IrType::RefMut(inner) => Ok(TypeRef::Ref {
            inner: Box::new(manifest_type_ref_from_ir(inner)?),
        }),
        IrType::Decimal { precision, scale } => Ok(TypeRef::Applied {
            name: "decimal".to_string(),
            args: vec![
                TypeRef::TypeParam {
                    name: precision.to_string(),
                },
                TypeRef::TypeParam {
                    name: scale.to_string(),
                },
            ],
        }),
        IrType::Unknown => Err("cannot publish unknown implementation-bound type metadata".to_string()),
    }
}

/// Split a canonical dotted source-module name into manifest path segments.
fn source_module_path_segments(name: &str) -> Vec<String> {
    name.split('.')
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect()
}

impl IrGenerationOptions<'_> {
    /// Build options for an ordinary single-program generation pass.
    fn ordinary() -> Self {
        Self {
            generated_union_types: HashMap::new(),
            qualify_union_types_from_crate: false,
            callable_name_resolutions: None,
            callable_name_used_signature_keys: None,
            collect_function_arg_signatures_for_imported_generic_callable_name_trait: false,
            direct_generated_path_support_items: None,
        }
    }
}

/// IR-based Rust code generator
///
/// This is the unified entrypoint for code generation. It uses the typed IR and syn/quote for code emission.
pub struct IrCodegen<'a> {
    /// The current program being generated
    current_program: Option<&'a Program>,
    /// Dependency modules to include before main.
    ///
    /// Stores both the flat module name (used for build graph identity) and the nested module path
    /// segments (used for correct Rust qualification in codegen).
    dependency_modules: Vec<(&'a str, &'a Program, Option<Vec<String>>)>,
    /// Source-derived dependency symbols used for Rust qualification but linked from an external artifact.
    ///
    /// The compiler typechecks provider imports against checked contracts. Once a module is supplied by a compiled
    /// provider, codegen must retain those contracts' canonical symbol paths without treating the module as a
    /// consumer-local Rust source module.
    dependency_symbol_modules: Vec<(&'a str, &'a Program, Option<Vec<String>>)>,
    /// Canonical nested paths learned while lowering emitted source dependencies for root metadata emission.
    source_dependency_module_paths: Vec<(&'a Program, Vec<String>)>,
    /// Whether serde is needed for emitted Rust derives or helpers.
    // Serde still affects emitted Rust imports and derive augmentation in IR emission, so this remains an
    // emission-internal signal even after project-level requirement collection moved to provider manifests.
    needs_serde: bool,
    /// Fixtures available for test functions (name -> (has_teardown, dependencies))
    fixtures: HashMap<String, (bool, Vec<String>)>,
    /// Rust crates imported via `import rust::` or `from rust::`
    rust_crates: HashSet<String>,
    /// Crate roots required to keep public class-field Rust identities nameable through a compiled provider.
    provider_rust_bridge_roots: BTreeSet<String>,
    /// Whether to emit the Zen of Incan at the start of main (set by `import this`)
    emit_zen_in_main: bool,
    /// Functions imported from external Rust crates (name -> true for external)
    external_rust_functions: HashSet<String>,
    /// Declared Rust crate names from `incan.toml [rust-dependencies]` (RFC 013 / RFC 023).
    ///
    /// When set, internal typechecking (used to obtain `TypeCheckInfo` for lowering) will validate `rust.module()`
    /// crate segments against this set.
    declared_crate_names: Option<HashSet<String>>,
    /// Shared provider and feature projection used by checking, lowering, and emission.
    provider_plan: Option<Arc<ProviderPlan>>,
    /// Whether generated Rust should deny warnings so tests can prove normal emission stays warning-clean.
    strict_generated_lints: bool,
    /// Private IR items called by generated code that is appended outside normal IR emission.
    externally_reachable_items: HashSet<String>,
    /// Private dependency-module IR items called by generated code appended inside that module.
    externally_reachable_items_by_module: HashMap<Vec<String>, HashSet<String>>,
    /// Public serialized value-enum identities for library builds, keyed by source identity (`module.Type`).
    public_ordinal_type_identities: HashMap<String, String>,
    /// Whether non-stdlib dependency modules keep public items that are not otherwise reachable.
    preserve_dependency_public_items: bool,
    /// Dependency module paths that should typecheck with source-visible public import rules.
    public_typecheck_module_paths: HashSet<Vec<String>>,
    /// Canonical defining package identity supplied by the command that owns the generated artifact.
    registry_package_identity: Option<String>,
    /// Package origin used for canonical identities emitted by a compiled-library producer.
    ///
    /// Ordinary program builds leave this absent and retain module-owned identities. Library builds set it before
    /// checking so producer symbols and consumer-hydrated manifest identities encode the same origin.
    canonical_emission_package_identity: Option<String>,
    /// Canonical source-module path for the root program when its parsed AST lacks a source path.
    root_source_module_name: Option<String>,
    /// Shared stdlib source metadata cache reused across the repeated internal typecheck/lowering passes that codegen
    /// performs for multi-module builds.
    stdlib_cache: StdlibAstCache,
    /// Main-module facts supplied by the owning compilation session.
    ///
    /// Direct backend API callers may omit this temporarily; that fallback is removed when every caller constructs its
    /// lowering request from a compilation-session analysis (#225).
    prechecked_main_type_info: Option<TypeCheckInfo>,
    /// Dependency facts from the same session analysis, keyed by module identity.
    prechecked_dependency_type_info: HashMap<Vec<String>, TypeCheckInfo>,
    /// Bound-bearing implementation headers resolved from the exact IR emitted for this compilation.
    implementation_bound_requirements: Vec<CapturedImplementationBoundRequirement>,
    /// Authoritative checked-API path for a library root while collecting manifest metadata.
    metadata_root_module_path: Option<Vec<String>>,
    /// Manifest/workspace root for rust-inspect-backed typechecking during IR generation.
    #[cfg(feature = "rust_inspect")]
    rust_inspect_manifest_dir: Option<PathBuf>,
}

impl<'a> IrCodegen<'a> {
    /// Create a new IR-based code generator
    pub fn new() -> Self {
        Self {
            current_program: None,
            dependency_modules: Vec::new(),
            dependency_symbol_modules: Vec::new(),
            source_dependency_module_paths: Vec::new(),
            needs_serde: false,
            external_rust_functions: HashSet::new(),
            fixtures: HashMap::new(),
            rust_crates: HashSet::new(),
            provider_rust_bridge_roots: BTreeSet::new(),
            emit_zen_in_main: false,
            declared_crate_names: None,
            provider_plan: None,
            strict_generated_lints: false,
            externally_reachable_items: HashSet::new(),
            externally_reachable_items_by_module: HashMap::new(),
            public_ordinal_type_identities: HashMap::new(),
            preserve_dependency_public_items: true,
            public_typecheck_module_paths: HashSet::new(),
            registry_package_identity: None,
            canonical_emission_package_identity: None,
            root_source_module_name: None,
            stdlib_cache: StdlibAstCache::new(),
            prechecked_main_type_info: None,
            prechecked_dependency_type_info: HashMap::new(),
            implementation_bound_requirements: Vec::new(),
            metadata_root_module_path: None,
            #[cfg(feature = "rust_inspect")]
            rust_inspect_manifest_dir: None,
        }
    }

    /// Return the stable module key used by source imports and CLI collection for one dependency module.
    fn dependency_module_key(name: &str, path_segments: &Option<Vec<String>>) -> String {
        path_segments
            .as_deref()
            .map(canonicalize_source_module_segments)
            .map(|segments| segments.join("_"))
            .unwrap_or_else(|| name.to_string())
    }

    /// Give an internal typecheck pass the canonical source paths already supplied to codegen for its dependencies.
    ///
    /// The dependency cache key is an emission detail and may flatten multiple source paths to the same spelling.
    /// Rechecking without this mapping would therefore discard declaration ownership that the compilation request
    /// already knew, leaving ordinary `module.function(...)` calls unable to carry their canonical target into IR.
    fn register_dependency_module_paths(
        checker: &mut crate::frontend::typechecker::TypeChecker,
        dependencies: &[(&str, &Program, Option<Vec<String>>)],
    ) {
        for (name, _, path_segments) in dependencies {
            if let Some(path_segments) = path_segments {
                checker
                    .register_dependency_module_path_segments(name, canonicalize_source_module_segments(path_segments));
            }
        }
    }

    /// Capture bound-bearing implementation headers from one inferred IR module.
    fn capture_implementation_bound_requirements(&mut self, module_path: Vec<String>, program: &IrProgram) {
        for requirement in super::trait_bound_inference::collect_local_implementation_bound_requirements(program) {
            let target_visibility = program
                .declarations
                .iter()
                .find_map(|declaration| match &declaration.kind {
                    super::decl::IrDeclKind::Struct(target) if target.name == requirement.target_type => {
                        Some(target.visibility)
                    }
                    super::decl::IrDeclKind::Enum(target) if target.name == requirement.target_type => {
                        Some(target.visibility)
                    }
                    _ => None,
                })
                .map(CapturedImplementationTargetVisibility::SameProgram)
                .unwrap_or(CapturedImplementationTargetVisibility::Unknown);
            let captured = CapturedImplementationBoundRequirement {
                module_path: module_path.clone(),
                requirement,
                target_visibility,
            };
            if !self.implementation_bound_requirements.contains(&captured) {
                self.implementation_bound_requirements.push(captured);
            }
        }
        self.implementation_bound_requirements.sort_by(|left, right| {
            left.module_path
                .cmp(&right.module_path)
                .then(left.requirement.target_type.cmp(&right.requirement.target_type))
                .then(
                    left.requirement
                        .trait_source_name
                        .cmp(&right.requirement.trait_source_name),
                )
        });
    }

    /// Return the transitive local source dependency subset needed to typecheck one program.
    ///
    /// Codegen typechecking must mirror the CLI checker: a module should see its declared local imports and their
    /// transitive signature dependencies, not every module collected for the output project. Importing the whole
    /// dependency universe lets same-name public helpers from unrelated modules collide before `from ... import ... as
    /// ...` collection, which changes behavior between `--check` and `--emit-rust`.
    fn imported_dependency_modules_for_program(
        &self,
        program: &Program,
        dependencies: &[(&'a str, &'a Program, Option<Vec<String>>)],
        self_key: Option<&str>,
    ) -> Vec<(&'a str, &'a Program)> {
        let mut module_idx_by_key = HashMap::new();
        for (idx, (name, _, path_segments)) in dependencies.iter().enumerate() {
            module_idx_by_key.insert(Self::dependency_module_key(name, path_segments), idx);
        }

        let mut selected = BTreeSet::new();
        let mut pending = self.direct_imported_dependency_indexes(program, &module_idx_by_key, self_key);
        while let Some(idx) = pending.pop() {
            let (name, ast, path_segments) = &dependencies[idx];
            let dep_key = Self::dependency_module_key(name, path_segments);
            if self_key == Some(dep_key.as_str()) || !selected.insert(idx) {
                continue;
            }
            pending.extend(self.direct_imported_dependency_indexes(ast, &module_idx_by_key, Some(dep_key.as_str())));
        }

        selected
            .into_iter()
            .map(|idx| {
                let (name, ast, _) = dependencies[idx];
                (name, ast)
            })
            .collect()
    }

    /// Return direct dependency-module indexes named by source imports in one program.
    fn direct_imported_dependency_indexes(
        &self,
        program: &Program,
        module_idx_by_key: &HashMap<String, usize>,
        self_key: Option<&str>,
    ) -> Vec<usize> {
        let mut dep_indexes = BTreeSet::new();
        for decl in &program.declarations {
            let Declaration::Import(import) = &decl.node else {
                continue;
            };
            match &import.kind {
                ImportKind::From { module, .. } => {
                    if module.parent_levels > 0 || module.segments.is_empty() {
                        continue;
                    }
                    let key = canonicalize_source_module_segments(&module.segments).join("_");
                    if self_key != Some(key.as_str())
                        && let Some(dep_idx) = module_idx_by_key.get(&key).copied()
                    {
                        dep_indexes.insert(dep_idx);
                    } else if self
                        .provider_plan
                        .as_deref()
                        .is_some_and(|plan| plan.bootstrap_owns_sdk_module(&module.segments))
                        && module.segments.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT)
                    {
                        let physical_key = canonicalize_source_module_segments(&module.segments[1..]).join("_");
                        if self_key != Some(physical_key.as_str())
                            && let Some(dep_idx) = module_idx_by_key.get(&physical_key).copied()
                        {
                            dep_indexes.insert(dep_idx);
                        }
                    }
                }
                ImportKind::Module(path) => {
                    if path.parent_levels > 0 || path.segments.is_empty() {
                        continue;
                    }
                    let bootstrap_physical = self
                        .provider_plan
                        .as_deref()
                        .is_some_and(|plan| plan.bootstrap_owns_sdk_module(&path.segments))
                        .then(|| path.segments[1..].to_vec());
                    let mut candidate_paths = Vec::new();
                    if let Some(physical) = bootstrap_physical {
                        candidate_paths.push(physical);
                    }
                    candidate_paths.push(path.segments.clone());
                    for candidate in candidate_paths {
                        let full_key = canonicalize_source_module_segments(&candidate).join("_");
                        if self_key != Some(full_key.as_str())
                            && let Some(dep_idx) = module_idx_by_key.get(&full_key).copied()
                        {
                            dep_indexes.insert(dep_idx);
                        }
                        if candidate.len() > 1 {
                            let parent_key =
                                canonicalize_source_module_segments(&candidate[..candidate.len() - 1]).join("_");
                            if self_key != Some(parent_key.as_str())
                                && let Some(dep_idx) = module_idx_by_key.get(&parent_key).copied()
                            {
                                dep_indexes.insert(dep_idx);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        dep_indexes.into_iter().collect()
    }

    /// Build a registry for explicit canonical cross-module calls.
    fn canonical_registry_for_programs<'program>(
        programs: impl IntoIterator<Item = (&'program [String], &'program IrProgram)>,
    ) -> FunctionRegistry {
        let programs: Vec<_> = programs.into_iter().collect();
        let mut registry = FunctionRegistry::new();
        for (module_path, program) in &programs {
            for (name, signature) in program.function_registry.iter() {
                let mut canonical_path = (*module_path).to_vec();
                canonical_path.push(name.clone());
                if let Some(identity) = program.function_registry.canonical_identity(name) {
                    registry.register_canonical_path_projection(
                        &canonical_path,
                        program.function_registry.source_name(name).unwrap_or(name).to_string(),
                        identity.clone(),
                        signature.params.clone(),
                        signature.return_type.clone(),
                    );
                } else {
                    registry.register_canonical_path(
                        &canonical_path,
                        signature.params.clone(),
                        signature.return_type.clone(),
                    );
                }
            }
        }

        let mut pending_reexports = Vec::new();
        for (module_path, program) in &programs {
            for reexport in &program.function_reexports {
                let mut alias_path = (*module_path).to_vec();
                alias_path.push(reexport.name.clone());
                pending_reexports.push((alias_path, reexport.target_path.clone()));
            }
        }
        while !pending_reexports.is_empty() {
            let mut unresolved = Vec::new();
            let mut made_progress = false;
            for (alias_path, target_path) in pending_reexports {
                if registry.get_canonical_path(&alias_path).is_some() {
                    made_progress = true;
                    continue;
                }
                if let Some(signature) = registry.get_canonical_path(&target_path).cloned() {
                    if let Some(identity) = registry.canonical_identity_for_path(&target_path).cloned() {
                        registry.register_canonical_path_projection(
                            &alias_path,
                            identity.declaration_name.clone(),
                            identity,
                            signature.params.clone(),
                            signature.return_type.clone(),
                        );
                    } else {
                        registry.register_canonical_path(
                            &alias_path,
                            signature.params.clone(),
                            signature.return_type.clone(),
                        );
                    }
                    made_progress = true;
                } else {
                    unresolved.push((alias_path, target_path));
                }
            }
            if !made_progress {
                break;
            }
            pending_reexports = unresolved;
        }
        registry
    }

    /// Apply dependency symbol metadata to generated Rust codegen state.
    fn apply_dependency_symbol_metadata(
        emitter: &mut IrEmitter<'_>,
        metadata: &DependencySymbolMetadata,
        provider_plan: Option<&ProviderPlan>,
    ) {
        let stdlib_module_paths = provider_plan
            .map(ProviderPlan::active_std_module_paths)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|path| {
                path.strip_prefix(&[stdlib::STDLIB_ROOT.to_string()])
                    .map(<[String]>::to_vec)
            })
            .collect();
        emitter.set_compiled_sdk_module_paths(stdlib_module_paths);
        emitter.set_type_module_paths(metadata.module_paths.clone(), metadata.ambiguous_type_names.clone());
        emitter.set_value_module_paths(
            metadata.value_module_paths.clone(),
            metadata.ambiguous_value_names.clone(),
        );
        let mut enum_type_names = metadata.enum_type_names.clone();
        if let Some(plan) = provider_plan {
            for provider in plan.active_sdk_records() {
                let Some(manifest) = provider.manifest.as_deref() else {
                    continue;
                };
                enum_type_names.extend(manifest.exports.enums.iter().map(|enum_| enum_.name.clone()));
                enum_type_names.extend(
                    manifest
                        .contract_metadata
                        .api
                        .iter()
                        .flat_map(|api| api.modules.iter())
                        .flat_map(|module| module.declarations.iter())
                        .filter_map(|declaration| match declaration {
                            crate::frontend::api_metadata::ApiDeclaration::Enum(enum_) => Some(enum_.name.clone()),
                            _ => None,
                        }),
                );
            }
        }
        emitter.set_dependency_enum_types(enum_type_names);
        if let Some(plan) = provider_plan {
            emitter.seed_public_dependency_nominal_metadata(plan.library_manifest_index());
            for provider in plan.active_sdk_records() {
                if let Some(manifest) = provider.manifest.as_deref() {
                    emitter.seed_sdk_provider_manifest_metadata(manifest);
                }
            }
        }
    }

    /// Configure source-import emission with the checked module graph for this generated crate.
    fn configure_source_import_paths(
        emitter: &mut IrEmitter<'_>,
        current_module: Option<&str>,
        source_module_paths: &HashSet<Vec<String>>,
    ) {
        emitter.set_source_module_paths(source_module_paths.clone());
        emitter.set_current_source_module_path(
            current_module.map(|module| module.split('.').map(str::to_string).collect()),
        );
    }

    /// Enable strict generated Rust lint validation for `--emit-rust --strict`.
    pub fn set_strict_generated_lints(&mut self, enabled: bool) {
        self.strict_generated_lints = enabled;
    }

    /// Set private generated Rust entrypoints called by code injected after IR emission.
    pub fn set_externally_reachable_items(&mut self, names: HashSet<String>) {
        self.externally_reachable_items = names;
    }

    /// Set private generated Rust entrypoints called by code injected into dependency modules.
    pub fn set_externally_reachable_items_by_module(&mut self, names: HashMap<Vec<String>, HashSet<String>>) {
        self.externally_reachable_items_by_module = names;
    }

    /// Set public serialized value-enum identities for library emission.
    pub fn set_public_ordinal_type_identities(&mut self, identities: HashMap<String, String>) {
        self.public_ordinal_type_identities = identities;
    }

    /// Collect the OrdinalKey bridge facts needed by the emitter for this program.
    fn ordinal_bridge_config(&self, uses_std_ordinal_contract: bool) -> OrdinalBridgeConfig {
        OrdinalBridgeConfig::for_crate_root(
            uses_std_ordinal_contract,
            self.provider_plan.as_deref().map(ProviderPlan::library_manifest_index),
        )
    }

    /// Collect `TryFrom[str]` bridge facts needed at the generated crate root.
    fn string_try_from_bridge_config(&self, uses_contract: bool) -> StringTryFromBridgeConfig {
        StringTryFromBridgeConfig::for_crate_root(uses_contract)
    }

    /// Apply collected OrdinalKey bridge metadata to a freshly created emitter.
    fn apply_ordinal_bridge_config(&self, emitter: &mut IrEmitter, config: &OrdinalBridgeConfig) {
        emitter.set_emit_std_ordinal_value_enum_impls(config.emit_std_ordinal_value_enum_impls);
        emitter.set_external_ordinal_value_enums(config.external_value_enums.clone());
        emitter.set_external_ordinal_custom_keys(config.external_custom_keys.clone());
        emitter.set_public_ordinal_type_identities(self.public_ordinal_type_identities.clone());
    }

    /// Apply compiler-provided `TryFrom[str]` bridge metadata to a freshly created emitter.
    fn apply_string_try_from_bridge_config(&self, emitter: &mut IrEmitter, config: &StringTryFromBridgeConfig) {
        emitter.set_emit_std_string_try_from_newtype_impls(config.emit_local_newtype_impls);
    }

    /// Apply every temporary source-owned capability bridge to a freshly created emitter.
    fn apply_capability_bridge_configs(
        &self,
        emitter: &mut IrEmitter,
        ordinal: &OrdinalBridgeConfig,
        string_conversion: &StringTryFromBridgeConfig,
    ) {
        self.apply_ordinal_bridge_config(emitter, ordinal);
        self.apply_string_try_from_bridge_config(emitter, string_conversion);
    }

    /// Give an emitter the package context needed to render self-package canonical paths through `crate::...`.
    fn apply_canonical_emission_context(&self, emitter: &mut IrEmitter) {
        emitter.set_current_package_identity(self.canonical_emission_package_identity.clone());
    }

    /// Set whether non-stdlib dependency modules preserve their public API surface during emission.
    ///
    /// Library builds keep this enabled so public dependency declarations remain available at the Rust crate boundary.
    /// Binary and test harness builds can disable it so unused dependency declarations are pruned instead of warning.
    pub fn set_preserve_dependency_public_items(&mut self, enabled: bool) {
        self.preserve_dependency_public_items = enabled;
    }

    /// Set the package identity used when materializing explicit package-level registry subjects.
    pub fn set_registry_package_identity(&mut self, identity: Option<String>) {
        self.registry_package_identity = identity;
    }

    /// Set the package origin for source declarations emitted into a compiled-library artifact.
    pub fn set_canonical_emission_package_identity(&mut self, identity: Option<String>) {
        self.canonical_emission_package_identity = identity;
    }

    /// Set the root compilation-unit identity when parsing did not retain a source path.
    pub fn set_root_source_module_name(&mut self, name: Option<String>) {
        self.root_source_module_name = name;
    }

    /// Set dependency module paths that should typecheck with public source import rules.
    ///
    /// CLI test batches can emit individual test files as generated dependency modules so each file keeps its own Rust
    /// module scope. Those test files are still user source and must typecheck like focused `incan test file.incn`
    /// runs, not like compiler-internal source dependencies that may inspect private module items.
    pub fn set_public_typecheck_module_paths(&mut self, paths: HashSet<Vec<String>>) {
        self.public_typecheck_module_paths = paths;
    }

    /// Seed codegen with stdlib metadata already collected by an earlier typecheck phase.
    pub(crate) fn set_stdlib_cache(&mut self, cache: StdlibAstCache) {
        self.stdlib_cache = cache;
    }

    /// Supply the checked lowering inputs owned by one compilation session.
    ///
    /// Production command paths use this to prevent lowering from rechecking source after diagnostics and semantic
    /// facts have already been produced.
    pub(crate) fn set_prechecked_type_info(
        &mut self,
        main: TypeCheckInfo,
        dependencies: HashMap<Vec<String>, TypeCheckInfo>,
    ) {
        self.prechecked_main_type_info = Some(main);
        self.prechecked_dependency_type_info = dependencies;
    }

    /// Return session-owned facts for one dependency module when supplied.
    fn prechecked_dependency_type_info(&self, path: &[String]) -> Option<TypeCheckInfo> {
        self.prechecked_dependency_type_info.get(path).cloned()
    }

    /// Set declared Rust crate names from `incan.toml [rust-dependencies]`. (RFC 031)
    ///
    /// This is used for validating `rust.module()` paths during the internal typechecking that precedes IR lowering.
    pub fn set_declared_crate_names(&mut self, names: HashSet<String>) {
        self.declared_crate_names = Some(names);
    }

    /// Set the consumer-side library manifest index for focused `pub::` tests and embedding adapters.
    pub fn set_library_manifest_index(&mut self, index: LibraryManifestIndex) {
        self.provider_plan = Some(Arc::new(ProviderPlan::for_library_index(index)));
    }

    /// Set one in-memory SDK provider manifest for focused compiler tests.
    #[doc(hidden)]
    pub fn set_sdk_provider_manifest(&mut self, manifest: LibraryManifest) {
        let library_index = self
            .provider_plan
            .as_deref()
            .map(ProviderPlan::library_manifest_index)
            .cloned()
            .unwrap_or_default();
        self.provider_plan = Some(Arc::new(ProviderPlan::for_in_memory_sdk_manifest(
            library_index,
            manifest,
        )));
    }

    /// Set SDK-provider module paths already derived from a producer entrypoint or checked manifest.
    ///
    /// Compiler frontends should normally call [`Self::set_sdk_provider_manifest`]. This lower-level hook supports
    /// source-backed codegen fixtures and embedders that already own equivalent checked module discovery.
    #[doc(hidden)]
    pub fn set_sdk_provider_module_paths(&mut self, module_paths: Vec<Vec<String>>) {
        let library_index = self
            .provider_plan
            .as_deref()
            .map(ProviderPlan::library_manifest_index)
            .cloned()
            .unwrap_or_default();
        self.provider_plan = Some(Arc::new(ProviderPlan::for_in_memory_sdk_modules(
            library_index,
            module_paths,
        )));
    }

    /// Set the immutable provider plan shared across every compiler stage.
    pub fn set_provider_plan(&mut self, plan: Arc<ProviderPlan>) {
        self.provider_plan = Some(plan);
    }

    /// Set the manifest/workspace root used for rust-inspect-backed typechecking during IR generation.
    #[cfg(feature = "rust_inspect")]
    pub fn set_rust_inspect_manifest_dir(&mut self, dir: PathBuf) {
        self.rust_inspect_manifest_dir = Some(dir);
    }

    /// Get the Rust crates imported via `import rust::` or `from rust::`
    pub fn rust_crates(&self) -> &HashSet<String> {
        &self.rust_crates
    }

    /// Register a fixture for test code generation
    pub fn add_fixture(&mut self, name: &str, has_teardown: bool, dependencies: Vec<String>) {
        self.fixtures.insert(name.to_string(), (has_teardown, dependencies));
    }

    /// Check if serde is needed.
    #[cfg(test)]
    fn needs_serde(&self) -> bool {
        self.needs_serde
    }

    /// Apply codegen's shared project context to an internal typechecker pass.
    fn configure_typechecker(
        &self,
        tc: &mut crate::frontend::typechecker::TypeChecker,
        module_path: Option<&[String]>,
    ) {
        tc.stdlib_cache = self.stdlib_cache.clone();
        let package_identity = crate::frontend::module::declaration_package_identity(
            self.canonical_emission_package_identity.as_deref(),
            module_path,
        );
        tc.set_current_package_identity(package_identity);
        if let Some(names) = self.declared_crate_names.clone() {
            tc.set_declared_crate_names(names);
        }
        if let Some(plan) = self.provider_plan.clone() {
            tc.set_provider_plan(plan);
        }
        #[cfg(feature = "rust_inspect")]
        if let Some(dir) = self.rust_inspect_manifest_dir.clone() {
            tc.set_rust_inspect_manifest_dir(dir);
        }
    }

    /// Prefix internal codegen typecheck diagnostics with the module being lowered.
    fn typecheck_errors_for_module(module: &str, mut errors: Vec<CompileError>) -> GenerationError {
        for error in &mut errors {
            error.message = format!("in module `{module}`: {}", error.message);
        }
        GenerationError::TypeCheck(errors)
    }

    /// Preserve stdlib metadata warmed by an internal typechecker pass for later codegen passes.
    fn capture_typechecker_stdlib_cache(&mut self, tc: &crate::frontend::typechecker::TypeChecker) {
        self.stdlib_cache = tc.stdlib_cache.clone();
    }

    /// Apply codegen's shared metadata context to one AST lowering pass.
    fn configure_lowering(&self, lowering: &mut AstLowering) {
        lowering.set_stdlib_cache(self.stdlib_cache.clone());
        lowering.set_provider_plan(self.provider_plan.clone());
        // A release seed compiles compiler-owned provider source into its sealed direct-rustc closure. Give that
        // source the same trusted public-stdlib identity as the SDK publisher; normal Oven consumers never set this
        // marker and therefore cannot acquire provider-only lowering behavior.
        lowering.set_sdk_provider_build(
            env::var_os(SDK_PROVIDER_BUILD_ENV).is_some() || env::var_os(OVEN_LOAF_ENV).is_some(),
        );
        lowering.set_registry_package_identity(self.registry_package_identity.clone());
    }

    /// Add a dependency module (for multi-file compilation)
    pub fn add_module(&mut self, module_name: &'a str, module_ast: &'a Program) {
        self.dependency_modules.push((module_name, module_ast, None));
    }

    /// Add a dependency module with its nested module path segments.
    ///
    /// This is used by the CLI multi-file nested mode where a module like `api.routes` is emitted as
    /// `crate::api::routes` in Rust (even though we may use a flattened name like `api_routes` for internal identity).
    pub fn add_module_with_path_segments(
        &mut self,
        module_name: &'a str,
        module_ast: &'a Program,
        path_segments: Vec<String>,
    ) {
        self.dependency_modules
            .push((module_name, module_ast, Some(path_segments)));
    }

    /// Add dependency source metadata without scheduling that module for local Rust emission.
    ///
    /// This remains available for non-emitted source dependencies. Compiled SDK-provider imports instead derive
    /// their semantics from the compiled artifact manifest and resolve Rust symbols through the linked artifact crate.
    pub fn add_dependency_symbol_module_with_path_segments(
        &mut self,
        module_name: &'a str,
        module_ast: &'a Program,
        path_segments: Vec<String>,
    ) {
        self.dependency_symbol_modules
            .push((module_name, module_ast, Some(path_segments)));
    }

    /// Return emitted and metadata-only dependencies, deduplicated by canonical source module identity.
    fn dependency_modules_for_symbol_metadata(&self) -> Vec<(&'a str, &'a Program, Option<Vec<String>>)> {
        let mut modules = self.dependency_modules.clone();
        for module in &self.dependency_symbol_modules {
            let key = Self::dependency_module_key(module.0, &module.2);
            if !modules
                .iter()
                .any(|candidate| Self::dependency_module_key(candidate.0, &candidate.2) == key)
            {
                modules.push(module.clone());
            }
        }
        modules
    }

    /// Lower metadata-only stdlib modules enough to discover anonymous union wrappers owned by the artifact crate.
    ///
    /// Anonymous unions have stable structural names but no source-level name to place in the `.incnlib` contract yet.
    /// Until that manifest capability exists, this source-derived registry preserves one Rust nominal identity without
    /// re-emitting the provider modules in every consumer.
    fn compiled_sdk_metadata_programs(&mut self) -> Result<CompiledSdkMetadataPrograms, GenerationError> {
        if let Some(plan) = self.provider_plan.as_deref() {
            let mut has_compiled_provider = false;
            for provider in plan.active_sdk_records() {
                let Some(_manifest) = provider.manifest.as_deref() else {
                    continue;
                };
                has_compiled_provider = true;
            }
            if has_compiled_provider {
                return Ok(Vec::new());
            }
        }
        if self.dependency_symbol_modules.is_empty() {
            return Ok(Vec::new());
        }

        let dependencies = self.dependency_modules_for_symbol_metadata();
        let symbol_modules = self.dependency_symbol_modules.clone();
        let mut programs = Vec::new();
        for (module_name, module_ast, path_segments) in symbol_modules {
            let Some(path_segments) = path_segments.as_ref() else {
                continue;
            };
            if path_segments.first().map(String::as_str) != Some(stdlib::INCAN_STD_NAMESPACE) {
                continue;
            }
            let module_key = Self::dependency_module_key(module_name, &Some(path_segments.clone()));
            let module_type_info = {
                use crate::frontend::typechecker::TypeChecker;
                let mut tc = TypeChecker::new();
                self.configure_typechecker(&mut tc, Some(path_segments.as_slice()));
                Self::register_dependency_module_paths(&mut tc, &dependencies);
                tc.set_current_module_path(Some(canonicalize_source_module_segments(path_segments)));
                let typecheck_deps =
                    self.imported_dependency_modules_for_program(module_ast, &dependencies, Some(&module_key));
                let result = match tc.check_with_imports_allow_private(module_ast, &typecheck_deps) {
                    Ok(()) => tc.type_info().clone(),
                    Err(errs) => return Err(Self::typecheck_errors_for_module(&module_key, errs)),
                };
                self.capture_typechecker_stdlib_cache(&tc);
                result
            };
            self.collect_provider_rust_bridge_roots(&module_type_info)?;
            let mut lowering = AstLowering::new_with_type_info(module_type_info);
            self.configure_lowering(&mut lowering);
            lowering.set_current_source_module_name(Some(path_segments.join(".")));
            lowering.seed_dependency_trait_decls(&dependencies)?;
            let ir = lowering.lower_program(module_ast)?;
            programs.push((path_segments.clone(), ir));
        }
        Ok(programs)
    }

    /// Backfill nested module path segments for a dependency module by name.
    ///
    /// This is primarily used by tests or older call sites that only registered a flat
    /// module name via `add_module()`. If a matching module entry exists and has no
    /// path segments yet, this sets them.
    pub fn set_module_path_segments(&mut self, module_name: &str, path_segments: Vec<String>) {
        if let Some((_name, _ast, segs)) = self
            .dependency_modules
            .iter_mut()
            .find(|(name, _, _)| *name == module_name)
            && segs.is_none()
        {
            *segs = Some(path_segments);
        }
    }

    // =========================================================================
    // Feature Detection
    // =========================================================================

    /// Scan a program for external Rust function imports
    fn collect_external_rust_functions(&mut self, program: &Program) {
        use crate::frontend::ast::{Declaration, ImportKind};

        for decl in &program.declarations {
            if let Declaration::Import(import) = &decl.node {
                match &import.kind {
                    // from rust::crate import items
                    ImportKind::RustFrom { items, .. } => {
                        for item in items {
                            let func_name = item.alias.as_ref().unwrap_or(&item.name);
                            self.external_rust_functions.insert(func_name.clone());
                        }
                    }
                    // Legacy: from rust::crate import items (parsed as From with rust:: module)
                    ImportKind::From { module, items }
                        if !module.segments.is_empty() && module.segments.first() == Some(&"rust".to_string()) =>
                    {
                        for item in items {
                            let func_name = item.alias.as_ref().unwrap_or(&item.name);
                            self.external_rust_functions.insert(func_name.clone());
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// Scan a program for serde-backed derives.
    ///
    /// This remains an internal compatibility hook because serde-backed derives and legacy
    /// `json_stringify` usage can still require serde emission without import-activated provider
    /// metadata.
    fn update_serde_requirement(&mut self, program: &Program) {
        if detect_serde_usage(program) {
            self.needs_serde = true;
        }
    }

    // (helper methods removed in favor of centralized scanners)

    /// Collect rust crates from imports
    fn collect_rust_crates(&mut self, program: &Program) {
        let crates = scan_collect_rust_crates(program);
        for c in crates {
            self.rust_crates.insert(c);
        }
    }

    /// Publish the checked crate roots required by public class-field Rust identities.
    ///
    /// Consumer-generated declarations cannot name a transitive Cargo dependency directly. Library-mode crates expose
    /// only roots selected from checked public class layouts, including compiled providers that own inherited fields.
    /// Ordinary application builds remain unchanged.
    fn attach_provider_rust_dependency_bridge(&self, main_code: String) -> String {
        if !self.preserve_dependency_public_items {
            return main_code;
        }
        let mut crates = self
            .provider_rust_bridge_roots
            .iter()
            .filter(|crate_name| !crate::frontend::rust_type_display::is_shared_rust_crate(crate_name))
            .map(|crate_name| rust_keywords::escape_keyword(&crate_name.replace('-', "_")))
            .collect::<Vec<_>>();
        crates.sort();
        crates.dedup();
        if crates.is_empty() {
            return main_code;
        }
        let reexports = crates
            .into_iter()
            .map(|crate_name| format!("    pub use ::{crate_name};"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("{main_code}\n#[doc(hidden)]\npub mod __incan_provider_rust {{\n{reexports}\n}}\n")
    }

    /// Accumulate the exact provider and Rust crate roots required by checked public class layouts.
    fn collect_provider_rust_bridge_roots(&mut self, type_info: &TypeCheckInfo) -> Result<(), GenerationError> {
        if !self.preserve_dependency_public_items {
            return Ok(());
        }
        for layout in type_info
            .declarations
            .class_layouts
            .values()
            .filter(|layout| layout.is_public)
        {
            for field in &layout.fields {
                if let Some(provider) = &field.provider_library {
                    self.provider_rust_bridge_roots.insert(provider.clone());
                    continue;
                }
                let roots = crate::frontend::rust_type_display::public_bridge_roots(&field.ty, &layout.type_params)
                    .map_err(|message| {
                        GenerationError::TypeCheck(vec![CompileError::type_error(message, Default::default())])
                    })?;
                self.provider_rust_bridge_roots.extend(roots);
            }
        }
        Ok(())
    }

    /// Check for `import this`
    fn check_for_this_import(&mut self, program: &Program) {
        if scan_check_for_this_import(program) {
            self.emit_zen_in_main = true;
        }
    }

    // =========================================================================
    // Code Generation - Main Entry Points
    // =========================================================================

    /// Generate Rust code from an Incan program (single-file mode)
    ///
    /// This is the main entry point for code generation. It:
    /// 1. Scans for feature usage (serde, async, web, etc.)
    /// 2. Lowers the AST to IR
    /// 3. Emits Rust code using syn/quote
    /// 4. Formats with prettyplease
    ///
    /// **Note**: This is a convenience method that returns error comments on failure.
    /// For production use, prefer [`try_generate`](Self::try_generate) which returns
    /// a proper `Result`.
    #[tracing::instrument(skip_all)]
    pub fn generate(mut self, program: &'a Program) -> String {
        match self.try_generate_internal(program) {
            Ok(code) => code,
            Err(e) => format!("// Generation error: {}\n", e),
        }
    }

    /// Generate Rust code from an Incan program (single-file mode, fallible)
    ///
    /// This is the recommended entry point for code generation. It:
    /// 1. Scans for feature usage (serde, async, web, etc.)
    /// 2. Lowers the AST to IR
    /// 3. Emits Rust code using syn/quote
    /// 4. Formats with prettyplease
    ///
    /// ## Errors
    ///
    /// Returns `GenerationError::TypeCheck` if the module or one of its participating dependencies fails
    /// typechecking, `GenerationError::Lowering` if AST lowering fails, or `GenerationError::Emission` if IR emission
    /// fails.
    ///
    /// ## Examples
    ///
    /// ```rust,ignore
    /// use incan::backend::IrCodegen;
    ///
    /// let codegen = IrCodegen::new();
    /// let rust_code = codegen.try_generate(&ast)?;
    /// ```
    #[tracing::instrument(skip_all)]
    pub fn try_generate(mut self, program: &'a Program) -> Result<String, GenerationError> {
        self.try_generate_internal(program)
    }

    /// Generate one library root and return the compiler metadata inferred from the same lowered IR.
    pub(crate) fn try_generate_with_metadata(
        mut self,
        program: &'a Program,
        root_module_path: &[String],
    ) -> Result<(String, IrGenerationMetadata), GenerationError> {
        self.metadata_root_module_path = Some(root_module_path.to_vec());
        let code = self.try_generate_internal(program)?;
        Ok((
            code,
            IrGenerationMetadata {
                implementation_bound_requirements: std::mem::take(&mut self.implementation_bound_requirements),
            },
        ))
    }

    /// Internal implementation of try_generate (takes &mut self)
    fn try_generate_internal(&mut self, program: &'a Program) -> Result<String, GenerationError> {
        self.current_program = Some(program);
        self.implementation_bound_requirements.clear();

        // Scan for emission-relevant features
        self.update_serde_requirement(program);
        self.collect_rust_crates(program);
        self.check_for_this_import(program);
        self.collect_external_rust_functions(program);

        // Scan dependencies
        for (_mod_name, dep_ast, _mod_path_segments) in &self.dependency_modules.clone() {
            self.update_serde_requirement(dep_ast);
            self.collect_rust_crates(dep_ast);
            self.collect_external_rust_functions(dep_ast);
        }

        // Use the IR pipeline: AST → IR → Rust
        self.try_generate_via_ir(program, &HashSet::new())
    }

    /// Generate code via the IR pipeline (fallible version)
    fn try_generate_via_ir(
        &mut self,
        program: &Program,
        internal_module_roots: &HashSet<String>,
    ) -> Result<String, GenerationError> {
        self.try_generate_via_ir_with_union_config(program, internal_module_roots, IrGenerationOptions::ordinary())
    }

    /// Generate code via the IR pipeline with optional crate-root union sharing for multi-file source modules.
    fn try_generate_via_ir_with_union_config(
        &mut self,
        program: &Program,
        internal_module_roots: &HashSet<String>,
        mut options: IrGenerationOptions<'_>,
    ) -> Result<String, GenerationError> {
        let dependency_modules = self.dependency_modules.clone();
        let dependency_symbol_modules = self.dependency_modules_for_symbol_metadata();
        let compiled_stdlib_metadata_programs = self.compiled_sdk_metadata_programs()?;
        let deps: Vec<(&str, &Program)> = dependency_modules.iter().map(|(name, ast, _)| (*name, *ast)).collect();

        // RFC 021: Make alias-aware lowering work across module boundaries by seeding alias maps
        // for models declared in dependency modules as well.
        let global_aliases = collect_model_field_aliases(program, &deps);
        let dependency_symbol_metadata = collect_dependency_symbol_metadata(&dependency_symbol_modules);
        let uses_std_ordinal_contract = compilation_imports_std_ordinal_contract(program, &dependency_symbol_modules);
        let ordinal_bridge = self.ordinal_bridge_config(uses_std_ordinal_contract);
        let string_try_from_bridge = self.string_try_from_bridge_config(
            compilation_imports_std_string_try_from_contract(program, &dependency_symbol_modules),
        );
        let (needs_serialize, needs_deserialize) = collect_serde_derives(program, &deps);
        let root_module_path = source_module_identity_path(
            program,
            self.root_source_module_name
                .as_deref()
                .map(|name| name.split('.').map(str::to_owned).collect()),
            None,
        );

        // Typecheck to obtain reusable type information for lowering.
        //
        // Strict policy: if typechecking fails, do NOT proceed to lowering/codegen.
        let type_info_opt = if let Some(type_info) = self.prechecked_main_type_info.clone() {
            type_info
        } else {
            use crate::frontend::typechecker::TypeChecker;
            let mut tc = TypeChecker::new();
            self.configure_typechecker(&mut tc, root_module_path.as_deref());
            Self::register_dependency_module_paths(&mut tc, &dependency_modules);
            tc.set_current_module_path(root_module_path.clone());
            let typecheck_deps = self.imported_dependency_modules_for_program(program, &dependency_modules, None);
            let result = match tc.check_with_imports(program, &typecheck_deps) {
                Ok(()) => tc.type_info().clone(),
                Err(errs) => return Err(GenerationError::TypeCheck(errs)),
            };
            self.capture_typechecker_stdlib_cache(&tc);
            result
        };
        self.collect_provider_rust_bridge_roots(&type_info_opt)?;

        // Lower AST to IR using typechecker output when available
        let mut lowering = AstLowering::new_with_type_info(type_info_opt);
        self.configure_lowering(&mut lowering);
        lowering.set_current_source_module_name(root_module_path.as_ref().map(|path| path.join(".")));
        lowering.seed_dependency_trait_decls(&dependency_modules)?;
        lowering.seed_struct_field_aliases(global_aliases.clone());
        let mut ir_program = lowering.lower_program(program)?;
        if self.needs_serde {
            add_serde_to_newtypes(&mut ir_program, needs_serialize, needs_deserialize);
        }

        // RFC 023: Infer trait bounds for generic functions.
        super::trait_bound_inference::infer_trait_bounds(&mut ir_program);
        if let Some(reachable_items) = options.direct_generated_path_support_items {
            record_direct_generated_path_support_items_from_ir(reachable_items, &ir_program);
        }
        let callable_name_use_facts =
            IrEmitter::callable_name_use_facts_for_program(&ir_program, &self.externally_reachable_items, true);
        let needs_function_arg_signatures = callable_name_use_facts.generic_trait_used
            || options.collect_function_arg_signatures_for_imported_generic_callable_name_trait;
        if let Some(used_keys) = options.callable_name_used_signature_keys.as_deref_mut() {
            used_keys.extend(callable_name_use_facts.signature_keys.iter().cloned());
            if needs_function_arg_signatures {
                used_keys.extend(callable_name_use_facts.function_arg_signature_keys.iter().cloned());
            }
        }
        if let Some(resolutions) = options.callable_name_resolutions.as_deref_mut() {
            IrEmitter::add_callable_name_resolutions_for_program(resolutions, Vec::new(), &ir_program);
        }
        let callable_name_resolutions_for_emit = options
            .callable_name_resolutions
            .as_ref()
            .map(|resolutions| (**resolutions).clone())
            .unwrap_or_default();
        let mut callable_name_used_signature_keys_for_emit = options
            .callable_name_used_signature_keys
            .as_ref()
            .map(|used_keys| (**used_keys).clone())
            .unwrap_or_default();
        if needs_function_arg_signatures {
            callable_name_used_signature_keys_for_emit.extend(callable_name_use_facts.function_arg_signature_keys);
        }

        let mut dependency_ir_programs = Vec::new();
        for (dep_name, dep_ast, dep_path_segments) in dependency_modules.clone() {
            let canonical_dep_path_segments = self
                .source_dependency_module_paths
                .iter()
                .find_map(|(source, path)| std::ptr::eq(*source, dep_ast).then_some(path.clone()))
                .or(dep_path_segments.clone());
            let dep_path = canonical_dep_path_segments
                .clone()
                .unwrap_or_else(|| vec![dep_name.to_string()]);
            let dep_type_info = if let Some(type_info) = self.prechecked_dependency_type_info(&dep_path) {
                type_info
            } else {
                use crate::frontend::typechecker::TypeChecker;
                let mut tc = TypeChecker::new();
                self.configure_typechecker(&mut tc, Some(dep_path.as_slice()));
                Self::register_dependency_module_paths(&mut tc, &dependency_modules);
                tc.set_current_module_path(Some(dep_path.clone()));
                let dep_key = Self::dependency_module_key(dep_name, &dep_path_segments);
                let typecheck_deps =
                    self.imported_dependency_modules_for_program(dep_ast, &dependency_modules, Some(&dep_key));
                let result = match tc.check_with_imports_allow_private(dep_ast, &typecheck_deps) {
                    Ok(()) => tc.type_info().clone(),
                    Err(errs) => return Err(Self::typecheck_errors_for_module(&dep_key, errs)),
                };
                self.capture_typechecker_stdlib_cache(&tc);
                result
            };
            let mut dep_lowering = AstLowering::new_with_type_info(dep_type_info);
            self.configure_lowering(&mut dep_lowering);
            dep_lowering.set_current_source_module_name(
                canonical_dep_path_segments
                    .clone()
                    .map(|segments| segments.join("."))
                    .or_else(|| {
                        dep_ast
                            .source_path
                            .as_deref()
                            .and_then(crate::frontend::module::logical_module_name_from_source_path)
                    }),
            );
            dep_lowering.seed_dependency_trait_decls(&dependency_modules)?;
            dep_lowering.seed_struct_field_aliases(global_aliases.clone());
            let mut dep_ir = dep_lowering.lower_program(dep_ast)?;
            super::trait_bound_inference::infer_trait_bounds(&mut dep_ir);
            let module_path = canonical_dep_path_segments.unwrap_or_else(|| vec![dep_name.to_string()]);
            dependency_ir_programs.push((module_path, dep_ir));
        }
        let dependency_programs = dependency_ir_programs
            .iter()
            .map(|(_, dep_ir)| dep_ir)
            .collect::<Vec<_>>();
        super::trait_bound_inference::propagate_trait_bounds_from_programs(&mut ir_program, &dependency_programs);
        let root_module_path = self.metadata_root_module_path.clone().unwrap_or_else(|| {
            ir_program
                .source_module_name
                .as_deref()
                .map(source_module_path_segments)
                .unwrap_or_default()
        });
        self.capture_implementation_bound_requirements(root_module_path, &ir_program);
        for (module_path, dependency_program) in &dependency_ir_programs {
            self.capture_implementation_bound_requirements(module_path.clone(), dependency_program);
        }
        let source_module_paths = dependency_ir_programs
            .iter()
            .map(|(module_path, _)| module_path.clone())
            .collect::<HashSet<_>>();
        let canonical_registry = Self::canonical_registry_for_programs(
            dependency_ir_programs
                .iter()
                .map(|(module_path, dep_ir)| (module_path.as_slice(), dep_ir))
                .chain(
                    compiled_stdlib_metadata_programs
                        .iter()
                        .map(|(module_path, dep_ir)| (module_path.as_slice(), dep_ir)),
                ),
        );

        // Emit IR to Rust code
        let use_emit_service = env::var("INCAN_EMIT_SERVICE").ok().as_deref() == Some("1");
        if use_emit_service {
            let mut svc = EmitService::new_from_program(&ir_program);
            // Configure inner emitter
            let inner = svc.inner_mut();
            self.apply_canonical_emission_context(inner);
            inner.set_internal_module_roots(internal_module_roots.clone());
            Self::configure_source_import_paths(inner, ir_program.source_module_name.as_deref(), &source_module_paths);
            if self.emit_zen_in_main {
                inner.set_emit_zen(true);
            }
            Self::apply_dependency_symbol_metadata(inner, &dependency_symbol_metadata, self.provider_plan.as_deref());
            inner.set_needs_serde(self.needs_serde);
            inner.set_external_rust_functions(self.external_rust_functions.clone());
            inner.set_strict_generated_lints(self.strict_generated_lints);
            inner.set_externally_reachable_items(self.externally_reachable_items.clone());
            self.apply_capability_bridge_configs(inner, &ordinal_bridge, &string_try_from_bridge);
            inner.set_qualify_union_types_from_crate(options.qualify_union_types_from_crate);
            inner.set_generated_union_types(options.generated_union_types);
            inner.set_canonical_function_registry(canonical_registry.clone());
            inner.set_callable_name_current_module_path(Vec::new());
            inner.set_callable_name_resolutions(callable_name_resolutions_for_emit);
            inner.set_callable_name_used_signature_keys(callable_name_used_signature_keys_for_emit);
            inner.set_callable_name_local_registry(ir_program.function_registry.clone());
            for (_, dep_ir) in &dependency_ir_programs {
                inner.seed_dependency_nominal_metadata_from_program(dep_ir);
            }
            for (_, dep_ir) in &compiled_stdlib_metadata_programs {
                inner.seed_dependency_nominal_metadata_from_program(dep_ir);
            }
            Ok(svc.emit_program(&ir_program)?)
        } else {
            let mut emitter = IrEmitter::new(&ir_program.function_registry);
            self.apply_canonical_emission_context(&mut emitter);
            emitter.set_internal_module_roots(internal_module_roots.clone());
            Self::configure_source_import_paths(
                &mut emitter,
                ir_program.source_module_name.as_deref(),
                &source_module_paths,
            );
            if self.emit_zen_in_main {
                emitter.set_emit_zen(true);
            }
            Self::apply_dependency_symbol_metadata(
                &mut emitter,
                &dependency_symbol_metadata,
                self.provider_plan.as_deref(),
            );
            emitter.set_needs_serde(self.needs_serde);
            emitter.set_external_rust_functions(self.external_rust_functions.clone());
            emitter.set_strict_generated_lints(self.strict_generated_lints);
            emitter.set_externally_reachable_items(self.externally_reachable_items.clone());
            self.apply_capability_bridge_configs(&mut emitter, &ordinal_bridge, &string_try_from_bridge);
            emitter.set_qualify_union_types_from_crate(options.qualify_union_types_from_crate);
            emitter.set_generated_union_types(options.generated_union_types);
            emitter.set_canonical_function_registry(canonical_registry.clone());
            emitter.set_callable_name_current_module_path(Vec::new());
            emitter.set_callable_name_resolutions(callable_name_resolutions_for_emit);
            emitter.set_callable_name_used_signature_keys(callable_name_used_signature_keys_for_emit);
            emitter.set_callable_name_local_registry(ir_program.function_registry.clone());
            for (_, dep_ir) in &dependency_ir_programs {
                emitter.seed_dependency_nominal_metadata_from_program(dep_ir);
            }
            for (_, dep_ir) in &compiled_stdlib_metadata_programs {
                emitter.seed_dependency_nominal_metadata_from_program(dep_ir);
            }
            Ok(emitter.emit_program(&ir_program)?)
        }
    }

    /// Generate Rust code for a dependency module (not the main module)
    ///
    /// **Note**: This is a convenience method that returns error comments on failure.
    /// For production use, prefer [`try_generate_module`](Self::try_generate_module).
    pub fn generate_module(&mut self, module_name: &str, program: &Program) -> String {
        match self.try_generate_module(module_name, program) {
            Ok(code) => code,
            Err(e) => format!("// Generation error: {}\n", e),
        }
    }

    /// Generate Rust code for a dependency module (not the main module, fallible)
    ///
    /// ## Errors
    ///
    /// Returns `GenerationError::TypeCheck` if module typechecking fails, `GenerationError::Lowering` if AST lowering
    /// fails, or `GenerationError::Emission` if IR emission fails.
    pub fn try_generate_module(&mut self, module_name: &str, program: &Program) -> Result<String, GenerationError> {
        let dependency_modules = self.dependency_modules.clone();
        let deps: Vec<(&str, &Program)> = dependency_modules.iter().map(|(name, ast, _)| (*name, *ast)).collect();
        let global_aliases = collect_model_field_aliases(program, &deps);
        let module_metadata = dependency_modules
            .iter()
            .find(|(name, ast, _)| *name == module_name && std::ptr::eq(*ast, program));
        let module_key = module_metadata
            .map(|(name, _, path_segments)| Self::dependency_module_key(name, path_segments))
            .unwrap_or_else(|| module_name.to_string());
        let module_path_segments = module_metadata.and_then(|(_, _, path_segments)| path_segments.clone());
        let module_identity_path =
            source_module_identity_path(program, module_path_segments.clone(), Some(module_name));
        let module_type_info = {
            use crate::frontend::typechecker::TypeChecker;
            let mut tc = TypeChecker::new();
            self.configure_typechecker(&mut tc, module_identity_path.as_deref());
            Self::register_dependency_module_paths(&mut tc, &dependency_modules);
            tc.set_current_module_path(module_identity_path.clone());
            let typecheck_deps =
                self.imported_dependency_modules_for_program(program, &dependency_modules, Some(&module_key));
            let result = match tc.check_with_imports_allow_private(program, &typecheck_deps) {
                Ok(()) => tc.type_info().clone(),
                Err(errs) => return Err(Self::typecheck_errors_for_module(&module_key, errs)),
            };
            self.capture_typechecker_stdlib_cache(&tc);
            result
        };
        self.collect_provider_rust_bridge_roots(&module_type_info)?;
        // Use the IR pipeline for module generation too
        let mut lowering = AstLowering::new_with_type_info(module_type_info);
        self.configure_lowering(&mut lowering);
        lowering.set_current_source_module_name(module_identity_path.as_ref().map(|path| path.join(".")));
        lowering.seed_dependency_trait_decls(&dependency_modules)?;
        lowering.seed_struct_field_aliases(global_aliases.clone());
        let mut ir_program = lowering.lower_program(program)?;

        // RFC 023: Infer trait bounds for generic functions.
        super::trait_bound_inference::infer_trait_bounds(&mut ir_program);
        let mut dependency_ir_programs = Vec::new();
        for (dep_name, dep_ast, dep_path_segments) in dependency_modules.clone() {
            if dep_name == module_name {
                continue;
            }
            let dep_key = Self::dependency_module_key(dep_name, &dep_path_segments);
            let dep_identity_path = source_module_identity_path(dep_ast, dep_path_segments.clone(), Some(dep_name));
            let dep_type_info = {
                use crate::frontend::typechecker::TypeChecker;
                let mut tc = TypeChecker::new();
                self.configure_typechecker(&mut tc, dep_identity_path.as_deref());
                Self::register_dependency_module_paths(&mut tc, &dependency_modules);
                tc.set_current_module_path(dep_identity_path.clone());
                let typecheck_deps =
                    self.imported_dependency_modules_for_program(dep_ast, &dependency_modules, Some(&dep_key));
                let result = match tc.check_with_imports_allow_private(dep_ast, &typecheck_deps) {
                    Ok(()) => tc.type_info().clone(),
                    Err(errs) => return Err(Self::typecheck_errors_for_module(&dep_key, errs)),
                };
                self.capture_typechecker_stdlib_cache(&tc);
                result
            };
            let mut dep_lowering = AstLowering::new_with_type_info(dep_type_info);
            self.configure_lowering(&mut dep_lowering);
            dep_lowering.set_current_source_module_name(dep_identity_path.as_ref().map(|path| path.join(".")));
            dep_lowering.seed_dependency_trait_decls(&dependency_modules)?;
            dep_lowering.seed_struct_field_aliases(global_aliases.clone());
            let mut dep_ir = dep_lowering.lower_program(dep_ast)?;
            super::trait_bound_inference::infer_trait_bounds(&mut dep_ir);
            dependency_ir_programs.push(dep_ir);
        }
        let dependency_programs = dependency_ir_programs.iter().collect::<Vec<_>>();
        super::trait_bound_inference::propagate_trait_bounds_from_programs(&mut ir_program, &dependency_programs);

        // Best-effort: treat registered dependency module names as internal roots.
        // (This is most relevant for the non-nested multi-file API.)
        let internal_roots: HashSet<String> = self
            .dependency_modules
            .iter()
            .map(|(name, _, _)| (*name).to_string())
            .collect();

        let ordinal_bridge = OrdinalBridgeConfig::for_internal_module(imports_std_ordinal_contract(program));
        let string_try_from_bridge =
            StringTryFromBridgeConfig::for_internal_module(imports_std_string_try_from_contract(program));
        let use_emit_service = env::var("INCAN_EMIT_SERVICE").ok().as_deref() == Some("1");
        if use_emit_service {
            let mut svc = EmitService::new_from_program(&ir_program);
            let inner = svc.inner_mut();
            self.apply_canonical_emission_context(inner);
            inner.set_internal_module_roots(internal_roots);
            inner.set_externally_reachable_items(self.externally_reachable_items.clone());
            self.apply_capability_bridge_configs(inner, &ordinal_bridge, &string_try_from_bridge);
            Ok(svc.emit_program(&ir_program)?)
        } else {
            let mut emitter = IrEmitter::new(&ir_program.function_registry);
            self.apply_canonical_emission_context(&mut emitter);
            emitter.set_internal_module_roots(internal_roots);
            if self.emit_zen_in_main {
                emitter.set_emit_zen(true);
            }
            emitter.set_needs_serde(self.needs_serde);
            emitter.set_externally_reachable_items(self.externally_reachable_items.clone());
            self.apply_capability_bridge_configs(&mut emitter, &ordinal_bridge, &string_try_from_bridge);
            Ok(emitter.emit_program(&ir_program)?)
        }
    }

    /// Generate Rust code for a multi-file project
    ///
    /// **Note**: This is a convenience method that returns error comments on failure.
    /// For production use, prefer [`try_generate_multi_file`](Self::try_generate_multi_file).
    pub fn generate_multi_file(
        mut self,
        program: &'a Program,
        module_names: &[&str],
    ) -> (String, HashMap<String, String>) {
        match self.try_generate_multi_file_internal(program, module_names) {
            Ok(result) => result,
            Err(e) => (format!("// Generation error: {}\n", e), HashMap::new()),
        }
    }

    /// Generate Rust code for a multi-file project (fallible)
    ///
    /// ## Errors
    ///
    /// Returns `GenerationError::Lowering` if AST lowering fails for any module, or
    /// `GenerationError::Emission` if IR emission fails for any module.
    pub fn try_generate_multi_file(
        mut self,
        program: &'a Program,
        module_names: &[&str],
    ) -> Result<(String, HashMap<String, String>), GenerationError> {
        self.try_generate_multi_file_internal(program, module_names)
    }

    /// Generate flat dependency modules with generated-use pruning.
    ///
    /// Dependency modules keep imported/reachable declarations for binary-style emission and can preserve non-stdlib
    /// public items when library surfaces are being generated.
    fn try_generate_multi_file_internal(
        &mut self,
        program: &'a Program,
        module_names: &[&str],
    ) -> Result<(String, HashMap<String, String>), GenerationError> {
        self.current_program = Some(program);
        self.source_dependency_module_paths.clear();

        // Scan all modules for emission-relevant features
        self.update_serde_requirement(program);
        self.collect_rust_crates(program);

        for (_mod_name, dep_ast, _mod_path_segments) in &self.dependency_modules.clone() {
            self.update_serde_requirement(dep_ast);
            self.collect_rust_crates(dep_ast);
        }

        let internal_roots: HashSet<String> = module_names.iter().map(|s| (*s).to_string()).collect();

        let dependency_modules = self.dependency_modules.clone();
        let dependency_symbol_modules = self.dependency_modules_for_symbol_metadata();
        let compiled_stdlib_metadata_programs = self.compiled_sdk_metadata_programs()?;
        let deps: Vec<(&str, &Program)> = dependency_modules.iter().map(|(name, ast, _)| (*name, *ast)).collect();
        let global_aliases = collect_model_field_aliases(program, &deps);
        let dependency_symbol_metadata = collect_dependency_symbol_metadata(&dependency_symbol_modules);
        let uses_std_ordinal_contract = compilation_imports_std_ordinal_contract(program, &dependency_symbol_modules);
        let ordinal_bridge = OrdinalBridgeConfig::for_internal_module(uses_std_ordinal_contract);
        let string_try_from_bridge = StringTryFromBridgeConfig::for_internal_module(
            compilation_imports_std_string_try_from_contract(program, &dependency_symbol_modules),
        );
        let mut dependency_reachable_items = collect_externally_reachable_items_by_module(program, &dependency_modules);

        // Generate module files
        let mut lowered_modules = Vec::new();
        for (name, ast, path_segments) in dependency_modules.clone() {
            if !module_names.contains(&name) {
                continue;
            }
            let module_identity_path = source_module_identity_path(ast, path_segments.clone(), Some(name));
            let module_type_info = {
                use crate::frontend::typechecker::TypeChecker;
                let mut tc = TypeChecker::new();
                self.configure_typechecker(&mut tc, module_identity_path.as_deref());
                Self::register_dependency_module_paths(&mut tc, &dependency_modules);
                tc.set_current_module_path(module_identity_path.clone());
                let module_key = Self::dependency_module_key(name, &path_segments);
                let typecheck_deps =
                    self.imported_dependency_modules_for_program(ast, &dependency_modules, Some(&module_key));
                let result = match tc.check_with_imports_allow_private(ast, &typecheck_deps) {
                    Ok(()) => tc.type_info().clone(),
                    Err(errs) => return Err(Self::typecheck_errors_for_module(&module_key, errs)),
                };
                self.capture_typechecker_stdlib_cache(&tc);
                result
            };
            self.collect_provider_rust_bridge_roots(&module_type_info)?;
            let mut lowering = AstLowering::new_with_type_info(module_type_info);
            self.configure_lowering(&mut lowering);
            lowering.set_current_source_module_name(module_identity_path.as_ref().map(|path| path.join(".")));
            lowering.seed_dependency_trait_decls(&dependency_modules)?;
            lowering.seed_struct_field_aliases(global_aliases.clone());
            let mut ir = lowering.lower_program(ast)?;
            // Do not auto-add serde derives to dependency modules.
            // Global serde usage in the main module must not mutate unrelated dependency
            // newtypes (e.g., stdlib wrapper types like std.web.request.Query/Path).
            super::trait_bound_inference::infer_trait_bounds(&mut ir);
            record_direct_generated_path_support_items_from_ir(&mut dependency_reachable_items, &ir);
            let module_path = path_segments.clone().unwrap_or_else(|| vec![name.to_string()]);
            self.source_dependency_module_paths.push((ast, module_path.clone()));
            lowered_modules.push((name.to_string(), module_path, ir));
        }
        for idx in 0..lowered_modules.len() {
            let (left, rest) = lowered_modules.split_at_mut(idx);
            let Some((_, current_ir, tail)) = rest
                .split_first_mut()
                .map(|((name, _path, ir), tail)| (name.clone(), ir, tail))
            else {
                continue;
            };
            let external_programs: Vec<&super::IrProgram> = left
                .iter()
                .map(|(_, _, ir)| ir)
                .chain(tail.iter().map(|(_, _, ir)| ir))
                .collect();
            super::trait_bound_inference::propagate_trait_bounds_from_programs(current_ir, &external_programs);
        }
        let all_module_canonical_registry = Self::canonical_registry_for_programs(
            lowered_modules
                .iter()
                .map(|(_, module_path, ir)| (module_path.as_slice(), ir))
                .chain(
                    compiled_stdlib_metadata_programs
                        .iter()
                        .map(|(module_path, ir)| (module_path.as_slice(), ir)),
                ),
        );
        let mut shared_union_types = HashMap::new();
        for (_, _, ir) in &lowered_modules {
            shared_union_types.extend(IrEmitter::collect_union_types_from_program(ir));
        }

        // Generate main file after dependency lowering so it can own shared crate-root union wrappers.
        let mut callable_name_resolutions = HashMap::new();
        let mut callable_name_used_signature_keys = HashSet::new();
        let mut callable_name_function_arg_signature_keys = HashSet::new();
        let mut generic_callable_name_trait_used = false;
        for (_, module_path, ir) in &lowered_modules {
            IrEmitter::add_callable_name_resolutions_for_program(
                &mut callable_name_resolutions,
                module_path.clone(),
                ir,
            );
            let mut reachable_items = dependency_reachable_items.get(module_path).cloned().unwrap_or_default();
            if let Some(injected_items) = self.externally_reachable_items_by_module.get(module_path) {
                reachable_items.extend(injected_items.iter().cloned());
            }
            let preserve_public_items =
                should_preserve_dependency_public_items(module_path, self.preserve_dependency_public_items);
            let callable_name_use_facts =
                IrEmitter::callable_name_use_facts_for_program(ir, &reachable_items, preserve_public_items);
            callable_name_used_signature_keys.extend(callable_name_use_facts.signature_keys);
            callable_name_function_arg_signature_keys.extend(callable_name_use_facts.function_arg_signature_keys);
            generic_callable_name_trait_used |= callable_name_use_facts.generic_trait_used;
        }
        if generic_callable_name_trait_used {
            callable_name_used_signature_keys.extend(callable_name_function_arg_signature_keys);
        }

        let main_code = self.try_generate_via_ir_with_union_config(
            program,
            &internal_roots,
            IrGenerationOptions {
                generated_union_types: shared_union_types,
                qualify_union_types_from_crate: true,
                callable_name_resolutions: Some(&mut callable_name_resolutions),
                callable_name_used_signature_keys: Some(&mut callable_name_used_signature_keys),
                collect_function_arg_signatures_for_imported_generic_callable_name_trait:
                    generic_callable_name_trait_used,
                direct_generated_path_support_items: Some(&mut dependency_reachable_items),
            },
        )?;
        let main_code = self.attach_provider_rust_dependency_bridge(main_code);

        let source_module_paths = lowered_modules
            .iter()
            .map(|(_, module_path, _)| module_path.clone())
            .collect::<HashSet<_>>();
        let mut modules = HashMap::new();
        for (name, module_path, ir) in &lowered_modules {
            let mut reachable_items = dependency_reachable_items.get(module_path).cloned().unwrap_or_default();
            if let Some(injected_items) = self.externally_reachable_items_by_module.get(module_path) {
                reachable_items.extend(injected_items.iter().cloned());
            }
            let preserve_public_items =
                should_preserve_dependency_public_items(module_path, self.preserve_dependency_public_items);
            let use_emit_service = env::var("INCAN_EMIT_SERVICE").ok().as_deref() == Some("1");
            let module_code = if use_emit_service {
                let mut svc = EmitService::new_from_program(ir);
                let inner = svc.inner_mut();
                self.apply_canonical_emission_context(inner);
                inner.set_internal_module_roots(internal_roots.clone());
                Self::configure_source_import_paths(inner, ir.source_module_name.as_deref(), &source_module_paths);
                inner.set_preserve_public_items(preserve_public_items);
                inner.set_externally_reachable_items(reachable_items.clone());
                Self::apply_dependency_symbol_metadata(
                    inner,
                    &dependency_symbol_metadata,
                    self.provider_plan.as_deref(),
                );
                inner.set_external_rust_functions(self.external_rust_functions.clone());
                inner.set_qualify_union_types_from_crate(true);
                inner.set_emit_generated_union_definitions(false);
                inner.set_canonical_function_registry(all_module_canonical_registry.clone());
                inner.set_callable_name_current_module_path(module_path.clone());
                inner.set_callable_name_resolutions(callable_name_resolutions.clone());
                inner.set_callable_name_used_signature_keys(callable_name_used_signature_keys.clone());
                self.apply_capability_bridge_configs(inner, &ordinal_bridge, &string_try_from_bridge);
                for (_, _, dep_ir) in &lowered_modules {
                    inner.seed_dependency_nominal_metadata_from_program(dep_ir);
                }
                for (_, dep_ir) in &compiled_stdlib_metadata_programs {
                    inner.seed_dependency_nominal_metadata_from_program(dep_ir);
                }
                svc.emit_program(ir)?
            } else {
                let mut emitter = IrEmitter::new(&ir.function_registry);
                self.apply_canonical_emission_context(&mut emitter);
                emitter.set_internal_module_roots(internal_roots.clone());
                Self::configure_source_import_paths(
                    &mut emitter,
                    ir.source_module_name.as_deref(),
                    &source_module_paths,
                );
                emitter.set_preserve_public_items(preserve_public_items);
                emitter.set_externally_reachable_items(reachable_items);
                Self::apply_dependency_symbol_metadata(
                    &mut emitter,
                    &dependency_symbol_metadata,
                    self.provider_plan.as_deref(),
                );
                emitter.set_external_rust_functions(self.external_rust_functions.clone());
                emitter.set_qualify_union_types_from_crate(true);
                emitter.set_emit_generated_union_definitions(false);
                emitter.set_canonical_function_registry(all_module_canonical_registry.clone());
                emitter.set_callable_name_current_module_path(module_path.clone());
                emitter.set_callable_name_resolutions(callable_name_resolutions.clone());
                emitter.set_callable_name_used_signature_keys(callable_name_used_signature_keys.clone());
                self.apply_capability_bridge_configs(&mut emitter, &ordinal_bridge, &string_try_from_bridge);
                for (_, _, dep_ir) in &lowered_modules {
                    emitter.seed_dependency_nominal_metadata_from_program(dep_ir);
                }
                for (_, dep_ir) in &compiled_stdlib_metadata_programs {
                    emitter.seed_dependency_nominal_metadata_from_program(dep_ir);
                }
                emitter.emit_program(ir)?
            };
            modules.insert(name.clone(), module_code);
        }

        Ok((main_code, modules))
    }

    /// Generate Rust code for a multi-file project with nested module paths
    ///
    /// **Note**: This is a convenience method that returns error comments on failure.
    /// For production use, prefer [`try_generate_multi_file_nested`](Self::try_generate_multi_file_nested).
    pub fn generate_multi_file_nested(
        mut self,
        program: &'a Program,
        module_paths: &[Vec<String>],
    ) -> (String, HashMap<Vec<String>, String>) {
        match self.try_generate_multi_file_nested_internal(program, module_paths) {
            Ok(result) => result,
            Err(e) => (format!("// Generation error: {}\n", e), HashMap::new()),
        }
    }

    /// Generate Rust code for a multi-file project with nested module paths (fallible)
    ///
    /// ## Errors
    ///
    /// Returns `GenerationError::Lowering` if AST lowering fails for any module, or
    /// `GenerationError::Emission` if IR emission fails for any module.
    pub fn try_generate_multi_file_nested(
        mut self,
        program: &'a Program,
        module_paths: &[Vec<String>],
    ) -> Result<(String, HashMap<Vec<String>, String>), GenerationError> {
        self.try_generate_multi_file_nested_internal(program, module_paths)
    }

    /// Generate a nested library project and return metadata inferred from the same lowered IR.
    pub(crate) fn try_generate_multi_file_nested_with_metadata(
        mut self,
        program: &'a Program,
        module_paths: &[Vec<String>],
        root_module_path: &[String],
    ) -> Result<NestedLibraryGeneration, GenerationError> {
        self.metadata_root_module_path = Some(root_module_path.to_vec());
        let generated = self.try_generate_multi_file_nested_internal(program, module_paths)?;
        Ok((
            generated,
            IrGenerationMetadata {
                implementation_bound_requirements: std::mem::take(&mut self.implementation_bound_requirements),
            },
        ))
    }

    /// Generate nested dependency modules with generated-use pruning.
    ///
    /// Dependency modules keep imported/reachable declarations for binary-style emission and can preserve non-stdlib
    /// public items when library surfaces are being generated.
    fn try_generate_multi_file_nested_internal(
        &mut self,
        program: &'a Program,
        module_paths: &[Vec<String>],
    ) -> Result<(String, HashMap<Vec<String>, String>), GenerationError> {
        self.current_program = Some(program);
        self.source_dependency_module_paths.clear();
        self.implementation_bound_requirements.clear();

        // Backfill nested module path segments for dependency modules when they were registered
        // via the legacy `add_module()` API (flat names only).
        //
        // The CLI typically registers both: a flat name like "api_routes" and the nested path
        // segments ["api", "routes"]. Tests may register only the flat name.
        for path in module_paths {
            let flat = path.join("_");
            if let Some((_name, _ast, segs)) = self
                .dependency_modules
                .iter_mut()
                .find(|(name, _, _)| *name == flat.as_str())
                && segs.is_none()
            {
                *segs = Some(path.clone());
            }
        }

        // Scan all modules for emission-relevant features
        self.update_serde_requirement(program);
        self.collect_rust_crates(program);

        for (_mod_name, dep_ast, _mod_path_segments) in &self.dependency_modules.clone() {
            self.update_serde_requirement(dep_ast);
            self.collect_rust_crates(dep_ast);
        }

        let internal_roots: HashSet<String> = module_paths.iter().filter_map(|p| p.first().cloned()).collect();

        let dependency_modules = self.dependency_modules.clone();
        let dependency_symbol_modules = self.dependency_modules_for_symbol_metadata();
        let compiled_stdlib_metadata_programs = self.compiled_sdk_metadata_programs()?;
        let deps: Vec<(&str, &Program)> = dependency_modules.iter().map(|(name, ast, _)| (*name, *ast)).collect();
        let global_aliases = collect_model_field_aliases(program, &deps);
        let dependency_symbol_metadata = collect_dependency_symbol_metadata(&dependency_symbol_modules);
        let uses_std_ordinal_contract = compilation_imports_std_ordinal_contract(program, &dependency_symbol_modules);
        let ordinal_bridge = OrdinalBridgeConfig::for_internal_module(uses_std_ordinal_contract);
        let string_try_from_bridge = StringTryFromBridgeConfig::for_internal_module(
            compilation_imports_std_string_try_from_contract(program, &dependency_symbol_modules),
        );
        let mut dependency_reachable_items = collect_externally_reachable_items_by_module(program, &dependency_modules);

        // Generate module files by path
        let mut lowered_modules = Vec::new();
        for (name, ast, stored_path_segments) in dependency_modules.clone() {
            let matching_path = if let Some(stored_path_segments) = &stored_path_segments {
                module_paths.iter().find(|path| *path == stored_path_segments)
            } else {
                // Legacy callers may still register only a flat module name. Prefer explicit path segments when they
                // exist because distinct paths such as `a_b` and `a/b` share the same underscore-joined fallback.
                module_paths.iter().find(|path| path.join("_") == *name)
            };
            if let Some(path) = matching_path {
                let module_type_info = if let Some(type_info) = self.prechecked_dependency_type_info(path) {
                    type_info
                } else {
                    use crate::frontend::typechecker::TypeChecker;
                    let mut tc = TypeChecker::new();
                    self.configure_typechecker(&mut tc, Some(path.as_slice()));
                    Self::register_dependency_module_paths(&mut tc, &dependency_modules);
                    tc.set_current_module_path(Some(canonicalize_source_module_segments(path)));
                    let self_key = canonicalize_source_module_segments(path).join("_");
                    let typecheck_deps =
                        self.imported_dependency_modules_for_program(ast, &dependency_modules, Some(&self_key));
                    let result = if self.public_typecheck_module_paths.contains(path) {
                        tc.check_with_imports(ast, &typecheck_deps)
                    } else {
                        tc.check_with_imports_allow_private(ast, &typecheck_deps)
                    };
                    let result = match result {
                        Ok(()) => tc.type_info().clone(),
                        Err(errs) => {
                            return Err(Self::typecheck_errors_for_module(&path.join("."), errs));
                        }
                    };
                    self.capture_typechecker_stdlib_cache(&tc);
                    result
                };
                self.collect_provider_rust_bridge_roots(&module_type_info)?;
                let mut lowering = AstLowering::new_with_type_info(module_type_info);
                self.configure_lowering(&mut lowering);
                lowering.set_current_source_module_name(Some(path.join(".")));
                lowering.seed_dependency_trait_decls(&dependency_modules)?;
                lowering.seed_struct_field_aliases(global_aliases.clone());
                let mut ir = lowering.lower_program(ast)?;
                // Do not auto-add serde derives to dependency modules.
                // Global serde usage in the main module must not mutate unrelated dependency
                // newtypes (e.g., stdlib wrapper types like std.web.request.Query/Path).
                super::trait_bound_inference::infer_trait_bounds(&mut ir);
                record_direct_generated_path_support_items_from_ir(&mut dependency_reachable_items, &ir);
                self.source_dependency_module_paths.push((ast, path.clone()));
                lowered_modules.push((path.clone(), ir));
            }
        }
        for idx in 0..lowered_modules.len() {
            let (left, rest) = lowered_modules.split_at_mut(idx);
            let Some((_, current_ir, tail)) = rest
                .split_first_mut()
                .map(|((path, ir), tail)| (path.clone(), ir, tail))
            else {
                continue;
            };
            let external_programs: Vec<&super::IrProgram> = left
                .iter()
                .map(|(_, ir)| ir)
                .chain(tail.iter().map(|(_, ir)| ir))
                .collect();
            super::trait_bound_inference::propagate_trait_bounds_from_programs(current_ir, &external_programs);
        }
        let all_module_canonical_registry = Self::canonical_registry_for_programs(
            lowered_modules.iter().map(|(path, ir)| (path.as_slice(), ir)).chain(
                compiled_stdlib_metadata_programs
                    .iter()
                    .map(|(path, ir)| (path.as_slice(), ir)),
            ),
        );
        let mut shared_union_types = HashMap::new();
        for (_, ir) in &lowered_modules {
            shared_union_types.extend(IrEmitter::collect_union_types_from_program(ir));
        }

        // Generate main file after dependency lowering so it can own shared crate-root union wrappers.
        let mut callable_name_resolutions = HashMap::new();
        let mut callable_name_used_signature_keys = HashSet::new();
        let mut callable_name_function_arg_signature_keys = HashSet::new();
        let mut generic_callable_name_trait_used = false;
        for (path, ir) in &lowered_modules {
            IrEmitter::add_callable_name_resolutions_for_program(&mut callable_name_resolutions, path.clone(), ir);
            let mut reachable_items = dependency_reachable_items.get(path).cloned().unwrap_or_default();
            if let Some(injected_items) = self.externally_reachable_items_by_module.get(path) {
                reachable_items.extend(injected_items.iter().cloned());
            }
            let preserve_public_items =
                should_preserve_dependency_public_items(path, self.preserve_dependency_public_items);
            let callable_name_use_facts =
                IrEmitter::callable_name_use_facts_for_program(ir, &reachable_items, preserve_public_items);
            callable_name_used_signature_keys.extend(callable_name_use_facts.signature_keys);
            callable_name_function_arg_signature_keys.extend(callable_name_use_facts.function_arg_signature_keys);
            generic_callable_name_trait_used |= callable_name_use_facts.generic_trait_used;
        }
        if generic_callable_name_trait_used {
            callable_name_used_signature_keys.extend(callable_name_function_arg_signature_keys);
        }

        let main_code = self.try_generate_via_ir_with_union_config(
            program,
            &internal_roots,
            IrGenerationOptions {
                generated_union_types: shared_union_types,
                qualify_union_types_from_crate: true,
                callable_name_resolutions: Some(&mut callable_name_resolutions),
                callable_name_used_signature_keys: Some(&mut callable_name_used_signature_keys),
                collect_function_arg_signatures_for_imported_generic_callable_name_trait:
                    generic_callable_name_trait_used,
                direct_generated_path_support_items: Some(&mut dependency_reachable_items),
            },
        )?;
        let main_code = self.attach_provider_rust_dependency_bridge(main_code);

        let source_module_paths = lowered_modules
            .iter()
            .map(|(module_path, _)| module_path.clone())
            .collect::<HashSet<_>>();
        let mut modules = HashMap::new();
        for (path, ir) in &lowered_modules {
            let mut reachable_items = dependency_reachable_items.get(path).cloned().unwrap_or_default();
            if let Some(injected_items) = self.externally_reachable_items_by_module.get(path) {
                reachable_items.extend(injected_items.iter().cloned());
            }
            let preserve_public_items =
                should_preserve_dependency_public_items(path, self.preserve_dependency_public_items);
            let use_emit_service = env::var("INCAN_EMIT_SERVICE").ok().as_deref() == Some("1");
            let module_code = if use_emit_service {
                let mut svc = EmitService::new_from_program(ir);
                let inner = svc.inner_mut();
                self.apply_canonical_emission_context(inner);
                inner.set_internal_module_roots(internal_roots.clone());
                Self::configure_source_import_paths(inner, ir.source_module_name.as_deref(), &source_module_paths);
                inner.set_preserve_public_items(preserve_public_items);
                inner.set_externally_reachable_items(reachable_items.clone());
                Self::apply_dependency_symbol_metadata(
                    inner,
                    &dependency_symbol_metadata,
                    self.provider_plan.as_deref(),
                );
                inner.set_external_rust_functions(self.external_rust_functions.clone());
                inner.set_qualify_union_types_from_crate(true);
                inner.set_emit_generated_union_definitions(false);
                inner.set_canonical_function_registry(all_module_canonical_registry.clone());
                inner.set_callable_name_current_module_path(path.clone());
                inner.set_callable_name_resolutions(callable_name_resolutions.clone());
                inner.set_callable_name_used_signature_keys(callable_name_used_signature_keys.clone());
                self.apply_capability_bridge_configs(inner, &ordinal_bridge, &string_try_from_bridge);
                for (_, dep_ir) in &lowered_modules {
                    inner.seed_dependency_nominal_metadata_from_program(dep_ir);
                }
                for (_, dep_ir) in &compiled_stdlib_metadata_programs {
                    inner.seed_dependency_nominal_metadata_from_program(dep_ir);
                }
                svc.emit_program(ir)?
            } else {
                let mut emitter = IrEmitter::new(&ir.function_registry);
                self.apply_canonical_emission_context(&mut emitter);
                emitter.set_internal_module_roots(internal_roots.clone());
                Self::configure_source_import_paths(
                    &mut emitter,
                    ir.source_module_name.as_deref(),
                    &source_module_paths,
                );
                emitter.set_preserve_public_items(preserve_public_items);
                emitter.set_externally_reachable_items(reachable_items);
                Self::apply_dependency_symbol_metadata(
                    &mut emitter,
                    &dependency_symbol_metadata,
                    self.provider_plan.as_deref(),
                );
                emitter.set_external_rust_functions(self.external_rust_functions.clone());
                emitter.set_qualify_union_types_from_crate(true);
                emitter.set_emit_generated_union_definitions(false);
                emitter.set_canonical_function_registry(all_module_canonical_registry.clone());
                emitter.set_callable_name_current_module_path(path.clone());
                emitter.set_callable_name_resolutions(callable_name_resolutions.clone());
                emitter.set_callable_name_used_signature_keys(callable_name_used_signature_keys.clone());
                self.apply_capability_bridge_configs(&mut emitter, &ordinal_bridge, &string_try_from_bridge);
                for (_, dep_ir) in &lowered_modules {
                    emitter.seed_dependency_nominal_metadata_from_program(dep_ir);
                }
                for (_, dep_ir) in &compiled_stdlib_metadata_programs {
                    emitter.seed_dependency_nominal_metadata_from_program(dep_ir);
                }
                emitter.emit_program(ir)?
            };
            modules.insert(path.clone(), module_code);
        }

        Ok((main_code, modules))
    }
}

impl Default for IrCodegen<'_> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    mod canonical_projection {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/support/canonical_projection.rs"
        ));
    }
    use crate::frontend::library_manifest_index::{
        LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
    };
    use crate::frontend::{lexer, parser};
    use crate::library_manifest::{
        ConstExport, FunctionExport, LibraryManifest, ModelExport, ParamExport, ParamKindExport, TypeRef,
    };
    use canonical_projection::{projected_identities, projected_identity, projected_name};
    use incan_semantics_core::{SemanticSourceTargetKind, SymbolOrigin};
    use std::collections::HashMap;
    #[cfg(feature = "rust_inspect")]
    use std::fs;

    fn must_ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => panic!("unexpected error: {err:?}"),
        }
    }

    fn must_some<T>(value: Option<T>, context: &str) -> T {
        match value {
            Some(v) => v,
            None => panic!("{context}"),
        }
    }

    /// Build lowering with the frontend-owned ownership proof for one source type annotation.
    fn lowering_with_mutable_reference_projection(
        source: &str,
        annotation: &str,
        projections: Vec<crate::frontend::typechecker::MutableRustTypeArgumentProjection>,
    ) -> Result<AstLowering, Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;

        let tokens = lexer::lex(source).map_err(|errors| format!("lex errors: {errors:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errors| format!("parse errors: {errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker
            .check_program(&ast)
            .map_err(|errors| format!("typecheck errors: {errors:?}"))?;
        let start = source
            .find(annotation)
            .ok_or_else(|| format!("projection annotation `{annotation}` must occur in source"))?;
        let mut type_info = checker.type_info().clone();
        type_info
            .rust
            .mutable_reference_type_argument_projections
            .insert((start, start + annotation.len()), projections);
        Ok(AstLowering::new_with_type_info(type_info))
    }

    fn generate(source: &str) -> String {
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));
        must_ok(IrCodegen::new().try_generate(&ast))
    }

    fn generate_with_sdk_provider_modules(source: &str, modules: Vec<Vec<String>>) -> String {
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));
        let mut codegen = IrCodegen::new();
        codegen.set_sdk_provider_module_paths(modules);
        must_ok(codegen.try_generate(&ast))
    }

    fn assert_no_generated_unused_lint_allows(code: &str) {
        assert!(!code.contains("#[allow(dead_code)]"), "{code}");
        assert!(!code.contains("#[allow(unused_imports)]"), "{code}");
        assert!(!code.contains("#[allow(dead_code, unused_variables)]"), "{code}");
    }

    fn compact_rust(code: &str) -> String {
        code.chars().filter(|character| !character.is_whitespace()).collect()
    }

    #[test]
    fn overloaded_source_functions_keep_distinct_canonical_projections() {
        let code = generate(
            r#"
pub def convert(value: int) -> int:
  return value

pub def convert(value: str) -> str:
  return value
"#,
        );
        let identities = projected_identities(&code, "convert", SemanticSourceTargetKind::Function);
        assert_eq!(
            identities.len(),
            2,
            "each overload needs its own source identity: {code}"
        );
    }

    #[test]
    fn library_generation_metadata_uses_checked_root_module_path() {
        use crate::frontend::api_metadata::{
            CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
        };
        use crate::frontend::typechecker::TypeChecker;

        let source = r#"
trait Walk:
    def copy(self) -> Self: ...

pub model Stream[R] with Walk:
    value: R

    def copy(self) -> Self:
        return self
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));
        let root_module_path = vec!["main".to_string()];
        let (_code, metadata) = must_ok(IrCodegen::new().try_generate_with_metadata(&ast, &root_module_path));

        assert!(metadata.implementation_bound_requirements.iter().any(|captured| {
            captured.module_path == root_module_path
                && captured.requirement.target_type == "Stream"
                && captured.target_visibility == CapturedImplementationTargetVisibility::SameProgram(Visibility::Public)
                && captured.requirement.type_params.iter().any(|type_param| {
                    type_param
                        .bounds
                        .iter()
                        .any(|bound| bound.trait_path == incan_core::lang::trait_bounds::rust::CLONE)
                })
        }));

        let mut checker = TypeChecker::new();
        must_ok(checker.check_program(&ast));
        let mut manifest = LibraryManifest::new("root_impl", "0.1.0");
        manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            package: None,
            modules: vec![collect_checked_api_metadata(&ast, &checker, root_module_path)],
            public_namespaces: Vec::new(),
        });
        must_ok(metadata.apply_to_library_manifest(&mut manifest));
        let implementation_type_params = manifest
            .contract_metadata
            .api
            .as_ref()
            .and_then(|api| api.modules.first())
            .and_then(|module| {
                module.declarations.iter().find_map(|declaration| match declaration {
                    ApiDeclaration::Model(model) if model.name == "Stream" => model.trait_adoptions.first(),
                    _ => None,
                })
            })
            .map(|adoption| adoption.implementation_type_params.as_slice());
        assert!(
            implementation_type_params.is_some_and(|type_params| type_params.iter().any(|type_param| {
                type_param.name == "R"
                    && type_param
                        .bounds
                        .iter()
                        .any(|bound| bound.trait_path == incan_core::lang::trait_bounds::rust::CLONE)
            })),
            "the checked root adoption must receive its inferred implementation header"
        );
    }

    #[test]
    fn private_implementation_metadata_is_omitted_but_unknown_visibility_fails_closed()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
trait Walk:
    def copy(self) -> Self: ...

model PrivateStream[R] with Walk:
    value: R

    def copy(self) -> Self:
        return self
"#;
        let tokens = lexer::lex(source).map_err(|errors| format!("lex errors: {errors:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errors| format!("parse errors: {errors:?}"))?;
        let module_path = vec!["private_impl".to_string()];
        let (_code, metadata) = IrCodegen::new().try_generate_with_metadata(&ast, &module_path)?;
        let captured = metadata
            .implementation_bound_requirements
            .iter()
            .find(|captured| captured.requirement.target_type == "PrivateStream")
            .ok_or("private implementation should retain its inferred requirement internally")?;
        assert_eq!(
            captured.target_visibility,
            CapturedImplementationTargetVisibility::SameProgram(Visibility::Private)
        );

        let mut private_manifest = LibraryManifest::new("private_impl", "0.1.0");
        metadata.apply_to_library_manifest(&mut private_manifest)?;

        let mut unknown_requirement = captured.clone();
        unknown_requirement.target_visibility = CapturedImplementationTargetVisibility::Unknown;
        let unknown_metadata = IrGenerationMetadata {
            implementation_bound_requirements: vec![unknown_requirement],
        };
        let mut unknown_manifest = LibraryManifest::new("unknown_impl", "0.1.0");
        let Err(error) = unknown_metadata.apply_to_library_manifest(&mut unknown_manifest) else {
            return Err("an unknown implementation target must remain fail-closed".into());
        };
        assert!(
            error.contains("had no checked manifest adoption"),
            "unexpected unknown-target diagnostic: {error}"
        );
        Ok(())
    }

    #[test]
    fn implementation_metadata_preserves_decimal_type_arguments() {
        assert_eq!(
            must_ok(manifest_type_ref_from_ir(&IrType::Decimal {
                precision: 18,
                scale: 4,
            })),
            TypeRef::Applied {
                name: "decimal".to_string(),
                args: vec![
                    TypeRef::TypeParam { name: "18".to_string() },
                    TypeRef::TypeParam { name: "4".to_string() },
                ],
            }
        );
    }

    #[test]
    fn trait_method_alias_emits_required_adopted_method_issue1055() {
        let code = generate(
            r#"
trait Renamable:
  def where(self, value: int) -> int: ...

class Example with Renamable:
  where = alias filter

  def filter(self, value: int) -> int:
    return value

def main() -> None:
  println(Example().where(7))
"#,
        );

        assert!(
            code.contains("impl Renamable for Example") && code.contains("fn r#where(&self, value: i64) -> i64"),
            "expected the trait alias to emit the required method name, got:\n{code}"
        );
        assert!(
            code.contains(&format!(
                "pub fn {}(\n        &self,\n        value: i64,\n    ) -> i64",
                projected_name(&code, "filter", SemanticSourceTargetKind::Method)
            )),
            "expected the alias target to remain available as an inherent method, got:\n{code}"
        );
    }

    #[test]
    fn partial_function_codegen_emits_wrapper_with_defaulted_preset() {
        let code = generate(
            r#"
pub def route(method: str, path: str) -> str:
  return method

pub get = partial route(method="GET")

pub def use() -> str:
  return get(path="/health")
"#,
        );
        let get = projected_name(&code, "get", SemanticSourceTargetKind::Partial);
        let route = projected_name(&code, "route", SemanticSourceTargetKind::Function);
        assert!(code.contains(&format!("pub fn {get}(")), "{code}");
        assert!(code.contains("\"GET\""), "{code}");
        assert!(code.contains(&format!("{route}(")), "{code}");
        assert!(
            code.contains(&format!(
                "{get}(\n        \"GET\".to_string(),\n        \"/health\".to_string(),\n    )"
            )),
            "{code}"
        );
    }

    #[test]
    fn local_partial_codegen_captures_a_defaulted_overrideable_preset() {
        let code = generate(
            r#"
def route(method: str, path: str) -> str:
  return method + path

pub def use() -> str:
  get = partial route(method="GET")
  return get(path="/health")
"#,
        );
        assert!(code.contains("move |method: Option<String>, path: String|"), "{code}");
        assert!(code.contains("unwrap_or_else"), "{code}");
        assert!(code.contains("get(None"), "{code}");
    }

    #[test]
    fn local_partial_codegen_materializes_a_trailing_residual_default() {
        let code = generate(
            r#"
def route(method: str, path: str, content_type: str = "text") -> str:
  return method + path + content_type

pub def use() -> str:
  get = partial route(method="GET")
  return get("/health")
"#,
        );
        assert!(
            code.contains("let get = {\n        let __incan_partial_preset_0_method"),
            "{code}"
        );
        assert!(code.contains("\"GET\".to_string()"), "{code}");
        assert!(code.contains("return get(None"), "{code}");
    }

    #[test]
    fn partial_model_constructor_codegen_emits_wrapper_with_defaulted_preset() {
        let code = generate(
            r#"
pub model Reader:
  pub layer: str
  pub format: str

pub BronzeReader = partial Reader(layer="bronze", format="delta")

pub def use() -> Reader:
  return BronzeReader()
"#,
        );
        let bronze_reader = projected_name(&code, "BronzeReader", SemanticSourceTargetKind::Partial);
        assert!(code.contains(&format!("pub fn {bronze_reader}(")), "{code}");
        assert!(code.contains("\"bronze\""), "{code}");
        assert!(code.contains("\"delta\""), "{code}");
        assert!(code.contains("Reader {"), "{code}");
    }

    #[test]
    fn trait_method_partial_codegen_emits_default_method_wrapper() {
        let code = generate(
            r#"
trait Named:
  def label(self, prefix: str) -> str:
    return prefix
  short = partial label(prefix="name")

model User with Named:
  name: str

pub def use(user: User) -> str:
  return user.short()
"#,
        );
        assert!(code.contains("fn short"), "{code}");
        let label = projected_name(&code, "label", SemanticSourceTargetKind::Method);
        assert!(code.contains(&format!("return self\n            .{label}(")), "{code}");
        assert!(code.contains("user.short(\"name\".to_string())"), "{code}");
    }

    #[test]
    fn method_partial_codegen_resolves_alias_target() {
        let code = generate(
            r#"
model User:
  name: str
  def label(self, prefix: str) -> str:
    return prefix
  display = label
  short = partial display(prefix="name")

pub def use(user: User) -> str:
  return user.short()
"#,
        );
        assert!(code.contains("fn short"), "{code}");
        let label = projected_name(&code, "label", SemanticSourceTargetKind::Method);
        assert!(code.contains(&format!("return self\n            .{label}(")), "{code}");
        assert!(code.contains("user.short(\"name\".to_string())"), "{code}");
    }

    #[test]
    fn normal_codegen_does_not_emit_blanket_generated_lint_allows() {
        let code = generate(
            r#"
def helper(value: int) -> int:
  return value

def main() -> None:
  return
"#,
        );

        assert!(!code.contains("#![allow(unused_imports, dead_code, unused_variables)]"));
        assert!(!code.contains("use incan_stdlib::prelude::*;"));
        assert!(!code.contains("use incan_derive::{FieldInfo, IncanClass};"));
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn top_level_callable_alias_lowers_calls_to_target_and_public_reexport() {
        let code = generate(
            r#"
pub def avg(x: int) -> int:
  return x

mean = avg
pub average = alias avg

def main() -> int:
  return mean(10)
"#,
        );
        let avg = projected_name(&code, "avg", SemanticSourceTargetKind::Function);
        let compact = compact_rust(&code);
        assert!(compact.contains(&format!("pubfn{avg}(x:i64,)->i64")), "{code}");
        assert!(compact.contains(&format!("pubuse{avg}asaverage;")), "{code}");
        assert!(compact.contains(&format!("return{avg}(10,);")), "{code}");
        assert!(!code.contains("fn mean"), "{code}");
    }

    #[test]
    fn top_level_keyword_named_callable_alias_uses_raw_identifier_reexport() {
        let code = generate(
            r#"
pub def modulo_value(value: int) -> int:
  return value

pub mod = alias modulo_value

def main() -> int:
  return mod(10)
"#,
        );
        let modulo_value = projected_name(&code, "modulo_value", SemanticSourceTargetKind::Function);
        assert!(
            code.contains(&format!("pub fn {modulo_value}(\n    value: i64,\n) -> i64")),
            "{code}"
        );
        assert!(code.contains(&format!("pub use {modulo_value} as r#mod;")), "{code}");
        assert!(
            code.contains(&format!("return {modulo_value}(\n        10,\n    );")),
            "{code}"
        );
    }

    #[test]
    fn top_level_alias_to_keyword_named_callable_uses_raw_identifier_target_path() {
        let code = generate(
            r#"
pub def mod(value: int) -> int:
  return value

pub modulo = alias mod
"#,
        );
        let modulo = projected_name(&code, "mod", SemanticSourceTargetKind::Function);
        assert!(
            code.contains(&format!("pub fn {modulo}(\n    value: i64,\n) -> i64")),
            "{code}"
        );
        assert!(code.contains(&format!("pub use {modulo} as modulo;")), "{code}");
    }

    #[test]
    fn top_level_qualified_alias_preserves_target_path() {
        let code = generate_with_sdk_provider_modules(
            r#"
import std.math as math

pub root = math.sqrt
"#,
            vec![vec!["math".to_string()]],
        );
        assert!(code.contains("pub use crate::__incan_std::math as math;"), "{code}");
        let sqrt = projected_name(&code, "sqrt", SemanticSourceTargetKind::Function);
        assert!(code.contains(&format!("pub use math::{sqrt} as root;")), "{code}");
    }

    #[test]
    fn normal_codegen_keeps_used_private_helpers_without_dead_code_allows() {
        let code = generate(
            r#"
def helper(value: int) -> int:
  return value

def main() -> None:
  print(helper(1))
"#,
        );

        let helper = projected_name(&code, "helper", SemanticSourceTargetKind::Function);
        assert!(
            code.contains(&format!("fn {helper}(\n    value: i64,\n) -> i64")),
            "{code}"
        );
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn normal_codegen_prunes_unused_private_helpers() {
        let code = generate(
            r#"
def helper(value: int) -> int:
  return value

def main() -> None:
  print("done")
"#,
        );

        assert!(!code.contains("fn helper"), "{code}");
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn normal_codegen_prunes_unused_dependency_public_items_for_binary_mode() {
        let constants_module = parse_program(
            r#"
pub def api_version() -> str:
  return "v1"

pub def max_page_size() -> int:
  return 100

pub def default_timeout() -> int:
  return 30
"#,
        );
        let main_module = parse_program(
            r#"
from shared.constants import api_version, max_page_size

def main() -> None:
  print(api_version())
  print(max_page_size())
"#,
        );
        let constants_path = vec!["shared".to_string(), "constants".to_string()];
        let mut codegen = IrCodegen::new();
        codegen.set_preserve_dependency_public_items(false);
        codegen.add_module_with_path_segments("shared_constants", &constants_module, constants_path.clone());

        let (_main_code, rust_modules) =
            must_ok(codegen.try_generate_multi_file_nested(&main_module, std::slice::from_ref(&constants_path)));
        let constants_code = must_some(
            rust_modules.get(&constants_path),
            "missing generated shared.constants module",
        );

        assert!(
            constants_code.contains(&format!(
                "pub fn {}() -> String",
                projected_name(constants_code, "api_version", SemanticSourceTargetKind::Function)
            )),
            "{constants_code}"
        );
        assert!(
            constants_code.contains(&format!(
                "pub fn {}() -> i64",
                projected_name(constants_code, "max_page_size", SemanticSourceTargetKind::Function)
            )),
            "{constants_code}"
        );
        assert!(!constants_code.contains("default_timeout"), "{constants_code}");
        assert_no_generated_unused_lint_allows(constants_code);
    }

    #[test]
    fn normal_codegen_prunes_unreachable_stdlib_dependency_public_items_for_generated_projects() {
        let gzip_module = parse_program(
            r#"
pub def compress(data: bytes) -> bytes:
  return data

pub def decompress(data: bytes) -> bytes:
  return data
"#,
        );
        let main_module = parse_program(
            r#"
from std.compression.gzip import decompress

def main() -> None:
  _ = decompress(b"data")
"#,
        );
        let gzip_path = vec!["__incan_std".to_string(), "compression".to_string(), "gzip".to_string()];
        let mut codegen = IrCodegen::new();
        codegen.set_preserve_dependency_public_items(false);
        codegen.add_module_with_path_segments("__incan_std_compression_gzip", &gzip_module, gzip_path.clone());

        let (_main_code, rust_modules) =
            must_ok(codegen.try_generate_multi_file_nested(&main_module, std::slice::from_ref(&gzip_path)));
        let gzip_code = must_some(
            rust_modules.get(&gzip_path),
            "missing generated std.compression.gzip module",
        );

        assert!(
            projected_identities(gzip_code, "compress", SemanticSourceTargetKind::Function).is_empty(),
            "{gzip_code}"
        );
        let decompress = projected_name(gzip_code, "decompress", SemanticSourceTargetKind::Function);
        assert!(gzip_code.contains(&format!("pub fn {decompress}(")), "{gzip_code}");
        assert_no_generated_unused_lint_allows(gzip_code);
    }

    #[test]
    fn normal_codegen_can_preserve_dependency_public_items_for_library_mode() {
        let constants_module = parse_program(
            r#"
pub def api_version() -> str:
  return "v1"

pub def default_timeout() -> int:
  return 30
"#,
        );
        let main_module = parse_program(
            r#"
from shared.constants import api_version

def main() -> None:
  print(api_version())
"#,
        );
        let constants_path = vec!["shared".to_string(), "constants".to_string()];
        let mut codegen = IrCodegen::new();
        codegen.set_preserve_dependency_public_items(true);
        codegen.add_module_with_path_segments("shared_constants", &constants_module, constants_path.clone());

        let (_main_code, rust_modules) =
            must_ok(codegen.try_generate_multi_file_nested(&main_module, std::slice::from_ref(&constants_path)));
        let constants_code = must_some(
            rust_modules.get(&constants_path),
            "missing generated shared.constants module",
        );

        assert!(
            constants_code.contains(&format!(
                "pub fn {}() -> String",
                projected_name(constants_code, "api_version", SemanticSourceTargetKind::Function)
            )),
            "{constants_code}"
        );
        assert!(
            constants_code.contains(&format!(
                "pub fn {}() -> i64",
                projected_name(constants_code, "default_timeout", SemanticSourceTargetKind::Function)
            )),
            "{constants_code}"
        );
        assert_no_generated_unused_lint_allows(constants_code);
    }

    #[test]
    fn library_mode_reexports_rust_dependencies_through_compiler_owned_bridge() {
        let mut codegen = IrCodegen::new();
        codegen.set_preserve_dependency_public_items(true);
        codegen
            .provider_rust_bridge_roots
            .extend(["rust_shadow".to_string(), "type".to_string(), "std".to_string()]);
        codegen.rust_crates.insert("private_implementation".to_string());

        let code = codegen.attach_provider_rust_dependency_bridge("pub fn marker() {}\n".to_string());

        assert!(code.contains("pub mod __incan_provider_rust"), "{code}");
        assert!(code.contains("pub use ::rust_shadow;"), "{code}");
        assert!(code.contains("pub use ::r#type;"), "{code}");
        assert!(!code.contains("pub use ::std;"), "{code}");
        assert!(!code.contains("private_implementation"), "{code}");
    }

    #[test]
    fn normal_codegen_keeps_external_generated_entrypoints() {
        let tokens = must_ok(lexer::lex(
            r#"
def test_generated_entrypoint() -> None:
  return
"#,
        ));
        let ast = must_ok(parser::parse(&tokens));
        let mut codegen = IrCodegen::new();
        codegen.set_externally_reachable_items(std::collections::HashSet::from([String::from(
            "test_generated_entrypoint",
        )]));
        let code = must_ok(codegen.try_generate(&ast));

        let entrypoint = projected_name(&code, "test_generated_entrypoint", SemanticSourceTargetKind::Function);
        assert!(code.contains(&format!("fn {entrypoint}(")), "{code}");
        assert!(
            code.contains(&format!("use {entrypoint} as test_generated_entrypoint;")),
            "the generated harness must retain its source-facing call path:\n{code}"
        );
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn canonical_functions_keep_their_existing_rust_facing_names() {
        let code = generate(
            r#"
pub def public_value() -> int:
  return 42

def test_generated_entrypoint() -> None:
  return
"#,
        );

        let public_projection = projected_name(&code, "public_value", SemanticSourceTargetKind::Function);
        assert!(
            code.contains(&format!("pub use {public_projection} as public_value;")),
            "public Rust consumers must retain the source-facing name:\n{code}"
        );
    }

    #[test]
    fn normal_codegen_prunes_unused_rust_imports() {
        let code = generate(
            r#"
import rust::std::collections::HashMap

def main() -> None:
  print("done")
"#,
        );

        assert!(!code.contains("use std::collections::HashMap;"), "{code}");
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn normal_codegen_keeps_used_rust_import_aliases() {
        let code = generate(
            r#"
import rust::std::f64::consts as consts

def main() -> None:
  _ = consts.PI
"#,
        );

        assert!(code.contains("use std::f64::consts as consts;"), "{code}");
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn generated_use_analysis_keeps_rust_extension_trait_imports() {
        use crate::backend::ir::decl::{
            FunctionParam, IrFunction, IrImportItem, IrImportOrigin, IrImportQualifier, IrRustTraitImport, Visibility,
        };
        use crate::backend::ir::expr::{
            IrCallArg, IrCallArgKind, IrExprKind, MethodCallArgPolicy, VarAccess, VarRefKind,
        };
        use crate::backend::ir::{IrDecl, IrDeclKind, IrProgram, IrStmt, IrStmtKind, IrType, Mutability, TypedExpr};

        let mut program = IrProgram::new();
        program.declarations.push(IrDecl::new(IrDeclKind::Import {
            visibility: Visibility::Private,
            origin: IrImportOrigin::Standard,
            qualifier: IrImportQualifier::None,
            path: vec![String::from("rand")],
            alias: None,
            items: vec![
                IrImportItem {
                    name: String::from("Rng"),
                    alias: None,
                    canonical: None,
                    is_static: false,
                    force_reexport: false,
                    rust_trait_import: Some(IrRustTraitImport {
                        trait_path: String::from("rand::Rng"),
                        definition_path: None,
                        methods: vec![String::from("gen_range")],
                    }),
                },
                IrImportItem {
                    name: String::from("thread_rng"),
                    alias: None,
                    canonical: None,
                    is_static: false,
                    force_reexport: false,
                    rust_trait_import: None,
                },
            ],
        }));
        let rng_ty = IrType::Struct(String::from("rand::rngs::ThreadRng"));
        program.declarations.push(IrDecl::new(IrDeclKind::Function(IrFunction {
            name: String::from("main"),
            docstring: None,
            params: Vec::<FunctionParam>::new(),
            return_type: IrType::Unit,
            body: vec![
                IrStmt::new(IrStmtKind::Let {
                    name: String::from("rng"),
                    ty: rng_ty.clone(),
                    type_annotation: None,
                    mutability: Mutability::Mutable,
                    value: TypedExpr::new(
                        IrExprKind::Call {
                            func: Box::new(TypedExpr::new(
                                IrExprKind::Var {
                                    name: String::from("thread_rng"),
                                    access: VarAccess::Move,
                                    ref_kind: VarRefKind::ExternalRustName,
                                },
                                IrType::Function {
                                    params: Vec::new(),
                                    ret: Box::new(rng_ty.clone()),
                                },
                            )),
                            type_args: Vec::new(),
                            args: Vec::new(),
                            callable_signature: None,
                            canonical_path: None,
                        },
                        rng_ty.clone(),
                    ),
                }),
                IrStmt::new(IrStmtKind::Expr(TypedExpr::new(
                    IrExprKind::MethodCall {
                        receiver: Box::new(TypedExpr::new(
                            IrExprKind::Var {
                                name: String::from("rng"),
                                access: VarAccess::Read,
                                ref_kind: VarRefKind::Value,
                            },
                            rng_ty,
                        )),
                        method: String::from("gen_range"),
                        dispatch: None,
                        type_args: Vec::new(),
                        args: vec![IrCallArg {
                            name: None,
                            kind: IrCallArgKind::Positional,
                            expr: TypedExpr::new(
                                IrExprKind::Range {
                                    start: Some(Box::new(TypedExpr::new(IrExprKind::Int(1), IrType::Int))),
                                    end: Some(Box::new(TypedExpr::new(IrExprKind::Int(7), IrType::Int))),
                                    inclusive: false,
                                },
                                IrType::Unknown,
                            ),
                        }],
                        callable_signature: None,
                        arg_policy: MethodCallArgPolicy::Default,
                    },
                    IrType::Int,
                ))),
            ],
            is_async: false,
            is_generator: false,
            visibility: Visibility::Private,
            type_params: Vec::new(),
            is_extern: false,
            rust_extern_name: None,
            rust_attributes: Vec::new(),
            lint_allows: Vec::new(),
        })));

        let mut emitter = IrEmitter::new(&program.function_registry);
        let code = must_ok(emitter.emit_program(&program));

        assert!(code.contains("use ::rand::Rng;"), "{code}");
        assert!(code.contains("use ::rand::thread_rng;"), "{code}");
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn generated_use_analysis_keeps_only_selected_same_name_rust_extension_trait_import() {
        use crate::backend::ir::decl::{
            FunctionParam, IrFunction, IrImportItem, IrImportOrigin, IrImportQualifier, IrRustTraitImport, IrStruct,
            IrStructKind, Visibility,
        };
        use crate::backend::ir::expr::{IrExprKind, IrMethodDispatch, MethodCallArgPolicy, VarAccess, VarRefKind};
        use crate::backend::ir::{IrDecl, IrDeclKind, IrProgram, IrStmt, IrStmtKind, IrType, Mutability, TypedExpr};

        let mut program = IrProgram::new();
        program.declarations.push(IrDecl::new(IrDeclKind::Import {
            visibility: Visibility::Private,
            origin: IrImportOrigin::Standard,
            qualifier: IrImportQualifier::None,
            path: vec![String::from("demo")],
            alias: None,
            items: vec![
                IrImportItem {
                    name: String::from("AlphaRender"),
                    alias: None,
                    canonical: None,
                    is_static: false,
                    force_reexport: false,
                    rust_trait_import: Some(IrRustTraitImport {
                        trait_path: String::from("demo::AlphaRender"),
                        definition_path: None,
                        methods: vec![String::from("render")],
                    }),
                },
                IrImportItem {
                    name: String::from("BetaRender"),
                    alias: None,
                    canonical: None,
                    is_static: false,
                    force_reexport: false,
                    rust_trait_import: Some(IrRustTraitImport {
                        trait_path: String::from("demo::BetaRender"),
                        definition_path: None,
                        methods: vec![String::from("render")],
                    }),
                },
            ],
        }));
        program.declarations.push(IrDecl::new(IrDeclKind::Struct(IrStruct {
            kind: IrStructKind::Model,
            name: String::from("Widget"),
            docstring: None,
            fields: Vec::new(),
            derives: Vec::new(),
            visibility: Visibility::Private,
            type_params: Vec::new(),
            derive_rust_modules: std::collections::HashMap::new(),
            lint_allows: Vec::new(),
        })));
        let widget_ty = IrType::Struct(String::from("Widget"));
        program.declarations.push(IrDecl::new(IrDeclKind::Function(IrFunction {
            name: String::from("main"),
            docstring: None,
            params: Vec::<FunctionParam>::new(),
            return_type: IrType::Unit,
            body: vec![
                IrStmt::new(IrStmtKind::Let {
                    name: String::from("widget"),
                    ty: widget_ty.clone(),
                    type_annotation: None,
                    mutability: Mutability::Immutable,
                    value: TypedExpr::new(
                        IrExprKind::Struct {
                            name: String::from("Widget"),
                            fields: Vec::new(),
                            fill_defaults: false,
                        },
                        widget_ty.clone(),
                    ),
                }),
                IrStmt::new(IrStmtKind::Expr(TypedExpr::new(
                    IrExprKind::MethodCall {
                        receiver: Box::new(TypedExpr::new(
                            IrExprKind::Var {
                                name: String::from("widget"),
                                access: VarAccess::Read,
                                ref_kind: VarRefKind::Value,
                            },
                            widget_ty,
                        )),
                        method: String::from("render"),
                        dispatch: Some(IrMethodDispatch::RustExtensionTraitImport {
                            binding: String::from("AlphaRender"),
                        }),
                        type_args: Vec::new(),
                        args: Vec::new(),
                        callable_signature: None,
                        arg_policy: MethodCallArgPolicy::Default,
                    },
                    IrType::String,
                ))),
            ],
            is_async: false,
            is_generator: false,
            visibility: Visibility::Private,
            type_params: Vec::new(),
            is_extern: false,
            rust_extern_name: None,
            rust_attributes: Vec::new(),
            lint_allows: Vec::new(),
        })));

        let mut emitter = IrEmitter::new(&program.function_registry);
        let code = must_ok(emitter.emit_program(&program));

        assert!(code.contains("use ::demo::AlphaRender;"), "{code}");
        assert!(!code.contains("use ::demo::BetaRender;"), "{code}");
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn generated_use_analysis_keeps_rust_trait_candidates_without_metadata() {
        use crate::backend::ir::decl::{
            FunctionParam, IrFunction, IrImportItem, IrImportOrigin, IrImportQualifier, Visibility,
        };
        use crate::backend::ir::expr::{
            IrCallArg, IrCallArgKind, IrExprKind, MethodCallArgPolicy, VarAccess, VarRefKind,
        };
        use crate::backend::ir::{IrDecl, IrDeclKind, IrProgram, IrStmt, IrStmtKind, IrType, Mutability, TypedExpr};

        let mut program = IrProgram::new();
        program.declarations.push(IrDecl::new(IrDeclKind::Import {
            visibility: Visibility::Private,
            origin: IrImportOrigin::Standard,
            qualifier: IrImportQualifier::None,
            path: vec![String::from("rand")],
            alias: None,
            items: vec![
                IrImportItem {
                    name: String::from("Rng"),
                    alias: None,
                    canonical: None,
                    is_static: false,
                    force_reexport: false,
                    rust_trait_import: None,
                },
                IrImportItem {
                    name: String::from("thread_rng"),
                    alias: None,
                    canonical: None,
                    is_static: false,
                    force_reexport: false,
                    rust_trait_import: None,
                },
            ],
        }));
        let rng_ty = IrType::Struct(String::from("rand::rngs::ThreadRng"));
        program.declarations.push(IrDecl::new(IrDeclKind::Function(IrFunction {
            name: String::from("main"),
            docstring: None,
            params: Vec::<FunctionParam>::new(),
            return_type: IrType::Unit,
            body: vec![
                IrStmt::new(IrStmtKind::Let {
                    name: String::from("rng"),
                    ty: rng_ty.clone(),
                    type_annotation: None,
                    mutability: Mutability::Mutable,
                    value: TypedExpr::new(
                        IrExprKind::Call {
                            func: Box::new(TypedExpr::new(
                                IrExprKind::Var {
                                    name: String::from("thread_rng"),
                                    access: VarAccess::Move,
                                    ref_kind: VarRefKind::ExternalRustName,
                                },
                                IrType::Function {
                                    params: Vec::new(),
                                    ret: Box::new(rng_ty.clone()),
                                },
                            )),
                            type_args: Vec::new(),
                            args: Vec::new(),
                            callable_signature: None,
                            canonical_path: None,
                        },
                        rng_ty.clone(),
                    ),
                }),
                IrStmt::new(IrStmtKind::Expr(TypedExpr::new(
                    IrExprKind::MethodCall {
                        receiver: Box::new(TypedExpr::new(
                            IrExprKind::Var {
                                name: String::from("rng"),
                                access: VarAccess::Read,
                                ref_kind: VarRefKind::Value,
                            },
                            rng_ty,
                        )),
                        method: String::from("gen_range"),
                        dispatch: None,
                        type_args: Vec::new(),
                        args: vec![IrCallArg {
                            name: None,
                            kind: IrCallArgKind::Positional,
                            expr: TypedExpr::new(
                                IrExprKind::Range {
                                    start: Some(Box::new(TypedExpr::new(IrExprKind::Int(1), IrType::Int))),
                                    end: Some(Box::new(TypedExpr::new(IrExprKind::Int(7), IrType::Int))),
                                    inclusive: false,
                                },
                                IrType::Unknown,
                            ),
                        }],
                        callable_signature: None,
                        arg_policy: MethodCallArgPolicy::Default,
                    },
                    IrType::Int,
                ))),
            ],
            is_async: false,
            is_generator: false,
            visibility: Visibility::Private,
            type_params: Vec::new(),
            is_extern: false,
            rust_extern_name: None,
            rust_attributes: Vec::new(),
            lint_allows: Vec::new(),
        })));

        let mut emitter = IrEmitter::new(&program.function_registry);
        let code = must_ok(emitter.emit_program(&program));

        assert!(code.contains("use ::rand::Rng;"), "{code}");
        assert!(code.contains("use ::rand::thread_rng;"), "{code}");
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn generated_use_analysis_keeps_rust_trait_for_associated_method_on_rust_type() {
        use crate::backend::ir::decl::{
            FunctionParam, IrFunction, IrImportItem, IrImportOrigin, IrImportQualifier, IrRustTraitImport, Visibility,
        };
        use crate::backend::ir::expr::{
            IrCallArg, IrCallArgKind, IrExprKind, MethodCallArgPolicy, VarAccess, VarRefKind,
        };
        use crate::backend::ir::{IrDecl, IrDeclKind, IrProgram, IrStmt, IrStmtKind, IrType, TypedExpr};

        let mut program = IrProgram::new();
        program.declarations.push(IrDecl::new(IrDeclKind::Import {
            visibility: Visibility::Private,
            origin: IrImportOrigin::Standard,
            qualifier: IrImportQualifier::None,
            path: vec![String::from("sha2")],
            alias: None,
            items: vec![
                IrImportItem {
                    name: String::from("Digest"),
                    alias: None,
                    canonical: None,
                    is_static: false,
                    force_reexport: false,
                    rust_trait_import: Some(IrRustTraitImport {
                        trait_path: String::from("sha2::Digest"),
                        definition_path: Some(String::from("digest::digest::Digest")),
                        methods: vec![String::from("digest")],
                    }),
                },
                IrImportItem {
                    name: String::from("Sha256"),
                    alias: None,
                    canonical: None,
                    is_static: false,
                    force_reexport: false,
                    rust_trait_import: None,
                },
            ],
        }));
        program.declarations.push(IrDecl::new(IrDeclKind::Function(IrFunction {
            name: String::from("main"),
            docstring: None,
            params: Vec::<FunctionParam>::new(),
            return_type: IrType::Unit,
            body: vec![IrStmt::new(IrStmtKind::Expr(TypedExpr::new(
                IrExprKind::MethodCall {
                    receiver: Box::new(TypedExpr::new(
                        IrExprKind::Var {
                            name: String::from("Sha256"),
                            access: VarAccess::Copy,
                            ref_kind: VarRefKind::ExternalRustName,
                        },
                        IrType::Unknown,
                    )),
                    method: String::from("digest"),
                    dispatch: None,
                    type_args: Vec::new(),
                    args: vec![IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: TypedExpr::new(IrExprKind::Bytes(b"abc".to_vec()), IrType::Bytes),
                    }],
                    callable_signature: None,
                    arg_policy: MethodCallArgPolicy::Default,
                },
                IrType::Bytes,
            )))],
            is_async: false,
            is_generator: false,
            visibility: Visibility::Private,
            type_params: Vec::new(),
            is_extern: false,
            rust_extern_name: None,
            rust_attributes: Vec::new(),
            lint_allows: Vec::new(),
        })));

        let mut emitter = IrEmitter::new(&program.function_registry);
        let code = must_ok(emitter.emit_program(&program));

        assert!(code.contains("use ::sha2::Digest;"), "{code}");
        assert!(code.contains("use ::sha2::Sha256;"), "{code}");
        assert!(code.contains("Sha256::digest"), "{code}");
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn normal_codegen_omits_dead_code_expect_for_generated_field_reflection_reads() {
        let code = generate(
            r#"
model User:
  name: str
  age: int

def main() -> None:
  let user = User(name="Ada", age=42)
  print(user.name)
"#,
        );

        assert!(code.contains("name: String"), "{code}");
        assert!(
            code.contains("impl incan_stdlib::reflection::HasFieldValueReflection for User"),
            "{code}"
        );
        assert!(code.contains("\"age\" => Some(format!(\"{}\", self.age))"), "{code}");
        assert!(
            !code.contains("#[expect(dead_code"),
            "fields read by generated value reflection should not carry dead-code expectations:\n{code}"
        );
        assert!(
            !code.contains("#[allow(dead_code"),
            "fields read by generated value reflection should not carry dead-code allows:\n{code}"
        );
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn normal_codegen_emits_value_reflection_for_optional_scalar_fields() {
        let code = generate(
            r#"
model ProbeRow:
  label: str
  optional_label: Option[str]

def main() -> None:
  row = ProbeRow(label="paid", optional_label=None)
  _ = row
  print("ok")
"#,
        );
        let compact = code.chars().filter(|c| !c.is_whitespace()).collect::<String>();

        assert!(
            code.contains("impl incan_stdlib::reflection::HasFieldValueReflection for ProbeRow"),
            "{code}"
        );
        assert!(
            compact.contains("\"optional_label\"=>{Some(match&self.optional_label"),
            "{code}"
        );
        assert!(
            compact
                .contains("match&self.optional_label{Some(value)=>format!(\"{}\",value),None=>\"None\".to_string(),}"),
            "{code}"
        );
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn string_membership_probe_borrows_loop_binding_used_later_in_branch_issue1057() {
        let code = generate(
            r#"
def names() -> list[str]:
  return ["orders", "missing"]

def first_missing() -> str:
  registered = set(["orders"])
  for name in names():
    if name not in registered:
      return f"missing:{name}"
  return ""

def main() -> None:
  assert first_missing() == "missing:missing"
"#,
        );

        assert!(
            code.contains("let __incan_probe = &name;"),
            "membership must borrow the loop binding before its later branch use:\n{code}"
        );
        assert!(
            code.contains("AsRef::<str>::as_ref(&__incan_probe)")
                || code.contains("<_ as AsRef<str>>::as_ref(&__incan_probe)"),
            "membership should borrow the probe binding at its point of use:\n{code}"
        );
        assert!(
            !code.contains("let __incan_probe = name;"),
            "membership must not move the loop binding into its probe:\n{code}"
        );
        assert!(
            !code.contains("name.clone()"),
            "the ownership planner should borrow, not synthesize a clone:\n{code}"
        );
    }

    #[test]
    fn string_membership_probe_borrows_non_variable_owned_string_issue1066() {
        let code = generate(
            r#"
def observation_id_text(value: int) -> str:
  return f"obs-{value}"

def is_known(observation_id: int, observation_ids: list[str]) -> bool:
  if observation_id_text(observation_id) in observation_ids:
    return True
  return False

def main() -> None:
  assert is_known(1, ["obs-1", "obs-2"])
"#,
        );

        assert!(
            code.contains("AsRef::<str>::as_ref(&__incan_probe)")
                || code.contains("<_ as AsRef<str>>::as_ref(&__incan_probe)"),
            "a call-result probe must be borrowed at its point of use:\n{code}"
        );
        assert!(
            !code.contains("AsRef::<str>::as_ref(__incan_probe)")
                && !code.contains("<_ as AsRef<str>>::as_ref(__incan_probe)"),
            "an owned call result must never reach AsRef::as_ref by value (E0308):\n{code}"
        );
    }

    #[test]
    fn string_membership_probe_keeps_literal_and_borrowed_probes_well_formed_issue1066() {
        let code = generate(
            r#"
def has_orders(names: list[str]) -> bool:
  return "orders" in names

def main() -> None:
  assert has_orders(["orders"])
"#,
        );

        assert!(
            !code.contains("let __incan_probe = &&"),
            "broadening the probe guard must not double-borrow an already-referenced value:\n{code}"
        );
        assert!(
            code.contains("AsRef::<str>::as_ref(&__incan_probe)")
                || code.contains("<_ as AsRef<str>>::as_ref(&__incan_probe)"),
            "literal membership should still route through the AsRef template:\n{code}"
        );
    }

    #[test]
    fn normal_codegen_expects_unread_private_fields_when_value_reflection_is_not_emitted() {
        let code = generate(
            r#"
model Box[T]:
  value: T

def main() -> None:
  let box = Box[int](value=42)
  print("ok")
"#,
        );

        assert!(
            code.contains(
                "#[expect(dead_code, reason = \"retained for Incan private field semantics\")]\n    value: T"
            ),
            "{code}"
        );
        assert!(!code.contains("HasFieldValueReflection for Box"), "{code}");
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn normal_codegen_skips_value_reflection_for_non_scalar_fields() {
        let code = generate(
            r#"
model Batch:
  values: list[int]

def main() -> None:
  let batch = Batch(values=[1, 2, 3])
  _ = batch
  print("ok")
"#,
        );

        assert!(
            !code.contains("impl incan_stdlib::reflection::HasFieldValueReflection for Batch"),
            "{code}"
        );
        assert!(
            code.contains(
                "#[expect(dead_code, reason = \"retained for Incan private field semantics\")]\n    values: Vec<i64>"
            ),
            "{code}"
        );
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn generated_rust_warning_clean() -> Result<(), Box<dyn std::error::Error>> {
        use crate::backend::project::ProjectGenerator;
        use std::process::Command;

        let code = generate(
            r#"
import rust::std::f64::consts as consts

model User:
  name: str
  age: int

def helper(value: int) -> int:
  return value

def main() -> None:
  let user = User(name="Ada", age=42)
  print(user.name)
  print(helper(1))
  _ = consts.PI
"#,
        );
        assert_no_generated_unused_lint_allows(&code);

        let tmp = tempfile::tempdir()?;
        let generator = ProjectGenerator::new(tmp.path(), "warning_clean_codegen", true);
        generator.generate(&code)?;

        let capability = crate::oven::compiler_suite_env::OvenCompilerSuiteCapability::from_environment(
            crate::oven::compiler_suite_env::OVEN_COMPILER_SUITE_CAPABILITY_ENV,
        )
        .map_err(std::io::Error::other)?;
        let output = if let Some(capability) = capability {
            if !capability.externs.contains_key("incan_stdlib") {
                return Err("stored Oven compiler suite direct-rustc capability omitted incan_stdlib".into());
            }
            let mut command = Command::new(&capability.rustc);
            command
                .arg("--edition=2024")
                .arg("--crate-name")
                .arg("warning_clean_codegen")
                .arg("--emit=metadata")
                .arg("--out-dir")
                .arg(tmp.path().join("oven-warning-check"))
                .arg("-Dwarnings")
                .env("CARGO_MANIFEST_DIR", tmp.path())
                .env("CARGO_PKG_NAME", "warning_clean_codegen")
                .env("CARGO_PKG_VERSION", "0.1.0");
            for dependency_path in &capability.dependency_search_paths {
                command
                    .arg("-L")
                    .arg(format!("dependency={}", dependency_path.display()));
            }
            for (crate_name, path) in &capability.externs {
                command.arg("--extern").arg(format!("{crate_name}={}", path.display()));
            }
            command.arg(generator.crate_root_path()).output()?
        } else {
            Command::new("cargo")
                .arg("check")
                .current_dir(tmp.path())
                .env("CARGO_NET_OFFLINE", "true")
                .env("RUSTFLAGS", "-Dwarnings")
                .output()?
        };

        assert!(
            output.status.success(),
            "generated Rust should pass the configured warning check with -Dwarnings\nstderr:\n{}\nstdout:\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        Ok(())
    }

    #[test]
    fn normal_codegen_uses_underscore_for_unused_parameters() {
        let code = generate(
            r#"
def helper(value: int, unused: int) -> int:
  return value

def main() -> None:
  print(helper(1, 2))
"#,
        );

        let helper = projected_name(&code, "helper", SemanticSourceTargetKind::Function);
        assert!(
            code.contains(&format!("fn {helper}(\n    value: i64,\n    _: i64,\n) -> i64")),
            "{code}"
        );
        assert!(!code.contains("#[allow(unused_variables)]"), "{code}");
    }

    #[test]
    fn normal_codegen_uses_underscore_for_unused_locals() {
        let code = generate(
            r#"
def main() -> None:
  let unused = "value"
  print("done")
"#,
        );

        assert!(code.contains("let _unused = \"value\".to_string();"), "{code}");
        assert!(!code.contains("let unused = \"value\".to_string();"), "{code}");
        assert!(!code.contains("#[allow(unused_variables)]"), "{code}");
    }

    #[test]
    fn normal_codegen_unused_local_scan_respects_shadowing() {
        let code = generate(
            r#"
def main() -> None:
  let unused = "outer"
  if true:
    let unused = "inner"
    print(unused)
"#,
        );

        assert!(code.contains("let _unused = \"outer\".to_string();"), "{code}");
        assert!(code.contains("let unused = \"inner\".to_string();"), "{code}");
        assert!(!code.contains("#[allow(unused_variables)]"), "{code}");
    }

    #[test]
    fn strict_codegen_emits_denies_without_generated_scoped_allows() {
        let ast = parse_program(
            r#"
def helper(value: int) -> int:
  return value

def main() -> None:
  return
"#,
        );
        let mut codegen = IrCodegen::new();
        codegen.set_strict_generated_lints(true);
        let code = must_ok(codegen.try_generate(&ast));

        assert!(code.contains("#![deny(unused_imports, dead_code, unused_variables)]"));
        assert!(!code.contains("#![allow("));
        assert!(!code.contains("#[allow(dead_code"));
        assert!(!code.contains("#[allow(unused_variables"));
    }

    #[test]
    fn built_in_derive_macros_are_path_qualified() {
        let code = generate(
            r#"
model User:
  name: str

def main() -> None:
  let user = User(name="Ada")
  print(user.name)
"#,
        );

        assert!(code.contains("#[derive(Debug, Clone, incan_derive::FieldInfo, incan_derive::IncanClass)]"));
        assert!(!code.contains("use incan_derive::{FieldInfo, IncanClass};"));
    }

    /// Parse an Incan program into an AST
    fn parse_program(source: &str) -> Program {
        let tokens = must_ok(lexer::lex(source));
        must_ok(parser::parse(&tokens))
    }

    fn parse_program_result(source: &str) -> Result<Program, Box<dyn std::error::Error>> {
        let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        Ok(ast)
    }

    fn read_stdlib_program(path: &str) -> Result<Program, Box<dyn std::error::Error>> {
        let source = std::fs::read_to_string(path)?;
        parse_program_result(&source)
    }

    /// Parse and scan a source snippet to determine whether serde runtime support is required.
    fn detects_serde(source: &str) -> bool {
        let ast = parse_program(source);
        let mut codegen = IrCodegen::new();
        codegen.update_serde_requirement(&ast);
        codegen.needs_serde()
    }

    #[cfg(feature = "rust_inspect")]
    fn seeded_rust_inspect_workspace() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "ra_seeded_codegen_probe"
version = "0.1.0"
edition = "2021"
"#,
        )?;
        Ok(tmp)
    }

    #[cfg(feature = "rust_inspect")]
    fn reqwest_shaped_rust_inspect_workspace() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "reqwest"
version = "0.0.0"
edition = "2021"
"#,
        )?;
        fs::create_dir_all(tmp.path().join("src"))?;
        fs::write(
            tmp.path().join("src").join("lib.rs"),
            r#"
pub struct Client;

pub struct RequestBuilder;

pub trait IntoUrl {}

impl IntoUrl for &str {}

impl Client {
    pub fn new() -> Client {
        Client
    }

    pub fn post<U: IntoUrl>(&self, _url: U) -> RequestBuilder {
        RequestBuilder
    }
}

impl RequestBuilder {
    pub fn json<T: ?Sized>(self, _json: &T) -> RequestBuilder {
        self
    }
}
"#,
        )?;
        Ok(tmp)
    }

    /// Write the tiny Rust crate used to prove root trait imports remain in scope during direct module generation.
    #[cfg(feature = "rust_inspect")]
    fn write_message_trait_probe_crate(root: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
        fs::create_dir_all(root.join("src"))?;
        fs::write(
            root.join("Cargo.toml"),
            r#"[package]
name = "message_probe"
version = "0.1.0"
edition = "2021"
"#,
        )?;
        fs::write(
            root.join("src").join("lib.rs"),
            r#"
pub struct Packet;

pub trait Message {
    fn encode_to_vec(&self) -> Vec<u8>;
}

impl Message for Packet {
    fn encode_to_vec(&self) -> Vec<u8> {
        vec![1, 2, 3]
    }
}
"#,
        )?;
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    fn prewarm_metadata(manifest_dir: &std::path::Path, paths: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
        let inspector =
            crate::rust_inspect::Inspector::new(crate::rust_inspect::InspectorConfig::new(manifest_dir.to_path_buf()));
        inspector.prewarm(paths.iter().map(|p| (*p).to_string()).collect::<Vec<_>>(), &|_| ())?;
        Ok(())
    }

    fn db_module_program() -> Program {
        parse_program(
            r#"
model Database:
  id: int
"#,
        )
    }

    fn main_module_program() -> Program {
        parse_program(
            r#"
def main() -> None:
  return
"#,
        )
    }

    fn library_index_with_widgets_exports() -> LibraryManifestIndex {
        let mut artifact_root = std::env::temp_dir();
        artifact_root.push("incan_test_widgets_artifacts");
        artifact_root.push("target");
        artifact_root.push("lib");

        let mut manifest = LibraryManifest::new("widgets_core", "0.1.0");
        manifest.exports.models.push(ModelExport {
            name: "Widget".to_string(),
            type_params: Vec::new(),
            traits: Vec::new(),
            trait_adoptions: Vec::new(),
            derives: Vec::new(),
            fields: Vec::new(),
            properties: Vec::new(),
            methods: Vec::new(),
        });
        manifest.exports.functions.push(FunctionExport {
            name: "make_widget".to_string(),
            emitted_name: None,
            type_params: Vec::new(),
            params: vec![ParamExport {
                name: "name".to_string(),
                ty: TypeRef::Named {
                    name: "str".to_string(),
                },
                kind: ParamKindExport::Normal,
                has_default: false,
                default: None,
            }],
            return_type: TypeRef::Named {
                name: "Widget".to_string(),
            },
            is_async: false,
        });
        manifest.exports.consts.push(ConstExport {
            name: "DEFAULT_NAME".to_string(),
            ty: TypeRef::Named {
                name: "str".to_string(),
            },
        });
        LibraryManifestIndex::from_entries(HashMap::from([(
            "widgets".to_string(),
            LibraryManifestIndexEntry::Loaded {
                manifest: Box::new(manifest),
                metadata: LibraryArtifactMetadata::from_crate_root("widgets", "widgets_core", artifact_root),
            },
        )]))
    }

    fn generate_nested_store_code(store_source: &str) -> String {
        let db_module = db_module_program();
        let store_module = parse_program(store_source);
        let main_module = main_module_program();

        let mut codegen = IrCodegen::new();
        codegen.add_module("db_schema", &db_module);
        codegen.add_module("store_json_store", &store_module);

        let db_path = vec!["db".to_string(), "schema".to_string()];
        let store_path = vec!["store".to_string(), "json_store".to_string()];
        let (_main_code, rust_modules) =
            must_ok(codegen.try_generate_multi_file_nested(&main_module, &[db_path.clone(), store_path.clone()]));

        must_some(rust_modules.get(&store_path), "missing generated nested store module").to_string()
    }

    fn generate_non_nested_store_code(store_source: &str, db_module_name: &str) -> String {
        let db_module = db_module_program();
        let store_module = parse_program(store_source);
        let main_module = main_module_program();

        let mut codegen = IrCodegen::new();
        codegen.add_module(db_module_name, &db_module);
        codegen.add_module("store", &store_module);

        let (_main_code, modules) = must_ok(codegen.try_generate_multi_file(&main_module, &[db_module_name, "store"]));

        must_some(modules.get("store"), "missing generated non-nested store module").to_string()
    }

    fn nested_module_code(modules: &[(&str, &str, Vec<&str>)], target_path: &[&str]) -> String {
        let main_module = main_module_program();
        let mut codegen = IrCodegen::new();
        let parsed_modules = modules
            .iter()
            .map(|(flat_name, source, path)| {
                (
                    (*flat_name).to_string(),
                    parse_program(source),
                    path.iter().map(|segment| (*segment).to_string()).collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        for (flat_name, program, _) in &parsed_modules {
            codegen.add_module(flat_name, program);
        }
        let paths = parsed_modules
            .iter()
            .map(|(_, _, path)| path.clone())
            .collect::<Vec<_>>();

        let (_main_code, rust_modules) = must_ok(codegen.try_generate_multi_file_nested(&main_module, &paths));
        let target = target_path
            .iter()
            .map(|segment| (*segment).to_string())
            .collect::<Vec<_>>();
        must_some(rust_modules.get(&target), "missing generated nested target module").to_string()
    }

    #[test]
    fn nested_decorated_generic_original_inherits_imported_reflection_bounds() {
        let code = nested_module_code(
            &[
                (
                    "substrait_schema",
                    r#"
def requires_clone[T with Clone]() -> str:
  return "clone"

pub def reflected_schema_marker[T]() -> str:
  return f"{T.__class_name__()}:{len(T.__fields__())}:{requires_clone[T]()}"
"#,
                    vec!["substrait", "schema"],
                ),
                (
                    "functions_csv_from_csv",
                    r#"
from substrait.schema import reflected_schema_marker

def registered_application(parts: list[str]) -> str:
  return parts[0]

def register[F]() -> ((F) -> F):
  return (func) => remember[F](func)

def remember[F](func: F) -> F:
  if func.__name__ == "":
    return func
  return func

@register()
pub def from_csv[T]() -> str:
  return registered_application([reflected_schema_marker[T]()])
"#,
                    vec!["functions", "csv", "from_csv"],
                ),
            ],
            &["functions", "csv", "from_csv"],
        );

        assert!(
            code.contains("fn __incan_original_from_csv<\n    T: incan_stdlib::reflection::HasTypeClassName")
                || code
                    .contains("fn __incan_original_from_csv<\n    T: incan_stdlib::reflection::HasTypeFieldMetadata"),
            "{code}"
        );
        assert!(
            code.contains("incan_stdlib::reflection::HasTypeClassName")
                && code.contains("incan_stdlib::reflection::HasTypeFieldMetadata")
                && code.contains("+ Clone"),
            "{code}"
        );
    }

    #[test]
    fn test_simple_function() {
        let code = generate(
            r#"
pub def add(a: int, b: int) -> int:
  return a + b
"#,
        );
        let add = projected_name(&code, "add", SemanticSourceTargetKind::Function);
        assert!(
            compact_rust(&code).contains(&format!("fn{add}(a:i64,b:i64,)->i64")),
            "{code}"
        );
        assert!(code.contains("a + b"));
    }

    #[test]
    fn test_model_generation() {
        let code = generate(
            r#"
pub model User:
  pub name: str
  pub age: int
"#,
        );
        assert!(code.contains("struct User"));
        assert!(code.contains("name: String"));
        assert!(code.contains("age: i64"));
    }

    #[test]
    fn test_serde_detection() {
        let source = r#"
from std.serde import json

@derive(json)
model Config:
  name: str
"#;
        assert!(detects_serde(source));
    }

    #[test]
    fn test_serde_detection_single_derive() {
        let source = r#"
from std.serde.json import Serialize

@derive(Serialize)
model User:
  id: int
"#;
        assert!(detects_serde(source));
    }

    #[test]
    fn test_no_serde_when_not_used() {
        let source = r#"
@derive(Clone, Debug)
model User:
  id: int
"#;
        assert!(!detects_serde(source));
    }

    #[test]
    fn test_serde_detection_json_stringify_builtin() {
        let source = r#"
def main() -> None:
  _ = json_stringify(123)
"#;
        assert!(detects_serde(source));
    }

    #[test]
    fn test_serde_detection_json_stringify_in_if_condition() {
        let source = r#"
def main() -> None:
  if json_stringify(1) == "1":
    pass
"#;
        assert!(detects_serde(source));
    }

    #[test]
    fn test_serde_detection_json_stringify_in_elif_body() {
        let source = r#"
def main() -> None:
  if true:
    pass
  elif false:
    _ = json_stringify(1)
"#;
        assert!(detects_serde(source));
    }

    #[test]
    fn test_serde_detection_json_stringify_in_while_condition() {
        let source = r#"
def main() -> None:
  while json_stringify(1) == "1":
    break
"#;
        assert!(detects_serde(source));
    }

    #[test]
    fn test_serde_detection_json_stringify_in_for_iterator() {
        let source = r#"
def main() -> None:
  for item in [json_stringify(1)]:
    _ = item
"#;
        assert!(detects_serde(source));
    }

    #[test]
    fn test_fstring_generation() {
        let code = generate(
            r#"
pub def greet(name: str) -> str:
  return f"Hello, {name}!"
"#,
        );
        assert!(code.contains(r#"incan_stdlib::strings::fstring"#));
        assert!(code.contains(r#"["Hello, ", "!"]"#));
    }

    #[test]
    fn test_struct_instantiation() {
        let code = generate(
            r#"
model Point:
  x: int
  y: int

def main() -> None:
  p = Point(x=10, y=20)
"#,
        );
        assert!(code.contains("Point {"));
        assert!(code.contains("x: 10"));
        assert!(code.contains("y: 20"));
    }

    #[test]
    fn test_enum_generation() {
        let code = generate(
            r#"
pub enum Status:
  Active
  Inactive
"#,
        );
        assert!(code.contains("enum Status"));
        assert!(code.contains("Active"));
        assert!(code.contains("Inactive"));
    }

    #[test]
    fn test_multi_file_imports_use_crate_prefix() {
        let store_code = generate_nested_store_code(
            r#"
from db.schema import Database

pub def touch(db: Database) -> None:
  return
"#,
        );
        assert!(store_code.contains("use crate::db::schema::Database;"));
        assert!(!store_code.contains("use db::schema::Database;"));
    }

    #[test]
    fn top_level_partial_keeps_one_projection_through_reexport_and_consumer_alias()
    -> Result<(), Box<dyn std::error::Error>> {
        let provider = parse_program_result(
            r#"
pub model Spec:
  pub namespace: str
  pub policy: str

pub portable = partial Spec(namespace="core")
"#,
        )?;
        let facade = parse_program_result("pub from provider import portable\n")?;
        let main = parse_program_result(
            r#"
from facade import portable as make_spec

def main() -> None:
  let spec = make_spec(policy="portable")
  println(spec.namespace)
"#,
        )?;
        let provider_path = vec!["provider".to_string()];
        let facade_path = vec!["facade".to_string()];
        let mut codegen = IrCodegen::new();
        codegen.add_module_with_path_segments("provider", &provider, provider_path.clone());
        codegen.add_module_with_path_segments("facade", &facade, facade_path.clone());

        let (main_code, modules) =
            codegen.try_generate_multi_file_nested(&main, &[provider_path.clone(), facade_path.clone()])?;
        let provider_code = modules.get(&provider_path).ok_or("missing generated provider module")?;
        let facade_code = modules.get(&facade_path).ok_or("missing generated facade module")?;
        let projection = provider_code
            .lines()
            .find_map(|line| {
                line.trim_start()
                    .strip_prefix("pub fn __incan_v1_")
                    .and_then(|tail| tail.split('(').next())
                    .map(|payload| format!("__incan_v1_{payload}"))
            })
            .ok_or("provider partial did not emit an incan-v1 function projection")?;

        assert!(
            facade_code.contains(&projection),
            "facade reexport did not bind the provider partial projection `{projection}`:\n{facade_code}"
        );
        assert!(
            facade_code.contains(&format!("pub use crate::provider::{projection} as portable;")),
            "the public facade must retain the partial's Rust-facing name:\n{facade_code}"
        );
        assert!(
            main_code.contains(&projection),
            "consumer alias did not bind or call the provider partial projection `{projection}`:\n{main_code}"
        );
        assert!(
            !facade_code.contains("provider::portable") && !main_code.contains("facade::portable"),
            "partial import/reexport fell back to a source spelling:\nfacade:\n{facade_code}\nconsumer:\n{main_code}"
        );
        Ok(())
    }

    #[test]
    fn source_static_declaration_reads_writes_and_init_share_one_projection() -> Result<(), Box<dyn std::error::Error>>
    {
        let program = parse_program_result(
            r#"
pub static counter: int = 0

pub def increment() -> int:
  counter = counter + 1
  return counter
"#,
        )?;
        let generated = IrCodegen::new().try_generate(&program)?;
        let projection = generated
            .lines()
            .find_map(|line| {
                line.trim_start()
                    .strip_prefix("pub static __incan_v1_")
                    .and_then(|tail| tail.split(':').next())
                    .map(|payload| format!("__incan_v1_{payload}"))
            })
            .ok_or("source static did not emit an incan-v1 projection")?;

        assert!(
            generated.matches(&projection).count() >= 4,
            "static declaration, module init, write, and read must share `{projection}`:\n{generated}"
        );
        assert!(
            generated.contains(&format!("pub use {projection} as COUNTER;")),
            "the public static must retain its existing Rust-facing name:\n{generated}"
        );
        assert!(
            !generated.contains("static COUNTER") && !generated.contains("COUNTER.with_"),
            "source static fell back to its raw Rust-global spelling:\n{generated}"
        );
        Ok(())
    }

    #[test]
    fn source_static_keeps_one_projection_through_reexport_and_consumer_alias() -> Result<(), Box<dyn std::error::Error>>
    {
        let provider = parse_program_result("pub static counter: int = 1\n")?;
        let facade = parse_program_result("pub from provider import counter\n")?;
        let main = parse_program_result(
            r#"
from facade import counter as shared

def main() -> None:
  println(shared)
"#,
        )?;
        let provider_path = vec!["provider".to_string()];
        let facade_path = vec!["facade".to_string()];
        let mut codegen = IrCodegen::new();
        codegen.add_module_with_path_segments("provider", &provider, provider_path.clone());
        codegen.add_module_with_path_segments("facade", &facade, facade_path.clone());

        let (main_code, modules) =
            codegen.try_generate_multi_file_nested(&main, &[provider_path.clone(), facade_path.clone()])?;
        let provider_code = modules.get(&provider_path).ok_or("missing generated provider module")?;
        let facade_code = modules.get(&facade_path).ok_or("missing generated facade module")?;
        let projection = provider_code
            .lines()
            .find_map(|line| {
                line.trim_start()
                    .strip_prefix("pub static __incan_v1_")
                    .and_then(|tail| tail.split(':').next())
                    .map(|payload| format!("__incan_v1_{payload}"))
            })
            .ok_or("provider static did not emit an incan-v1 projection")?;

        assert!(
            facade_code.contains(&projection),
            "facade reexport did not bind the provider static projection `{projection}`:\n{facade_code}"
        );
        assert!(
            facade_code.contains(&format!("pub use crate::provider::{projection} as COUNTER;")),
            "the public facade must retain the static's Rust-facing name:\n{facade_code}"
        );
        assert!(
            main_code.contains(&projection),
            "consumer alias did not bind and read the provider static projection `{projection}`:\n{main_code}"
        );
        assert!(
            !facade_code.contains("provider::COUNTER") && !main_code.contains("facade::COUNTER"),
            "static import/reexport fell back to a source spelling:\nfacade:\n{facade_code}\nconsumer:\n{main_code}"
        );
        Ok(())
    }

    #[test]
    fn generated_decorator_static_collision_keeps_distinct_identifiers_and_reads()
    -> Result<(), Box<dyn std::error::Error>> {
        let program = parse_program_result(
            r#"
pub static __incan_decorated_target: int = 41

def preserve[F]() -> ((F) -> F):
  return (func) => func

@preserve()
pub def target() -> int:
  return 1

pub def source_value() -> int:
  return __incan_decorated_target
"#,
        )?;
        let generated = IrCodegen::new().try_generate(&program)?;
        let source_projection = generated
            .lines()
            .find_map(|line| {
                line.trim_start()
                    .strip_prefix("pub static __incan_v1_")
                    .and_then(|tail| tail.split(':').next())
                    .map(|payload| format!("__incan_v1_{payload}"))
            })
            .ok_or("colliding source static did not emit an incan-v1 projection")?;

        assert!(
            generated.contains("static __INCAN_DECORATED_TARGET:")
                && generated.contains("__INCAN_DECORATED_TARGET.get()"),
            "the generated decorator cell and its wrapper read must retain the synthetic identifier:\n{generated}"
        );
        assert!(
            generated.matches(&source_projection).count() >= 3,
            "the colliding source static declaration, init, and read must share `{source_projection}`:\n{generated}"
        );
        assert!(
            !source_projection.contains("__INCAN_DECORATED_TARGET"),
            "the source and generated static spellings unexpectedly collapsed: {source_projection}"
        );
        Ok(())
    }

    #[test]
    fn generated_decorator_function_collision_keeps_distinct_identifiers_and_calls()
    -> Result<(), Box<dyn std::error::Error>> {
        let program = parse_program_result(
            r#"
def preserve[F]() -> ((F) -> F):
  return (func) => func

pub def __incan_original_target() -> int:
  return 41

@preserve()
pub def target() -> int:
  return 1
"#,
        )?;
        let generated = IrCodegen::new().try_generate(&program)?;
        let public_source_projections = generated
            .lines()
            .filter_map(|line| {
                line.trim_start()
                    .strip_prefix("pub fn __incan_v1_")
                    .and_then(|tail| tail.split('(').next())
                    .map(|payload| format!("__incan_v1_{payload}"))
            })
            .collect::<std::collections::BTreeSet<_>>();

        assert!(
            generated.contains("fn __incan_original_target("),
            "the generated decorator original did not retain its private physical name:\n{generated}"
        );
        assert!(
            generated.matches("__incan_original_target").count() >= 2,
            "the decorator application did not reference the generated original by its physical name:\n{generated}"
        );
        assert_eq!(
            public_source_projections.len(),
            2,
            "the colliding source declaration and decorated wrapper must retain two distinct public projections:\n{generated}"
        );
        assert!(
            !generated.contains("@generated/decorator-original"),
            "the collision-proof registry key leaked into generated Rust:\n{generated}"
        );
        Ok(())
    }

    #[test]
    fn same_module_public_function_and_static_aliases_bind_exact_projections() -> Result<(), Box<dyn std::error::Error>>
    {
        let program = parse_program_result(
            r#"
pub def average(left: int, right: int) -> int:
  return (left + right) // 2

pub mean = alias average
pub static total: int = 2
pub tally = alias total

pub def summarize() -> int:
  return mean(total, tally)
"#,
        )?;
        let generated = IrCodegen::new().try_generate(&program)?;
        let function_projection = generated
            .lines()
            .find_map(|line| {
                line.trim_start()
                    .strip_prefix("pub fn __incan_v1_")
                    .and_then(|tail| tail.split('(').next())
                    .map(|payload| format!("__incan_v1_{payload}"))
            })
            .ok_or("source function did not emit an incan-v1 projection")?;
        let static_projection = generated
            .lines()
            .find_map(|line| {
                line.trim_start()
                    .strip_prefix("pub static __incan_v1_")
                    .and_then(|tail| tail.split(':').next())
                    .map(|payload| format!("__incan_v1_{payload}"))
            })
            .ok_or("source static did not emit an incan-v1 projection")?;

        assert!(
            generated.contains(&format!("pub use {function_projection} as mean;")),
            "the public function alias did not bind its target projection:\n{generated}"
        );
        assert!(
            generated.contains(&format!("pub use {static_projection} as tally;")),
            "the public static alias did not bind its target projection:\n{generated}"
        );
        assert!(
            !generated.contains("pub use average as mean;") && !generated.contains("pub use total as tally;"),
            "same-module aliases fell back to raw source target names:\n{generated}"
        );
        Ok(())
    }

    #[test]
    fn nested_ordinary_binding_shadows_outer_static_binding_alias() -> Result<(), Box<dyn std::error::Error>> {
        let program = parse_program_result(
            r#"
static items: list[int] = []

def count_inner() -> int:
  let live = items
  if true:
    let live = [1, 2]
    return len(live)
  return len(live)
"#,
        )?;
        let mut codegen = IrCodegen::new();
        codegen.set_externally_reachable_items(std::collections::HashSet::from(["count_inner".to_string()]));
        let generated = codegen.try_generate(&program)?;

        assert!(
            generated.contains("let live = vec![") && generated.contains("live.len() as i64"),
            "the inner ordinary binding did not retain local value emission:\n{generated}"
        );
        assert!(
            !generated.contains("StaticBinding::from_static(&LIVE)") && !generated.contains("LIVE.get()"),
            "the inner ordinary binding inherited an outer static-binding classification:\n{generated}"
        );
        Ok(())
    }

    #[test]
    fn source_eq_magic_method_keeps_abi_slot_and_recoverable_projection() -> Result<(), Box<dyn std::error::Error>> {
        let program = parse_program_result(
            r#"
model Value:
  value: int

  def __eq__(self, other: Value) -> bool:
    return self.value == other.value

def same(left: Value, right: Value) -> bool:
  return left == right
"#,
        )?;
        let mut codegen = IrCodegen::new();
        codegen.set_externally_reachable_items(std::collections::HashSet::from(["same".to_string()]));
        let generated = codegen.try_generate(&program)?;

        assert!(
            generated.contains("impl PartialEq for Value"),
            "source __eq__ must retain Rust's required PartialEq ABI slot:\n{generated}"
        );
        assert!(
            generated.contains("pub fn __incan_v1_")
                && generated.contains("<Self as std::cmp::PartialEq>::eq(self, &other)"),
            "source __eq__ must expose a recoverable wrapper that invokes the ABI slot:\n{generated}"
        );
        assert!(
            !generated.contains("self.__eq__(other)"),
            "recoverable __eq__ wrapper must not call a nonexistent inherent method:\n{generated}"
        );
        Ok(())
    }

    #[test]
    fn result_map_err_closure_keeps_concrete_member_identity() {
        let generated = generate(
            r#"
pub model Failure:
  detail: str

  def message(self) -> str:
    return self.detail

pub def describe(result: Result[int, Failure]) -> Result[int, str]:
  return result.map_err((error) => error.message())
"#,
        );
        let projection = projected_name(&generated, "message", SemanticSourceTargetKind::Method);
        let compact = compact_rust(&generated);

        assert!(
            compact.contains(&format!("error.{projection}()")),
            "map_err must contextually type its error closure before member projection:\n{generated}"
        );
        assert!(
            !compact.contains("|error|error.message()"),
            "a contextually-known source member must not fall back to its raw spelling:\n{generated}"
        );
    }

    #[test]
    fn result_map_err_string_literal_closure_owns_its_checked_return() {
        let generated = generate(
            r#"
pub def normalize(result: Result[int, int]) -> Result[int, str]:
  return result.map_err((_error) => "malformed")
"#,
        );
        let compact = compact_rust(&generated);

        assert!(
            compact.contains("map_err(|_error|\"malformed\".to_string())"),
            "a contextually typed closure must materialize its checked str return:\n{generated}"
        );
    }

    #[test]
    fn test_multi_file_model_aliases_work_across_modules() {
        // DB module defines a model with an alias. Store module should be able to use the alias
        // in member access and constructor calls and still emit canonical Rust field names.
        let db_module = parse_program(
            r#"
model Account:
  type_ [alias="type"]: str
"#,
        );
        let store_module = parse_program(
            r#"
from db.schema import Account

pub def get_type(a: Account) -> str:
  return a.type

pub def make() -> Account:
  return Account(type="x")
"#,
        );
        let main_module = main_module_program();

        let mut codegen = IrCodegen::new();
        codegen.add_module("db_schema", &db_module);
        codegen.add_module("store_json_store", &store_module);

        let db_path = vec!["db".to_string(), "schema".to_string()];
        let store_path = vec!["store".to_string(), "json_store".to_string()];
        let (_main_code, rust_modules) =
            must_ok(codegen.try_generate_multi_file_nested(&main_module, &[db_path.clone(), store_path.clone()]));
        let store_code = must_some(rust_modules.get(&store_path), "missing generated store module").to_string();

        assert!(
            store_code.contains(".type_"),
            "expected canonical field access; got:\n{store_code}"
        );
        assert!(
            store_code.contains("Account { type_:"),
            "expected canonical struct field init; got:\n{store_code}"
        );
        assert!(
            !store_code.contains(".type;"),
            "should not emit Rust keyword field access"
        );
        assert!(
            !store_code.contains("Account { type:"),
            "should not emit Rust keyword field init"
        );
    }

    #[test]
    fn test_multi_file_model_aliases_work_with_import_alias() {
        let db_module = parse_program(
            r#"
model Account:
  type_ [alias="type"]: str
"#,
        );
        let store_module = parse_program(
            r#"
from db.schema import Account as A

pub def get_type(a: A) -> str:
  return a.type

pub def make() -> A:
  return A(type="x")
"#,
        );
        let main_module = main_module_program();

        let mut codegen = IrCodegen::new();
        codegen.add_module("db_schema", &db_module);
        codegen.add_module("store_json_store", &store_module);

        let db_path = vec!["db".to_string(), "schema".to_string()];
        let store_path = vec!["store".to_string(), "json_store".to_string()];
        let (_main_code, rust_modules) =
            must_ok(codegen.try_generate_multi_file_nested(&main_module, &[db_path.clone(), store_path.clone()]));
        let store_code = must_some(rust_modules.get(&store_path), "missing generated aliased store module").to_string();

        assert!(
            store_code.contains(".type_"),
            "expected canonical field access; got:\n{store_code}"
        );
        assert!(
            store_code.contains("A { type_:"),
            "expected canonical struct field init; got:\n{store_code}"
        );
    }

    #[test]
    fn test_multi_file_self_alias_resolution_in_dependency_module() {
        let db_module = parse_program(
            r#"
pub model Account:
  pub type_ [alias="type"]: str

  def get_type(self) -> str:
    return self.type
"#,
        );
        let main_module = main_module_program();

        let mut codegen = IrCodegen::new();
        codegen.add_module("db_schema", &db_module);

        let db_path = vec!["db".to_string(), "schema".to_string()];
        let (_main_code, rust_modules) =
            must_ok(codegen.try_generate_multi_file_nested(&main_module, std::slice::from_ref(&db_path)));
        let db_code = must_some(rust_modules.get(&db_path), "missing generated db module").to_string();

        assert!(
            db_code.contains("self.type_"),
            "expected canonical field access in dependency module; got:\n{db_code}"
        );
        assert!(
            !db_code.contains("self.type;"),
            "should not emit Rust keyword field access"
        );
    }

    #[test]
    fn test_same_named_stdlib_helpers_do_not_contaminate_nested_module_signatures()
    -> Result<(), Box<dyn std::error::Error>> {
        let main_module = parse_program_result(
            r#"
from std.testing import timeout
from std.async.time import timeout as async_timeout

def main() -> None:
  return
"#,
        )?;
        let testing_module = read_stdlib_program("crates/incan_stdlib/stdlib/testing.incn")?;
        let async_task_module = read_stdlib_program("crates/incan_stdlib/stdlib/async/task.incn")?;
        let async_time_module = read_stdlib_program("crates/incan_stdlib/stdlib/async/time.incn")?;
        let traits_error_module = read_stdlib_program("crates/incan_stdlib/stdlib/traits/error.incn")?;

        let testing_path = vec!["__incan_std".to_string(), "testing".to_string()];
        let async_task_path = vec!["__incan_std".to_string(), "async".to_string(), "task".to_string()];
        let async_time_path = vec!["__incan_std".to_string(), "async".to_string(), "time".to_string()];
        let traits_error_path = vec!["__incan_std".to_string(), "traits".to_string(), "error".to_string()];

        let mut codegen = IrCodegen::new();
        codegen.add_module_with_path_segments("__incan_std_testing", &testing_module, testing_path.clone());
        codegen.add_module_with_path_segments("__incan_std_async_task", &async_task_module, async_task_path.clone());
        codegen.add_module_with_path_segments("__incan_std_async_time", &async_time_module, async_time_path.clone());
        codegen.add_module_with_path_segments(
            "__incan_std_traits_error",
            &traits_error_module,
            traits_error_path.clone(),
        );

        let (_main_code, rust_modules) = codegen.try_generate_multi_file_nested(
            &main_module,
            &[
                testing_path.clone(),
                async_task_path,
                async_time_path,
                traits_error_path,
            ],
        )?;
        let testing_code = rust_modules
            .get(&testing_path)
            .ok_or_else(|| std::io::Error::other("missing generated std.testing module"))?;

        let timeout = projected_name(testing_code, "timeout", SemanticSourceTargetKind::Function);
        assert!(
            compact_rust(testing_code).contains(&format!("pubfn{timeout}(duration:String,)")),
            "std.testing.timeout should remain a non-generic marker wrapper; got:\n{testing_code}"
        );
        assert!(
            !testing_code.contains("RuntimeFuture"),
            "std.testing wrapper should not inherit std.async.time.timeout bounds; got:\n{testing_code}"
        );
        Ok(())
    }

    #[test]
    fn imported_stdlib_trait_default_expands_in_dependency_impl() -> Result<(), Box<dyn std::error::Error>> {
        let main_module = parse_program_result(
            r#"
from std.io import BytesIO

def main() -> None:
  return
"#,
        )?;
        let io_module = read_stdlib_program("crates/incan_stdlib/stdlib/io.incn")?;
        let traits_error_module = read_stdlib_program("crates/incan_stdlib/stdlib/traits/error.incn")?;

        let io_path = vec!["__incan_std".to_string(), "io".to_string()];
        let traits_error_path = vec!["__incan_std".to_string(), "traits".to_string(), "error".to_string()];

        let mut codegen = IrCodegen::new();
        codegen.add_module_with_path_segments("__incan_std_io", &io_module, io_path.clone());
        codegen.add_module_with_path_segments(
            "__incan_std_traits_error",
            &traits_error_module,
            traits_error_path.clone(),
        );

        let (_main_code, rust_modules) =
            codegen.try_generate_multi_file_nested(&main_module, &[io_path.clone(), traits_error_path])?;
        let io_code = rust_modules
            .get(&io_path)
            .ok_or_else(|| std::io::Error::other("missing generated std.io module"))?;

        assert!(
            io_code.contains("impl Error for IoError"),
            "expected IoError to adopt std.traits.error.Error; got:\n{io_code}"
        );
        assert!(
            io_code.contains("fn source(&self) -> Option<String>"),
            "expected imported Error.source default method to expand into IoError impl; got:\n{io_code}"
        );
        assert!(
            io_code.contains("MapFn: Clone + crate::__incan_std::traits::callable::Callable1<Vec<u8>, U>")
                && io_code.contains("Folder: Clone + crate::__incan_std::traits::callable::Callable2<U, Vec<u8>, U>"),
            "imported FallibleIterator defaults must retain callable bounds from their defining module; got:\n{io_code}"
        );
        assert!(
            io_code.contains("f.__call__(acc.clone(), item.clone())"),
            "imported FallibleIterator defaults must retain nominal Callable2 dispatch; got:\n{io_code}"
        );
        Ok(())
    }

    #[test]
    fn package_codegen_keeps_embedded_stdlib_method_identity_at_declaration_origin()
    -> Result<(), Box<dyn std::error::Error>> {
        let main_module = parse_program_result(
            r#"
from std.io import BytesIO

pub def oven_bytes() -> bytes:
  return BytesIO(b"oven").getvalue()
"#,
        )?;
        let io_module = read_stdlib_program("crates/incan_stdlib/stdlib/io.incn")?;
        let traits_error_module = read_stdlib_program("crates/incan_stdlib/stdlib/traits/error.incn")?;

        let io_path = vec!["__incan_std".to_string(), "io".to_string()];
        let traits_error_path = vec!["__incan_std".to_string(), "traits".to_string(), "error".to_string()];

        let mut codegen = IrCodegen::new();
        codegen.set_canonical_emission_package_identity(Some("oven_release_bytes_io".to_string()));
        codegen.add_module_with_path_segments("__incan_std_io", &io_module, io_path.clone());
        codegen.add_module_with_path_segments(
            "__incan_std_traits_error",
            &traits_error_module,
            traits_error_path.clone(),
        );

        let (main_code, rust_modules) =
            codegen.try_generate_multi_file_nested(&main_module, &[io_path.clone(), traits_error_path])?;
        let io_code = rust_modules
            .get(&io_path)
            .ok_or_else(|| std::io::Error::other("missing generated std.io module"))?;

        let referenced_getvalue = projected_name(&main_code, "getvalue", SemanticSourceTargetKind::Method);
        let declared_getvalue = projected_name(io_code, "getvalue", SemanticSourceTargetKind::Method);
        assert_eq!(
            referenced_getvalue, declared_getvalue,
            "a stdlib method reference must keep the identity assigned at its declaration site"
        );
        let getvalue_identity = projected_identity(io_code, "getvalue", SemanticSourceTargetKind::Method);
        assert_eq!(getvalue_identity.origin, SymbolOrigin::Module(io_path.clone()));

        let referenced_constructor = projected_name(&main_code, "BytesIO", SemanticSourceTargetKind::Function);
        let declared_constructor = projected_name(io_code, "BytesIO", SemanticSourceTargetKind::Function);
        assert_eq!(
            referenced_constructor, declared_constructor,
            "a stdlib constructor reference must keep the identity assigned at its declaration site"
        );
        let constructor_identity = projected_identity(io_code, "BytesIO", SemanticSourceTargetKind::Function);
        assert_eq!(constructor_identity.origin, SymbolOrigin::Module(io_path));
        Ok(())
    }

    #[test]
    fn streaming_hash_helpers_import_io_error_for_reader_chunk_failures() -> Result<(), Box<dyn std::error::Error>> {
        let streaming_module = read_stdlib_program("crates/incan_stdlib/stdlib/hash/_streaming.incn")?;
        let streaming_code = IrCodegen::new().try_generate(&streaming_module)?;
        let compact_streaming_code = compact_rust(&streaming_code);

        let reader_digest = projected_name(&streaming_code, "reader_digest", SemanticSourceTargetKind::Function);
        let feed_digest_reader = projected_name(
            &streaming_code,
            "_feed_digest_reader",
            SemanticSourceTargetKind::Function,
        );
        assert!(
            compact_streaming_code.contains(&format!("pubfn{reader_digest}<R:BinaryReader,>")),
            "the public reader API must retain its source-declared BinaryReader-only contract; got:\n{streaming_code}"
        );
        assert!(
            compact_streaming_code.contains(&format!("{feed_digest_reader}<H:ByteDigestHasher,R:BinaryReader,>")),
            "streaming over ReaderChunks<R> must preserve the source-declared BinaryReader contract without a hidden Clone requirement; got:\n{streaming_code}"
        );
        assert!(
            !compact_streaming_code.contains("R:BinaryReader+Clone"),
            "streaming hash dispatch must move mutually exclusive reader uses instead of narrowing public or private contracts with Clone; got:\n{streaming_code}"
        );
        assert!(
            streaming_code.contains("pub use crate::__incan_std::io::IoError;"),
            "std.hash._streaming must import the IoError carried by BinaryReader chunks; got:\n{streaming_code}"
        );
        assert!(
            compact_streaming_code.contains("FallibleIterator::<Vec<u8>,HashError,>")
                && compact_streaming_code.contains("|error:IoError|"),
            "streaming reader helpers must preserve the imported chunk error at the mapping boundary and the mapped hash error afterward; got:\n{streaming_code}"
        );
        Ok(())
    }

    #[test]
    fn compression_auto_moves_non_clone_decoder_match_bindings() -> Result<(), Box<dyn std::error::Error>> {
        let auto_module = read_stdlib_program("crates/incan_stdlib/stdlib/compression/_auto.incn")?;
        let auto_code = IrCodegen::new().try_generate(&auto_module)?;

        assert!(
            auto_code.contains("let mut adapter = reader;"),
            "a final assignment from a Rust decoder match binding must move without assuming Clone"
        );
        assert!(
            !auto_code.contains("let mut adapter = reader.clone();"),
            "non-Clone Rust decoder match bindings must not receive backend-inserted clones"
        );
        Ok(())
    }

    #[test]
    fn test_rust_imports_do_not_use_crate_prefix() {
        let code = generate(
            r#"
from rust::time import Duration

pub def touch(duration: Duration) -> None:
  return
"#,
        );
        assert!(code.contains("use ::time::Duration;"));
        assert!(!code.contains("use crate::time::Duration;"));
    }

    #[test]
    fn test_rust_style_external_crate_import_is_not_forced_under_crate() {
        let code = generate(
            r#"
import serde::Serialize

pub def touch(value: Serialize) -> None:
  return
"#,
        );
        assert!(code.contains("use serde::Serialize;"));
        assert!(!code.contains("use crate::serde::Serialize;"));
    }

    #[test]
    fn test_relative_from_import_uses_super_prefix() {
        let store_code = generate_nested_store_code(
            r#"
from ..db.schema import Database

pub def touch(db: Database) -> None:
  return
"#,
        );
        assert!(store_code.contains("use super::db::schema::Database;"));
        assert!(!store_code.contains("use crate::db::schema::Database;"));
    }

    #[test]
    fn test_multi_file_imports_rust_style_module_import_uses_crate_prefix() {
        let store_code = generate_nested_store_code(
            r#"
import db::schema::Database

pub def touch(db: Database) -> None:
  return
"#,
        );
        assert!(store_code.contains("use crate::db::schema::Database;"));
        assert!(!store_code.contains("use db::schema::Database;"));
    }

    #[test]
    fn test_non_nested_multi_file_api_sets_internal_module_roots() {
        let store_code = generate_non_nested_store_code(
            r#"
from db import Database

pub def touch(db: Database) -> None:
  return
"#,
            "db",
        );
        assert!(store_code.contains("use crate::db::Database;"));
        assert!(!store_code.contains("use db::Database;"));
    }

    #[test]
    fn test_non_nested_multi_file_nested_modules_use_crate_prefix() {
        let store_code = generate_non_nested_store_code(
            r#"
from db.schema import Database

pub def touch(db: Database) -> None:
  return
"#,
            "db_schema",
        );
        assert!(store_code.contains("use crate::db::schema::Database;"));
        assert!(!store_code.contains("use db::schema::Database;"));
    }

    #[test]
    fn test_pub_from_import_emits_dependency_crate_item_paths() {
        let ast = parse_program(
            r#"
from pub::widgets import Widget as PublicWidget, make_widget

def main() -> None:
  w: PublicWidget = make_widget("ok")
"#,
        );
        let mut codegen = IrCodegen::new();
        codegen.set_library_manifest_index(library_index_with_widgets_exports());
        let code = must_ok(codegen.try_generate(&ast));
        assert!(code.contains("use widgets::Widget as PublicWidget;"));
        assert!(code.contains("widgets::make_widget(\"ok\".to_string())"));
        assert!(!code.contains("use widgets::make_widget;"));
        assert!(!code.contains("pub use widgets::Widget as PublicWidget;"));
        assert!(!code.contains("pub use widgets::make_widget;"));
        assert!(!code.contains("pub::widgets"));
    }

    #[test]
    fn test_pub_import_expressions_codegen() {
        let source = r#"
from pub::widgets import Widget, make_widget, DEFAULT_NAME

def main() -> None:
  mut w: Widget = make_widget(DEFAULT_NAME)
"#;
        let ast = parse_program(source);
        let mut codegen = IrCodegen::new();
        codegen.set_library_manifest_index(library_index_with_widgets_exports());
        let code = must_ok(codegen.try_generate(&ast));
        assert!(
            code.contains("let _w: Widget = widgets::make_widget(DEFAULT_NAME.to_string());"),
            "Generated code did not match expected. Code was:\n{code}"
        );
    }

    #[test]
    fn test_pub_module_import_alias_emits_use_alias() {
        let ast = parse_program(
            r#"
import pub::widgets as widgets_alias

def main() -> None:
  widgets_alias.make_widget("ok")
"#,
        );
        let mut codegen = IrCodegen::new();
        codegen.set_library_manifest_index(library_index_with_widgets_exports());
        let code = must_ok(codegen.try_generate(&ast));
        assert!(code.contains("use widgets as widgets_alias;"));
        assert!(!code.contains("pub use widgets as widgets_alias;"));
        assert!(!code.contains("use pub::widgets"));
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_borrows_rust_backed_free_function_args_from_metadata() -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use incan_core::interop::{RustFunctionSig, RustItemKind, RustItemMetadata, RustParam, RustVisibility};

        let source = r#"
from rust::demo import Thing
from rust::demo import takes_ref

pub def forward(value: Thing) -> None:
  takes_ref(value)
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let tmp = seeded_rust_inspect_workspace()?;
        let manifest_dir = tmp.path().to_path_buf();
        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(manifest_dir.clone());
        tc.rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::takes_ref".to_string(),
                    definition_path: Some("demo::takes_ref".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Function(RustFunctionSig {
                        type_params: Vec::new(),
                        params: vec![RustParam {
                            name: Some("value".to_string()),
                            type_display: "&demo::Thing".to_string(),
                        }],
                        return_type: "()".to_string(),
                        is_async: false,
                        is_unsafe: false,
                    }),
                },
            )
            .map_err(|e| std::io::Error::other(format!("seed rust-inspect function: {e}")))?;
        tc.check_program(&ast)
            .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|err| std::io::Error::other(format!("lowering failed: {err:?}")))?;

        let mut codegen = IrCodegen::new();
        codegen.collect_external_rust_functions(&ast);

        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        emitter.set_external_rust_functions(codegen.external_rust_functions.clone());
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|err| std::io::Error::other(format!("emit failed: {err:?}")))?;

        assert!(
            code.contains("takes_ref(&value);"),
            "expected borrowed rust free-function arg in generated code; got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_boxes_variant_payloads_whatever_argument_shape() -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use incan_core::interop::{
            RustItemKind, RustItemMetadata, RustPayloadCarrier, RustTypeInfo, RustTypeShape, RustVariantInfo,
            RustVisibility,
        };

        // A Rust enum variant that stores `Box<i64>`; Incan records the payload as `i64` plus its carrier, so every
        // argument shape — a literal, a call result, a method result — must reach the constructor inside `Box::new`.
        let source = r#"
from rust::demo import Kind

def identity(value: int) -> int:
  return value

pub def build(values: List[int]) -> List[Kind]:
  return [Kind.Tuple(1), Kind.Tuple(identity(2)), Kind.Tuple(values[0]), Kind.Tuple(len(values))]
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let tmp = seeded_rust_inspect_workspace()?;
        let manifest_dir = tmp.path().to_path_buf();
        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(manifest_dir.clone());
        tc.rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::Kind".to_string(),
                    definition_path: Some("demo::Kind".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Type(RustTypeInfo {
                        type_params: Vec::new(),
                        type_param_defaults: Vec::new(),
                        mutable_reference_type_params: Vec::new(),
                        expanded_derive_traits: Vec::new(),
                        has_const_params: false,
                        alias_target: None,
                        metadata_completeness: Default::default(),
                        methods: vec![],
                        implemented_traits: Vec::new(),
                        fields: vec![],
                        variants: vec![RustVariantInfo {
                            name: "Tuple".to_string(),
                            fields: vec![RustTypeShape::Int],
                            field_carriers: vec![RustPayloadCarrier::Boxed],
                        }],
                    }),
                },
            )
            .map_err(|e| std::io::Error::other(format!("seed rust-inspect kind: {e}")))?;
        tc.check_program(&ast)
            .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|err| std::io::Error::other(format!("lowering failed: {err:?}")))?;
        let mut codegen = IrCodegen::new();
        codegen.collect_external_rust_functions(&ast);
        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        emitter.set_external_rust_functions(codegen.external_rust_functions.clone());
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|err| std::io::Error::other(format!("emit failed: {err:?}")))?;

        let boxed = code.matches("Box::new(").count();
        assert_eq!(
            boxed, 4,
            "every variant payload must be boxed regardless of argument shape; got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_borrows_as_fd_generic_args_from_metadata() -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use incan_core::interop::{RustFunctionSig, RustItemKind, RustItemMetadata, RustParam, RustVisibility};

        let source = r#"
from rust::demo import File
from rust::demo import flock

pub def retain(file: File) -> File:
  flock(file)
  return file
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let tmp = seeded_rust_inspect_workspace()?;
        let manifest_dir = tmp.path().to_path_buf();
        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(manifest_dir.clone());
        tc.rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::flock".to_string(),
                    definition_path: Some("demo::flock".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Function(RustFunctionSig {
                        type_params: Vec::new(),
                        params: vec![RustParam {
                            name: Some("fd".to_string()),
                            type_display: "&impl AsFd".to_string(),
                        }],
                        return_type: "()".to_string(),
                        is_async: false,
                        is_unsafe: false,
                    }),
                },
            )
            .map_err(|error| std::io::Error::other(format!("seed rust-inspect function: {error}")))?;
        tc.check_program(&ast)
            .map_err(|errors| std::io::Error::other(format!("typecheck failed: {errors:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|error| std::io::Error::other(format!("lowering failed: {error:?}")))?;

        let mut codegen = IrCodegen::new();
        codegen.collect_external_rust_functions(&ast);

        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        emitter.set_external_rust_functions(codegen.external_rust_functions.clone());
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|error| std::io::Error::other(format!("emit failed: {error:?}")))?;

        assert!(
            code.contains("flock(&file);"),
            "expected an AsFd generic argument to borrow the retained file; got:\n{code}"
        );
        assert!(
            code.contains("return file;"),
            "the retained file must remain available after the AsFd call; got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_borrows_retained_rustix_file_for_as_fd_generic_free_function()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::backend::project::ProjectGenerator;
        use crate::manifest::{DependencySource, DependencySpec};
        use crate::rust_inspect::write_rustix_as_fd_probe_crate;

        let source = r#"
from rust::rustix::fs import File, FlockOperation, flock

pub def retain(file: File) -> File:
  flock(file, FlockOperation.LockExclusive)
  file.sync_all()
  return file
"#;
        let tmp = tempfile::tempdir()?;
        write_rustix_as_fd_probe_crate(tmp.path())?;

        let ast = parse_program_result(source)?;
        let mut codegen = IrCodegen::new();
        codegen.set_rust_inspect_manifest_dir(tmp.path().to_path_buf());
        let code = codegen
            .try_generate(&ast)
            .map_err(|error| std::io::Error::other(format!("codegen failed: {error}")))?;

        assert!(
            code.contains("flock(&file, FlockOperation::LockExclusive);"),
            "expected rustix::fs::flock to borrow the retained File through its AsFd generic; got:\n{code}"
        );
        assert!(
            code.contains("file.sync_all();") && code.contains("return file;"),
            "the retained file must remain available for sync_all and return after flock; got:\n{code}"
        );

        let generated_root = tmp.path().join("generated");
        let mut generator = ProjectGenerator::new(&generated_root, "retained_rustix_file", false);
        generator.set_dependencies(vec![DependencySpec {
            crate_name: "rustix".to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path {
                path: tmp.path().to_path_buf(),
            },
            optional: false,
            package: None,
        }]);
        generator.generate(&code)?;
        let capability = crate::oven::compiler_suite_env::OvenCompilerSuiteCapability::from_environment(
            crate::oven::compiler_suite_env::OVEN_COMPILER_SUITE_CAPABILITY_ENV,
        )
        .map_err(std::io::Error::other)?;
        let output = if let Some(capability) = capability {
            let fixture_output = tmp.path().join("oven-rustix-fixture");
            std::fs::create_dir_all(&fixture_output)?;
            let fixture = std::process::Command::new(&capability.rustc)
                .args(["--edition=2021", "--crate-name", "rustix", "--crate-type=rlib"])
                .arg(tmp.path().join("src/lib.rs"))
                .arg("--out-dir")
                .arg(&fixture_output)
                .output()?;
            assert!(
                fixture.status.success(),
                "expected the direct-rustc rustix fixture to compile. stderr:\n{}\nstdout:\n{}",
                String::from_utf8_lossy(&fixture.stderr),
                String::from_utf8_lossy(&fixture.stdout)
            );

            let rustix_rlib = fixture_output.join("librustix.rlib");
            let mut command = std::process::Command::new(&capability.rustc);
            command
                .args([
                    "--edition=2024",
                    "--crate-name",
                    "retained_rustix_file",
                    "--crate-type=lib",
                    "--emit=metadata",
                ])
                .arg("--out-dir")
                .arg(generated_root.join("oven-check"))
                .env("CARGO_MANIFEST_DIR", &generated_root)
                .env("CARGO_PKG_NAME", "retained_rustix_file")
                .env("CARGO_PKG_VERSION", "0.1.0");
            for dependency_path in &capability.dependency_search_paths {
                command
                    .arg("-L")
                    .arg(format!("dependency={}", dependency_path.display()));
            }
            for (crate_name, path) in &capability.externs {
                command.arg("--extern").arg(format!("{crate_name}={}", path.display()));
            }
            command
                .arg("--extern")
                .arg(format!("rustix={}", rustix_rlib.display()))
                .arg(generator.crate_root_path())
                .output()?
        } else {
            std::process::Command::new("cargo")
                .args(["check", "--offline"])
                .current_dir(&generated_root)
                .output()?
        };
        assert!(
            output.status.success(),
            "expected the generated retained-file Rust to compile. stderr:\n{}\nstdout:\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_materializes_owner_specialized_rust_associated_function_arguments()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use incan_core::interop::{
            RustFunctionSig, RustItemKind, RustItemMetadata, RustMethodSig, RustParam, RustTypeInfo, RustVisibility,
        };

        let source = r#"
from rust::demo import PairFactory

def accept_pair(value: PairFactory[i64, str]) -> None:
  pass

pub def build_pair() -> None:
  accept_pair(PairFactory.new(7, "marker"))
"#;
        let tokens =
            lexer::lex(source).map_err(|errors| std::io::Error::other(format!("lexing failed: {errors:?}")))?;
        let ast =
            parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("parsing failed: {errors:?}")))?;

        let tmp = seeded_rust_inspect_workspace()?;
        let manifest_dir = tmp.path().to_path_buf();
        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(manifest_dir.clone());
        tc.rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::PairFactory".to_string(),
                    definition_path: Some("demo::PairFactory".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Type(RustTypeInfo {
                        type_params: vec!["T".to_string(), "U".to_string()],
                        type_param_defaults: Vec::new(),
                        mutable_reference_type_params: Vec::new(),
                        expanded_derive_traits: Vec::new(),
                        has_const_params: false,
                        alias_target: None,
                        metadata_completeness: Default::default(),
                        methods: vec![RustMethodSig {
                            name: "new".to_string(),
                            signature: RustFunctionSig {
                                type_params: Vec::new(),
                                params: vec![
                                    RustParam {
                                        name: Some("value".to_string()),
                                        type_display: "T".to_string(),
                                    },
                                    RustParam {
                                        name: Some("marker".to_string()),
                                        type_display: "U".to_string(),
                                    },
                                ],
                                return_type: "demo::PairFactory<T, U>".to_string(),
                                is_async: false,
                                is_unsafe: false,
                            },
                        }],
                        implemented_traits: Vec::new(),
                        fields: Vec::new(),
                        variants: Vec::new(),
                    }),
                },
            )
            .map_err(|err| std::io::Error::other(format!("seed rust-inspect type: {err}")))?;
        tc.check_program(&ast)
            .map_err(|errors| std::io::Error::other(format!("typecheck failed: {errors:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|error| std::io::Error::other(format!("lowering failed: {error:?}")))?;
        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|error| std::io::Error::other(format!("emit failed: {error:?}")))?;
        let compact = code.split_whitespace().collect::<String>();

        assert!(
            compact.contains("PairFactory::new(7,\"marker\".to_string())")
                || compact.contains("PairFactory::new(7,\"marker\".into())"),
            "expected the owner-specialized String parameter to materialize at the Rust boundary; got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_emits_named_field_struct_literal_for_imported_rust_type_constructor()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use incan_core::interop::{
            RustFieldInfo, RustItemKind, RustItemMetadata, RustTypeInfo, RustTypeShape, RustVisibility,
        };

        let source = r#"
from rust::demo import Pair

pub def make_pair() -> Pair:
  return Pair(1, 2)
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let tmp = seeded_rust_inspect_workspace()?;
        let manifest_dir = tmp.path().to_path_buf();
        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(manifest_dir.clone());
        tc.rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::Pair".to_string(),
                    definition_path: Some("demo::Pair".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Type(RustTypeInfo {
                        type_params: Vec::new(),
                        type_param_defaults: Vec::new(),
                        mutable_reference_type_params: Vec::new(),
                        expanded_derive_traits: Vec::new(),
                        has_const_params: false,
                        alias_target: None,
                        metadata_completeness: Default::default(),
                        methods: Vec::new(),
                        implemented_traits: Vec::new(),
                        fields: vec![
                            RustFieldInfo {
                                name: "zeta".to_string(),
                                type_display: "i64".to_string(),
                                type_shape: RustTypeShape::Int,
                            },
                            RustFieldInfo {
                                name: "alpha".to_string(),
                                type_display: "i64".to_string(),
                                type_shape: RustTypeShape::Int,
                            },
                        ],
                        variants: Vec::new(),
                    }),
                },
            )
            .map_err(|e| std::io::Error::other(format!("seed rust-inspect type: {e}")))?;
        tc.check_program(&ast)
            .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|err| std::io::Error::other(format!("lowering failed: {err:?}")))?;
        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|err| std::io::Error::other(format!("emit failed: {err:?}")))?;

        assert!(
            code.contains("Pair {") && code.contains("zeta: 1") && code.contains("alpha: 2"),
            "expected named-field Rust struct literal in generated code; got:\n{code}"
        );
        assert!(
            !code.contains("Pair(1, 2)"),
            "imported named-field Rust structs must not emit tuple-style constructors; got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_emits_tuple_struct_constructor_for_imported_rust_type() -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use incan_core::interop::{
            RustFieldInfo, RustFunctionSig, RustItemKind, RustItemMetadata, RustMethodSig, RustParam, RustTypeInfo,
            RustTypeShape, RustVisibility,
        };

        let source = r#"
from rust::demo import ClearColor, Color

pub def clear() -> ClearColor:
  return ClearColor(Color.srgb(0.15, 0.55, 0.95))
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let tmp = seeded_rust_inspect_workspace()?;
        let manifest_dir = tmp.path().to_path_buf();
        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(manifest_dir.clone());
        tc.rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::ClearColor".to_string(),
                    definition_path: Some("demo::ClearColor".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Type(RustTypeInfo {
                        type_params: Vec::new(),
                        type_param_defaults: Vec::new(),
                        mutable_reference_type_params: Vec::new(),
                        expanded_derive_traits: Vec::new(),
                        has_const_params: false,
                        alias_target: None,
                        metadata_completeness: Default::default(),
                        methods: Vec::new(),
                        implemented_traits: Vec::new(),
                        fields: vec![RustFieldInfo {
                            name: String::new(),
                            type_display: "demo::Color".to_string(),
                            type_shape: RustTypeShape::RustPath {
                                path: "demo::Color".to_string(),
                                args: Vec::new(),
                            },
                        }],
                        variants: Vec::new(),
                    }),
                },
            )
            .map_err(|error| std::io::Error::other(format!("seed ClearColor metadata: {error}")))?;
        tc.rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::Color".to_string(),
                    definition_path: Some("demo::Color".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Type(RustTypeInfo {
                        type_params: Vec::new(),
                        type_param_defaults: Vec::new(),
                        mutable_reference_type_params: Vec::new(),
                        expanded_derive_traits: Vec::new(),
                        has_const_params: false,
                        alias_target: None,
                        metadata_completeness: Default::default(),
                        methods: vec![RustMethodSig {
                            name: "srgb".to_string(),
                            signature: RustFunctionSig {
                                type_params: Vec::new(),
                                params: vec![
                                    RustParam {
                                        name: Some("red".to_string()),
                                        type_display: "f32".to_string(),
                                    },
                                    RustParam {
                                        name: Some("green".to_string()),
                                        type_display: "f32".to_string(),
                                    },
                                    RustParam {
                                        name: Some("blue".to_string()),
                                        type_display: "f32".to_string(),
                                    },
                                ],
                                return_type: "demo::Color".to_string(),
                                is_async: false,
                                is_unsafe: false,
                            },
                        }],
                        implemented_traits: Vec::new(),
                        fields: Vec::new(),
                        variants: Vec::new(),
                    }),
                },
            )
            .map_err(|error| std::io::Error::other(format!("seed Color metadata: {error}")))?;
        tc.check_program(&ast)
            .map_err(|errors| std::io::Error::other(format!("typecheck failed: {errors:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|error| std::io::Error::other(format!("lowering failed: {error:?}")))?;
        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|error| std::io::Error::other(format!("emit failed: {error:?}")))?;

        assert!(
            code.contains("ClearColor(Color::srgb(0.15, 0.55, 0.95))"),
            "expected tuple-struct Rust constructor, got:\n{code}"
        );
        assert!(
            !code.contains("return ClearColor {"),
            "tuple structs must not emit named-field syntax, got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_preserves_owned_mutable_direct_rust_parameter() -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use incan_core::interop::{
            RustFunctionSig, RustItemKind, RustItemMetadata, RustMethodSig, RustParam, RustTypeInfo, RustVisibility,
        };

        let source = r#"
from rust::demo import Commands

pub class System:
  def setup(self, mut commands: Commands) -> None:
    commands.spawn_empty()

pub def setup(mut commands: Commands) -> None:
  commands.spawn_empty()

pub def retain(mut commands: List[Commands]) -> None:
  pass
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let tmp = seeded_rust_inspect_workspace()?;
        let manifest_dir = tmp.path().to_path_buf();
        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(manifest_dir.clone());
        tc.rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::Commands".to_string(),
                    definition_path: Some("demo::Commands".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Type(RustTypeInfo {
                        type_params: Vec::new(),
                        type_param_defaults: Vec::new(),
                        mutable_reference_type_params: Vec::new(),
                        expanded_derive_traits: Vec::new(),
                        has_const_params: false,
                        alias_target: None,
                        metadata_completeness: Default::default(),
                        methods: vec![RustMethodSig {
                            name: "spawn_empty".to_string(),
                            signature: RustFunctionSig {
                                type_params: Vec::new(),
                                params: vec![RustParam {
                                    name: Some("self".to_string()),
                                    type_display: "&mut demo::Commands".to_string(),
                                }],
                                return_type: "()".to_string(),
                                is_async: false,
                                is_unsafe: false,
                            },
                        }],
                        implemented_traits: Vec::new(),
                        fields: Vec::new(),
                        variants: Vec::new(),
                    }),
                },
            )
            .map_err(|error| std::io::Error::other(format!("seed Commands metadata: {error}")))?;
        tc.check_program(&ast)
            .map_err(|errors| std::io::Error::other(format!("typecheck failed: {errors:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|error| std::io::Error::other(format!("lowering failed: {error:?}")))?;
        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|error| std::io::Error::other(format!("emit failed: {error:?}")))?;

        assert!(
            code.matches("mut commands: Commands").count() == 2 && code.contains("commands.spawn_empty();"),
            "expected free-function and method owned mutable Rust handles, got:\n{code}"
        );
        assert!(
            !code.contains("commands: &mut Commands"),
            "direct Rust system parameters must not be rewritten as borrowed Incan aggregates, got:\n{code}"
        );
        assert!(
            code.contains("&mut Vec<Commands>") && !code.contains("mut commands: Vec<Commands>"),
            "mutable Incan containers containing Rust values must retain their borrowed ABI, got:\n{code}"
        );
        Ok(())
    }

    #[test]
    fn test_codegen_preserves_explicit_mutable_rust_generic_arguments_in_mutating_for_loop()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from rust::demo import FooBar, Gadget, Widget

pub def move_items(mut items: FooBar[tuple[&mut Widget, &mut Gadget]]) -> None:
  for widget, gadget in items.iter_mut():
    widget.position = 1.0
    gadget.speed = 1.0
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));
        let mut checker = crate::frontend::typechecker::TypeChecker::new();
        checker
            .check_program(&ast)
            .map_err(|errors| std::io::Error::other(format!("typecheck failed: {errors:?}")))?;
        let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|error| std::io::Error::other(format!("lowering failed: {error:?}")))?;
        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|error| std::io::Error::other(format!("emit failed: {error:?}")))?;

        assert!(
            code.contains("FooBar<(&mut Widget, &mut Gadget)>"),
            "explicit mutable Rust type arguments must be retained, got:\n{code}"
        );
        assert!(
            code.contains("mut items: FooBar<(&mut Widget, &mut Gadget)>"),
            "a mutable direct Rust generic must keep its owned outer ABI, got:\n{code}"
        );
        assert!(
            code.contains("for (mut widget, mut gadget) in items.iter_mut()"),
            "source mutation must mark destructured Rust iterator bindings mutable, got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_projects_metadata_directed_mutable_rust_generic_arguments_without_nominal_matching()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::backend::ir::Mutability;
        use crate::frontend::typechecker::TypeChecker;
        use incan_core::interop::{
            RustItemKind, RustItemMetadata, RustMutableReferenceCandidate, RustMutableReferenceTypeParam, RustTypeInfo,
            RustVisibility,
        };

        let source = r#"
from rust::demo import FooBar as ProviderHandle, Gadget, Widget

pub def move_items(mut items: ProviderHandle[tuple[Widget, Gadget]]) -> None:
  for widget, gadget in items.iter_mut():
    widget.position = 1.0
    gadget.speed = 1.0
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));
        let tmp = seeded_rust_inspect_workspace()?;
        let manifest_dir = tmp.path().to_path_buf();
        let mut checker = TypeChecker::new();
        checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
        checker
            .rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::FooBar".to_string(),
                    definition_path: Some("demo::FooBar".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Type(RustTypeInfo {
                        type_params: vec!["T".to_string()],
                        type_param_defaults: Vec::new(),
                        mutable_reference_type_params: vec![RustMutableReferenceTypeParam {
                            type_param: "T".to_string(),
                            direct_trait_bounds: vec!["demo::MutableData".to_string()],
                            mutable_reference_candidates: vec![RustMutableReferenceCandidate {
                                required_traits: Vec::new(),
                                required_associated_type_bindings: Vec::new(),
                                fallback_is_complete: true,
                            }],
                            tuple_composition_arities: vec![2],
                        }],
                        expanded_derive_traits: Vec::new(),
                        has_const_params: false,
                        alias_target: None,
                        metadata_completeness: Default::default(),
                        methods: Vec::new(),
                        implemented_traits: Vec::new(),
                        fields: Vec::new(),
                        variants: Vec::new(),
                    }),
                },
            )
            .map_err(|error| std::io::Error::other(format!("seed Rust generic metadata: {error}")))?;
        checker
            .check_program(&ast)
            .map_err(|errors| std::io::Error::other(format!("typecheck failed: {errors:?}")))?;
        let annotation = "ProviderHandle[tuple[Widget, Gadget]]";
        let start = source
            .find(annotation)
            .ok_or("projection annotation missing from source")?;
        assert_eq!(
            checker
                .type_info()
                .rust
                .mutable_reference_type_argument_projections
                .get(&(start, start + annotation.len())),
            Some(&vec![crate::frontend::typechecker::MutableRustTypeArgumentProjection {
                argument_position: 0,
                reference_leaf_paths: vec![vec![0], vec![1]],
            }]),
            "the frontend must preserve the structural foreign-contract decision for lowering"
        );
        let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|error| std::io::Error::other(format!("lowering failed: {error:?}")))?;
        let signature = ir_program
            .function_registry
            .get("move_items")
            .ok_or("missing projected function signature")?;
        assert_eq!(signature.params[0].mutability, Mutability::OwnedMutable);
        assert_eq!(
            signature.params[0].ty.rust_name(),
            "ProviderHandle<(&mut Widget, &mut Gadget)>",
            "the callable registry must carry the same projected ABI that final lowering emits"
        );
        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|error| std::io::Error::other(format!("emit failed: {error:?}")))?;

        assert!(
            code.contains("mut items: ProviderHandle<(&mut Widget, &mut Gadget)>"),
            "frontend-owned metadata must project an arbitrary imported alias structurally, then own the outer handle and borrow the payload, got:\n{code}"
        );
        assert!(
            code.contains("for (mut widget, mut gadget) in items.iter_mut()"),
            "mutated destructured Rust iterator bindings must remain mutable, got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_does_not_project_tuple_without_inspected_composition_contract()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use incan_core::interop::{
            RustItemKind, RustItemMetadata, RustMutableReferenceCandidate, RustMutableReferenceTypeParam, RustTypeInfo,
            RustVisibility,
        };

        let source = r#"
from rust::demo import FooBar as ProviderHandle, Gadget, Widget

pub def inspect(mut items: ProviderHandle[tuple[Widget, Gadget]]) -> None:
  pass
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));
        let tmp = seeded_rust_inspect_workspace()?;
        let manifest_dir = tmp.path().to_path_buf();
        let mut checker = TypeChecker::new();
        checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
        checker
            .rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::FooBar".to_string(),
                    definition_path: Some("demo::FooBar".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Type(RustTypeInfo {
                        type_params: vec!["T".to_string()],
                        type_param_defaults: Vec::new(),
                        mutable_reference_type_params: vec![RustMutableReferenceTypeParam {
                            type_param: "T".to_string(),
                            direct_trait_bounds: vec!["demo::MutableData".to_string()],
                            mutable_reference_candidates: vec![RustMutableReferenceCandidate {
                                required_traits: Vec::new(),
                                required_associated_type_bindings: Vec::new(),
                                fallback_is_complete: true,
                            }],
                            tuple_composition_arities: Vec::new(),
                        }],
                        expanded_derive_traits: Vec::new(),
                        has_const_params: false,
                        alias_target: None,
                        metadata_completeness: Default::default(),
                        methods: Vec::new(),
                        implemented_traits: Vec::new(),
                        fields: Vec::new(),
                        variants: Vec::new(),
                    }),
                },
            )
            .map_err(|error| std::io::Error::other(format!("seed Rust generic metadata: {error}")))?;
        checker
            .check_program(&ast)
            .map_err(|errors| std::io::Error::other(format!("typecheck failed: {errors:?}")))?;

        assert!(
            checker
                .type_info()
                .rust
                .mutable_reference_type_argument_projections
                .is_empty(),
            "tuple leaves must remain owned when inspection has not proved element-wise composition"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_preserves_direct_foreign_argument_when_only_a_sibling_needs_mutable_reference()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use incan_core::interop::{
            RustImplementedTrait, RustItemKind, RustItemMetadata, RustMutableReferenceCandidate,
            RustMutableReferenceTypeParam, RustTypeInfo, RustVisibility,
        };

        let source = r#"
from rust::demo import Entity, FooBar as ProviderHandle, Widget

pub def move_items(mut items: ProviderHandle[tuple[Entity, Widget]]) -> None:
  pass
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));
        let tmp = seeded_rust_inspect_workspace()?;
        let manifest_dir = tmp.path().to_path_buf();
        let mut checker = TypeChecker::new();
        checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
        let projection_rule = RustMutableReferenceTypeParam {
            type_param: "T".to_string(),
            direct_trait_bounds: vec!["demo::MutableData".to_string()],
            mutable_reference_candidates: vec![RustMutableReferenceCandidate {
                required_traits: Vec::new(),
                required_associated_type_bindings: Vec::new(),
                fallback_is_complete: true,
            }],
            tuple_composition_arities: vec![2],
        };
        checker
            .rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::FooBar".to_string(),
                    definition_path: Some("demo::FooBar".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Type(RustTypeInfo {
                        type_params: vec!["T".to_string()],
                        type_param_defaults: Vec::new(),
                        mutable_reference_type_params: vec![projection_rule],
                        expanded_derive_traits: Vec::new(),
                        has_const_params: false,
                        alias_target: None,
                        metadata_completeness: Default::default(),
                        methods: Vec::new(),
                        implemented_traits: Vec::new(),
                        fields: Vec::new(),
                        variants: Vec::new(),
                    }),
                },
            )
            .map_err(|error| std::io::Error::other(format!("seed generic metadata: {error}")))?;
        checker
            .rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::Entity".to_string(),
                    definition_path: Some("demo::Entity".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Type(RustTypeInfo {
                        type_params: Vec::new(),
                        type_param_defaults: Vec::new(),
                        mutable_reference_type_params: Vec::new(),
                        expanded_derive_traits: Vec::new(),
                        has_const_params: false,
                        alias_target: None,
                        metadata_completeness: Default::default(),
                        methods: Vec::new(),
                        implemented_traits: vec![RustImplementedTrait {
                            path: "demo::MutableData".to_string(),
                            mutable_reference: false,
                        }],
                        fields: Vec::new(),
                        variants: Vec::new(),
                    }),
                },
            )
            .map_err(|error| std::io::Error::other(format!("seed direct trait metadata: {error}")))?;
        checker
            .check_program(&ast)
            .map_err(|errors| std::io::Error::other(format!("typecheck failed: {errors:?}")))?;

        let annotation = "ProviderHandle[tuple[Entity, Widget]]";
        let start = source.find(annotation).ok_or("projection annotation missing")?;
        assert_eq!(
            checker
                .type_info()
                .rust
                .mutable_reference_type_argument_projections
                .get(&(start, start + annotation.len())),
            Some(&vec![crate::frontend::typechecker::MutableRustTypeArgumentProjection {
                argument_position: 0,
                reference_leaf_paths: vec![vec![1]],
            }]),
            "a direct bound implementation must remain owned while only the unsatisfied sibling selects the reference alternative"
        );
        let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|error| std::io::Error::other(format!("lowering failed: {error:?}")))?;
        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|error| std::io::Error::other(format!("emit failed: {error:?}")))?;
        assert!(
            code.contains("ProviderHandle<(Entity, &mut Widget)>"),
            "the direct argument must not be borrowed, got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_fails_closed_after_solver_rejects_associated_type_candidate()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;

        let tmp = tempfile::tempdir()?;
        fs::create_dir_all(tmp.path().join("src"))?;
        let provider = tmp.path().join("solver_provider");
        fs::create_dir_all(provider.join("src"))?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "ra_solver_codegen_consumer"
version = "0.1.0"
edition = "2021"

[dependencies]
solver_provider = { path = "solver_provider" }
"#,
        )?;
        fs::write(tmp.path().join("src/lib.rs"), "pub fn consumer() {}\n")?;
        fs::write(
            provider.join("Cargo.toml"),
            r#"[package]
name = "solver_provider"
version = "0.1.0"
edition = "2021"
"#,
        )?;
        fs::write(
            provider.join("src/lib.rs"),
            r#"use core::marker::PhantomData;

pub trait QueryData {}
pub trait Component { type Mutability; }
pub struct Mutable;
pub struct Immutable;
pub struct Dynamic;
pub struct Static;
pub struct FooBar<T: QueryData>(PhantomData<T>);

impl Component for Dynamic { type Mutability = Mutable; }
impl Component for Static { type Mutability = Immutable; }
impl<T: Component<Mutability = Mutable>> QueryData for &mut T {}
"#,
        )?;

        let source = r#"
from rust::solver_provider import Dynamic, FooBar, Static

pub def update(mut values: FooBar[Dynamic]) -> None:
  pass

pub def inspect(mut values: FooBar[Static]) -> None:
  pass
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));
        let manifest_dir = tmp.path().to_path_buf();
        let mut checker = TypeChecker::new();
        checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
        for path in [
            "solver_provider::FooBar",
            "solver_provider::Dynamic",
            "solver_provider::Static",
        ] {
            checker
                .rust_inspect_cache
                .get_or_extract_complete(&manifest_dir, path, &|_| ())
                .map_err(|error| std::io::Error::other(format!("extract {path}: {error}")))?;
        }
        checker
            .check_program(&ast)
            .map_err(|errors| std::io::Error::other(format!("typecheck failed: {errors:?}")))?;

        let dynamic = "FooBar[Dynamic]";
        let dynamic_start = source.find(dynamic).ok_or("dynamic annotation missing")?;
        assert_eq!(
            checker
                .type_info()
                .rust
                .mutable_reference_type_argument_projections
                .get(&(dynamic_start, dynamic_start + dynamic.len())),
            Some(&vec![crate::frontend::typechecker::MutableRustTypeArgumentProjection {
                argument_position: 0,
                reference_leaf_paths: vec![vec![]],
            }]),
            "the complete solver must accept the matching associated-type candidate"
        );
        let static_annotation = "FooBar[Static]";
        let static_start = source.find(static_annotation).ok_or("static annotation missing")?;
        assert!(
            !checker
                .type_info()
                .rust
                .mutable_reference_type_argument_projections
                .contains_key(&(static_start, static_start + static_annotation.len())),
            "an authoritative solver rejection must not be weakened to the bare Component trait"
        );

        let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|error| std::io::Error::other(format!("lowering failed: {error:?}")))?;
        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|error| std::io::Error::other(format!("emit failed: {error:?}")))?;
        assert!(
            code.contains("FooBar<&mut Dynamic>"),
            "expected mutable candidate, got:\n{code}"
        );
        assert!(
            code.contains("FooBar<Static>") && !code.contains("FooBar<&mut Static>"),
            "solver-negative associated-type candidate must remain unprojected, got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_projects_local_type_from_actual_rust_derive_output() -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use incan_core::interop::{
            RustAssociatedTypeBinding, RustAssociatedTypeRequirement, RustExpandedDeriveTrait, RustItemKind,
            RustItemMetadata, RustMutableReferenceCandidate, RustMutableReferenceTypeParam, RustTraitInfo,
            RustTypeInfo, RustVisibility,
        };

        let source = r#"
from rust::demo import FooBar as ProviderHandle, Component
from rust::demo_derive import Component as ComponentMacro

@derive(Component)
model Velocity:
  x: f32

@rust.derive(ComponentMacro)
model ExplicitVelocity:
  x: f32

@rust.derive("demo_derive::Component")
model StringPathVelocity:
  x: f32

pub def inspect(mut values: ProviderHandle[Velocity]) -> None:
  pass

pub def inspect_explicit(mut values: ProviderHandle[ExplicitVelocity]) -> None:
  pass

pub def inspect_string_path(mut values: ProviderHandle[StringPathVelocity]) -> None:
  pass
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));
        let tmp = seeded_rust_inspect_workspace()?;
        let manifest_dir = tmp.path().to_path_buf();
        let mut checker = TypeChecker::new();
        checker.set_declared_crate_names(std::collections::HashSet::from([
            "demo".to_string(),
            "demo_derive".to_string(),
        ]));
        checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
        checker
            .rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::FooBar".to_string(),
                    definition_path: Some("demo::FooBar".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Type(RustTypeInfo {
                        type_params: vec!["T".to_string()],
                        type_param_defaults: Vec::new(),
                        mutable_reference_type_params: vec![RustMutableReferenceTypeParam {
                            type_param: "T".to_string(),
                            direct_trait_bounds: vec!["provider::QueryData".to_string()],
                            mutable_reference_candidates: vec![RustMutableReferenceCandidate {
                                required_traits: vec!["provider::Component".to_string()],
                                required_associated_type_bindings: vec![RustAssociatedTypeRequirement {
                                    trait_path: "provider::Component".to_string(),
                                    name: "Mutability".to_string(),
                                    value_path: "provider::Mutable".to_string(),
                                }],
                                fallback_is_complete: true,
                            }],
                            tuple_composition_arities: Vec::new(),
                        }],
                        expanded_derive_traits: Vec::new(),
                        has_const_params: false,
                        alias_target: None,
                        metadata_completeness: Default::default(),
                        methods: Vec::new(),
                        implemented_traits: Vec::new(),
                        fields: Vec::new(),
                        variants: Vec::new(),
                    }),
                },
            )
            .map_err(|error| std::io::Error::other(format!("seed owner metadata: {error}")))?;
        checker
            .rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::Component".to_string(),
                    definition_path: Some("provider::Component".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Trait(RustTraitInfo {
                        items: Vec::new(),
                        derive_macro: Some(incan_core::interop::RustMacroInfo {
                            expanded_traits: vec![RustExpandedDeriveTrait {
                                path: "provider::Component".to_string(),
                                associated_type_bindings: vec![RustAssociatedTypeBinding {
                                    name: "Mutability".to_string(),
                                    value_path: "provider::Mutable".to_string(),
                                }],
                            }],
                        }),
                    }),
                },
            )
            .map_err(|error| std::io::Error::other(format!("seed derive metadata: {error}")))?;
        checker
            .rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo_derive::Component".to_string(),
                    definition_path: Some("demo_derive::Component".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Macro(incan_core::interop::RustMacroInfo {
                        expanded_traits: vec![RustExpandedDeriveTrait {
                            path: "provider::Component".to_string(),
                            associated_type_bindings: vec![RustAssociatedTypeBinding {
                                name: "Mutability".to_string(),
                                value_path: "provider::Mutable".to_string(),
                            }],
                        }],
                    }),
                },
            )
            .map_err(|error| std::io::Error::other(format!("seed derive macro metadata: {error}")))?;
        checker
            .check_program(&ast)
            .map_err(|errors| std::io::Error::other(format!("typecheck failed: {errors:?}")))?;

        let annotation = "ProviderHandle[Velocity]";
        let start = source.find(annotation).ok_or("provider annotation missing")?;
        assert_eq!(
            checker
                .type_info()
                .rust
                .mutable_reference_type_argument_projections
                .get(&(start, start + annotation.len())),
            Some(&vec![crate::frontend::typechecker::MutableRustTypeArgumentProjection {
                argument_position: 0,
                reference_leaf_paths: vec![vec![]],
            }]),
            "a local type may project mutably only from the exact inspected derive expansion"
        );
        let explicit_annotation = "ProviderHandle[ExplicitVelocity]";
        let explicit_start = source
            .find(explicit_annotation)
            .ok_or("explicit provider annotation missing")?;
        assert_eq!(
            checker
                .type_info()
                .rust
                .mutable_reference_type_argument_projections
                .get(&(explicit_start, explicit_start + explicit_annotation.len())),
            Some(&vec![crate::frontend::typechecker::MutableRustTypeArgumentProjection {
                argument_position: 0,
                reference_leaf_paths: vec![vec![]],
            }]),
            "the documented explicit Rust derive route must retain the same exact expansion evidence"
        );
        let string_path_annotation = "ProviderHandle[StringPathVelocity]";
        let string_path_start = source
            .find(string_path_annotation)
            .ok_or("string-path provider annotation missing")?;
        assert_eq!(
            checker
                .type_info()
                .rust
                .mutable_reference_type_argument_projections
                .get(&(string_path_start, string_path_start + string_path_annotation.len(),)),
            Some(&vec![crate::frontend::typechecker::MutableRustTypeArgumentProjection {
                argument_position: 0,
                reference_leaf_paths: vec![vec![]],
            }]),
            "an explicit declared Rust derive path must retain the same exact expansion evidence"
        );

        let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|error| std::io::Error::other(format!("lowering failed: {error:?}")))?;
        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|error| std::io::Error::other(format!("emit failed: {error:?}")))?;
        assert!(
            code.contains("ProviderHandle<&mut Velocity>"),
            "expected actual derive output to drive mutable lowering, got:\n{code}"
        );
        assert!(
            code.contains("ProviderHandle<&mut ExplicitVelocity>"),
            "expected explicit Rust derive output to drive mutable lowering, got:\n{code}"
        );
        assert!(
            code.contains("ProviderHandle<&mut StringPathVelocity>"),
            "expected explicit Rust derive path output to drive mutable lowering, got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_rejects_inexact_expanded_derive_trait_contracts() -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use incan_core::interop::{
            RustAssociatedTypeBinding, RustAssociatedTypeRequirement, RustExpandedDeriveTrait, RustItemKind,
            RustItemMetadata, RustMutableReferenceCandidate, RustMutableReferenceTypeParam, RustTypeInfo,
            RustVisibility,
        };

        let source = r#"
from rust::demo import FooBar as ProviderHandle, Gadget, Widget

pub def inspect(mut values: ProviderHandle[Widget]) -> None:
  pass

pub def inspect_other(mut values: ProviderHandle[Gadget]) -> None:
  pass
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));
        let tmp = seeded_rust_inspect_workspace()?;
        let manifest_dir = tmp.path().to_path_buf();
        let mut checker = TypeChecker::new();
        checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
        checker
            .rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::FooBar".to_string(),
                    definition_path: Some("demo::FooBar".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Type(RustTypeInfo {
                        type_params: vec!["T".to_string()],
                        type_param_defaults: Vec::new(),
                        mutable_reference_type_params: vec![RustMutableReferenceTypeParam {
                            type_param: "T".to_string(),
                            direct_trait_bounds: vec!["demo::MutableData".to_string()],
                            mutable_reference_candidates: vec![RustMutableReferenceCandidate {
                                required_traits: vec!["provider::Component".to_string()],
                                required_associated_type_bindings: vec![RustAssociatedTypeRequirement {
                                    trait_path: "provider::Component".to_string(),
                                    name: "Mutability".to_string(),
                                    value_path: "provider::Mutable".to_string(),
                                }],
                                fallback_is_complete: true,
                            }],
                            tuple_composition_arities: Vec::new(),
                        }],
                        expanded_derive_traits: Vec::new(),
                        has_const_params: false,
                        alias_target: None,
                        metadata_completeness: Default::default(),
                        methods: Vec::new(),
                        implemented_traits: Vec::new(),
                        fields: Vec::new(),
                        variants: Vec::new(),
                    }),
                },
            )
            .map_err(|error| std::io::Error::other(format!("seed owner metadata: {error}")))?;
        checker
            .rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::Widget".to_string(),
                    definition_path: Some("demo::Widget".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Type(RustTypeInfo {
                        type_params: Vec::new(),
                        type_param_defaults: Vec::new(),
                        mutable_reference_type_params: Vec::new(),
                        expanded_derive_traits: vec![RustExpandedDeriveTrait {
                            path: "other::Component".to_string(),
                            associated_type_bindings: vec![RustAssociatedTypeBinding {
                                name: "Mutability".to_string(),
                                value_path: "provider::Mutable".to_string(),
                            }],
                        }],
                        has_const_params: false,
                        alias_target: None,
                        metadata_completeness: Default::default(),
                        methods: Vec::new(),
                        implemented_traits: Vec::new(),
                        fields: Vec::new(),
                        variants: Vec::new(),
                    }),
                },
            )
            .map_err(|error| std::io::Error::other(format!("seed candidate metadata: {error}")))?;
        checker
            .rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::Gadget".to_string(),
                    definition_path: Some("demo::Gadget".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Type(RustTypeInfo {
                        type_params: Vec::new(),
                        type_param_defaults: Vec::new(),
                        mutable_reference_type_params: Vec::new(),
                        expanded_derive_traits: vec![RustExpandedDeriveTrait {
                            path: "provider::Component".to_string(),
                            associated_type_bindings: vec![RustAssociatedTypeBinding {
                                name: "Mutability".to_string(),
                                value_path: "provider::Immutable".to_string(),
                            }],
                        }],
                        has_const_params: false,
                        alias_target: None,
                        metadata_completeness: Default::default(),
                        methods: Vec::new(),
                        implemented_traits: Vec::new(),
                        fields: Vec::new(),
                        variants: Vec::new(),
                    }),
                },
            )
            .map_err(|error| std::io::Error::other(format!("seed associated candidate metadata: {error}")))?;
        checker
            .check_program(&ast)
            .map_err(|errors| std::io::Error::other(format!("typecheck failed: {errors:?}")))?;

        assert!(
            checker
                .type_info()
                .rust
                .mutable_reference_type_argument_projections
                .is_empty(),
            "neither a same-name trait from another path nor the wrong associated type may satisfy the candidate"
        );
        Ok(())
    }

    #[test]
    fn test_codegen_projects_each_metadata_directed_mutable_rust_generic_argument()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from rust::demo import FooBar as ProviderHandle, Gadget, Widget

pub def replace_items(mut items: ProviderHandle[Widget, Gadget]) -> None:
  pass
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));
        let mut lowering = lowering_with_mutable_reference_projection(
            source,
            "ProviderHandle[Widget, Gadget]",
            vec![
                crate::frontend::typechecker::MutableRustTypeArgumentProjection {
                    argument_position: 0,
                    reference_leaf_paths: vec![vec![]],
                },
                crate::frontend::typechecker::MutableRustTypeArgumentProjection {
                    argument_position: 1,
                    reference_leaf_paths: vec![vec![]],
                },
            ],
        )?;
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|error| std::io::Error::other(format!("lowering failed: {error:?}")))?;
        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|error| std::io::Error::other(format!("emit failed: {error:?}")))?;

        assert!(
            code.contains("ProviderHandle<&mut Widget, &mut Gadget>"),
            "every frontend-recorded generic argument position must be projected through the imported provider alias, got:\n{code}"
        );
        Ok(())
    }

    #[test]
    fn test_codegen_projects_metadata_directed_mutable_rust_generic_arguments_in_trait_method()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::backend::ir::Mutability;
        use crate::backend::ir::decl::IrDeclKind;

        let source = r#"
from rust::demo import FooBar as ProviderHandle, Gadget, Widget

trait ReplacesItems:
  def replace(self, mut items: ProviderHandle[tuple[Widget, Gadget]]) -> None: ...
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));
        let mut lowering = lowering_with_mutable_reference_projection(
            source,
            "ProviderHandle[tuple[Widget, Gadget]]",
            vec![crate::frontend::typechecker::MutableRustTypeArgumentProjection {
                argument_position: 0,
                reference_leaf_paths: vec![vec![0], vec![1]],
            }],
        )?;
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|error| std::io::Error::other(format!("lowering failed: {error:?}")))?;
        let trait_decl = ir_program
            .declarations
            .iter()
            .find_map(|decl| match &decl.kind {
                IrDeclKind::Trait(trait_decl) if trait_decl.name == "ReplacesItems" => Some(trait_decl),
                _ => None,
            })
            .ok_or("missing lowered trait declaration")?;
        let method = trait_decl.methods.first().ok_or("missing lowered trait method")?;

        assert_eq!(method.params[1].mutability, Mutability::OwnedMutable);
        assert_eq!(
            method.params[1].ty.rust_name(),
            "ProviderHandle<(&mut Widget, &mut Gadget)>",
            "trait methods must retain the same owned generic Rust ABI as free functions"
        );
        Ok(())
    }

    #[test]
    fn test_codegen_leaves_unconfigured_rust_generics_literal() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from rust::demo import FooBar, Gadget, Widget

pub def inspect(mut items: FooBar[tuple[Widget, Gadget]]) -> None:
  pass
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));
        let mut checker = crate::frontend::typechecker::TypeChecker::new();
        checker
            .check_program(&ast)
            .map_err(|errors| std::io::Error::other(format!("typecheck failed: {errors:?}")))?;
        let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|error| std::io::Error::other(format!("lowering failed: {error:?}")))?;
        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|error| std::io::Error::other(format!("emit failed: {error:?}")))?;

        assert!(
            code.contains("FooBar<(Widget, Gadget)>") && !code.contains("FooBar<(&mut Widget, &mut Gadget)>"),
            "unconfigured foreign generics must preserve their literal source type, got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_preserves_f32_arithmetic_at_rust_boundary_issue1219() -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use incan_core::interop::{RustFunctionSig, RustItemKind, RustItemMetadata, RustParam, RustVisibility};

        let source = r#"
from rust::demo import accept_f32

pub def translate(time: f32, velocity: f32) -> f32:
  return accept_f32(-time + time * velocity)
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let tmp = seeded_rust_inspect_workspace()?;
        let manifest_dir = tmp.path().to_path_buf();
        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(manifest_dir.clone());
        tc.rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::accept_f32".to_string(),
                    definition_path: Some("demo::accept_f32".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Function(RustFunctionSig {
                        type_params: Vec::new(),
                        params: vec![RustParam {
                            name: Some("value".to_string()),
                            type_display: "f32".to_string(),
                        }],
                        return_type: "f32".to_string(),
                        is_async: false,
                        is_unsafe: false,
                    }),
                },
            )
            .map_err(|error| std::io::Error::other(format!("seed accept_f32 metadata: {error}")))?;
        tc.check_program(&ast)
            .map_err(|errors| std::io::Error::other(format!("typecheck failed: {errors:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|error| std::io::Error::other(format!("lowering failed: {error:?}")))?;
        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|error| std::io::Error::other(format!("emit failed: {error:?}")))?;

        assert!(
            compact_rust(&code).contains(&format!(
                "pubfn{}(time:f32,velocity:f32,)->f32",
                projected_name(&code, "translate", SemanticSourceTargetKind::Function)
            )) && code.contains("accept_f32(")
                && code.matches("incan_stdlib::num::require_finite_f32").count() == 6,
            "expected exact f32 arithmetic to retain its width and finite invariant across the imported Rust f32 \
             boundary, got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_emits_raw_rust_field_names_for_keyword_fields_issue725() -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use incan_core::interop::{
            RustFieldInfo, RustItemKind, RustItemMetadata, RustTypeInfo, RustTypeShape, RustVisibility,
        };

        let source = r#"
from rust::demo import JoinRel

pub def get_type(join: JoinRel) -> int:
  return join.type + join.match + join.type_

pub def rebuild(join: JoinRel) -> JoinRel:
  return JoinRel(type=join.type, match=join.match, type_=join.type_)
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let tmp = seeded_rust_inspect_workspace()?;
        let manifest_dir = tmp.path().to_path_buf();
        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(manifest_dir.clone());
        tc.rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::JoinRel".to_string(),
                    definition_path: Some("demo::JoinRel".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Type(RustTypeInfo {
                        type_params: Vec::new(),
                        type_param_defaults: Vec::new(),
                        mutable_reference_type_params: Vec::new(),
                        expanded_derive_traits: Vec::new(),
                        has_const_params: false,
                        alias_target: None,
                        metadata_completeness: Default::default(),
                        methods: Vec::new(),
                        implemented_traits: Vec::new(),
                        fields: vec![
                            RustFieldInfo {
                                name: "type".to_string(),
                                type_display: "i64".to_string(),
                                type_shape: RustTypeShape::Int,
                            },
                            RustFieldInfo {
                                name: "match".to_string(),
                                type_display: "i64".to_string(),
                                type_shape: RustTypeShape::Int,
                            },
                            RustFieldInfo {
                                name: "type_".to_string(),
                                type_display: "i64".to_string(),
                                type_shape: RustTypeShape::Int,
                            },
                        ],
                        variants: Vec::new(),
                    }),
                },
            )
            .map_err(|e| std::io::Error::other(format!("seed rust-inspect type: {e}")))?;
        tc.check_program(&ast)
            .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|err| std::io::Error::other(format!("lowering failed: {err:?}")))?;
        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|err| std::io::Error::other(format!("emit failed: {err:?}")))?;

        assert!(
            code.contains("join.r#type")
                && code.contains("join.r#match")
                && code.contains("join.type_")
                && code.contains("r#type: join.r#type")
                && code.contains("r#match: join.r#match")
                && code.contains("type_: join.type_"),
            "expected keyword fields to emit raw Rust identifiers while ordinary trailing-underscore fields stay unchanged; got:\n{code}"
        );
        assert!(
            !code.contains("r#type: join.type_") && !code.contains("type_: join.r#type"),
            "Rust keyword fields and ordinary trailing-underscore fields must not be cross-wired; got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_uses_source_field_names_for_metadata_free_rust_type_constructor()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;

        let source = r#"
from rust::demo import Pair

pub def make_pair() -> Pair:
  return Pair(zeta=1, alpha=2)
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let mut tc = TypeChecker::new();
        tc.check_program(&ast)
            .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|err| std::io::Error::other(format!("lowering failed: {err:?}")))?;
        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|err| std::io::Error::other(format!("emit failed: {err:?}")))?;

        assert!(
            code.contains("Pair {") && code.contains("zeta: 1") && code.contains("alpha: 2"),
            "expected source-named Rust struct literal in generated code; got:\n{code}"
        );
        assert!(
            !code.contains("Pair(zeta = 1, alpha = 2)") && !code.contains("Pair(1, 2)"),
            "metadata-free named Rust constructors must not emit call syntax; got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_borrows_rust_backed_method_args_from_metadata() -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use incan_core::interop::{
            RustFunctionSig, RustItemKind, RustItemMetadata, RustMethodSig, RustParam, RustTypeInfo, RustVisibility,
        };

        let source = r#"
from rust::demo import Builder

model Payload:
  name: str

pub def forward(payload: Payload) -> int:
  builder = Builder.new()
  return builder.json(payload)
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let tmp = seeded_rust_inspect_workspace()?;
        let manifest_dir = tmp.path().to_path_buf();
        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(manifest_dir.clone());
        tc.rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::Builder".to_string(),
                    definition_path: Some("demo::Builder".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Type(RustTypeInfo {
                        type_params: Vec::new(),
                        type_param_defaults: Vec::new(),
                        mutable_reference_type_params: Vec::new(),
                        expanded_derive_traits: Vec::new(),
                        has_const_params: false,
                        alias_target: None,
                        metadata_completeness: Default::default(),
                        methods: vec![
                            RustMethodSig {
                                name: "new".to_string(),
                                signature: RustFunctionSig {
                                    type_params: Vec::new(),
                                    params: Vec::new(),
                                    return_type: "demo::Builder".to_string(),
                                    is_async: false,
                                    is_unsafe: false,
                                },
                            },
                            RustMethodSig {
                                name: "json".to_string(),
                                signature: RustFunctionSig {
                                    type_params: Vec::new(),
                                    params: vec![RustParam {
                                        name: Some("value".to_string()),
                                        type_display: "&T".to_string(),
                                    }],
                                    return_type: "i64".to_string(),
                                    is_async: false,
                                    is_unsafe: false,
                                },
                            },
                        ],
                        implemented_traits: Vec::new(),
                        fields: Vec::new(),
                        variants: Vec::new(),
                    }),
                },
            )
            .map_err(|e| std::io::Error::other(format!("seed rust-inspect type: {e}")))?;
        tc.check_program(&ast)
            .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|err| std::io::Error::other(format!("lowering failed: {err:?}")))?;

        let mut codegen = IrCodegen::new();
        codegen.collect_external_rust_functions(&ast);

        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        emitter.set_external_rust_functions(codegen.external_rust_functions.clone());
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|err| std::io::Error::other(format!("emit failed: {err:?}")))?;

        assert!(
            code.contains("builder.json(&payload);"),
            "expected borrowed rust method arg in generated code; got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_borrows_reqwest_json_payload_returned_from_registry_client()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;

        let source = r#"
from rust::reqwest import Client

model Payload:
  name: str

pub def forward(payload: Payload) -> None:
  builder = Client.new().post("https://example.invalid")
  _ = builder.json(payload)
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let tmp = reqwest_shaped_rust_inspect_workspace()?;
        let manifest_dir = tmp.path().to_path_buf();
        prewarm_metadata(&manifest_dir, &["reqwest::Client"])?;

        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(manifest_dir);
        tc.check_program(&ast)
            .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|err| std::io::Error::other(format!("lowering failed: {err:?}")))?;

        let mut codegen = IrCodegen::new();
        codegen.collect_external_rust_functions(&ast);

        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        emitter.set_external_rust_functions(codegen.external_rust_functions.clone());
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|err| std::io::Error::other(format!("emit failed: {err:?}")))?;

        assert!(
            code.contains("builder.json(&payload);"),
            "expected registry-returned reqwest RequestBuilder::json payload to be borrowed; got:\n{code}"
        );
        assert!(
            code.contains(r#"Client::new().post("https://example.invalid")"#),
            "expected generic reqwest Client::post string literal to keep inferable &str shape; got:\n{code}"
        );
        assert!(
            !code.contains(r#".post("https://example.invalid".into())"#),
            "generic reqwest Client::post must not force ambiguous `.into()` on string literals; got:\n{code}"
        );
        Ok(())
    }

    #[test]
    fn test_codegen_keeps_nested_rust_associated_calls_type_like_when_outer_receiver_is_unknown()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;

        let source = r#"
from rust::datafusion::execution::context import SessionContext
from rust::datafusion::dataframe import DataFrameWriteOptions

pub def f(uri: str) -> None:
  ctx = SessionContext.new()
  _ = ctx.write_csv(uri, DataFrameWriteOptions.new(), None)
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let mut tc = TypeChecker::new();
        tc.check_program(&ast)
            .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|err| std::io::Error::other(format!("lowering failed: {err:?}")))?;

        let mut codegen = IrCodegen::new();
        codegen.collect_external_rust_functions(&ast);

        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        emitter.set_external_rust_functions(codegen.external_rust_functions.clone());
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|err| std::io::Error::other(format!("emit failed: {err:?}")))?;

        assert!(
            code.contains("ctx.write_csv(&uri, DataFrameWriteOptions::new(), None::<_>);"),
            "expected nested rust associated call to keep :: syntax; got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_borrows_async_rust_backed_free_function_args_from_metadata()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use incan_core::interop::{RustFunctionSig, RustItemKind, RustItemMetadata, RustParam, RustVisibility};

        let source = r#"
from std.async import sleep
from rust::demo import State
from rust::demo import Plan
from rust::demo import consume

pub async def run(state: State, plan: Plan) -> None:
  await sleep(0.01)
  await consume(state, plan)
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let tmp = seeded_rust_inspect_workspace()?;
        let manifest_dir = tmp.path().to_path_buf();
        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(manifest_dir.clone());
        tc.rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::consume".to_string(),
                    definition_path: Some("demo::consume".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Function(RustFunctionSig {
                        type_params: Vec::new(),
                        params: vec![
                            RustParam {
                                name: Some("state".to_string()),
                                type_display: "&demo::State".to_string(),
                            },
                            RustParam {
                                name: Some("plan".to_string()),
                                type_display: "&demo::Plan".to_string(),
                            },
                        ],
                        return_type: "()".to_string(),
                        is_async: true,
                        is_unsafe: false,
                    }),
                },
            )
            .map_err(|e| std::io::Error::other(format!("seed rust-inspect function: {e}")))?;
        tc.check_program(&ast)
            .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|err| std::io::Error::other(format!("lowering failed: {err:?}")))?;

        let mut codegen = IrCodegen::new();
        codegen.collect_external_rust_functions(&ast);

        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        emitter.set_external_rust_functions(codegen.external_rust_functions.clone());
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|err| std::io::Error::other(format!("emit failed: {err:?}")))?;

        assert!(
            code.contains("consume(&state, &plan).await"),
            "expected borrowed async rust free-function args in generated code; got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_awaits_async_rust_backed_method_from_metadata() -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use incan_core::interop::{
            RustFunctionSig, RustItemKind, RustItemMetadata, RustMethodSig, RustParam, RustTypeInfo, RustVisibility,
        };

        let source = r#"
import std.async
from rust::demo import SessionContext
from rust::demo import CsvReadOptions
from rust::demo import make_context
from rust::demo import make_options

pub async def register_csv() -> None:
  ctx = make_context()
  opts = make_options()
  match await ctx.register_csv("orders", "orders.csv", opts):
    Ok(_) => pass
    Err(_) => pass
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let tmp = seeded_rust_inspect_workspace()?;
        let manifest_dir = tmp.path().to_path_buf();
        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(manifest_dir.clone());
        tc.rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::SessionContext".to_string(),
                    definition_path: Some("demo::SessionContext".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Type(RustTypeInfo {
                        type_params: Vec::new(),
                        type_param_defaults: Vec::new(),
                        mutable_reference_type_params: Vec::new(),
                        expanded_derive_traits: Vec::new(),
                        has_const_params: false,
                        alias_target: None,
                        metadata_completeness: Default::default(),
                        methods: vec![
                            RustMethodSig {
                                name: "new".to_string(),
                                signature: RustFunctionSig {
                                    type_params: Vec::new(),
                                    params: Vec::new(),
                                    return_type: "demo::SessionContext".to_string(),
                                    is_async: false,
                                    is_unsafe: false,
                                },
                            },
                            RustMethodSig {
                                name: "register_csv".to_string(),
                                signature: RustFunctionSig {
                                    type_params: Vec::new(),
                                    params: vec![
                                        RustParam {
                                            name: Some("self".to_string()),
                                            type_display: "&self".to_string(),
                                        },
                                        RustParam {
                                            name: Some("name".to_string()),
                                            type_display: "&str".to_string(),
                                        },
                                        RustParam {
                                            name: Some("path".to_string()),
                                            type_display: "&str".to_string(),
                                        },
                                        RustParam {
                                            name: Some("options".to_string()),
                                            type_display: "demo::CsvReadOptions".to_string(),
                                        },
                                    ],
                                    return_type: "Result<(), demo::DataFusionError>".to_string(),
                                    is_async: true,
                                    is_unsafe: false,
                                },
                            },
                        ],
                        implemented_traits: Vec::new(),
                        fields: vec![],
                        variants: vec![],
                    }),
                },
            )
            .map_err(|e| std::io::Error::other(format!("seed rust-inspect context: {e}")))?;
        tc.rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::CsvReadOptions".to_string(),
                    definition_path: Some("demo::CsvReadOptions".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Type(RustTypeInfo {
                        type_params: Vec::new(),
                        type_param_defaults: Vec::new(),
                        mutable_reference_type_params: Vec::new(),
                        expanded_derive_traits: Vec::new(),
                        has_const_params: false,
                        alias_target: None,
                        metadata_completeness: Default::default(),
                        methods: vec![RustMethodSig {
                            name: "new".to_string(),
                            signature: RustFunctionSig {
                                type_params: Vec::new(),
                                params: Vec::new(),
                                return_type: "demo::CsvReadOptions".to_string(),
                                is_async: false,
                                is_unsafe: false,
                            },
                        }],
                        implemented_traits: Vec::new(),
                        fields: vec![],
                        variants: vec![],
                    }),
                },
            )
            .map_err(|e| std::io::Error::other(format!("seed rust-inspect options: {e}")))?;
        tc.rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::make_context".to_string(),
                    definition_path: Some("demo::make_context".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Function(RustFunctionSig {
                        type_params: Vec::new(),
                        params: Vec::new(),
                        return_type: "demo::SessionContext".to_string(),
                        is_async: false,
                        is_unsafe: false,
                    }),
                },
            )
            .map_err(|e| std::io::Error::other(format!("seed rust-inspect context factory: {e}")))?;
        tc.rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::make_options".to_string(),
                    definition_path: Some("demo::make_options".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Function(RustFunctionSig {
                        type_params: Vec::new(),
                        params: Vec::new(),
                        return_type: "demo::CsvReadOptions".to_string(),
                        is_async: false,
                        is_unsafe: false,
                    }),
                },
            )
            .map_err(|e| std::io::Error::other(format!("seed rust-inspect options factory: {e}")))?;
        tc.check_program(&ast)
            .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|err| std::io::Error::other(format!("lowering failed: {err:?}")))?;

        let mut codegen = IrCodegen::new();
        codegen.collect_external_rust_functions(&ast);

        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        emitter.set_external_rust_functions(codegen.external_rust_functions.clone());
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|err| std::io::Error::other(format!("emit failed: {err:?}")))?;

        assert!(
            code.contains("ctx.register_csv(") && code.contains(").await"),
            "expected async Rust method call to be awaited in generated code; got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_borrows_async_rust_backed_free_function_args_from_real_rust_inspect()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use crate::rust_inspect::write_async_result_probe_crate;

        let source = r#"
from std.async import sleep
from rust::ra_async_result_probe import State
from rust::ra_async_result_probe import Plan
from rust::ra_async_result_probe import consume

pub async def run(state: State, plan: Plan) -> None:
  await sleep(0.01)
  await consume(state, plan)
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let tmp = tempfile::tempdir()?;
        write_async_result_probe_crate(tmp.path())?;

        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(tmp.path().to_path_buf());
        prewarm_metadata(
            tmp.path(),
            &[
                "ra_async_result_probe::State",
                "ra_async_result_probe::Plan",
                "ra_async_result_probe::consume",
            ],
        )?;
        tc.check_program(&ast)
            .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|err| std::io::Error::other(format!("lowering failed: {err:?}")))?;

        let mut codegen = IrCodegen::new();
        codegen.collect_external_rust_functions(&ast);

        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        emitter.set_external_rust_functions(codegen.external_rust_functions.clone());
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|err| std::io::Error::other(format!("emit failed: {err:?}")))?;

        assert!(
            code.contains("consume(&state, &plan).await"),
            "expected borrowed async rust free-function args from real metadata; got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_borrows_async_rust_backed_free_function_args_from_generated_lock_workspace()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::backend::project::ProjectGenerator;
        use crate::frontend::typechecker::TypeChecker;
        use crate::manifest::{DependencySource, DependencySpec};
        use crate::rust_inspect::write_hyphenated_function_probe_crate;

        let source = r#"
from std.async import sleep
from rust::foo_bar import State
from rust::foo_bar import Plan
from rust::foo_bar::consumer import consume

pub async def run(state: State, plan: Plan) -> None:
  await sleep(0.01)
  await consume(state, plan)
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let tmp = tempfile::tempdir()?;
        let dep_root = tmp.path().join("foo-bar-dep");
        write_hyphenated_function_probe_crate(&dep_root)?;

        let lock_root = tmp.path().join("generated_lock");
        let mut generator = ProjectGenerator::new(&lock_root, "lock_probe", true);
        generator.set_dependencies(vec![DependencySpec {
            crate_name: "foo-bar".to_string(),
            version: None,
            features: vec![],
            default_features: true,
            source: DependencySource::Path { path: dep_root.clone() },
            optional: false,
            package: None,
        }]);
        generator.generate("fn main() {}\n")?;

        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(lock_root.clone());
        prewarm_metadata(
            &lock_root,
            &["foo_bar::State", "foo_bar::Plan", "foo_bar::consumer::consume"],
        )?;
        tc.check_program(&ast)
            .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|err| std::io::Error::other(format!("lowering failed: {err:?}")))?;

        let mut codegen = IrCodegen::new();
        codegen.collect_external_rust_functions(&ast);

        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        emitter.set_external_rust_functions(codegen.external_rust_functions.clone());
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|err| std::io::Error::other(format!("emit failed: {err:?}")))?;

        assert!(
            code.contains("consume(&state, &plan).await"),
            "expected borrowed async rust free-function args from generated lock workspace; got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_nested_module_codegen_borrows_async_rust_args_from_generated_lock_workspace()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::backend::project::ProjectGenerator;
        use crate::manifest::{DependencySource, DependencySpec};
        use crate::rust_inspect::write_hyphenated_function_probe_crate;

        let main_module = parse_program(
            r#"
def main() -> None:
  return
"#,
        );
        let dep_module = parse_program(
            r#"
from std.async import sleep
from rust::foo_bar import State
from rust::foo_bar import Plan
from rust::foo_bar::consumer import consume

pub async def run(state: State, plan: Plan) -> None:
  await sleep(0.01)
  await consume(state, plan)
"#,
        );

        let tmp = tempfile::tempdir()?;
        let dep_root = tmp.path().join("foo-bar-dep");
        write_hyphenated_function_probe_crate(&dep_root)?;

        let lock_root = tmp.path().join("generated_lock");
        let mut generator = ProjectGenerator::new(&lock_root, "lock_probe", true);
        generator.set_dependencies(vec![DependencySpec {
            crate_name: "foo-bar".to_string(),
            version: None,
            features: vec![],
            default_features: true,
            source: DependencySource::Path { path: dep_root.clone() },
            optional: false,
            package: None,
        }]);
        generator.generate("fn main() {}\n")?;

        let worker_path = vec!["worker".to_string()];
        let mut codegen = IrCodegen::new();
        codegen.set_rust_inspect_manifest_dir(lock_root);
        codegen.add_module_with_path_segments("worker", &dep_module, worker_path.clone());

        let (_main_code, rust_modules) =
            must_ok(codegen.try_generate_multi_file_nested(&main_module, std::slice::from_ref(&worker_path)));
        let worker_code = must_some(rust_modules.get(&worker_path), "missing generated worker module");

        assert!(
            worker_code.contains("consume(&state, &plan).await"),
            "expected borrowed async rust free-function args in generated nested module; got:\n{worker_code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_try_generate_module_keeps_root_rust_trait_import_issue827() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_message_trait_probe_crate(tmp.path())?;

        let worker_module = parse_program(
            r#"
from rust::message_probe import Message, Packet

pub def encode_packet(packet: Packet) -> None:
  _ = packet.encode_to_vec()
"#,
        );
        let mut codegen = IrCodegen::new();
        codegen.set_rust_inspect_manifest_dir(tmp.path().to_path_buf());
        codegen.add_module("worker", &worker_module);

        let code = must_ok(codegen.try_generate_module("worker", &worker_module));

        assert!(
            code.contains("use ::message_probe::{Message, Packet};")
                || (code.contains("use ::message_probe::Message;") && code.contains("use ::message_probe::Packet;")),
            "expected module generation to preserve root Rust trait import needed by encode_to_vec(); got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_borrows_async_rust_args_after_rust_method_return() -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use crate::rust_inspect::write_async_result_probe_crate;

        let source = r#"
from std.async import sleep
from rust::ra_async_result_probe import SessionContext
from rust::ra_async_result_probe import Plan
from rust::ra_async_result_probe import consume

pub async def run(plan: Plan) -> None:
  ctx = SessionContext.new()
  state = ctx.state()
  await sleep(0.01)
  await consume(state, plan)
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let tmp = tempfile::tempdir()?;
        write_async_result_probe_crate(tmp.path())?;

        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(tmp.path().to_path_buf());
        prewarm_metadata(
            tmp.path(),
            &[
                "ra_async_result_probe::SessionContext",
                "ra_async_result_probe::Plan",
                "ra_async_result_probe::consume",
            ],
        )?;
        tc.check_program(&ast)
            .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|err| std::io::Error::other(format!("lowering failed: {err:?}")))?;

        let mut codegen = IrCodegen::new();
        codegen.collect_external_rust_functions(&ast);

        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        emitter.set_external_rust_functions(codegen.external_rust_functions.clone());
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|err| std::io::Error::other(format!("emit failed: {err:?}")))?;

        assert!(
            code.contains("consume(&state, &plan).await"),
            "expected borrowed async rust free-function args after rust method return; got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_ir_codegen_uses_configured_rust_inspect_workspace_for_async_borrows()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::rust_inspect::write_hyphenated_function_probe_crate;

        let tmp = tempfile::tempdir()?;
        let dep_root = tmp.path().join("foo-bar-dep");
        write_hyphenated_function_probe_crate(&dep_root)?;

        let host_root = tmp.path().join("host");
        std::fs::create_dir_all(host_root.join("src"))?;
        std::fs::write(
            host_root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"host\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies.foo_bar]\npackage = \"foo-bar\"\npath = \"{}\"\n",
                dep_root.display()
            ),
        )?;
        std::fs::write(host_root.join("src/lib.rs"), "pub fn touch() {}\n")?;

        let source = r#"
from std.async import sleep
from rust::foo_bar import State
from rust::foo_bar import Plan
from rust::foo_bar::consumer import consume

pub async def run(state: State, plan: Plan) -> None:
  await sleep(0.01)
  await consume(state, plan)
"#;
        let ast = parse_program(source);
        let mut codegen = IrCodegen::new();
        codegen.set_rust_inspect_manifest_dir(host_root);
        let code = must_ok(codegen.try_generate(&ast));

        assert!(
            code.contains("consume(&state, &plan).await"),
            "expected IrCodegen to preserve borrowed async args via the configured metadata workspace; got:\n{code}"
        );
        Ok(())
    }

    #[test]
    fn test_codegen_emits_explicit_function_call_type_args() {
        let source = r#"
def id[T](x: T) -> T:
  return x

pub def run() -> int:
  return id[int](1)
"#;
        let ast = parse_program(source);
        let code = must_ok(IrCodegen::new().try_generate(&ast));
        let id = projected_name(&code, "id", SemanticSourceTargetKind::Function);
        assert!(
            compact_rust(&code).contains(&format!("{id}::<i64,>(1)")),
            "expected explicit function type args to emit Rust turbofish, got:\n{code}"
        );
    }

    #[test]
    fn test_codegen_emits_explicit_method_call_type_args() {
        let source = r#"
class Box:
  def pick[T](self, value: T) -> T:
    return value

pub def run() -> int:
  let b = Box()
  return b.pick[int](1)
"#;
        let ast = parse_program(source);
        let code = must_ok(IrCodegen::new().try_generate(&ast));
        let pick = projected_name(&code, "pick", SemanticSourceTargetKind::Method);
        assert!(
            compact_rust(&code).contains(&format!("{pick}::<i64,>(1)")),
            "expected explicit method type args to emit Rust turbofish, got:\n{code}"
        );
    }

    #[test]
    fn test_codegen_emits_full_turbofish_for_mixed_explicit_and_inferred_type_args() {
        let source = r#"
def pair_map[T, U](x: T, y: U) -> int:
  return 0

pub def run() -> int:
  return pair_map[int, _](1, 2)
"#;
        let ast = parse_program(source);
        let code = must_ok(IrCodegen::new().try_generate(&ast));
        let pair_map = projected_name(&code, "pair_map", SemanticSourceTargetKind::Function);
        assert!(
            compact_rust(&code).contains(&format!("{pair_map}::<i64,i64,>(1,2)")),
            "expected full turbofish for mixed explicit/`_` call-site generics, got:\n{code}"
        );
    }

    #[test]
    fn try_generate_module_uses_checked_composed_newtype_conversion_plan() {
        let ast = parse_program(
            r#"
from std.environ import get_as
from std.traits.convert import TryFrom

type Port = newtype int
type WrappedPort = newtype Port

def read() -> None:
  get_as[WrappedPort]("PORT")
"#,
        );
        let mut codegen = IrCodegen::new();
        let code = must_ok(codegen.try_generate_module("env_types", &ast));
        assert!(
            code.contains("for WrappedPort"),
            "expected checked composed-newtype bridge in generated module:\n{code}"
        );
    }
}
