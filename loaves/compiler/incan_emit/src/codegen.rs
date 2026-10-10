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
//! use incan_emit::IrCodegen;
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
//! The `try_generate*` family of methods return `Result<_, GenerationError>`, allowing callers to handle lowering and
//! emission errors explicitly. The `generate*` methods are convenience wrappers that return error comments on failure
//! (useful for debugging but not recommended for production).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::env;
#[cfg(feature = "rust_inspect")]
use std::path::PathBuf;
use std::sync::Arc;

use incan_frontend::api_metadata::ApiDeclaration;
use incan_frontend::ast::{Declaration, ImportKind, Program};
use incan_frontend::diagnostics::CompileError;
use incan_frontend::library_manifest::{
    ExportIdentityKind, ImplementationAssociatedTypeExport, ImplementationTraitBoundExport,
    ImplementationTraitBoundOriginExport, ImplementationTypeParamExport, LibraryManifest, TypeBoundExport,
    TypeParamExport, TypeRef,
};
use incan_frontend::library_manifest_index::LibraryManifestIndex;
use incan_frontend::module::canonicalize_source_module_segments;
use incan_frontend::provider::{ProviderPlan, SDK_PROVIDER_BUILD_ENV};
use incan_frontend::typechecker::TypeCheckInfo;
use incan_frontend::typechecker::stdlib_loader::StdlibAstCache;
#[cfg(test)]
use incan_lang::lang::traits;
use incan_lang::lang::{rust_keywords, stdlib, trait_bounds};
use oven_model::compiler_suite_env::OVEN_LOAF_ENV;

use crate::emit::CallableNameResolution;
use crate::{EmitError, EmitService, IrEmitter};
use incan_ir::decl::{FunctionParamDefault, IrTraitBoundOrigin, IrTypeParam, Visibility};
use incan_ir::scanners::{
    check_for_this_import as scan_check_for_this_import, collect_rust_crates as scan_collect_rust_crates,
    detect_serde_usage,
};
use incan_ir::types::{IrType, manifest_type_ref_from_ir};
use incan_ir::{AstLowering, FunctionRegistry, IrDeclKind, IrExpr, IrExprKind, IrProgram, LoweringErrors};

mod capability_bridge;
mod dependency_metadata;
mod ordinal_bridge;
mod serde_activation;
mod string_try_from_bridge;

use dependency_metadata::{
    DependencySymbolMetadata, collect_dependency_symbol_metadata,
    collect_externally_reachable_items_by_module_with_cache, collect_model_field_aliases,
    publish_default_constructed_fields, record_default_path_items_from_ir,
    record_direct_generated_path_support_items_from_ir, should_preserve_dependency_public_items, source_module_origins,
    source_module_rust_paths,
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
                .and_then(incan_frontend::module::logical_module_name_from_source_path)
                .map(|name| name.split('.').map(str::to_owned).collect())
        })
        .or_else(|| fallback_name.map(|name| vec![name.to_string()]))?;
    Some(canonicalize_source_module_segments(&path))
}

/// Error during Rust code generation.
///
/// This error type wraps all possible errors that can occur during code generation, including AST lowering errors and
/// IR emission errors.
///
/// ## Examples
///
/// ```rust,ignore
/// use incan_emit::{GenerationError, IrCodegen};
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
    generated_union_types: HashMap<String, incan_ir::types::IrType>,
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
    requirement: crate::trait_bound_inference::ImplementationBoundRequirement,
    target_visibility: CapturedImplementationTargetVisibility,
}

/// Inferred bounds on one exported model or class's inherent implementation and methods.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedInherentBounds {
    module_path: Vec<String>,
    target_type: String,
    owner_type_params: Vec<IrTypeParam>,
    methods: Vec<(String, Vec<IrTypeParam>)>,
}

/// Inferred bounds on one exported free function or trait method.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CapturedCallableBounds {
    Function {
        module_path: Vec<String>,
        name: String,
        type_params: Vec<IrTypeParam>,
    },
    TraitMethod {
        module_path: Vec<String>,
        trait_name: String,
        method_name: String,
        type_params: Vec<IrTypeParam>,
    },
}

/// Visibility of an implementation target resolved within the IR program that owns the implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapturedImplementationTargetVisibility {
    SameProgram(Visibility),
    Unknown,
}

/// Compiler-owned metadata discovered while lowering and inferring one generated library.
#[derive(Debug, Clone, Default)]
pub struct IrGenerationMetadata {
    implementation_bound_requirements: Vec<CapturedImplementationBoundRequirement>,
    inherent_bounds: Vec<CapturedInherentBounds>,
    /// Inferred free-function and trait-method bounds resolved from the exact emitted IR.
    callable_bounds: Vec<CapturedCallableBounds>,
    emitted_declaration_types: Vec<crate::emit::native_unions::EmittedDeclarationTypes>,
    native_unions: Vec<incan_frontend::library_manifest::NativeUnionExport>,
    provider_plan: Option<Arc<ProviderPlan>>,
}

impl IrGenerationMetadata {
    /// Publish exact implementation headers into the checked trait adoptions that consumers already resolve.
    pub fn apply_to_library_manifest(&self, manifest: &mut LibraryManifest) -> Result<(), String> {
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
        for captured in &self.inherent_bounds {
            if let Some(api) = manifest.contract_metadata.api.as_mut() {
                for module in &mut api.modules {
                    if module.module_path != captured.module_path {
                        continue;
                    }
                    for declaration in &mut module.declarations {
                        match declaration {
                            ApiDeclaration::Model(model) if model.name == captured.target_type => {
                                merge_inferred_type_params(&mut model.type_params, &captured.owner_type_params)?;
                                merge_inferred_method_type_params(&mut model.methods, &captured.methods)?;
                            }
                            ApiDeclaration::Class(class) if class.name == captured.target_type => {
                                merge_inferred_type_params(&mut class.type_params, &captured.owner_type_params)?;
                                merge_inferred_method_type_params(&mut class.methods, &captured.methods)?;
                            }
                            _ => {}
                        }
                    }
                }
            }

            let mut source_path = captured.module_path.clone();
            source_path.push(captured.target_type.clone());
            let public_exports = manifest
                .contract_metadata
                .identity_graph
                .exports
                .iter()
                .filter(|identity| identity.source_path == source_path)
                .map(|identity| (identity.public_name.clone(), identity.kind))
                .collect::<Vec<_>>();
            for (public_name, kind) in public_exports {
                match kind {
                    ExportIdentityKind::Model => {
                        if let Some(model) = manifest
                            .exports
                            .models
                            .iter_mut()
                            .find(|model| model.name == public_name)
                        {
                            merge_inferred_type_params(&mut model.type_params, &captured.owner_type_params)?;
                            merge_inferred_method_type_params(&mut model.methods, &captured.methods)?;
                        }
                    }
                    ExportIdentityKind::Class => {
                        if let Some(class) = manifest
                            .exports
                            .classes
                            .iter_mut()
                            .find(|class| class.name == public_name)
                        {
                            merge_inferred_type_params(&mut class.type_params, &captured.owner_type_params)?;
                            merge_inferred_method_type_params(&mut class.methods, &captured.methods)?;
                        }
                    }
                    _ => {}
                }
            }
        }
        for captured in &self.callable_bounds {
            apply_callable_bounds_to_manifest(manifest, captured)?;
        }
        for captured in &self.emitted_declaration_types {
            captured.apply(manifest);
        }
        manifest.contract_metadata.native_unions = self.native_unions.clone();
        crate::emit::native_unions::preserve_native_aliases(manifest, self.provider_plan.as_deref())?;
        Ok(())
    }
}

/// Merge compiler-inferred bounds into matching manifest type parameters without replacing source-declared bounds.
///
/// The emitted IR carries every bound a type parameter needs, the source-declared ones included, spelled as the Rust
/// trait each lowers to and without the declaration's module path. A bound naming a trait the parameter already
/// declares is that declaration, not an inferred requirement, so it is not published a second time: its IR spelling
/// would not resolve like the declared bound in a consumer, where a trait method's bound over its trait's own type
/// parameter (`Callable1[E, F]`) keeps `E` as a nominal name the adopter's type arguments never replace.
fn merge_inferred_type_params(target: &mut [TypeParamExport], inferred: &[IrTypeParam]) -> Result<(), String> {
    for inferred_param in inferred {
        let Some(target_param) = target.iter_mut().find(|param| param.name == inferred_param.name) else {
            continue;
        };
        for bound in &inferred_param.bounds {
            if declares_bound_trait(&target_param.bounds, &bound.trait_path) {
                continue;
            }
            let exported = inferred_type_bound_export(bound)?;
            if !target_param.bounds.iter().any(|existing| {
                existing.name == exported.name
                    && existing.type_args == exported.type_args
                    && existing.source_name == exported.source_name
                    && existing.module_path == exported.module_path
            }) {
                target_param.bounds.push(exported);
            }
        }
    }
    Ok(())
}

/// Return whether a source-declared bound among `bounds` names the trait an emitted bound's Rust path lowers from.
fn declares_bound_trait(bounds: &[TypeBoundExport], rust_trait_path: &str) -> bool {
    let trait_name = match trait_bounds::rust_to_incan(rust_trait_path) {
        Some(incan_name) => incan_name,
        None => bound_trait_leaf(rust_trait_path),
    };
    bounds
        .iter()
        .filter(|bound| !bound.inferred)
        .any(|bound| bound_trait_leaf(bound.source_name.as_deref().unwrap_or(&bound.name)) == trait_name)
}

/// Return a bound's trait name without the module or crate path it is spelled with.
fn bound_trait_leaf(path: &str) -> &str {
    path.rsplit(['.', ':']).next().unwrap_or(path)
}

/// Map each import alias a program binds to the name of the item it imports.
fn import_alias_names(program: &IrProgram) -> HashMap<String, String> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| match &declaration.kind {
            IrDeclKind::Import { items, .. } => Some(items),
            _ => None,
        })
        .flatten()
        .filter_map(|item| Some((item.alias.clone()?, item.name.clone())))
        .collect()
}

/// Spell each bound of `type_params` by the imported item's own name where the source wrote an import alias, so a
/// declared bound written through the alias (`T with RustSerialize`) is recognized as the declaration it restates.
fn unaliased_type_params(type_params: &[IrTypeParam], aliases: &HashMap<String, String>) -> Vec<IrTypeParam> {
    type_params
        .iter()
        .map(|param| {
            let mut param = param.clone();
            for bound in &mut param.bounds {
                if let Some(name) = aliases.get(&bound.trait_path) {
                    bound.trait_path = name.clone();
                }
            }
            param
        })
        .collect()
}

/// Merge inferred type-parameter bounds into the corresponding exported methods.
fn merge_inferred_method_type_params(
    methods: &mut [impl InferredMethodExport],
    inferred: &[(String, Vec<IrTypeParam>)],
) -> Result<(), String> {
    for method in methods {
        let method_name = method.method_name().to_string();
        for (_, type_params) in inferred.iter().filter(|(name, _)| name == &method_name) {
            merge_inferred_type_params(method.method_type_params_mut(), type_params)?;
        }
    }
    Ok(())
}

/// Shared access to method metadata in checked API and public manifest exports.
trait InferredMethodExport {
    /// The method's source name, which inferred bounds are keyed by.
    fn method_name(&self) -> &str;
    /// The method's exported type parameters, whose bounds inference extends in place.
    fn method_type_params_mut(&mut self) -> &mut [TypeParamExport];
}

impl InferredMethodExport for incan_frontend::api_metadata::ApiMethod {
    fn method_name(&self) -> &str {
        &self.name
    }

    fn method_type_params_mut(&mut self) -> &mut [TypeParamExport] {
        &mut self.type_params
    }
}

impl InferredMethodExport for incan_frontend::library_manifest::MethodExport {
    fn method_name(&self) -> &str {
        &self.name
    }

    fn method_type_params_mut(&mut self) -> &mut [TypeParamExport] {
        &mut self.type_params
    }
}

/// Convert an inferred IR trait requirement into the checked manifest shape.
fn inferred_type_bound_export(bound: &incan_ir::decl::IrTraitBound) -> Result<TypeBoundExport, String> {
    Ok(TypeBoundExport {
        name: bound.trait_path.clone(),
        source_name: None,
        module_path: None,
        type_args: bound
            .type_args
            .iter()
            .map(manifest_type_ref_from_ir)
            .collect::<Result<Vec<_>, _>>()?,
        implementation_type_params: Vec::new(),
        inferred: true,
    })
}

/// Publish inferred free-function and trait-method bounds to checked API and public export surfaces (#1826).
fn apply_callable_bounds_to_manifest(
    manifest: &mut LibraryManifest,
    captured: &CapturedCallableBounds,
) -> Result<(), String> {
    match captured {
        CapturedCallableBounds::Function {
            module_path,
            name,
            type_params,
        } => {
            if let Some(api) = manifest.contract_metadata.api.as_mut() {
                for module in &mut api.modules {
                    if module.module_path == *module_path {
                        for declaration in &mut module.declarations {
                            if let ApiDeclaration::Function(function) = declaration
                                && function.name == *name
                            {
                                merge_inferred_type_params(&mut function.type_params, type_params)?;
                            }
                        }
                    }
                }
            }
            let mut source_path = module_path.clone();
            source_path.push(name.clone());
            let public_names = manifest
                .contract_metadata
                .identity_graph
                .exports
                .iter()
                .filter(|identity| identity.kind == ExportIdentityKind::Function && identity.source_path == source_path)
                .map(|identity| identity.public_name.clone())
                .collect::<Vec<_>>();
            for public_name in public_names {
                if let Some(function) = manifest
                    .exports
                    .functions
                    .iter_mut()
                    .find(|function| function.name == public_name)
                {
                    merge_inferred_type_params(&mut function.type_params, type_params)?;
                }
            }
        }
        CapturedCallableBounds::TraitMethod {
            module_path,
            trait_name,
            method_name,
            type_params,
        } => {
            if let Some(api) = manifest.contract_metadata.api.as_mut() {
                for module in &mut api.modules {
                    if module.module_path == *module_path {
                        for declaration in &mut module.declarations {
                            if let ApiDeclaration::Trait(trait_decl) = declaration
                                && trait_decl.name == *trait_name
                            {
                                for method in &mut trait_decl.methods {
                                    if method.name == *method_name {
                                        merge_inferred_type_params(&mut method.type_params, type_params)?;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let mut source_path = module_path.clone();
            source_path.push(trait_name.clone());
            let public_names = manifest
                .contract_metadata
                .identity_graph
                .exports
                .iter()
                .filter(|identity| identity.kind == ExportIdentityKind::Trait && identity.source_path == source_path)
                .map(|identity| identity.public_name.clone())
                .collect::<Vec<_>>();
            for public_name in public_names {
                if let Some(trait_export) = manifest
                    .exports
                    .traits
                    .iter_mut()
                    .find(|trait_export| trait_export.name == public_name)
                {
                    for method in &mut trait_export.methods {
                        if method.name == *method_name {
                            merge_inferred_type_params(&mut method.type_params, type_params)?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
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
    requirement: &crate::trait_bound_inference::ImplementationBoundRequirement,
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
                        // A function type's `Fn` bound spells a parameter or return type, never an implementation
                        // header, so it has no manifest form.
                        IrTraitBoundOrigin::FunctionType => {
                            return Err(format!(
                                "a function type's `{}` bound cannot bound an implementation header",
                                bound.trait_path
                            ));
                        }
                    },
                })
            })
            .collect::<Result<Vec<_>, String>>()?,
    })
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

/// RFC 097 identity embedded in an Oven-materialized Rust caller artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerIdentity {
    /// Incan package name.
    pub package_name: String,
    /// Incan package version.
    pub package_version: String,
    /// Digest of the library-scoped selected export facet.
    pub caller_facet_id: String,
    /// Independently versioned caller ABI.
    pub caller_abi_version: String,
    /// Compatible Incan compiler version range.
    pub compiler_version_range: String,
    /// Schema version of this identity record.
    pub manifest_schema_version: u32,
    /// Rust target triple of the materialized artifact.
    pub target: String,
    /// Build profile of the materialized artifact.
    pub profile: String,
    /// Exact Oven receipt or Loaf asset identity that authorized materialization.
    pub receipt_reference: String,
}

/// IR-based Rust code generator
///
/// This is the unified entrypoint for code generation. It uses the typed IR and syn/quote for code emission.
pub struct IrCodegen<'a> {
    /// The current program being generated
    current_program: Option<&'a Program>,
    /// Dependency modules to include before main.
    ///
    /// Stores both the flat module name (used for build graph identity) and the nested module path segments (used for
    /// correct Rust qualification in codegen).
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
    /// Checked physical nominal routes shared by lowering and emitter metadata readers.
    foreign_pub_type_remappings: HashMap<String, HashMap<String, String>>,
    /// Whether to emit the Zen of Incan at the start of main (set by `import this`)
    emit_zen_in_main: bool,
    /// Functions imported from external Rust crates (name -> true for external) Rust functions imported by the
    /// program, recorded by `collect_external_rust_functions` for the emitter.
    pub external_rust_functions: HashSet<String>,
    /// Declared Rust crate names from `loaf.toml [rust-dependencies]` (RFC 013 / RFC 023).
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
    /// Checked source names selected for the Rust-hosted caller facet.
    caller_facet_exports: BTreeSet<String>,
    /// Inspectable identity bound to the selected caller facet, when this is a caller artifact.
    caller_identity: Option<CallerIdentity>,
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
    /// Whether the programs being generated are modules of the standard library, which the checker checks under the
    /// standard library's own rules.
    standard_library_source: bool,
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
    /// Inferred inherent-implementation and method bounds resolved from the exact emitted IR.
    inherent_bounds: Vec<CapturedInherentBounds>,
    /// Inferred free-function and trait-method bounds resolved from the exact emitted IR.
    callable_bounds: Vec<CapturedCallableBounds>,
    /// Authoritative checked-API path for a library root while collecting manifest metadata.
    metadata_root_module_path: Option<Vec<String>>,
    /// Checked API supplied by the publication caller, joined only to the same emitted source module.
    publication_api: Option<incan_frontend::api_metadata::CheckedApiMetadataPackage>,
    /// Checked public identities from the same publication, before its containing digest exists.
    publication_identities: incan_frontend::library_manifest::LibraryIdentityGraph,
    publication_package_name: String,
    /// Final crate-root emitted definitions, shared with source modules whose wrappers live at the root.
    emitted_union_definitions: HashMap<String, IrType>,
    /// Exact lowered nominal spellings from each module's accepted checker bindings.
    native_union_origins:
        HashMap<Vec<String>, BTreeMap<String, incan_frontend::library_manifest::NominalTypeOriginExport>>,
    emitted_declaration_types: Vec<crate::emit::native_unions::EmittedDeclarationTypes>,
    native_unions: Vec<incan_frontend::library_manifest::NativeUnionExport>,
    /// Manifest/workspace root for rust-inspect-backed typechecking during IR generation.
    #[cfg(feature = "rust_inspect")]
    rust_inspect_manifest_dir: Option<PathBuf>,
}

/// Fold one module's capture of an emitted union wrapper into the record already held for that wrapper.
///
/// A wrapper is captured once per module that mentions it, and each capture only sees what that module can:
/// `local_nominals` comes from the module's own checked declarations, so the module declaring the payload models
/// contributes every leaf binding while a module that merely accepts the union in a signature contributes none.
/// The bindings are therefore merged rather than compared — the published record has to name every leaf, not the
/// subset whichever module happened to be emitted first could see.
///
/// What must agree across modules is the wrapper's representation: its owner, and the payload members in producer
/// order, because consumers index the emitted `V0`, `V1`, ... variants by that order and may not recompute it. Two
/// captures disagreeing there, or binding one leaf name to two canonical identities, is a genuine defect and stays
/// an error — named precisely, so the next occurrence does not need an instrumented build to diagnose.
fn merge_native_union_capture(
    existing: &mut incan_frontend::library_manifest::NativeUnionExport,
    captured: incan_frontend::library_manifest::NativeUnionExport,
) -> Result<(), EmitError> {
    if existing.owner != captured.owner {
        return Err(EmitError::InternalInvariant(format!(
            "emitted union {} was captured under two owners",
            captured.rust_name
        )));
    }
    if existing.members != captured.members {
        return Err(EmitError::InternalInvariant(format!(
            "emitted union {} was captured with two different payload orders",
            captured.rust_name
        )));
    }
    for (name, canonical) in captured.local_nominals {
        match existing.local_nominals.entry(name) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(canonical);
            }
            std::collections::btree_map::Entry::Occupied(slot) => {
                if slot.get() != &canonical {
                    return Err(EmitError::InternalInvariant(format!(
                        "emitted union {} binds {} to two canonical identities",
                        captured.rust_name,
                        slot.key()
                    )));
                }
            }
        }
    }
    Ok(())
}

impl<'a> IrCodegen<'a> {
    /// Complete the callable signature of each call this module makes, through a canonical callee path, to a function
    /// another module of the compilation declares.
    ///
    /// A call whose lowering carried no signature takes the declaring module's parameters, defaults included, so the
    /// passes after this one and emission see the callee's declared surface (#1842). Every emitted module is such a
    /// caller, the crate root and each source module alike, so `dependency_programs` holds every other lowered module
    /// of the compilation keyed by its module path. A static such a default reads needs no binding in the caller:
    /// lowering names it through the module that declares it.
    fn complete_external_call_signatures<'p>(
        program: &mut IrProgram,
        dependency_programs: impl IntoIterator<Item = (&'p [String], &'p IrProgram)>,
    ) {
        use incan_ir::{Visitor, walk_expr};

        let mut callables = HashMap::new();
        for (module_path, dependency) in dependency_programs {
            for decl in &dependency.declarations {
                if let IrDeclKind::Function(function) = &decl.kind {
                    callables.insert(
                        (module_path.to_vec(), function.name.clone()),
                        incan_ir::FunctionSignature {
                            params: function.params.clone(),
                            return_type: function.return_type.clone(),
                        },
                    );
                }
            }
        }

        struct ExternalCallSignatures<'a> {
            callables: &'a HashMap<(Vec<String>, String), incan_ir::FunctionSignature>,
        }
        impl Visitor for ExternalCallSignatures<'_> {
            fn expr(&mut self, expr: &mut IrExpr) {
                if let IrExprKind::Call {
                    callable_signature,
                    canonical_path: Some(canonical_path),
                    ..
                } = &mut expr.kind
                    && callable_signature.is_none()
                    && let Some((callable_name, module_path)) = canonical_path.split_last()
                {
                    *callable_signature = self
                        .callables
                        .get(&(module_path.to_vec(), callable_name.clone()))
                        .cloned();
                }
                walk_expr(expr, self);
            }
        }

        /// Visit every expression a function evaluates: its source parameter defaults, then its body.
        fn visit_function(function: &mut incan_ir::IrFunction, visitor: &mut impl Visitor) {
            for param in &mut function.params {
                if let Some(FunctionParamDefault::Source(default)) = &mut param.default {
                    visitor.expr(default);
                }
            }
            for stmt in &mut function.body {
                visitor.stmt(stmt);
            }
        }

        let mut visitor = ExternalCallSignatures { callables: &callables };
        for stmt in &mut program.module_init {
            visitor.stmt(stmt);
        }
        for decl in &mut program.declarations {
            match &mut decl.kind {
                IrDeclKind::Function(function) => visit_function(function, &mut visitor),
                IrDeclKind::Impl(implementation) => {
                    for method in &mut implementation.methods {
                        visit_function(method, &mut visitor);
                    }
                }
                IrDeclKind::Trait(declaration) => {
                    for method in &mut declaration.methods {
                        visit_function(method, &mut visitor);
                    }
                }
                IrDeclKind::Struct(declaration) => {
                    for default in declaration.fields.iter_mut().filter_map(|field| field.default.as_mut()) {
                        visitor.expr(default);
                    }
                }
                IrDeclKind::Const { value, .. } | IrDeclKind::Static { value, .. } => visitor.expr(value),
                _ => {}
            }
        }
    }

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
            foreign_pub_type_remappings: HashMap::new(),
            emit_zen_in_main: false,
            declared_crate_names: None,
            provider_plan: None,
            strict_generated_lints: false,
            externally_reachable_items: HashSet::new(),
            externally_reachable_items_by_module: HashMap::new(),
            caller_facet_exports: BTreeSet::new(),
            caller_identity: None,
            public_ordinal_type_identities: HashMap::new(),
            preserve_dependency_public_items: true,
            public_typecheck_module_paths: HashSet::new(),
            registry_package_identity: None,
            canonical_emission_package_identity: None,
            root_source_module_name: None,
            standard_library_source: false,
            stdlib_cache: StdlibAstCache::new(),
            prechecked_main_type_info: None,
            prechecked_dependency_type_info: HashMap::new(),
            implementation_bound_requirements: Vec::new(),
            inherent_bounds: Vec::new(),
            callable_bounds: Vec::new(),
            metadata_root_module_path: None,
            publication_api: None,
            publication_identities: Default::default(),
            publication_package_name: String::new(),
            emitted_union_definitions: HashMap::new(),
            native_union_origins: HashMap::new(),
            emitted_declaration_types: Vec::new(),
            native_unions: Vec::new(),
            #[cfg(feature = "rust_inspect")]
            rust_inspect_manifest_dir: None,
        }
    }

    /// Select checked public names for `caller::incan` and bind their inspectable artifact identity.
    ///
    /// The driver validates representability before calling this method. Emission only re-exports the already emitted
    /// definitions, preserving one Rust definition and the source spelling for every selected item.
    pub fn with_caller_facet(mut self, exports: impl IntoIterator<Item = String>, identity: CallerIdentity) -> Self {
        self.caller_facet_exports.extend(exports);
        self.externally_reachable_items
            .extend(self.caller_facet_exports.iter().cloned());
        self.caller_identity = Some(identity);
        self
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
        checker: &mut incan_frontend::typechecker::TypeChecker,
        dependencies: &[(&str, &Program, Option<Vec<String>>)],
    ) {
        for (name, _, path_segments) in dependencies {
            if let Some(path_segments) = path_segments {
                checker
                    .register_dependency_module_path_segments(name, canonicalize_source_module_segments(path_segments));
            }
        }
    }

    /// Supply the checked publication surface before generating the Rust whose representations it will describe.
    pub fn set_publication_api(&mut self, api: Option<incan_frontend::api_metadata::CheckedApiMetadataPackage>) {
        self.publication_api = api;
    }

    /// Supply the exact public declaration identities used to bind producer-local native payloads.
    pub fn set_publication_identities(
        &mut self,
        package_name: String,
        identities: incan_frontend::library_manifest::LibraryIdentityGraph,
    ) {
        self.publication_identities = identities;
        self.publication_package_name = package_name;
    }

    /// Capture module-scoped public representations after the emitter has finalized its native wrapper table.
    fn capture_native_union_metadata(
        &mut self,
        emitter: &IrEmitter<'_>,
        path: &[String],
        program: &IrProgram,
    ) -> Result<(), EmitError> {
        self.emitted_union_definitions
            .extend(emitter.emitted_native_union_types());
        let Some(module) = self
            .publication_api
            .as_ref()
            .and_then(|api| api.modules.iter().find(|module| module.module_path == path))
        else {
            return Ok(());
        };
        let origins = self.native_union_origins.get(path).cloned().unwrap_or_default();
        let (declarations, definitions) = emitter.capture_native_union_metadata(
            module,
            program,
            &self.emitted_union_definitions,
            &origins,
            &self.publication_package_name,
            &self.publication_identities,
        )?;
        self.provider_rust_bridge_roots.extend(
            declarations
                .iter()
                .flat_map(|declaration| declaration.bridge_roots.iter().cloned()),
        );
        self.emitted_declaration_types.extend(declarations);
        for definition in definitions {
            let Some(existing) = self
                .native_unions
                .iter_mut()
                .find(|existing| existing.rust_name == definition.rust_name)
            else {
                self.native_unions.push(definition);
                continue;
            };
            merge_native_union_capture(existing, definition)?;
        }
        self.native_unions
            .sort_by(|left, right| left.rust_name.cmp(&right.rust_name));
        Ok(())
    }

    /// Capture bound-bearing implementation headers from one inferred IR module.
    fn capture_implementation_bound_requirements(&mut self, module_path: Vec<String>, program: &IrProgram) {
        for requirement in crate::trait_bound_inference::collect_local_implementation_bound_requirements(program) {
            let target_visibility = program
                .declarations
                .iter()
                .find_map(|declaration| match &declaration.kind {
                    incan_ir::decl::IrDeclKind::Struct(target) if target.name == requirement.target_type => {
                        Some(target.visibility)
                    }
                    incan_ir::decl::IrDeclKind::Enum(target) if target.name == requirement.target_type => {
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

    /// Capture inferred inherent-implementation headers and method generics for compiled-library consumers (#1819).
    fn capture_inherent_bounds(&mut self, module_path: Vec<String>, program: &IrProgram) {
        let aliases = import_alias_names(program);
        for declaration in &program.declarations {
            let IrDeclKind::Impl(implementation) = &declaration.kind else {
                continue;
            };
            if implementation.trait_name.is_some() {
                continue;
            }
            let public_target = program.declarations.iter().any(|candidate| match &candidate.kind {
                IrDeclKind::Struct(target) => {
                    target.name == implementation.target_type && !matches!(target.visibility, Visibility::Private)
                }
                _ => false,
            });
            if !public_target {
                continue;
            }
            let source_method_name = |emitted_name: &str| {
                program
                    .member_projections
                    .iter()
                    .find_map(|(owner, source_name, identity)| {
                        (owner == &implementation.target_type
                            && incan_semantics_core::encode_incan_symbol_identity(identity) == emitted_name)
                            .then(|| source_name.clone())
                    })
                    .unwrap_or_else(|| emitted_name.to_string())
            };
            let captured = CapturedInherentBounds {
                module_path: module_path.clone(),
                target_type: implementation.target_type.clone(),
                owner_type_params: unaliased_type_params(&implementation.type_params, &aliases),
                methods: implementation
                    .methods
                    .iter()
                    .map(|method| {
                        (
                            source_method_name(&method.name),
                            unaliased_type_params(&method.type_params, &aliases),
                        )
                    })
                    .collect(),
            };
            if !self.inherent_bounds.contains(&captured) {
                self.inherent_bounds.push(captured);
            }
        }
        self.inherent_bounds.sort_by(|left, right| {
            left.module_path
                .cmp(&right.module_path)
                .then(left.target_type.cmp(&right.target_type))
        });
    }

    /// Capture inferred bounds on exported free functions and trait methods (#1826).
    fn capture_callable_bounds(&mut self, module_path: Vec<String>, program: &IrProgram) {
        let aliases = import_alias_names(program);
        for declaration in &program.declarations {
            match &declaration.kind {
                IrDeclKind::Function(function) if !matches!(function.visibility, Visibility::Private) => {
                    let name = program
                        .function_registry
                        .source_name(&function.name)
                        .unwrap_or(&function.name)
                        .to_string();
                    let captured = CapturedCallableBounds::Function {
                        module_path: module_path.clone(),
                        name,
                        type_params: unaliased_type_params(&function.type_params, &aliases),
                    };
                    if !self.callable_bounds.contains(&captured) {
                        self.callable_bounds.push(captured);
                    }
                }
                IrDeclKind::Trait(trait_decl) if !matches!(trait_decl.visibility, Visibility::Private) => {
                    for method in &trait_decl.methods {
                        let captured = CapturedCallableBounds::TraitMethod {
                            module_path: module_path.clone(),
                            trait_name: trait_decl.name.clone(),
                            method_name: method.name.clone(),
                            type_params: unaliased_type_params(&method.type_params, &aliases),
                        };
                        if !self.callable_bounds.contains(&captured) {
                            self.callable_bounds.push(captured);
                        }
                    }
                }
                _ => {}
            }
        }
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
        foreign_type_routes: &HashMap<String, HashMap<String, String>>,
    ) -> Result<(), EmitError> {
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
                            incan_frontend::api_metadata::ApiDeclaration::Enum(enum_) => Some(enum_.name.clone()),
                            _ => None,
                        }),
                );
            }
        }
        emitter.set_dependency_enum_types(enum_type_names);
        if let Some(plan) = provider_plan {
            emitter.seed_public_dependency_nominal_metadata(
                plan.library_manifest_index(),
                foreign_type_routes,
                Some(plan),
            )?;
            for provider in plan.active_sdk_records() {
                if let Some(manifest) = provider.manifest.as_deref() {
                    emitter.seed_sdk_provider_manifest_metadata(
                        manifest,
                        Some(plan),
                        foreign_type_routes.get(&manifest.name),
                    )?;
                }
            }
        }
        Ok(())
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

    /// Mark the programs being generated as modules of the standard library, checked under the standard library's own
    /// rules, for a module generated without its `std.` module path. This typed publisher identity also restores the
    /// public stdlib mount during lowering, including the callable native-function bridge. Ordinary user modules
    /// must leave this flag false.
    pub fn set_standard_library_source(&mut self, standard_library_source: bool) {
        self.standard_library_source = standard_library_source;
    }

    /// Set dependency module paths that should typecheck with public source import rules.
    ///
    /// CLI test batches can emit individual test files as generated dependency modules so each file keeps its own Rust
    /// module scope. Those test files are still user source and must typecheck like focused `incan test file.incn`
    /// runs, not like compiler-internal source dependencies that may inspect private module items.
    pub fn set_public_typecheck_module_paths(&mut self, paths: HashSet<Vec<String>>) {
        self.public_typecheck_module_paths = paths;
    }

    /// Seed codegen with stdlib metadata already collected by an earlier typecheck phase, binding any selected plan.
    pub fn set_stdlib_cache(&mut self, cache: StdlibAstCache) {
        self.stdlib_cache = cache;
        if let Some(plan) = &self.provider_plan {
            self.stdlib_cache.bind_provider_plan(plan);
        }
    }

    /// Supply the checked lowering inputs owned by one compilation session.
    ///
    /// Production command paths use this to prevent lowering from rechecking source after diagnostics and semantic
    /// facts have already been produced.
    pub fn set_prechecked_type_info(&mut self, main: TypeCheckInfo, dependencies: HashMap<Vec<String>, TypeCheckInfo>) {
        self.prechecked_main_type_info = Some(main);
        self.prechecked_dependency_type_info = dependencies;
    }

    /// Return session-owned facts for one dependency module when supplied.
    fn prechecked_dependency_type_info(&self, path: &[String]) -> Option<TypeCheckInfo> {
        self.prechecked_dependency_type_info.get(path).cloned()
    }

    /// Set declared Rust crate names from `loaf.toml [rust-dependencies]`. (RFC 031)
    ///
    /// This is used for validating `rust.module()` paths during the internal typechecking that precedes IR lowering.
    pub fn set_declared_crate_names(&mut self, names: HashSet<String>) {
        self.declared_crate_names = Some(names);
    }

    /// Set the consumer-side library manifest index for focused `pub::` tests and embedding adapters.
    pub fn set_library_manifest_index(&mut self, index: LibraryManifestIndex) {
        self.set_provider_plan(Arc::new(ProviderPlan::for_library_index(index)));
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
        self.set_provider_plan(Arc::new(ProviderPlan::for_in_memory_sdk_manifest(
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
        self.set_provider_plan(Arc::new(ProviderPlan::for_in_memory_sdk_modules(
            library_index,
            module_paths,
        )));
    }

    /// Set the immutable provider plan shared across every compiler stage.
    ///
    /// This binds existing metadata to the plan's source policy; a refusal propagates from generation's error boundary.
    pub fn set_provider_plan(&mut self, plan: Arc<ProviderPlan>) {
        self.stdlib_cache.bind_provider_plan(&plan);
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
    fn configure_typechecker(&self, tc: &mut incan_frontend::typechecker::TypeChecker, module_path: Option<&[String]>) {
        tc.stdlib_cache = self.stdlib_cache.clone();
        tc.set_standard_library_source(self.standard_library_source);
        let package_identity = incan_frontend::module::declaration_package_identity(
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

    /// Revalidate retained source authority before generation and before handing emitted results to the caller.
    fn verify_retained_source_inputs(&self) -> Result<(), GenerationError> {
        self.stdlib_cache.verify_retained_sources().map_err(|error| {
            GenerationError::TypeCheck(vec![CompileError::type_error(
                format!("retained standard source metadata refused: {error}"),
                Default::default(),
            )])
        })
    }

    /// Prefix internal codegen typecheck diagnostics with the module being lowered.
    fn typecheck_errors_for_module(module: &str, mut errors: Vec<CompileError>) -> GenerationError {
        for error in &mut errors {
            error.message = format!("in module `{module}`: {}", error.message);
        }
        GenerationError::TypeCheck(errors)
    }

    /// Preserve stdlib metadata warmed by an internal typechecker pass for later codegen passes.
    fn capture_typechecker_stdlib_cache(&mut self, tc: &incan_frontend::typechecker::TypeChecker) {
        self.stdlib_cache = tc.stdlib_cache.clone();
    }

    /// Apply codegen's shared metadata context to one AST lowering pass.
    fn configure_lowering(&self, lowering: &mut AstLowering) {
        lowering.set_stdlib_cache(self.stdlib_cache.clone());
        lowering.set_provider_plan(self.provider_plan.clone());
        // The native publisher carries trusted SDK source identity in typed codegen context. Legacy release seeds
        // retain their explicit provider markers; ordinary caller modules acquire neither form of this privilege.
        lowering.set_sdk_provider_build(
            self.standard_library_source
                || env::var_os(SDK_PROVIDER_BUILD_ENV).is_some()
                || env::var_os(OVEN_LOAF_ENV).is_some(),
        );
        lowering.set_registry_package_identity(self.registry_package_identity.clone());
        lowering.set_source_module_rust_paths(source_module_rust_paths(
            self.dependency_modules
                .iter()
                .filter_map(|(name, program, path_segments)| {
                    let rust_path = path_segments.clone().unwrap_or_else(|| vec![(*name).to_string()]);
                    source_module_identity_path(program, path_segments.clone(), Some(name))
                        .map(|module_path| (module_path, rust_path))
                }),
            self.canonical_emission_package_identity.as_deref(),
        ));
        lowering.set_crate_nominal_context(Some(Arc::new(self.crate_nominal_context())));
    }

    /// Collect the nominal facts every module's lowering shares, from the modules this generator emits into one crate.
    ///
    /// The crate is the root program, emitted at the crate root, and each dependency module, emitted at its path. Each
    /// module is keyed by every logical path its checked identities can name it by. Metadata-only symbol modules are
    /// not emitted here and take no part, so a name they share with a crate module leaves that module's unions
    /// unchanged (#1796).
    fn crate_nominal_context(&self) -> incan_ir::lower::CrateNominalContext {
        let mut modules = Vec::new();
        if let Some(root) = self.current_program {
            let root_logical_paths = [
                source_module_identity_path(
                    root,
                    self.root_source_module_name
                        .as_deref()
                        .map(|name| name.split('.').map(str::to_owned).collect()),
                    None,
                ),
                self.metadata_root_module_path
                    .as_deref()
                    .map(canonicalize_source_module_segments),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            modules.push((root_logical_paths, Vec::new(), root));
        }
        for (name, ast, path_segments) in &self.dependency_modules {
            let rust_path = self
                .source_dependency_module_paths
                .iter()
                .find_map(|(source, path)| std::ptr::eq(*source, *ast).then_some(path.clone()))
                .or_else(|| path_segments.clone())
                .unwrap_or_else(|| vec![(*name).to_string()]);
            let logical_paths = [
                Some(canonicalize_source_module_segments(&rust_path)),
                source_module_identity_path(ast, path_segments.clone(), Some(*name)),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            modules.push((logical_paths, rust_path, *ast));
        }
        incan_ir::lower::CrateNominalContext::from_modules(modules)
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
                use incan_frontend::typechecker::TypeChecker;
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
            self.stdlib_cache = lowering.stdlib_cache.clone();
            programs.push((path_segments.clone(), ir));
        }
        Ok(programs)
    }

    /// Backfill nested module path segments for a dependency module by name.
    ///
    /// This is primarily used by tests or older call sites that only registered a flat module name via `add_module()`.
    /// If a matching module entry exists and has no path segments yet, this sets them.
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
    pub fn collect_external_rust_functions(&mut self, program: &Program) {
        use incan_frontend::ast::{Declaration, ImportKind};

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
    /// This remains an internal compatibility hook because serde-backed derives and legacy `json_stringify` usage can
    /// still require serde emission without import-activated provider metadata.
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

    /// Publish the checked crate roots required by public API nominal types and class-field Rust identities.
    ///
    /// Consumer-generated declarations cannot name a transitive Cargo dependency directly. Library-mode crates expose
    /// only roots selected from checked public API types and class layouts, including providers that own inherited
    /// fields. Ordinary application builds remain unchanged.
    fn attach_provider_rust_dependency_bridge(&self, main_code: String) -> String {
        if !self.preserve_dependency_public_items {
            return main_code;
        }
        let mut crates = self
            .provider_rust_bridge_roots
            .iter()
            .filter(|crate_name| !incan_frontend::rust_type_display::is_shared_rust_crate(crate_name))
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

    /// Attach the selected caller namespace and its top-level infrastructure identity record.
    ///
    /// Each selected item is re-exported from `crate::<name>`, its root declaration, so the caller namespace adds no
    /// second definition. The identity record sits at the top level of `caller`, which projected items never reach.
    fn attach_caller_facet(&self, main_code: String) -> Result<String, GenerationError> {
        let Some(identity) = self.caller_identity.as_ref() else {
            return Ok(main_code);
        };
        let names = self
            .caller_facet_exports
            .iter()
            .map(|name| {
                syn::parse_str::<syn::Ident>(name).map_err(|error| {
                    GenerationError::Emission(EmitError::SynParse(format!(
                        "caller export `{name}` is not a Rust identifier: {error}"
                    )))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let CallerIdentity {
            package_name,
            package_version,
            caller_facet_id,
            caller_abi_version,
            compiler_version_range,
            manifest_schema_version,
            target,
            profile,
            receipt_reference,
        } = identity;
        let tokens = quote::quote! {
            pub mod caller {
                #[derive(Debug, Clone, Copy)]
                pub struct Identity {
                    pub package_name: &'static str,
                    pub package_version: &'static str,
                    pub caller_facet_id: &'static str,
                    pub caller_abi_version: &'static str,
                    pub compiler_version_range: &'static str,
                    pub manifest_schema_version: u32,
                    pub target: &'static str,
                    pub profile: &'static str,
                    pub receipt_reference: &'static str,
                }
                pub const IDENTITY: Identity = Identity {
                    package_name: #package_name,
                    package_version: #package_version,
                    caller_facet_id: #caller_facet_id,
                    caller_abi_version: #caller_abi_version,
                    compiler_version_range: #compiler_version_range,
                    manifest_schema_version: #manifest_schema_version,
                    target: #target,
                    profile: #profile,
                    receipt_reference: #receipt_reference,
                };
                pub mod incan {
                    #(pub use crate::#names;)*
                }
            }
        };
        let file = syn::parse2::<syn::File>(tokens)
            .map_err(|error| GenerationError::Emission(EmitError::SynParse(error.to_string())))?;
        Ok(format!("{main_code}\n{}", prettyplease::unparse(&file)))
    }

    /// Accumulate the exact provider and Rust crate roots required by checked public API types and class layouts.
    fn collect_provider_rust_bridge_roots(&mut self, type_info: &TypeCheckInfo) -> Result<(), GenerationError> {
        for (library, routes) in &type_info.declarations.foreign_pub_type_remappings {
            self.foreign_pub_type_remappings
                .entry(library.clone())
                .or_default()
                .extend(routes.clone());
        }
        if !self.preserve_dependency_public_items {
            return Ok(());
        }
        self.provider_rust_bridge_roots
            .extend(type_info.declarations.public_type_bridge_roots.iter().cloned());
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
                let roots = incan_frontend::rust_type_display::public_bridge_roots(&field.ty, &layout.type_params)
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
    /// **Note**: This is a convenience method that returns error comments on failure. For production use, prefer
    /// [`try_generate`](Self::try_generate) which returns a proper `Result`.
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
    /// use incan_emit::IrCodegen;
    ///
    /// let codegen = IrCodegen::new();
    /// let rust_code = codegen.try_generate(&ast)?;
    /// ```
    #[tracing::instrument(skip_all)]
    pub fn try_generate(mut self, program: &'a Program) -> Result<String, GenerationError> {
        self.try_generate_internal(program)
    }

    /// Generate one library root and return the compiler metadata inferred from the same lowered IR.
    pub fn try_generate_with_metadata(
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
                inherent_bounds: std::mem::take(&mut self.inherent_bounds),
                callable_bounds: std::mem::take(&mut self.callable_bounds),
                emitted_declaration_types: std::mem::take(&mut self.emitted_declaration_types),
                native_unions: std::mem::take(&mut self.native_unions),
                provider_plan: self.provider_plan.clone(),
            },
        ))
    }

    /// Internal implementation of try_generate (takes &mut self)
    fn try_generate_internal(&mut self, program: &'a Program) -> Result<String, GenerationError> {
        self.verify_retained_source_inputs()?;
        self.current_program = Some(program);
        self.implementation_bound_requirements.clear();
        self.inherent_bounds.clear();
        self.callable_bounds.clear();

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
        let code = self.try_generate_via_ir(program, &HashSet::new())?;
        let code = self.attach_provider_rust_dependency_bridge(code);
        let code = self.attach_caller_facet(code)?;
        self.verify_retained_source_inputs()?;
        Ok(code)
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
            use incan_frontend::typechecker::TypeChecker;
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
        // This module is emitted at the crate root, so its items are reached through the root's empty path.
        for origin in source_module_origins(
            &root_module_path.clone().unwrap_or_default(),
            self.canonical_emission_package_identity.as_deref(),
        ) {
            lowering.add_source_module_rust_path(origin, Vec::new());
        }
        lowering.set_current_source_module_name(root_module_path.as_ref().map(|path| path.join(".")));
        lowering.seed_dependency_trait_decls(&dependency_modules)?;
        lowering.seed_struct_field_aliases(global_aliases.clone());
        let mut ir_program = lowering.lower_program(program)?;
        self.stdlib_cache = lowering.stdlib_cache.clone();
        if self.needs_serde {
            add_serde_to_newtypes(&mut ir_program, needs_serialize, needs_deserialize);
        }

        // RFC 023: Infer trait bounds for generic functions.
        crate::trait_bound_inference::infer_trait_bounds(&mut ir_program);
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
                use incan_frontend::typechecker::TypeChecker;
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
                            .and_then(incan_frontend::module::logical_module_name_from_source_path)
                    }),
            );
            dep_lowering.seed_dependency_trait_decls(&dependency_modules)?;
            dep_lowering.seed_struct_field_aliases(global_aliases.clone());
            let mut dep_ir = dep_lowering.lower_program(dep_ast)?;
            self.stdlib_cache = dep_lowering.stdlib_cache.clone();
            crate::trait_bound_inference::infer_trait_bounds(&mut dep_ir);
            let module_path = canonical_dep_path_segments.unwrap_or_else(|| vec![dep_name.to_string()]);
            dependency_ir_programs.push((module_path, dep_ir));
        }
        let dependency_programs = dependency_ir_programs
            .iter()
            .map(|(_, dep_ir)| dep_ir)
            .collect::<Vec<_>>();
        Self::complete_external_call_signatures(
            &mut ir_program,
            dependency_ir_programs
                .iter()
                .map(|(module_path, dep_ir)| (module_path.as_slice(), dep_ir)),
        );
        crate::trait_bound_inference::propagate_trait_bounds_from_programs(&mut ir_program, &dependency_programs);
        let root_module_path = self.metadata_root_module_path.clone().unwrap_or_else(|| {
            ir_program
                .source_module_name
                .as_deref()
                .map(source_module_path_segments)
                .unwrap_or_default()
        });
        self.native_union_origins
            .insert(root_module_path.clone(), lowering.native_publication_origins());
        self.capture_implementation_bound_requirements(root_module_path.clone(), &ir_program);
        self.capture_inherent_bounds(root_module_path.clone(), &ir_program);
        self.capture_callable_bounds(root_module_path.clone(), &ir_program);
        for (module_path, dependency_program) in &dependency_ir_programs {
            self.capture_implementation_bound_requirements(module_path.clone(), dependency_program);
            self.capture_inherent_bounds(module_path.clone(), dependency_program);
            self.capture_callable_bounds(module_path.clone(), dependency_program);
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
            inner.set_native_nominal_origins(
                self.native_union_origins
                    .get(&root_module_path)
                    .cloned()
                    .unwrap_or_default(),
            );
            inner.set_internal_module_roots(internal_module_roots.clone());
            Self::configure_source_import_paths(inner, ir_program.source_module_name.as_deref(), &source_module_paths);
            if self.emit_zen_in_main {
                inner.set_emit_zen(true);
            }
            Self::apply_dependency_symbol_metadata(
                inner,
                &dependency_symbol_metadata,
                self.provider_plan.as_deref(),
                &self.foreign_pub_type_remappings,
            )?;
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
            let code = svc.emit_program(&ir_program)?;
            self.capture_native_union_metadata(svc.inner_mut(), &root_module_path, &ir_program)?;
            Ok(code)
        } else {
            let mut emitter = IrEmitter::new(&ir_program.function_registry);
            self.apply_canonical_emission_context(&mut emitter);
            emitter.set_native_nominal_origins(
                self.native_union_origins
                    .get(&root_module_path)
                    .cloned()
                    .unwrap_or_default(),
            );
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
                &self.foreign_pub_type_remappings,
            )?;
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
            let code = emitter.emit_program(&ir_program)?;
            self.capture_native_union_metadata(&emitter, &root_module_path, &ir_program)?;
            Ok(code)
        }
    }

    /// Generate Rust code for a dependency module (not the main module)
    ///
    /// **Note**: This is a convenience method that returns error comments on failure. For production use, prefer
    /// [`try_generate_module`](Self::try_generate_module).
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
        self.verify_retained_source_inputs()?;
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
            use incan_frontend::typechecker::TypeChecker;
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
        self.stdlib_cache = lowering.stdlib_cache.clone();

        // RFC 023: Infer trait bounds for generic functions.
        crate::trait_bound_inference::infer_trait_bounds(&mut ir_program);
        let mut dependency_ir_programs = Vec::new();
        for (dep_name, dep_ast, dep_path_segments) in dependency_modules.clone() {
            if dep_name == module_name {
                continue;
            }
            let dep_key = Self::dependency_module_key(dep_name, &dep_path_segments);
            let dep_identity_path = source_module_identity_path(dep_ast, dep_path_segments.clone(), Some(dep_name));
            let dep_type_info = {
                use incan_frontend::typechecker::TypeChecker;
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
            self.stdlib_cache = dep_lowering.stdlib_cache.clone();
            crate::trait_bound_inference::infer_trait_bounds(&mut dep_ir);
            let dep_module_path = dep_identity_path.unwrap_or_else(|| vec![dep_name.to_string()]);
            dependency_ir_programs.push((dep_module_path, dep_ir));
        }
        let reachable_items = self.externally_reachable_items.clone();
        Self::complete_external_call_signatures(
            &mut ir_program,
            dependency_ir_programs
                .iter()
                .map(|(module_path, dep_ir)| (module_path.as_slice(), dep_ir)),
        );
        let dependency_programs = dependency_ir_programs
            .iter()
            .map(|(_, dep_ir)| dep_ir)
            .collect::<Vec<_>>();
        crate::trait_bound_inference::propagate_trait_bounds_from_programs(&mut ir_program, &dependency_programs);

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
            inner.set_externally_reachable_items(reachable_items);
            self.apply_capability_bridge_configs(inner, &ordinal_bridge, &string_try_from_bridge);
            let code = svc.emit_program(&ir_program)?;
            self.verify_retained_source_inputs()?;
            Ok(code)
        } else {
            let mut emitter = IrEmitter::new(&ir_program.function_registry);
            self.apply_canonical_emission_context(&mut emitter);
            emitter.set_internal_module_roots(internal_roots);
            if self.emit_zen_in_main {
                emitter.set_emit_zen(true);
            }
            emitter.set_needs_serde(self.needs_serde);
            emitter.set_externally_reachable_items(reachable_items);
            self.apply_capability_bridge_configs(&mut emitter, &ordinal_bridge, &string_try_from_bridge);
            let code = emitter.emit_program(&ir_program)?;
            self.verify_retained_source_inputs()?;
            Ok(code)
        }
    }

    /// Generate Rust code for a multi-file project
    ///
    /// **Note**: This is a convenience method that returns error comments on failure. For production use, prefer
    /// [`try_generate_multi_file`](Self::try_generate_multi_file).
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
    /// Returns `GenerationError::TypeCheck` if checking or retained source validation refuses,
    /// `GenerationError::Lowering` if AST lowering fails, or `GenerationError::Emission` if IR emission fails.
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
        self.verify_retained_source_inputs()?;
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
        let mut dependency_reachable_items = collect_externally_reachable_items_by_module_with_cache(
            program,
            &dependency_modules,
            &mut self.stdlib_cache,
        );

        // Generate module files
        let mut lowered_modules = Vec::new();
        let mut default_constructed_types = HashSet::new();
        for (name, ast, path_segments) in dependency_modules.clone() {
            if !module_names.contains(&name) {
                continue;
            }
            let module_identity_path = source_module_identity_path(ast, path_segments.clone(), Some(name));
            let module_type_info = {
                use incan_frontend::typechecker::TypeChecker;
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
            self.stdlib_cache = lowering.stdlib_cache.clone();
            // Do not auto-add serde derives to dependency modules. Global serde usage in the main module must not
            // mutate unrelated dependency newtypes (e.g., stdlib wrapper types like std.web.request.Query/Path).
            crate::trait_bound_inference::infer_trait_bounds(&mut ir);
            record_direct_generated_path_support_items_from_ir(&mut dependency_reachable_items, &ir);
            let module_path = path_segments.clone().unwrap_or_else(|| vec![name.to_string()]);
            record_default_path_items_from_ir(&mut dependency_reachable_items, &module_path, &ir);
            default_constructed_types.extend(lowering.default_constructed_foreign_types().iter().cloned());
            self.source_dependency_module_paths.push((ast, module_path.clone()));
            lowered_modules.push((name.to_string(), module_path, ir));
        }
        publish_default_constructed_fields(
            lowered_modules
                .iter_mut()
                .map(|(_, module_path, ir)| (module_path.as_slice(), ir)),
            &default_constructed_types,
        );
        for idx in 0..lowered_modules.len() {
            let (left, rest) = lowered_modules.split_at_mut(idx);
            let Some((current_ir, tail)) = rest.split_first_mut().map(|((_, _, ir), tail)| (ir, tail)) else {
                continue;
            };
            Self::complete_external_call_signatures(
                current_ir,
                left.iter()
                    .chain(tail.iter())
                    .map(|(_, module_path, ir)| (module_path.as_slice(), ir)),
            );
            let external_programs: Vec<&incan_ir::IrProgram> = left
                .iter()
                .map(|(_, _, ir)| ir)
                .chain(tail.iter().map(|(_, _, ir)| ir))
                .collect();
            crate::trait_bound_inference::propagate_trait_bounds_from_programs(current_ir, &external_programs);
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
                    &self.foreign_pub_type_remappings,
                )?;
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
                    &self.foreign_pub_type_remappings,
                )?;
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

        let main_code = self.attach_provider_rust_dependency_bridge(main_code);
        let main_code = self.attach_caller_facet(main_code)?;
        self.verify_retained_source_inputs()?;
        Ok((main_code, modules))
    }

    /// Generate Rust code for a multi-file project with nested module paths
    ///
    /// **Note**: This is a convenience method that returns error comments on failure. For production use, prefer
    /// [`try_generate_multi_file_nested`](Self::try_generate_multi_file_nested).
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
    /// Returns `GenerationError::TypeCheck` if checking or retained source validation refuses,
    /// `GenerationError::Lowering` if AST lowering fails, or `GenerationError::Emission` if IR emission fails.
    pub fn try_generate_multi_file_nested(
        mut self,
        program: &'a Program,
        module_paths: &[Vec<String>],
    ) -> Result<(String, HashMap<Vec<String>, String>), GenerationError> {
        self.try_generate_multi_file_nested_internal(program, module_paths)
    }

    /// Generate a nested library project and return metadata inferred from the same lowered IR.
    pub fn try_generate_multi_file_nested_with_metadata(
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
                inherent_bounds: std::mem::take(&mut self.inherent_bounds),
                callable_bounds: std::mem::take(&mut self.callable_bounds),
                emitted_declaration_types: std::mem::take(&mut self.emitted_declaration_types),
                native_unions: std::mem::take(&mut self.native_unions),
                provider_plan: self.provider_plan.clone(),
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
        self.verify_retained_source_inputs()?;
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
        let mut dependency_reachable_items = collect_externally_reachable_items_by_module_with_cache(
            program,
            &dependency_modules,
            &mut self.stdlib_cache,
        );

        // Generate module files by path
        let mut lowered_modules = Vec::new();
        let mut default_constructed_types = HashSet::new();
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
                    use incan_frontend::typechecker::TypeChecker;
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
                self.stdlib_cache = lowering.stdlib_cache.clone();
                self.native_union_origins
                    .insert(path.clone(), lowering.native_publication_origins());
                // Do not auto-add serde derives to dependency modules. Global serde usage in the main module must not
                // mutate unrelated dependency newtypes (e.g., stdlib wrapper types like std.web.request.Query/Path).
                crate::trait_bound_inference::infer_trait_bounds(&mut ir);
                record_direct_generated_path_support_items_from_ir(&mut dependency_reachable_items, &ir);
                record_default_path_items_from_ir(&mut dependency_reachable_items, path, &ir);
                default_constructed_types.extend(lowering.default_constructed_foreign_types().iter().cloned());
                self.source_dependency_module_paths.push((ast, path.clone()));
                lowered_modules.push((path.clone(), ir));
            }
        }
        publish_default_constructed_fields(
            lowered_modules.iter_mut().map(|(path, ir)| (path.as_slice(), ir)),
            &default_constructed_types,
        );
        for idx in 0..lowered_modules.len() {
            let (left, rest) = lowered_modules.split_at_mut(idx);
            let Some((current_ir, tail)) = rest.split_first_mut().map(|((_, ir), tail)| (ir, tail)) else {
                continue;
            };
            Self::complete_external_call_signatures(
                current_ir,
                left.iter()
                    .chain(tail.iter())
                    .map(|(module_path, ir)| (module_path.as_slice(), ir)),
            );
            let external_programs: Vec<&incan_ir::IrProgram> = left
                .iter()
                .map(|(_, ir)| ir)
                .chain(tail.iter().map(|(_, ir)| ir))
                .collect();
            crate::trait_bound_inference::propagate_trait_bounds_from_programs(current_ir, &external_programs);
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
                inner.set_native_nominal_origins(self.native_union_origins.get(path).cloned().unwrap_or_default());
                inner.set_internal_module_roots(internal_roots.clone());
                Self::configure_source_import_paths(inner, ir.source_module_name.as_deref(), &source_module_paths);
                inner.set_preserve_public_items(preserve_public_items);
                inner.set_externally_reachable_items(reachable_items.clone());
                Self::apply_dependency_symbol_metadata(
                    inner,
                    &dependency_symbol_metadata,
                    self.provider_plan.as_deref(),
                    &self.foreign_pub_type_remappings,
                )?;
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
                let code = svc.emit_program(ir)?;
                self.capture_native_union_metadata(svc.inner_mut(), path, ir)?;
                code
            } else {
                let mut emitter = IrEmitter::new(&ir.function_registry);
                self.apply_canonical_emission_context(&mut emitter);
                emitter.set_native_nominal_origins(self.native_union_origins.get(path).cloned().unwrap_or_default());
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
                    &self.foreign_pub_type_remappings,
                )?;
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
                let code = emitter.emit_program(ir)?;
                self.capture_native_union_metadata(&emitter, path, ir)?;
                code
            };
            modules.insert(path.clone(), module_code);
        }

        let main_code = self.attach_provider_rust_dependency_bridge(main_code);
        let main_code = self.attach_caller_facet(main_code)?;
        self.verify_retained_source_inputs()?;
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
    #[cfg(feature = "rust_inspect")]
    use crate::test_support::prewarm_metadata;
    use crate::test_support::{
        assert_no_generated_unused_lint_allows, generate, must_ok, must_some, parse_program, parse_program_result,
    };
    use incan_frontend::library_manifest::{
        ConstExport, FunctionExport, LibraryManifest, ModelExport, ParamExport, ParamKindExport, TypeRef,
    };
    use incan_frontend::library_manifest_index::{
        LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
    };
    use incan_frontend::{lexer, parser};
    use incan_semantics_core::{SemanticSourceTargetKind, SymbolOrigin, encode_incan_symbol_identity};
    use incan_test_support::canonical_projection::{projected_identities, projected_identity, projected_name};
    use std::collections::HashMap;
    #[cfg(feature = "rust_inspect")]
    use std::fs;

    /// Generated crate-root Rust and source-module Rust keyed by module path.
    type GeneratedProject = (String, HashMap<Vec<String>, String>);

    #[test]
    fn selected_caller_items_are_reexported_below_reserved_top_level() -> Result<(), Box<dyn std::error::Error>> {
        let program = parse_program_result(
            "pub model Plan:\n    pub version: int\n\n\npub def make_plan() -> Plan:\n    return Plan(version=3)\n",
        )?;
        let identity = CallerIdentity {
            package_name: "policy".to_string(),
            package_version: "1.2.3".to_string(),
            caller_facet_id: "sha256:facet".to_string(),
            caller_abi_version: "1".to_string(),
            compiler_version_range: ">=0.6.0-dev.7,<0.7.0".to_string(),
            manifest_schema_version: 1,
            target: "aarch64-apple-darwin".to_string(),
            profile: "debug".to_string(),
            receipt_reference: "sha256:receipt".to_string(),
        };
        let generated = IrCodegen::new()
            .with_caller_facet(["Plan".to_string(), "make_plan".to_string()], identity)
            .try_generate(&program)?;
        let compact = generated.split_whitespace().collect::<String>();
        assert!(compact.contains("pubmodcaller{"), "{generated}");
        assert!(compact.contains("pubconstIDENTITY:Identity"), "{generated}");
        assert!(compact.contains("pubmodincan{"), "{generated}");
        assert!(compact.contains("pubusecrate::Plan;"), "{generated}");
        assert!(compact.contains("pubusecrate::make_plan;"), "{generated}");
        assert!(!compact.contains("pubmodcaller{pubusecrate::Plan"), "{generated}");
        Ok(())
    }

    /// Build lowering with the frontend-owned ownership proof for one source type annotation.
    fn lowering_with_mutable_reference_projection(
        source: &str,
        annotation: &str,
        projections: Vec<incan_frontend::typechecker::MutableRustTypeArgumentProjection>,
    ) -> Result<AstLowering, Box<dyn std::error::Error>> {
        use incan_frontend::typechecker::TypeChecker;

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

    fn generate_with_sdk_provider_modules(source: &str, modules: Vec<Vec<String>>) -> String {
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));
        let mut codegen = IrCodegen::new();
        codegen.set_sdk_provider_module_paths(modules);
        must_ok(codegen.try_generate(&ast))
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
        use incan_frontend::api_metadata::{
            CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
        };
        use incan_frontend::typechecker::TypeChecker;

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
                        .any(|bound| bound.trait_path == incan_lang::lang::trait_bounds::rust::CLONE)
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
                        .any(|bound| bound.trait_path == incan_lang::lang::trait_bounds::rust::CLONE)
            })),
            "the checked root adoption must receive its inferred implementation header"
        );
    }

    /// A public generic adopter publishes both a direct subtrait and its implied supertrait before inferred
    /// implementation bounds are attached to the manifest.
    #[test]
    fn generic_supertrait_adoption_reaches_library_manifest() -> Result<(), Box<dyn std::error::Error>> {
        use incan_frontend::api_metadata::{
            CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
        };
        use incan_frontend::library_exports::collect_checked_public_exports;
        use incan_frontend::typechecker::TypeChecker;

        let source = r#"
pub trait Catalog[T with Clone]:
    def item(self) -> T: ...

pub trait OrderedCatalog[T with Clone] with Catalog[T]:
    def ordered(self) -> Self: ...

pub model Parcel[T with Clone] with OrderedCatalog:
    pub value: T

    def item(self) -> T:
        return self.value

    def ordered(self) -> Self:
        return self
"#;
        let tokens = lexer::lex(source).map_err(|errors| format!("lex errors: {errors:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errors| format!("parse errors: {errors:?}"))?;
        let module_path = vec!["main".to_string()];
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        checker
            .check_program(&ast)
            .map_err(|errors| format!("check errors: {errors:?}"))?;
        let exports = collect_checked_public_exports(&ast, &checker);
        let mut manifest = LibraryManifest::from_checked_exports("catalogs", "0.1.0", &exports);
        manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            package: None,
            modules: vec![collect_checked_api_metadata(&ast, &checker, module_path.clone())],
            public_namespaces: Vec::new(),
        });

        let mut codegen = IrCodegen::new();
        let mut type_info = checker.type_info().clone();
        for (name, span) in [("Catalog", (0, 70)), ("OrderedCatalog", (71, 160))] {
            type_info.declarations.resolved_import_identities.insert(
                name.to_string(),
                incan_semantics_core::CanonicalSymbolId {
                    namespace: incan_semantics_core::SymbolNamespace::OrdinaryLexical,
                    origin: incan_semantics_core::SymbolOrigin::Module(vec!["lib".to_string()]),
                    declaration_name: name.to_string(),
                    kind: incan_semantics_core::SemanticSourceTargetKind::Trait,
                    scope_discriminant: None,
                    declaration_span: incan_semantics_core::HirSourceSpan::new(span.0, span.1),
                },
            );
        }
        codegen.set_prechecked_type_info(type_info, HashMap::new());
        let (_, metadata) = codegen.try_generate_with_metadata(&ast, &module_path)?;
        metadata.apply_to_library_manifest(&mut manifest).map_err(|error| {
            format!(
                "{error}; requirements={:?}; model adoptions={:?}",
                metadata.implementation_bound_requirements, manifest.exports.models[0].trait_adoptions
            )
        })?;
        let parcel = manifest
            .exports
            .models
            .iter()
            .find(|model| model.name == "Parcel")
            .ok_or("missing Parcel export")?;
        assert!(
            parcel
                .trait_adoptions
                .iter()
                .any(
                    |adoption| adoption.source_name.as_deref().unwrap_or(&adoption.name) == "Catalog"
                        && !adoption.implementation_type_params.is_empty()
                ),
            "Catalog[T] did not receive its inferred implementation bounds: {:?}",
            parcel.trait_adoptions
        );
        Ok(())
    }

    /// Typed SDK source identity reaches lowering while an ordinary same-named module receives no callable bridge.
    #[test]
    fn typed_sdk_callable_source_emits_native_function_bridge() -> Result<(), Box<dyn std::error::Error>> {
        let source = "pub trait Callable1[Arg, Return]:\n    def __call__(self, argument: Arg) -> Return: ...\n";
        let ast = parser::parse(&lexer::lex(source).map_err(|errors| format!("{errors:?}"))?)
            .map_err(|errors| format!("{errors:?}"))?;
        let module_path = vec!["traits".to_string(), "callable".to_string()];
        let mut trusted = IrCodegen::new();
        trusted.set_standard_library_source(true);
        trusted.set_root_source_module_name(Some("traits.callable".to_string()));
        let (trusted_source, _) = trusted.try_generate_with_metadata(&ast, &module_path)?;
        assert!(trusted_source.contains("__IncanCallable"), "{trusted_source}");
        let mut ordinary = IrCodegen::new();
        ordinary.set_root_source_module_name(Some("traits.callable".to_string()));
        let (ordinary_source, _) = ordinary.try_generate_with_metadata(&ast, &module_path)?;
        assert!(!ordinary_source.contains("__IncanCallable"), "{ordinary_source}");
        Ok(())
    }

    /// A standard-library module keeps its mounted module identity when publishing a direct generic trait adoption.
    #[test]
    fn stdlib_local_generic_adoption_keeps_manifest_identity() -> Result<(), Box<dyn std::error::Error>> {
        use incan_frontend::api_metadata::{
            CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
        };
        use incan_frontend::library_exports::collect_checked_public_exports;
        use incan_frontend::typechecker::TypeChecker;

        let source = r#"
pub trait Iterator[T]:
    def next(mut self) -> Option[T]: ...

pub model BatchIterator[T, Source with Iterator[T]] with Iterator[list[T]]:
    pub source: Source

    def next(mut self) -> Option[list[T]]:
        return None
"#;
        let tokens = lexer::lex(source).map_err(|errors| format!("lex errors: {errors:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errors| format!("parse errors: {errors:?}"))?;
        let module_path = vec!["derives".to_string(), "collection".to_string()];
        let mut checker = TypeChecker::new();
        checker.set_standard_library_source(true);
        checker.set_current_module_path(Some(module_path.clone()));
        checker
            .check_program(&ast)
            .map_err(|errors| format!("check errors: {errors:?}"))?;
        let exports = collect_checked_public_exports(&ast, &checker);
        let mut manifest = LibraryManifest::from_checked_exports("incan_stdlib_core", "0.1.0", &exports);
        manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            package: None,
            modules: vec![collect_checked_api_metadata(&ast, &checker, module_path.clone())],
            public_namespaces: Vec::new(),
        });

        let mut codegen = IrCodegen::new();
        codegen.set_standard_library_source(true);
        let mut type_info = checker.type_info().clone();
        let iterator = traits::as_str(traits::TraitId::Iterator).to_string();
        type_info.declarations.resolved_import_identities.insert(
            iterator.clone(),
            incan_semantics_core::CanonicalSymbolId {
                namespace: incan_semantics_core::SymbolNamespace::OrdinaryLexical,
                origin: incan_semantics_core::SymbolOrigin::Module(module_path.clone()),
                declaration_name: iterator,
                kind: incan_semantics_core::SemanticSourceTargetKind::Trait,
                scope_discriminant: None,
                declaration_span: incan_semantics_core::HirSourceSpan::new(0, 0),
            },
        );
        codegen.set_prechecked_type_info(type_info, HashMap::new());
        let (_, metadata) = codegen.try_generate_with_metadata(&ast, &module_path)?;
        metadata.apply_to_library_manifest(&mut manifest).map_err(|error| {
            format!(
                "{error}; requirements={:?}; model adoptions={:?}",
                metadata.implementation_bound_requirements, manifest.exports.models[0].trait_adoptions
            )
        })?;
        Ok(())
    }

    /// Issue #1819: compiled-library metadata publishes the inferred inherent impl header and method-generic bounds
    /// that its generated Rust requires.
    #[test]
    fn compiled_library_inherent_method_bounds_reach_manifest_issue1819() -> Result<(), Box<dyn std::error::Error>> {
        use incan_frontend::api_metadata::{
            CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
        };
        use incan_frontend::library_exports::collect_checked_public_exports;
        use incan_frontend::typechecker::TypeChecker;

        let source = r#"
pub model Holder[V]:
    pub value: V

    def get(self) -> V:
        return self.value

    def first[T](self, items: list[T]) -> T:
        return items[0]
"#;
        let tokens = lexer::lex(source).map_err(|errors| format!("lex errors: {errors:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errors| format!("parse errors: {errors:?}"))?;
        let module_path = vec!["holders".to_string()];
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        checker
            .check_program(&ast)
            .map_err(|errors| format!("check errors: {errors:?}"))?;
        let exports = collect_checked_public_exports(&ast, &checker);
        let mut manifest = LibraryManifest::from_checked_exports("holders", "0.1.0", &exports);
        manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            package: None,
            modules: vec![collect_checked_api_metadata(&ast, &checker, module_path.clone())],
            public_namespaces: Vec::new(),
        });

        let (_, metadata) = IrCodegen::new().try_generate_with_metadata(&ast, &module_path)?;
        metadata.apply_to_library_manifest(&mut manifest)?;
        let holder = manifest
            .exports
            .models
            .iter()
            .find(|model| model.name == "Holder")
            .ok_or("missing Holder export")?;
        let owner_bounds = holder
            .type_params
            .iter()
            .find(|param| param.name == "V")
            .ok_or("missing Holder.V")?;
        assert!(
            owner_bounds
                .bounds
                .iter()
                .any(|bound| bound.name == incan_lang::lang::trait_bounds::rust::CLONE && bound.inferred),
            "Holder.V must publish the inferred Clone impl-header bound: {owner_bounds:?}"
        );
        let first_bounds = holder
            .methods
            .iter()
            .find(|method| method.name == "first")
            .and_then(|method| method.type_params.iter().find(|param| param.name == "T"))
            .ok_or("missing Holder.first.T")?;
        assert!(
            first_bounds
                .bounds
                .iter()
                .any(|bound| bound.name == incan_lang::lang::trait_bounds::rust::CLONE && bound.inferred),
            "Holder.first.T must publish its inferred Clone bound: {first_bounds:?}"
        );
        Ok(())
    }

    /// Issue #1826: inferred bounds on compiled free functions and trait methods survive publication, and a consumer
    /// must declare the forwarded bound instead of reaching rustc with an under-bounded generic signature.
    #[test]
    fn compiled_callable_bounds_reach_consumer_issue1826() -> Result<(), Box<dyn std::error::Error>> {
        use incan_frontend::api_metadata::{
            CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
        };
        use incan_frontend::library_exports::collect_checked_public_exports;
        use incan_frontend::library_manifest_index::{
            LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
        };
        use incan_frontend::typechecker::TypeChecker;

        let source = r#"
pub def first[K](items: list[K]) -> K:
    return items[0]

pub trait Picker:
    def pick[K](self, items: list[K]) -> K:
        return items[0]
"#;
        let tokens = lexer::lex(source).map_err(|errors| format!("lex errors: {errors:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errors| format!("parse errors: {errors:?}"))?;
        let module_path = vec!["lib".to_string()];
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        checker.set_current_package_identity(Some("picks".to_string()));
        checker
            .check_program(&ast)
            .map_err(|errors| format!("producer check errors: {errors:?}"))?;
        let exports = collect_checked_public_exports(&ast, &checker);
        let mut manifest = LibraryManifest::from_checked_exports("picks", "0.1.0", &exports);
        manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            package: None,
            modules: vec![collect_checked_api_metadata(&ast, &checker, module_path.clone())],
            public_namespaces: Vec::new(),
        });
        let mut codegen = IrCodegen::new();
        codegen.set_prechecked_type_info(checker.type_info().clone(), HashMap::new());
        let (_, metadata) = codegen.try_generate_with_metadata(&ast, &module_path)?;
        metadata.apply_to_library_manifest(&mut manifest)?;

        let clone_bound = |type_params: &[TypeParamExport], name: &str| {
            type_params.iter().any(|param| {
                param.name == name
                    && param
                        .bounds
                        .iter()
                        .any(|bound| bound.name == incan_lang::lang::trait_bounds::rust::CLONE && bound.inferred)
            })
        };
        let first = manifest
            .exports
            .functions
            .iter()
            .find(|function| function.name == "first")
            .ok_or("missing first export")?;
        assert!(clone_bound(&first.type_params, "K"), "first.K lost Clone: {first:?}");
        let pick = manifest
            .exports
            .traits
            .iter()
            .find(|trait_export| trait_export.name == "Picker")
            .and_then(|trait_export| trait_export.methods.iter().find(|method| method.name == "pick"))
            .ok_or("missing Picker.pick export")?;
        assert!(
            clone_bound(&pick.type_params, "K"),
            "Picker.pick.K lost Clone: {pick:?}"
        );

        let index = LibraryManifestIndex::from_entries(HashMap::from([(
            "picks".to_string(),
            LibraryManifestIndexEntry::Loaded {
                manifest: Box::new(manifest),
                metadata: LibraryArtifactMetadata::from_crate_root(
                    "picks",
                    "picks",
                    std::env::temp_dir().join("issue1826_picks"),
                ),
            },
        )]));
        let consumer_source = r#"
from pub::picks import first, Picker

def head[T](items: list[T]) -> T:
    return first(items)

def chosen[T, P with Picker](picker: P, items: list[T]) -> T:
    return picker.pick(items)
"#;
        let consumer_tokens =
            lexer::lex(consumer_source).map_err(|errors| format!("consumer lex errors: {errors:?}"))?;
        let consumer =
            parser::parse(&consumer_tokens).map_err(|errors| format!("consumer parse errors: {errors:?}"))?;
        let mut consumer_checker = TypeChecker::new();
        consumer_checker.set_library_manifest_index(index);
        let Err(errors) = consumer_checker.check_program(&consumer) else {
            return Err("an under-bounded consumer must be refused before Rust emission".into());
        };
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains(incan_lang::lang::traits::as_str(
                    incan_lang::lang::traits::TraitId::Clone
                ))),
            "consumer diagnostic must name Clone: {errors:?}"
        );
        Ok(())
    }

    /// Issue #1826: source-declared bounds are not republished as inferred ones. The emitted IR spells a bound over a
    /// trait's own type parameter (`Callable1[E, F]`) with `E` as a nominal name, which an adopter's type arguments
    /// never replace, and a bound declared through an import alias by the alias, which a consumer cannot resolve; a
    /// consumer calling a default through an adopter must see only the declared bound the adoption instantiates.
    #[test]
    fn declared_trait_method_bounds_are_not_republished_as_inferred_issue1826() -> Result<(), Box<dyn std::error::Error>>
    {
        use incan_frontend::api_metadata::{
            CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
        };
        use incan_frontend::library_exports::collect_checked_public_exports;
        use incan_frontend::library_manifest_index::{
            LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
        };
        use incan_frontend::typechecker::TypeChecker;

        let source = r#"
from std.traits.callable import Callable1
from std.traits.callable import Callable1 as Mapper


pub def apply[F with Mapper[int, int]](f: F, value: int) -> int:
    return f(value)


pub trait Failing[E]:
    def failure(self) -> E: ...

    def describe[F with Clone, Describe with (Clone, Callable1[E, F])](self, f: Describe) -> F:
        return f(self.failure())


pub model Lookup with Failing[str]:
    pub key: str

    def failure(self) -> str:
        return self.key
"#;
        let tokens = lexer::lex(source).map_err(|errors| format!("lex errors: {errors:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errors| format!("parse errors: {errors:?}"))?;
        let module_path = vec!["lib".to_string()];
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        checker.set_current_package_identity(Some("failing".to_string()));
        checker
            .check_program(&ast)
            .map_err(|errors| format!("producer check errors: {errors:?}"))?;
        let exports = collect_checked_public_exports(&ast, &checker);
        let mut manifest = LibraryManifest::from_checked_exports("failing", "0.1.0", &exports);
        manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            package: None,
            modules: vec![collect_checked_api_metadata(&ast, &checker, module_path.clone())],
            public_namespaces: Vec::new(),
        });
        let mut codegen = IrCodegen::new();
        codegen.set_prechecked_type_info(checker.type_info().clone(), HashMap::new());
        let (_, metadata) = codegen.try_generate_with_metadata(&ast, &module_path)?;
        metadata.apply_to_library_manifest(&mut manifest)?;

        let describe = manifest
            .exports
            .traits
            .iter()
            .find(|trait_export| trait_export.name == "Failing")
            .and_then(|trait_export| trait_export.methods.iter().find(|method| method.name == "describe"))
            .ok_or("missing Failing.describe export")?;
        let republished = describe
            .type_params
            .iter()
            .flat_map(|param| param.bounds.iter().filter(|bound| bound.inferred))
            .collect::<Vec<_>>();
        assert!(
            republished.is_empty(),
            "declared bounds must not be republished as inferred: {republished:?}"
        );
        let apply = manifest
            .exports
            .functions
            .iter()
            .find(|function| function.name == "apply")
            .ok_or("missing apply export")?;
        let aliased = apply
            .type_params
            .iter()
            .flat_map(|param| param.bounds.iter().filter(|bound| bound.inferred))
            .collect::<Vec<_>>();
        assert!(
            aliased.is_empty(),
            "a bound declared through an import alias must not be republished as inferred: {aliased:?}"
        );

        let index = LibraryManifestIndex::from_entries(HashMap::from([(
            "failing".to_string(),
            LibraryManifestIndexEntry::Loaded {
                manifest: Box::new(manifest),
                metadata: LibraryArtifactMetadata::from_crate_root(
                    "failing",
                    "failing",
                    std::env::temp_dir().join("issue1826_failing"),
                ),
            },
        )]));
        let consumer_source = r#"
from pub::failing import Lookup


def main() -> None:
    lookup = Lookup(key="missing")
    println(lookup.describe((error) => len(error)))
"#;
        let consumer_tokens =
            lexer::lex(consumer_source).map_err(|errors| format!("consumer lex errors: {errors:?}"))?;
        let consumer =
            parser::parse(&consumer_tokens).map_err(|errors| format!("consumer parse errors: {errors:?}"))?;
        let mut consumer_checker = TypeChecker::new();
        consumer_checker.set_library_manifest_index(index);
        consumer_checker
            .check_program(&consumer)
            .map_err(|errors| format!("the adopter's default must check against its declared bounds: {errors:?}"))?;
        Ok(())
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
            ..IrGenerationMetadata::default()
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
                origin: None,
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
        assert!(!code.contains("use incan_std_core::prelude::*;"));
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

    /// #1764: an alias of a module member is an import of that member, so the module binds the projection every call
    /// through the alias names, beside the alias's own name.
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
        assert!(
            code.contains(&format!("pub use crate::__incan_std::math::{sqrt};")),
            "{code}"
        );
        assert!(
            code.contains(&format!("pub use crate::__incan_std::math::{sqrt} as root;")),
            "{code}"
        );
    }

    #[test]
    fn std_prelude_module_import_does_not_reimport_sdk_facade_root() {
        let code = generate_with_sdk_provider_modules(
            "import std.prelude\n\ndef main() -> None:\n  pass\n",
            vec![vec!["prelude".to_string()]],
        );
        assert!(!compact_rust(&code).contains("usecrate::__incan_std;"), "{code}");
    }

    #[test]
    fn aliased_std_prelude_module_import_retains_requested_binding() {
        let code = generate_with_sdk_provider_modules(
            "import std.prelude as foundation\n\ndef main() -> None:\n  pass\n",
            vec![vec!["prelude".to_string()]],
        );
        assert!(
            compact_rust(&code).contains("pubusecrate::__incan_stdasfoundation;"),
            "{code}"
        );
    }

    #[test]
    fn std_root_module_import_uses_sdk_facade() {
        for (import, binding) in [("math", "math"), ("math as arithmetic", "arithmetic")] {
            let source = format!(
                "from std import {import}\n\npub def root(value: float) -> float:\n  return {binding}.sqrt(value)\n"
            );
            let code = generate_with_sdk_provider_modules(&source, vec![vec!["math".to_string()]]);
            let compact = compact_rust(&code);
            let expected = if binding == "math" {
                "usecrate::__incan_std::math;".to_string()
            } else {
                format!("usecrate::__incan_std::mathas{binding};")
            };
            assert!(compact.contains(&expected), "{code}");
            assert!(!compact.contains("usestd::math"), "{code}");
        }
    }

    /// #1434: a module import used only by a module derive is still a use of that module, because the emitted
    /// `impl module::Trait for T` names its binding. An aliased import must therefore survive generated-use pruning
    /// under the binding the source chose; an unaliased root module needs no `use` because the project declares it.
    #[test]
    fn module_derive_retains_its_trait_module_import() -> Result<(), Box<dyn std::error::Error>> {
        let codec = parse_program("__derives__ = [Encode]\n\n@rust.derive(\"Debug\")\npub trait Encode:\n  pass\n");
        for (import, binding, expected_import) in [
            ("import codec", "codec", None),
            ("import codec as formats", "formats", Some("usecrate::codecasformats;")),
        ] {
            let main = parse_program(&format!(
                "{import}\n\n@derive({binding})\nmodel Item:\n  value: int\n\ndef main() -> None:\n  item = Item(value=1)\n  println(item.value)\n"
            ));
            let mut codegen = IrCodegen::new();
            codegen.add_module("codec", &codec);
            let (main_code, _modules) = codegen.try_generate_multi_file(&main, &["codec"])?;
            let compact = compact_rust(&main_code);
            match expected_import {
                Some(expected_import) => assert!(compact.contains(expected_import), "{import}: {main_code}"),
                None => assert!(!compact.contains("usecrate::codec"), "{import}: {main_code}"),
            }
            assert!(
                compact.contains(&format!("impl{binding}::EncodeforItem{{}}")),
                "{import}: {main_code}"
            );
        }
        Ok(())
    }

    /// #1434: a compiled-SDK derive bundle reached through a root `std` import keeps the provider-qualified import
    /// projection, so `impl toml::TomlSerialize` resolves the SDK namespace rather than the external `toml` crate.
    #[test]
    fn std_root_module_derive_retains_sdk_facade_import() {
        for (import, binding) in [("toml", "toml"), ("toml as manifest", "manifest")] {
            let source = format!(
                "from std import {import}\n\n@derive({binding})\nmodel Project:\n  name: str\n\ndef main() -> None:\n  project = Project(name=\"demo\")\n  println(project.name)\n"
            );
            let code = generate_with_sdk_provider_modules(&source, vec![vec!["toml".to_string()]]);
            let compact = compact_rust(&code);
            let expected_import = if binding == "toml" {
                "usecrate::__incan_std::toml;".to_string()
            } else {
                format!("usecrate::__incan_std::tomlas{binding};")
            };
            assert!(compact.contains(&expected_import), "{import}: {code}");
            assert!(
                compact.contains(&format!("impl{binding}::TomlSerializeforProject{{}}"))
                    && compact.contains(&format!("impl{binding}::TomlDeserializeforProject{{}}")),
                "{import}: {code}"
            );
            assert!(!compact.contains("usetoml"), "{import}: {code}");
        }
    }

    /// #1431: a facade that re-exports a compiled-SDK `std.serde.json` trait still hands the consumer the stdlib
    /// trait identity, so the serde derive is forwarded and the backend `to_json` default is emitted, while the
    /// facade itself re-exports the SDK projection the consumer's `use` resolves through.
    #[test]
    fn sdk_facade_reexported_serde_json_trait_keeps_its_protocol() -> Result<(), Box<dyn std::error::Error>> {
        let facade = parse_program("from std.serde.json import Serialize\n");
        let main = parse_program(
            "from facade import Serialize\n\nmodel Payload with Serialize:\n  value: int\n\ndef main() -> None:\n  println(Payload(value=1).to_json())\n",
        );
        let mut codegen = IrCodegen::new();
        codegen.set_sdk_provider_module_paths(vec![vec!["serde".to_string(), "json".to_string()]]);
        codegen.add_module("facade", &facade);
        let (main_code, modules) = codegen.try_generate_multi_file(&main, &["facade"])?;
        let compact = compact_rust(&main_code);
        assert!(compact.contains("serde::Serialize,"), "{main_code}");
        assert!(
            compact.contains("implSerializeforPayload{fnto_json(&self)->String"),
            "{main_code}"
        );
        assert!(compact.contains("usecrate::facade::Serialize;"), "{main_code}");
        let facade_code = modules.get("facade").ok_or("missing facade output")?;
        assert!(
            compact_rust(facade_code).contains("pubusecrate::__incan_std::serde::json::Serialize;"),
            "{facade_code}"
        );
        Ok(())
    }

    #[test]
    fn rust_std_root_module_import_keeps_rust_namespace() {
        let code =
            generate("from rust::std import cmp\n\npub def maximum(a: int, b: int) -> int:\n  return cmp.max(a, b)\n");
        let compact = compact_rust(&code);
        assert!(compact.contains("use::std::cmp;"), "{code}");
        assert!(!compact.contains("crate::__incan_std::"), "{code}");
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
        use incan_ir::decl::{
            FunctionParam, IrFunction, IrImportItem, IrImportOrigin, IrImportQualifier, IrRustTraitImport, Visibility,
        };
        use incan_ir::expr::{IrCallArg, IrCallArgKind, IrExprKind, MethodCallArgPolicy, VarAccess, VarRefKind};
        use incan_ir::{IrDecl, IrDeclKind, IrProgram, IrStmt, IrStmtKind, IrType, Mutability, TypedExpr};

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
                        methods_known: true,
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
        use incan_ir::decl::{
            FunctionParam, IrFunction, IrImportItem, IrImportOrigin, IrImportQualifier, IrRustTraitImport, IrStruct,
            IrStructKind, Visibility,
        };
        use incan_ir::expr::{IrExprKind, IrMethodDispatch, MethodCallArgPolicy, VarAccess, VarRefKind};
        use incan_ir::{IrDecl, IrDeclKind, IrProgram, IrStmt, IrStmtKind, IrType, Mutability, TypedExpr};

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
                        methods_known: true,
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
                        methods_known: true,
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
            phantom_type_params: Vec::new(),
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
                            type_args: Vec::new(),
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
                            bindings: vec![String::from("AlphaRender")],
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
        use incan_ir::decl::{FunctionParam, IrFunction, IrImportItem, IrImportOrigin, IrImportQualifier, Visibility};
        use incan_ir::expr::{IrCallArg, IrCallArgKind, IrExprKind, MethodCallArgPolicy, VarAccess, VarRefKind};
        use incan_ir::{IrDecl, IrDeclKind, IrProgram, IrStmt, IrStmtKind, IrType, Mutability, TypedExpr};

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
        use incan_ir::decl::{
            FunctionParam, IrFunction, IrImportItem, IrImportOrigin, IrImportQualifier, IrRustTraitImport, Visibility,
        };
        use incan_ir::expr::{IrCallArg, IrCallArgKind, IrExprKind, MethodCallArgPolicy, VarAccess, VarRefKind};
        use incan_ir::{IrDecl, IrDeclKind, IrProgram, IrStmt, IrStmtKind, IrType, TypedExpr};

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
                        methods_known: true,
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
            code.contains("impl incan_std_core::reflection::HasFieldValueReflection for User"),
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
            code.contains("impl incan_std_core::reflection::HasFieldValueReflection for ProbeRow"),
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
            !code.contains("impl incan_std_core::reflection::HasFieldValueReflection for Batch"),
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

    fn read_stdlib_program(path: &str) -> Result<Program, Box<dyn std::error::Error>> {
        let source = std::fs::read_to_string(oven_model::toolchain_layout::development_root().join(path))?;
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
                is_mut: false,
                name: "name".to_string(),
                ty: TypeRef::Named {
                    origin: None,
                    name: "str".to_string(),
                },
                kind: ParamKindExport::Normal,
                has_default: false,
                default: None,
            }],
            return_type: TypeRef::Named {
                origin: None,
                name: "Widget".to_string(),
            },
            is_async: false,
        });
        manifest.exports.consts.push(ConstExport {
            name: "DEFAULT_NAME".to_string(),
            ty: TypeRef::Named {
                origin: None,
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

    #[test]
    fn canonical_module_item_import_keeps_function_projection_for_both_spellings()
    -> Result<(), Box<dyn std::error::Error>> {
        let helper = parse_program("pub def value() -> int:\n  return 42\n");
        for (import, binding) in [
            ("import helper::value", "value"),
            ("import helper::value as answer", "answer"),
            ("from helper import value", "value"),
            ("from helper import value as answer", "answer"),
        ] {
            let main = parse_program(&format!(
                "{import}\n\ndef main() -> None:\n  assert {binding}() == 42\n"
            ));
            let mut codegen = IrCodegen::new();
            codegen.add_module("helper", &helper);
            let (main_code, modules) = codegen.try_generate_multi_file(&main, &["helper"])?;
            let helper_code = modules.get("helper").ok_or("missing helper output")?;
            let projection = projected_name(helper_code, "value", SemanticSourceTargetKind::Function);
            assert!(
                main_code.contains(&format!("use crate::helper::{projection}")),
                "{import}: {main_code}"
            );
            assert!(main_code.contains(&format!("{projection}()")), "{import}: {main_code}");
        }
        Ok(())
    }

    /// One provider module declaring `Product`, `Answer = Product | int` and a function over `Answer`.
    fn same_named_union_module(label: &str, field_type: &str) -> Result<Program, Box<dyn std::error::Error>> {
        parse_program_result(&format!(
            "pub model Product:\n    pub value: {field_type}\n\n\npub type Answer = Product | int\n\n\npub def label(answer: Answer) -> str:\n    match answer:\n        Product(product) => return f\"{label} product {{product.value}}\"\n        int(number) => return f\"{label} number {{number}}\"\n\n\npub def wrap(value: {field_type}) -> Answer:\n    return Product(value=value)\n"
        ))
    }

    /// Return the one crate-root wrapper definition whose first variant carries `payload`.
    fn union_wrapper_with_payload(root_code: &str, payload: &str) -> Result<String, Box<dyn std::error::Error>> {
        let compact = compact_rust(root_code);
        let marker = format!("{{V0({payload}),V1(i64),}}");
        let end = compact
            .find(&marker)
            .ok_or_else(|| format!("no wrapper carries `{payload}`:\n{root_code}"))?;
        let start = compact[..end]
            .rfind("pubenum")
            .ok_or_else(|| format!("wrapper for `{payload}` has no enum header:\n{root_code}"))?;
        Ok(compact[start + "pubenum".len()..end].to_string())
    }

    /// #1796: two modules of one package that each declare `Product` and `Answer = Product | int` get one wrapper each.
    ///
    /// The wrapper name was hashed from the members' spellings, so both modules' unions shared one crate-root wrapper
    /// whose `Product` payload could name neither declaration, and publishing the package refused to bind that one
    /// wrapper's `Product` to two declarations. A member whose spelling two modules of the crate declare is now spelled
    /// by its declaring module, so each union has its own wrapper, carrying its own module's `Product`, and each module
    /// constructs and matches through the wrapper that carries its own type.
    #[test]
    fn same_named_nominals_in_two_modules_get_one_union_wrapper_each_issue1796()
    -> Result<(), Box<dyn std::error::Error>> {
        let first = same_named_union_module("first", "int")?;
        let second = same_named_union_module("second", "str")?;
        let root = parse_program_result("pub const PRODUCER: str = \"producer\"\n")?;
        let first_path = vec!["first".to_string()];
        let second_path = vec!["second".to_string()];
        let mut codegen = IrCodegen::new();
        codegen.add_module_with_path_segments("first", &first, first_path.clone());
        codegen.add_module_with_path_segments("second", &second, second_path.clone());
        let (root_code, modules) =
            codegen.try_generate_multi_file_nested(&root, &[first_path.clone(), second_path.clone()])?;

        let first_wrapper = union_wrapper_with_payload(&root_code, "crate::first::Product")?;
        let second_wrapper = union_wrapper_with_payload(&root_code, "crate::second::Product")?;
        assert_ne!(
            first_wrapper, second_wrapper,
            "each module's union needs its own wrapper:\n{root_code}"
        );

        for (path, own, other) in [
            (&first_path, &first_wrapper, &second_wrapper),
            (&second_path, &second_wrapper, &first_wrapper),
        ] {
            let code = compact_rust(modules.get(path).ok_or("missing generated module")?);
            assert!(
                code.contains(&format!("pubtypeAnswer=crate::{own};")),
                "{path:?} must alias its own wrapper:\n{code}"
            );
            assert!(
                code.contains(&format!("crate::{own}::V0(Product{{value:value}})")),
                "{path:?} must construct its own wrapper:\n{code}"
            );
            assert!(
                code.contains(&format!("crate::{own}::V0(product)")),
                "{path:?} must match through its own wrapper:\n{code}"
            );
            assert!(
                !code.contains(other.as_str()),
                "{path:?} must not reach the other wrapper:\n{code}"
            );
        }
        Ok(())
    }

    /// #1796: a union over a nominal no other module of the crate declares keeps the wrapper name it always had, so
    /// codegen snapshots and published wrapper names are unchanged wherever no two declarations share a spelling.
    #[test]
    fn union_over_an_unshared_nominal_keeps_its_wrapper_name_issue1796() -> Result<(), Box<dyn std::error::Error>> {
        let first = same_named_union_module("first", "int")?;
        let root = parse_program_result("pub const PRODUCER: str = \"producer\"\n")?;
        let first_path = vec!["first".to_string()];
        let mut codegen = IrCodegen::new();
        codegen.add_module_with_path_segments("first", &first, first_path.clone());
        let (_, modules) = codegen.try_generate_multi_file_nested(&root, std::slice::from_ref(&first_path))?;
        let code = compact_rust(modules.get(&first_path).ok_or("missing generated module")?);
        let unchanged = incan_ir::types::IrType::NamedGeneric(
            incan_ir::types::IR_UNION_TYPE_NAME.to_string(),
            vec![
                incan_ir::types::IrType::Struct("Product".to_string()),
                incan_ir::types::IrType::Int,
            ],
        )
        .union_type_name()
        .ok_or("a union has a wrapper name")?;
        assert!(
            code.contains(&format!("pubtypeAnswer=crate::{unchanged};")),
            "the wrapper name must stay `{unchanged}`:\n{code}"
        );
        Ok(())
    }

    /// #1796: in a package build, a sibling module that imports the shared `Product` under its own name and passes one
    /// to `first.label` builds `first`'s wrapper, the one `label` accepts.
    ///
    /// A package build gives the imported declaration the package's origin rather than a module origin, and the
    /// sibling's view of `label`'s parameter spelled `Product` unqualified, so it built a third wrapper that `label`
    /// does not accept.
    #[test]
    fn a_sibling_importing_a_shared_nominal_builds_the_declaring_modules_wrapper_issue1796()
    -> Result<(), Box<dyn std::error::Error>> {
        let first = same_named_union_module("first", "int")?;
        let second = same_named_union_module("second", "str")?;
        let user = parse_program_result(
            "from first import Product, label\n\n\npub def go() -> str:\n    return label(Product(value=1))\n",
        )?;
        let root = parse_program_result("pub const PRODUCER: str = \"producer\"\n")?;
        let paths = [
            vec!["first".to_string()],
            vec!["second".to_string()],
            vec!["user".to_string()],
        ];
        let mut codegen = IrCodegen::new();
        codegen.set_registry_package_identity(Some("producer".to_string()));
        codegen.set_canonical_emission_package_identity(Some("producer".to_string()));
        codegen.add_module_with_path_segments("first", &first, paths[0].clone());
        codegen.add_module_with_path_segments("second", &second, paths[1].clone());
        codegen.add_module_with_path_segments("user", &user, paths[2].clone());
        let (root_code, modules) = codegen.try_generate_multi_file_nested(&root, &paths)?;

        let first_wrapper = union_wrapper_with_payload(&root_code, "crate::first::Product")?;
        let user_code = compact_rust(modules.get(&paths[2]).ok_or("missing generated module")?);
        assert!(
            user_code.contains(&format!("crate::{first_wrapper}::V0(Product{{value:1}})")),
            "the sibling must build `first`'s wrapper `{first_wrapper}`:\n{user_code}"
        );
        Ok(())
    }

    /// #1796: a generic nominal two modules declare keeps its spelling inside a union member, so generating the
    /// crate-root wrapper for `Holder[int] | str` does not fail on a module path where it spells one identifier.
    #[test]
    fn a_union_over_a_shared_generic_nominal_still_generates_issue1796() -> Result<(), Box<dyn std::error::Error>> {
        let holder = "pub model Holder[T]:\n    pub item: T\n";
        let first = parse_program_result(&format!(
            "{holder}\n\npub type Held = Holder[int] | str\n\n\npub def hold() -> Held:\n    return Holder(item=1)\n"
        ))?;
        let second = parse_program_result(holder)?;
        let root = parse_program_result("pub const PRODUCER: str = \"producer\"\n")?;
        let paths = [vec!["first".to_string()], vec!["second".to_string()]];
        let mut codegen = IrCodegen::new();
        codegen.add_module_with_path_segments("first", &first, paths[0].clone());
        codegen.add_module_with_path_segments("second", &second, paths[1].clone());
        let (_, modules) = codegen.try_generate_multi_file_nested(&root, &paths)?;
        let first_code = compact_rust(modules.get(&paths[0]).ok_or("missing generated module")?);
        assert!(
            first_code.contains("pubtypeHeld=crate::__IncanUnion"),
            "`Held` must alias a crate-root wrapper:\n{first_code}"
        );
        Ok(())
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
            code.contains("fn __incan_original_from_csv<\n    T: incan_std_core::reflection::HasTypeClassName")
                || code
                    .contains("fn __incan_original_from_csv<\n    T: incan_std_core::reflection::HasTypeFieldMetadata"),
            "{code}"
        );
        assert!(
            code.contains("incan_std_core::reflection::HasTypeClassName")
                && code.contains("incan_std_core::reflection::HasTypeFieldMetadata")
                && code.contains("+ Clone"),
            "{code}"
        );
    }

    #[test]
    fn subtrait_default_satisfies_supertrait_slot_issue1825() {
        let code = generate(
            r#"
trait Root:
  def label(self) -> str: ...

trait Child with Root:
  def label(self) -> str:
    return "child"

model Item with Child:
  value: int

def describe[T with Root](item: T) -> str:
  return item.label()

def main() -> None:
  println(Item(value=1).label())
  println(describe(Item(value=1)))
"#,
        );
        let compact = compact_rust(&code);
        assert!(compact.contains("implRootforItem{fnlabel(&self)->String{"), "{code}");
        assert!(!compact.contains("traitChild:Root{fnlabel"), "{code}");
        assert!(!compact.contains("implChildforItem{fnlabel"), "{code}");
    }

    #[test]
    fn class_override_replaces_inherited_dispatch_issue1841() {
        let source = r#"
class Animal:
  id: int

  def grow(self) -> int:
    return 1

  def feed(self) -> int:
    return self.grow()

class Dog extends Animal:
  def grow(self) -> int:
    return 2

def main() -> None:
  println(Dog(id=1).feed())
"#;
        let code = generate(source);
        let mut grow_identities = projected_identities(&code, "grow", SemanticSourceTargetKind::Method)
            .into_iter()
            .collect::<Vec<_>>();
        grow_identities.sort_by_key(|identity| identity.declaration_span.start);
        assert_eq!(grow_identities.len(), 1, "{code}");
        assert!(grow_identities[0].declaration_span.start > source.find("class Dog").unwrap_or_default());
        let dog_grow = encode_incan_symbol_identity(&grow_identities[0]);
        let dog_impl = code
            .split("impl Dog")
            .nth(1)
            .and_then(|tail| tail.split("impl ").next())
            .unwrap_or_default();
        assert!(compact_rust(dog_impl).contains(&format!("self.{dog_grow}()")), "{code}");
    }

    #[test]
    fn generic_callable_name_emits_support_for_function_items_issue1865() {
        let code = generate(
            r#"
def make() -> int:
  return 1

def name_of[F](func: F) -> str:
  return func.__name__

def main() -> None:
  println(name_of(make))
"#,
        );
        assert!(code.contains("pub trait __IncanCallableName"), "{code}");
        assert!(code.contains("impl __IncanCallableName for fn() -> i64"), "{code}");
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
        assert!(code.contains(r#"incan_std_core::strings::fstring"#));
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

    /// Issue #1839: a public static re-export wins over an earlier private import of the same projection, and a later
    /// private import keeps the imported-static initializer needed by reads in the facade itself.
    #[test]
    fn static_reexport_and_repeat_import_keep_projection_and_initializer_issue1839()
    -> Result<(), Box<dyn std::error::Error>> {
        let provider = parse_program_result("pub static counter: int = 1\n")?;
        let provider_path = vec!["sprov".to_string()];

        let facade =
            parse_program_result("from sprov import counter\npub from sprov import counter as counter_alias\n")?;
        let main = parse_program_result(
            "from facade import counter_alias\n\n\ndef main() -> None:\n  println(counter_alias)\n",
        )?;
        let facade_path = vec!["facade".to_string()];
        let mut codegen = IrCodegen::new();
        codegen.add_module_with_path_segments("sprov", &provider, provider_path.clone());
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
            facade_code.contains(&format!("pub use crate::sprov::{projection} as COUNTER_ALIAS;")),
            "the facade must bind the projection publicly through its alias:\n{facade_code}"
        );
        let projection_imports = facade_code
            .lines()
            .filter(|line| line.contains(&projection) && line.contains("use"))
            .collect::<Vec<_>>();
        assert!(
            projection_imports
                .first()
                .is_some_and(|line| line.trim_start().starts_with("pub use")),
            "the public static binder must precede the private repeat: {projection_imports:?}\n{facade_code}"
        );
        assert!(
            main_code.contains(&projection),
            "the consumer must reach the provider projection through the facade:\n{main_code}"
        );

        let facade = parse_program_result(
            "pub from sprov import counter as counter_alias\nfrom sprov import counter\n\n\npub def read() -> int:\n  return counter\n",
        )?;
        let main = parse_program_result("from facade import read\n\n\ndef main() -> None:\n  println(read())\n")?;
        let mut codegen = IrCodegen::new();
        codegen.add_module_with_path_segments("sprov", &provider, provider_path.clone());
        codegen.add_module_with_path_segments("facade", &facade, facade_path.clone());
        let (_, modules) = codegen.try_generate_multi_file_nested(&main, &[provider_path, facade_path.clone()])?;
        let facade_code = modules.get(&facade_path).ok_or("missing generated facade module")?;
        assert!(
            facade_code.contains("__incan_init_imported_static_counter"),
            "the repeated private import must retain the static initializer import:\n{facade_code}"
        );
        Ok(())
    }

    /// Return Cargo's build-script and unit output directories under one target profile's `build` directory.
    fn build_output_directories(build_directory: &std::path::Path) -> Result<Vec<std::path::PathBuf>, std::io::Error> {
        let mut outputs = Vec::new();
        for package in std::fs::read_dir(build_directory)? {
            let package = package?;
            for fingerprint in std::fs::read_dir(package.path())? {
                let output = fingerprint?.path().join("out");
                if output.is_dir() {
                    outputs.push(output);
                }
            }
        }
        Ok(outputs)
    }

    /// Find the newest artifact whose file name satisfies `predicate` in any of `directories`.
    fn newest_artifact(
        directories: &[std::path::PathBuf],
        predicate: impl Fn(&str) -> bool,
    ) -> Result<Option<std::path::PathBuf>, std::io::Error> {
        let mut matches = Vec::new();
        for directory in directories {
            for entry in std::fs::read_dir(directory)? {
                let entry = entry?;
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                if predicate(name) {
                    let modified = entry.metadata().and_then(|metadata| metadata.modified()).ok();
                    matches.push((modified, entry.path()));
                }
            }
        }
        matches.sort_by_key(|(modified, _)| *modified);
        Ok(matches.pop().map(|(_, path)| path))
    }

    /// Compile a generated multi-file project as one library crate against the runtime this test binary links, so the
    /// assertion is rustc's. Each source module is written beside the crate root and declared from it, the layout a
    /// project build gives top-level modules.
    fn compile_generated_project(
        main_code: &str,
        modules: &HashMap<Vec<String>, String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let mut crate_root = main_code.to_string();
        for (path, code) in modules {
            let [name] = path.as_slice() else {
                return Err(format!("only top-level modules are laid out here, not `{}`", path.join(".")).into());
            };
            crate_root.push_str(&format!("\nmod {name};\n"));
            std::fs::write(directory.path().join(format!("{name}.rs")), code)?;
        }
        let input = directory.path().join("main.rs");
        std::fs::write(&input, &crate_root)?;
        let capability = oven_model::compiler_suite_env::OvenCompilerSuiteCapability::from_environment(
            oven_model::compiler_suite_env::OVEN_COMPILER_SUITE_CAPABILITY_ENV,
        )?;
        let rustc = capability
            .as_ref()
            .map(|capability| capability.rustc.clone())
            .unwrap_or_else(|| {
                std::env::var_os("RUSTC")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| "rustc".into())
            });
        let mut command = std::process::Command::new(rustc);
        command.args([
            "--edition=2024",
            "--crate-type=lib",
            "--crate-name=generated_project_fixture",
            "-A",
            "warnings",
        ]);
        if let Some(capability) = capability {
            for path in capability.dependency_search_paths {
                command.arg("-L").arg(format!("dependency={}", path.display()));
            }
            for (name, path) in capability.externs {
                command.arg("--extern").arg(format!("{name}={}", path.display()));
            }
        } else {
            let executable = std::env::current_exe()?;
            let target_profile = executable
                .ancestors()
                .find(|ancestor| ancestor.join("build").is_dir())
                .ok_or("test executable has no target profile directory")?;
            let mut directories = build_output_directories(&target_profile.join("build"))?;
            let deps = target_profile.join("deps");
            if deps.is_dir() {
                directories.push(deps);
            }
            for directory in &directories {
                command.arg("-L").arg(format!("dependency={}", directory.display()));
            }
            let stdlib = newest_artifact(&directories, |name| {
                name.starts_with("libincan_std_core-") && name.ends_with(".rlib")
            })?
            .ok_or("generated-Rust proof requires a compiled incan_std_core artifact")?;
            command
                .arg("--extern")
                .arg(format!("incan_std_core={}", stdlib.display()));
            let derive = newest_artifact(&directories, |name| {
                name.trim_start_matches("lib").starts_with("incan_derive-")
                    && std::path::Path::new(name)
                        .extension()
                        .and_then(|extension| extension.to_str())
                        == Some(std::env::consts::DLL_EXTENSION)
            })?
            .ok_or("generated-Rust proof requires a compiled incan_derive artifact")?;
            command
                .arg("--extern")
                .arg(format!("incan_derive={}", derive.display()));
        }
        let output = directory.path().join("libgenerated_project_fixture.rlib");
        let result = command.arg(&input).arg("-o").arg(output).output()?;
        let sources = modules
            .iter()
            .map(|(path, code)| format!("// ---- {} ----\n{code}", path.join(".")))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            result.status.success(),
            "{}\n// ---- crate root ----\n{crate_root}\n{sources}",
            String::from_utf8_lossy(&result.stderr)
        );
        Ok(())
    }

    /// Issue #1842: a default evaluated at a caller in another module reads the static projection, and runs the static
    /// initializer, of the module that declares the callable. The caller may be the crate root or another source
    /// module, whose IR is emitted on its own; both projects must build.
    #[test]
    fn cross_module_default_reading_static_keeps_projection_issue1842() -> Result<(), Box<dyn std::error::Error>> {
        let helpers =
            parse_program_result("pub static COUNT: int = 3\n\npub def take(n: int = COUNT) -> int:\n  return n\n")?;
        let helpers_path = vec!["helpers".to_string()];
        let static_projection = |helpers_code: &str| {
            helpers_code
                .lines()
                .find_map(|line| {
                    line.trim_start()
                        .strip_prefix("pub static __incan_v1_")
                        .and_then(|tail| tail.split(':').next())
                        .map(|payload| format!("__incan_v1_{payload}"))
                })
                .ok_or("helper static did not emit an incan-v1 projection")
        };

        // ---- The crate root calls the helper ----
        let main = parse_program_result("from helpers import take\n\n\ndef main() -> None:\n  println(take())\n")?;
        let mut codegen = IrCodegen::new();
        codegen.add_module_with_path_segments("helpers", &helpers, helpers_path.clone());
        let (main_code, modules) =
            codegen.try_generate_multi_file_nested(&main, std::slice::from_ref(&helpers_path))?;
        let projection = static_projection(modules.get(&helpers_path).ok_or("missing generated helpers module")?)?;
        let compact_main = compact_rust(&main_code);
        assert!(
            compact_main.contains(&format!("crate::helpers::{projection}"))
                && compact_main.contains("crate::helpers::__incan_init_module_statics()"),
            "the caller must initialize and read the default's static through its declaring module:\n{main_code}"
        );
        compile_generated_project(&main_code, &modules)?;

        // ---- Another source module calls the helper ----
        let caller = parse_program_result("from helpers import take\n\n\npub def run() -> int:\n  return take()\n")?;
        let main = parse_program_result("from caller import run\n\n\ndef main() -> None:\n  println(run())\n")?;
        let caller_path = vec!["caller".to_string()];
        let mut codegen = IrCodegen::new();
        codegen.add_module_with_path_segments("helpers", &helpers, helpers_path.clone());
        codegen.add_module_with_path_segments("caller", &caller, caller_path.clone());
        let (main_code, modules) =
            codegen.try_generate_multi_file_nested(&main, &[helpers_path.clone(), caller_path.clone()])?;
        let projection = static_projection(modules.get(&helpers_path).ok_or("missing generated helpers module")?)?;
        let caller_code = modules.get(&caller_path).ok_or("missing generated caller module")?;
        let compact_caller = compact_rust(caller_code);
        assert!(
            compact_caller.contains(&format!("crate::helpers::{projection}"))
                && compact_caller.contains("crate::helpers::__incan_init_module_statics()"),
            "a source-module caller must initialize and read the default's static through its declaring module:\n{caller_code}"
        );
        compile_generated_project(&main_code, &modules)?;
        Ok(())
    }

    /// Generate a crate-root program beside top-level source modules, given as `(name, source)` pairs, and compile the
    /// generated project with rustc. Returns the crate root's code and each module's code keyed by its path.
    fn generate_and_compile_project(
        main_source: &str,
        module_sources: &[(&str, &str)],
    ) -> Result<GeneratedProject, Box<dyn std::error::Error>> {
        let main = parse_program_result(main_source)?;
        let modules = module_sources
            .iter()
            .map(|(name, source)| Ok(((*name).to_string(), parse_program_result(source)?)))
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        let paths = modules.iter().map(|(name, _)| vec![name.clone()]).collect::<Vec<_>>();
        let mut codegen = IrCodegen::new();
        for ((name, program), path) in modules.iter().zip(&paths) {
            codegen.add_module_with_path_segments(name, program, path.clone());
        }
        let (main_code, generated) = codegen.try_generate_multi_file_nested(&main, &paths)?;
        compile_generated_project(&main_code, &generated)?;
        Ok((main_code, generated))
    }

    /// A default reads the static of the module that declares its callable, also at a caller that declares a static of
    /// the same name: the default's read names the declaring module's static by its path, and the caller's own name
    /// keeps reading the caller's static. The caller may be the crate root or another source module.
    #[test]
    fn default_static_read_beside_a_same_named_caller_static_builds() -> Result<(), Box<dyn std::error::Error>> {
        let helpers = "pub static COUNT: int = 3\n\npub def take(n: int = COUNT) -> int:\n    return n\n";
        let (main_code, _) = generate_and_compile_project(
            "from helpers import take\n\nstatic COUNT: int = 7\n\n\ndef main() -> None:\n    println(take())\n    println(COUNT)\n",
            &[("helpers", helpers)],
        )?;
        assert!(
            compact_rust(&main_code).contains("crate::helpers::__incan_init_module_statics()"),
            "the default must initialize and read the declaring module's static through its path:\n{main_code}"
        );
        generate_and_compile_project(
            "from caller import run\n\n\ndef main() -> None:\n    println(run())\n",
            &[
                ("helpers", helpers),
                (
                    "caller",
                    "from helpers import take\n\nstatic COUNT: int = 7\n\n\npub def run() -> int:\n    return take() + COUNT\n",
                ),
            ],
        )?;
        Ok(())
    }

    /// A default that reads a static list whole, through a method call on it or through a builtin reads the declaring
    /// module's list at a caller that declares a static list of the same name.
    #[test]
    fn default_static_list_reads_beside_a_same_named_caller_static_build() -> Result<(), Box<dyn std::error::Error>> {
        let helpers = r#"pub static ITEMS: list[int] = [1, 2]


pub def first(items: list[int] = ITEMS) -> int:
    return items[0]


pub def has(flag: bool = ITEMS.contains(2)) -> bool:
    return flag


pub def size(n: int = len(ITEMS)) -> int:
    return n
"#;
        let (main_code, _) = generate_and_compile_project(
            r#"from helpers import first, has, size

static ITEMS: list[str] = ["a"]


def main() -> None:
    println(first())
    println(has())
    println(size())
    println(len(ITEMS))
"#,
            &[("helpers", helpers)],
        )?;
        assert!(
            compact_rust(&main_code).contains("crate::helpers::__incan_init_module_statics();crate::helpers::"),
            "each default must read `helpers`' list, not the caller's:\n{main_code}"
        );
        Ok(())
    }

    /// A method default, a method-partial preset and a static method's default that read a static of the declaring
    /// module build at a caller in another module, also where the caller imports that static itself or declares its
    /// own static of the same name.
    #[test]
    fn default_static_read_in_method_defaults_across_modules_builds() -> Result<(), Box<dyn std::error::Error>> {
        let helpers = r#"pub static LIMIT: int = 3

pub class Box:
    pub v: int

    def get(self, n: int = LIMIT) -> int:
        return n + self.v

    def add(self, n: int) -> int:
        return n + self.v

    capped = partial add(n=LIMIT)

    @staticmethod
    def make(v: int = LIMIT) -> Box:
        return Box(v=v)
"#;
        generate_and_compile_project(
            "from helpers import Box\n\n\ndef main() -> None:\n    b = Box(v=1)\n    println(b.get())\n    println(b.capped())\n    println(Box.make().v)\n",
            &[("helpers", helpers)],
        )?;
        generate_and_compile_project(
            "from helpers import Box, LIMIT\n\nstatic OTHER: int = 9\n\n\ndef main() -> None:\n    b = Box(v=1)\n    println(b.get() + LIMIT + OTHER)\n",
            &[("helpers", helpers)],
        )?;
        generate_and_compile_project(
            "from helpers import Box\n\nstatic LIMIT: int = 5\n\n\ndef main() -> None:\n    println(Box(v=1).get() + LIMIT)\n",
            &[("helpers", helpers)],
        )?;
        Ok(())
    }

    /// A default that reads a static its module imports, or that calls a function whose own default reads a static,
    /// reads the module that declares the static at a caller that neither imports that static nor its module, even
    /// where the caller declares a static of the same name.
    #[test]
    fn default_static_read_through_imports_and_nested_defaults_builds() -> Result<(), Box<dyn std::error::Error>> {
        let (main_code, _) = generate_and_compile_project(
            "from helpers import take, wrap\n\nstatic LIMIT: int = 5\n\n\ndef main() -> None:\n    println(take() + wrap() + LIMIT)\n",
            &[
                ("config", "pub static LIMIT: int = 4\n"),
                (
                    "helpers",
                    "from config import LIMIT\n\n\ndef inner(k: int = LIMIT) -> int:\n    return k\n\n\npub def take(n: int = LIMIT) -> int:\n    return n\n\n\npub def wrap(n: int = inner()) -> int:\n    return n\n",
                ),
            ],
        )?;
        assert!(
            compact_rust(&main_code).contains("crate::config::__incan_init_module_statics()"),
            "the defaults must read `config`'s static rather than the caller's static of the same name:\n{main_code}"
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

    /// The stdlib namespace binding named `serde` must not capture generated serde derive paths.
    #[test]
    fn followups_b_serde_derives_are_crate_rooted_when_stdlib_serde_is_imported() {
        let generated = generate(
            r#"
from std import serde
from std.serde.json import Serialize

@derive(Serialize)
pub model Derived:
  value: int

pub model Adopted with Serialize:
  value: int
"#,
        );
        assert!(
            generated.contains("::serde::Serialize"),
            "serde derives must resolve from the crate root despite the local `serde` binding:\n{generated}"
        );
        assert!(
            !generated.contains("derive(Debug, Clone, serde::Serialize)"),
            "a relative serde derive remains shadowable:\n{generated}"
        );
    }

    /// Option identity against `None` must not require equality of the payload type.
    #[test]
    fn followups_b_option_none_identity_emits_presence_predicates() {
        let generated = generate(
            r#"
pub model Payload:
  value: int

pub def absent(value: Option[Payload]) -> bool:
  return value is None

pub def present(value: Option[Payload]) -> bool:
  return value is not None
"#,
        );
        assert!(
            generated.contains("value.is_none()"),
            "missing `is_none()`:\n{generated}"
        );
        assert!(
            generated.contains("value.is_some()"),
            "missing `is_some()`:\n{generated}"
        );
        assert!(
            !generated.contains("value == None") && !generated.contains("value != None"),
            "Option identity must not emit payload equality:\n{generated}"
        );
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
        let testing_module = read_stdlib_program("loaves/stdlib/testing/src/testing.incn")?;
        let async_task_module = read_stdlib_program("loaves/stdlib/async/src/async/task.incn")?;
        let async_time_module = read_stdlib_program("loaves/stdlib/async/src/async/time.incn")?;
        let traits_error_module = read_stdlib_program("loaves/stdlib/core/src/traits/error.incn")?;

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
        let io_module = read_stdlib_program("loaves/stdlib/system/src/io.incn")?;
        let traits_error_module = read_stdlib_program("loaves/stdlib/core/src/traits/error.incn")?;

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
            io_code.contains("impl crate::__incan_std::traits::error::Error for IoError"),
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
        let io_module = read_stdlib_program("loaves/stdlib/system/src/io.incn")?;
        let traits_error_module = read_stdlib_program("loaves/stdlib/core/src/traits/error.incn")?;

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
        let streaming_module = read_stdlib_program("loaves/stdlib/data/src/hash/_streaming.incn")?;
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
        let auto_module = read_stdlib_program("loaves/stdlib/compression/src/compression/_auto.incn")?;
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

    /// #1766: `..` climbs from the importing file's directory, so `store/json_store.incn` reaches the root's
    /// `db.schema`, and the import names that module by its crate-absolute path.
    #[test]
    fn test_relative_from_import_uses_super_prefix() {
        let store_code = generate_nested_store_code(
            r#"
from ..db.schema import Database

pub def touch(db: Database) -> None:
  return
"#,
        );
        assert!(store_code.contains("use crate::db::schema::Database;"), "{store_code}");
        assert!(!store_code.contains("use super::db::schema::Database;"), "{store_code}");
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
        use incan_frontend::typechecker::TypeChecker;
        use incan_lang::interop::{RustFunctionSig, RustItemKind, RustItemMetadata, RustParam, RustVisibility};

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
                        receiver_contract: None,
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
        use incan_frontend::typechecker::TypeChecker;
        use incan_lang::interop::{
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
        use incan_frontend::typechecker::TypeChecker;
        use incan_lang::interop::{RustFunctionSig, RustItemKind, RustItemMetadata, RustParam, RustVisibility};

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
                        receiver_contract: None,
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
    fn test_codegen_materializes_owner_specialized_rust_associated_function_arguments()
    -> Result<(), Box<dyn std::error::Error>> {
        use incan_frontend::typechecker::TypeChecker;
        use incan_lang::interop::{
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
                                receiver_contract: None,
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
        use incan_frontend::typechecker::TypeChecker;
        use incan_lang::interop::{
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
        use incan_frontend::typechecker::TypeChecker;
        use incan_lang::interop::{
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
                                receiver_contract: None,
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
        use incan_frontend::typechecker::TypeChecker;
        use incan_lang::interop::{
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
                                receiver_contract: None,
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
        let mut checker = incan_frontend::typechecker::TypeChecker::new();
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
        use incan_frontend::typechecker::TypeChecker;
        use incan_ir::Mutability;
        use incan_lang::interop::{
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
            Some(&vec![incan_frontend::typechecker::MutableRustTypeArgumentProjection {
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
        use incan_frontend::typechecker::TypeChecker;
        use incan_lang::interop::{
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
        use incan_frontend::typechecker::TypeChecker;
        use incan_lang::interop::{
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
            Some(&vec![incan_frontend::typechecker::MutableRustTypeArgumentProjection {
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
        use incan_frontend::typechecker::TypeChecker;

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
            Some(&vec![incan_frontend::typechecker::MutableRustTypeArgumentProjection {
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
        use incan_frontend::typechecker::TypeChecker;
        use incan_lang::interop::{
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
                        derive_macro: Some(incan_lang::interop::RustMacroInfo {
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
                    kind: RustItemKind::Macro(incan_lang::interop::RustMacroInfo {
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
            Some(&vec![incan_frontend::typechecker::MutableRustTypeArgumentProjection {
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
            Some(&vec![incan_frontend::typechecker::MutableRustTypeArgumentProjection {
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
            Some(&vec![incan_frontend::typechecker::MutableRustTypeArgumentProjection {
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
        use incan_frontend::typechecker::TypeChecker;
        use incan_lang::interop::{
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
                incan_frontend::typechecker::MutableRustTypeArgumentProjection {
                    argument_position: 0,
                    reference_leaf_paths: vec![vec![]],
                },
                incan_frontend::typechecker::MutableRustTypeArgumentProjection {
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
        use incan_ir::Mutability;
        use incan_ir::decl::IrDeclKind;

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
            vec![incan_frontend::typechecker::MutableRustTypeArgumentProjection {
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
        let mut checker = incan_frontend::typechecker::TypeChecker::new();
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
        use incan_frontend::typechecker::TypeChecker;
        use incan_lang::interop::{RustFunctionSig, RustItemKind, RustItemMetadata, RustParam, RustVisibility};

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
                        receiver_contract: None,
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
                && code.matches("incan_std_core::num::require_finite_f32").count() == 6,
            "expected exact f32 arithmetic to retain its width and finite invariant across the imported Rust f32 \
             boundary, got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_emits_raw_rust_field_names_for_keyword_fields_issue725() -> Result<(), Box<dyn std::error::Error>> {
        use incan_frontend::typechecker::TypeChecker;
        use incan_lang::interop::{
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
        use incan_frontend::typechecker::TypeChecker;

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
        use incan_frontend::typechecker::TypeChecker;
        use incan_lang::interop::{
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
                                    receiver_contract: None,
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
                                    receiver_contract: None,
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
        use incan_frontend::typechecker::TypeChecker;

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
        use incan_frontend::typechecker::TypeChecker;

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
        use incan_frontend::typechecker::TypeChecker;
        use incan_lang::interop::{RustFunctionSig, RustItemKind, RustItemMetadata, RustParam, RustVisibility};

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
                        receiver_contract: None,
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
        use incan_frontend::typechecker::TypeChecker;
        use incan_lang::interop::{
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
                                    receiver_contract: None,
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
                                    receiver_contract: None,
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
                                receiver_contract: None,
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
                        receiver_contract: None,
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
                        receiver_contract: None,
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
        use incan_frontend::typechecker::TypeChecker;
        use rust_inspect::test_fixtures::write_async_result_probe_crate;

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
        use incan_frontend::typechecker::TypeChecker;
        use rust_inspect::test_fixtures::write_async_result_probe_crate;

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
        use rust_inspect::test_fixtures::write_hyphenated_function_probe_crate;

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
    /// Exercise real TOML receiver facts under the library test root's declared registry-source authority.
    #[cfg(feature = "rust_inspect")]
    #[test]
    fn toml_traversal_uses_extracted_receiver_contracts() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from rust::toml_edit import Item

def traverse(item: Item, keys: list[str]) -> Item:
    mut current = item
    for key in keys:
        match current.get(key):
            Some(child) => current = child
            None => return Item.None
    return current.clone()

pub def observe(item: Item) -> Item:
    return traverse(item, ["project", "count"])
"#;
        let tokens =
            incan_frontend::lexer::lex(source).map_err(|errors| std::io::Error::other(format!("lex: {errors:?}")))?;
        let ast = incan_frontend::parser::parse(&tokens)
            .map_err(|errors| std::io::Error::other(format!("parse: {errors:?}")))?;
        let mut checker = incan_frontend::typechecker::TypeChecker::new();
        checker.set_rust_inspect_manifest_dir(oven_model::toolchain_layout::development_root());
        checker
            .check_program(&ast)
            .map_err(|errors| std::io::Error::other(format!("check: {errors:?}")))?;
        let ir = incan_ir::AstLowering::new_with_type_info(checker.type_info().clone())
            .lower_program(&ast)
            .map_err(|error| std::io::Error::other(format!("lower: {error:?}")))?;
        let generated = crate::IrEmitter::new(&ir.function_registry).emit_program(&ir)?;
        assert!(
            generated.contains("item: &Item"),
            "contracts: {:?}\n{generated}",
            checker.type_info().rust.receiver_contracts
        );
        assert!(!generated.contains("child.clone()"), "{generated}");
        Ok(())
    }

    /// Build one producer-local nominal binding for the union-capture merge tests.
    fn nominal_binding(name: &str, module: &str) -> incan_frontend::library_manifest::CanonicalIdentityExport {
        incan_frontend::library_manifest::CanonicalIdentityExport {
            namespace: incan_frontend::library_manifest::CanonicalIdentityNamespaceExport::OrdinaryLexical,
            origin: incan_frontend::library_manifest::CanonicalIdentityOriginExport::Package {
                library: "provider".to_string(),
                module_path: vec![module.to_string()],
            },
            declaration_name: name.to_string(),
            kind: "model".to_string(),
            declaration_span: incan_frontend::library_manifest::CanonicalIdentitySpanExport { start: 0, end: 1 },
        }
    }

    /// Build one capture of the same emitted wrapper as seen from a single module.
    fn union_capture(
        members: &[&str],
        nominals: &[(&str, &str)],
    ) -> incan_frontend::library_manifest::NativeUnionExport {
        incan_frontend::library_manifest::NativeUnionExport {
            owner: incan_frontend::library_manifest::NativeUnionOwnerExport::ContainingArtifact,
            rust_name: "__IncanUnionTest".to_string(),
            members: members
                .iter()
                .map(|name| incan_frontend::library_manifest::TypeRef::Named {
                    name: (*name).to_string(),
                    origin: None,
                })
                .collect(),
            local_nominals: nominals
                .iter()
                .map(|(name, module)| ((*name).to_string(), nominal_binding(name, module)))
                .collect(),
            checked_projection: None,
        }
    }

    #[test]
    fn a_wrapper_captured_from_two_modules_keeps_every_module_s_nominal_bindings()
    -> Result<(), Box<dyn std::error::Error>> {
        // The module declaring the payload models sees every leaf; a module that only accepts the union in a
        // signature sees none. Both are honest partial views of one wrapper, so the merge has to keep the union.
        let mut declaring = union_capture(&["Left", "Right"], &[("Left", "shapes"), ("Right", "shapes")]);
        let accepting = union_capture(&["Left", "Right"], &[]);
        merge_native_union_capture(&mut declaring, accepting)?;
        assert_eq!(
            declaring.local_nominals.keys().cloned().collect::<Vec<_>>(),
            vec!["Left".to_string(), "Right".to_string()]
        );

        let mut accepting = union_capture(&["Left", "Right"], &[]);
        let declaring = union_capture(&["Left", "Right"], &[("Left", "shapes"), ("Right", "shapes")]);
        merge_native_union_capture(&mut accepting, declaring)?;
        assert_eq!(
            accepting.local_nominals.keys().cloned().collect::<Vec<_>>(),
            vec!["Left".to_string(), "Right".to_string()]
        );
        Ok(())
    }

    #[test]
    fn a_wrapper_captured_with_two_payload_orders_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        // Consumers index the emitted `V0`, `V1`, ... variants by producer order, so disagreement here is a defect
        // rather than a partial view, and must not be merged away.
        let mut first = union_capture(&["Left", "Right"], &[]);
        let reordered = union_capture(&["Right", "Left"], &[]);
        let Err(error) = merge_native_union_capture(&mut first, reordered) else {
            return Err("a reordered payload list must not merge".into());
        };
        assert!(
            format!("{error:?}").contains("two different payload orders"),
            "the refusal must name the payload order: {error:?}"
        );
        Ok(())
    }

    #[test]
    fn a_leaf_bound_to_two_canonical_identities_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let mut first = union_capture(&["Left"], &[("Left", "shapes")]);
        let conflicting = union_capture(&["Left"], &[("Left", "other_module")]);
        let Err(error) = merge_native_union_capture(&mut first, conflicting) else {
            return Err("one leaf name bound to two identities must not merge".into());
        };
        assert!(
            format!("{error:?}").contains("two canonical identities"),
            "the refusal must name the conflicting binding: {error:?}"
        );
        Ok(())
    }
}
