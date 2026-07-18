//! Call expression lowering: struct constructors, builtin dispatch, newtype checked construction, and regular function
//! calls.

use std::collections::HashMap;

use super::super::super::decl::FunctionParam;
use super::super::super::expr::{
    BuiltinFn, IrCallArg, IrCallArgKind, IrDictEntry, IrExprKind, IrInteropCoercionKind, IrListEntry,
    Literal as IrLiteral, MatchArm, MethodCallArgPolicy, Pattern, VarAccess, VarRefKind,
};
use super::super::super::stmt::IrStmtKind;
use super::super::super::types::IrType;
use super::super::super::{FunctionSignature, IrStmt, Mutability, TypedExpr};
use super::super::AstLowering;
use super::super::errors::LoweringError;
use crate::frontend::api_metadata::{
    ApiDeclaration, function_export_from_api, function_export_from_api_projected, method_export_from_api,
};
use crate::frontend::ast::{self, TypeConstraintKey};
use crate::frontend::library_manifest_index::LibraryManifestIndexEntry;
use crate::frontend::partial_projection::{PartialPresetRef, merge_named_partial_args};
use crate::frontend::symbols::{CallableParam, NewtypePrimitiveConstraint, ResolvedType};
use crate::frontend::typechecker::{FixedUnpackPlan, RustArgCoercionKind, ValidatedNewtypeCoercionMode};
use crate::frontend::typechecker::{IdentKind, ResolvedOperatorKind};
use crate::library_manifest::{
    FunctionExport, LibraryManifest, MethodExport, ParamDefaultCallArgExport, ParamDefaultCallSignatureExport,
    ParamDefaultExport, ParamExport, ParamKindExport,
};
use crate::provider::{ProviderModuleResolution, ProviderRecord};
use incan_core::lang::keywords::{self, KeywordId};
use incan_core::lang::stdlib;
use incan_core::lang::stdlib::{STDLIB_BUILTINS, STDLIB_ROOT};
use incan_core::lang::surface::constructors::{self, ConstructorId};
use incan_core::lang::surface::types as surface_types;
use incan_core::lang::testing::{self, TestingAssertHelperId};
use incan_core::lang::types::collections::{self, CollectionTypeId};

const TYPE_CONSTRUCTOR_HOOK: &str = "__incan_new";
const API_CRATE_ROOT_SEGMENT: &str = "crate";

impl AstLowering {
    /// Return the builtin member name for an explicit `std.builtins.<name>` callee.
    pub(in crate::backend::ir::lower::expr) fn explicit_builtin_member_name(
        callee: &ast::Spanned<ast::Expr>,
    ) -> Option<&str> {
        let ast::Expr::Field(namespace, member) = &callee.node else {
            return None;
        };
        if Self::is_explicit_builtin_namespace_expr(namespace) {
            Some(member.as_str())
        } else {
            None
        }
    }

    /// Return whether an expression is the explicit builtin namespace `std.builtins`.
    pub(in crate::backend::ir::lower::expr) fn is_explicit_builtin_namespace_expr(
        expr: &ast::Spanned<ast::Expr>,
    ) -> bool {
        let ast::Expr::Field(root, namespace) = &expr.node else {
            return false;
        };
        namespace == STDLIB_BUILTINS && matches!(&root.node, ast::Expr::Ident(name) if name == STDLIB_ROOT)
    }

    /// Rebuild a callable signature from frontend metadata for rest-aware IR emission.
    fn callable_signature_from_params(&self, params: &[CallableParam], ret: &ResolvedType) -> FunctionSignature {
        FunctionSignature {
            params: params
                .iter()
                .enumerate()
                .map(|(idx, param)| {
                    let base_ty = self.lower_resolved_type(&param.ty);
                    let ty = Self::lower_param_container_type(param.kind, base_ty);
                    FunctionParam {
                        name: param.name.clone().unwrap_or_else(|| format!("__incan_arg_{idx}")),
                        ty,
                        mutability: super::super::super::types::Mutability::Immutable,
                        is_self: false,
                        kind: param.kind,
                        default: None,
                    }
                })
                .collect(),
            return_type: self.lower_resolved_type(ret),
        }
    }

    /// Rebuild a callable signature directly from a stdlib method declaration so default expressions survive import
    /// metadata boundaries.
    fn callable_signature_from_stdlib_method_decl(
        &mut self,
        method: &ast::MethodDecl,
    ) -> Result<FunctionSignature, LoweringError> {
        Ok(FunctionSignature {
            params: method
                .params
                .iter()
                .map(|param| {
                    let base_ty = self.lower_type(&param.node.ty.node);
                    let ty = Self::lower_param_container_type(param.node.kind, base_ty);
                    Ok(FunctionParam {
                        name: param.node.name.clone(),
                        ty,
                        mutability: if param.node.is_mut {
                            super::super::super::types::Mutability::Mutable
                        } else {
                            super::super::super::types::Mutability::Immutable
                        },
                        is_self: false,
                        kind: param.node.kind,
                        default: self.lower_param_default_expr(param.node.default.as_ref())?,
                    })
                })
                .collect::<Result<_, LoweringError>>()?,
            return_type: self.lower_type(&method.return_type.node),
        })
    }

    /// Rebuild a callable signature directly from a stdlib function declaration so default expressions survive import
    /// metadata boundaries.
    fn callable_signature_from_stdlib_function_decl(
        &mut self,
        func: &ast::FunctionDecl,
    ) -> Result<FunctionSignature, LoweringError> {
        Ok(FunctionSignature {
            params: func
                .params
                .iter()
                .map(|param| {
                    let base_ty = self.lower_type(&param.node.ty.node);
                    let ty = Self::lower_param_container_type(param.node.kind, base_ty);
                    Ok(FunctionParam {
                        name: param.node.name.clone(),
                        ty,
                        mutability: if param.node.is_mut {
                            super::super::super::types::Mutability::Mutable
                        } else {
                            super::super::super::types::Mutability::Immutable
                        },
                        is_self: false,
                        kind: param.node.kind,
                        default: self.lower_param_default_expr(param.node.default.as_ref())?,
                    })
                })
                .collect::<Result<_, LoweringError>>()?,
            return_type: self.lower_type(&func.return_type.node),
        })
    }

    /// Resolve a callable signature from a public dependency manifest, including materialized default expressions.
    fn callable_signature_for_imported_pub_path(&mut self, path: &[String]) -> Option<FunctionSignature> {
        if path.len() < 3 || path.first().map(String::as_str) != Some("pub") {
            return None;
        }
        let library = path.get(1)?;
        let function_name = path.last()?;
        let function = self.pub_function_export(library, function_name)?;
        Some(self.callable_signature_from_pub_function_export(library, &function))
    }

    /// Resolve the canonical imported callee path for identifier and module-qualified calls.
    fn imported_callee_path_for_expr(&self, expr: &ast::Expr) -> Option<Vec<String>> {
        match expr {
            ast::Expr::Ident(name) => self
                .active_trait_default_function_path(name)
                .or_else(|| self.import_aliases.get(name).cloned()),
            ast::Expr::Field(object, field) => {
                let mut path = self.imported_field_base_path(&object.node)?;
                path.push(field.clone());
                Some(path)
            }
            _ => None,
        }
    }

    /// Resolve the imported module path that roots a field-chain callee such as `widgets.make_widget`.
    fn imported_field_base_path(&self, expr: &ast::Expr) -> Option<Vec<String>> {
        match expr {
            ast::Expr::Ident(name) => self.import_aliases.get(name).cloned(),
            ast::Expr::Field(object, field) => {
                let mut path = self.imported_field_base_path(&object.node)?;
                path.push(field.clone());
                Some(path)
            }
            _ => None,
        }
    }

    /// Resolve `module.function(...)` syntax when the receiver is an imported stdlib or public dependency module.
    pub(in crate::backend::ir::lower) fn imported_module_function_callee_path(
        &self,
        receiver: &ast::Expr,
        method_name: &str,
    ) -> Option<Vec<String>> {
        let mut path = self.imported_field_base_path(receiver)?;
        match path.first().map(String::as_str) {
            Some(stdlib::STDLIB_ROOT) if self.is_provider_or_legacy_stdlib_module(&path) => {}
            Some("pub") => {
                let library = path.get(1)?;
                self.pub_function_export(library, method_name)?;
            }
            _ => return None,
        }
        path.push(method_name.to_string());
        Some(path)
    }

    /// Resolve stdlib module ownership through the active provider catalog, retaining the compiler registry only for
    /// legacy source-only sessions and the provider bootstrap that produces the checked SDK artifacts.
    fn is_provider_or_legacy_stdlib_module(&self, module_path: &[String]) -> bool {
        let Some(provider_plan) = self.provider_plan.as_deref() else {
            return stdlib::is_known_stdlib_module(module_path);
        };
        match provider_plan.resolve_module(module_path) {
            ProviderModuleResolution::Active(_) => true,
            ProviderModuleResolution::Unknown if provider_plan.bootstrap_owns_sdk_module(module_path) => {
                // The bootstrap grant authorizes one top-level namespace; it does not turn imported types and values
                // beneath that root into modules. Until the publisher emits its checked exact claims, source discovery
                // remains the narrow authority for deciding whether this exact path denotes a module.
                stdlib::is_known_stdlib_module(module_path)
            }
            ProviderModuleResolution::Unknown if !provider_plan.has_sdk_catalog() => {
                stdlib::is_known_stdlib_module(module_path)
            }
            ProviderModuleResolution::Disabled(_)
            | ProviderModuleResolution::Unavailable(_)
            | ProviderModuleResolution::Unknown => false,
        }
    }

    /// Fetch the public function export or projected alias export that backs an imported public callable.
    fn pub_function_export(&self, library: &str, function_name: &str) -> Option<FunctionExport> {
        let index = self.provider_plan.as_deref()?.library_manifest_index();
        let LibraryManifestIndexEntry::Loaded { manifest, .. } = index.get(library)? else {
            return None;
        };
        if let Some(function) = manifest
            .exports
            .functions
            .iter()
            .find(|function| function.name == function_name)
        {
            return Some(function.clone());
        }
        if let Some(function) = Self::api_function_export_for_public_name(manifest, function_name) {
            return Some(function);
        }
        manifest
            .exports
            .aliases
            .iter()
            .find(|alias| alias.name == function_name)
            .and_then(|alias| alias.projected_function.clone())
    }

    /// Resolve public callable aliases through the manifest identity graph before falling back to public-name scans.
    fn api_function_export_for_public_name(
        manifest: &crate::library_manifest::LibraryManifest,
        function_name: &str,
    ) -> Option<FunctionExport> {
        let target_path = manifest
            .contract_metadata
            .identity_graph
            .entry_for_public_name(function_name)
            .and_then(|entry| entry.target_path())?;
        let api = manifest.contract_metadata.api.as_ref()?;
        Self::api_function_export_for_target_path(api, target_path)
    }

    /// Resolve one checked API function from a module-qualified public callable target path.
    fn api_function_export_for_target_path(
        api: &crate::frontend::api_metadata::CheckedApiMetadataPackage,
        target_path: &[String],
    ) -> Option<FunctionExport> {
        let function_name = target_path.last()?;
        let path = if target_path
            .first()
            .is_some_and(|segment| segment == API_CRATE_ROOT_SEGMENT)
        {
            &target_path[1..]
        } else {
            target_path
        };
        let module_path = path.get(..path.len().saturating_sub(1))?;
        let module = api.modules.iter().find(|module| module.module_path == module_path)?;
        module
            .declarations
            .iter()
            .find_map(|declaration| Self::api_function_export_for_declaration(declaration, function_name))
    }

    /// Convert one checked API declaration into the function export requested by backend call planning.
    fn api_function_export_for_declaration(
        declaration: &ApiDeclaration,
        function_name: &str,
    ) -> Option<FunctionExport> {
        match declaration {
            ApiDeclaration::Function(function) if function.name == function_name => {
                Some(function_export_from_api(function))
            }
            ApiDeclaration::Alias(alias) if alias.name == function_name => alias
                .projected_function
                .as_ref()
                .map(function_export_from_api_projected),
            _ => None,
        }
    }

    /// Rebuild a public dependency callable signature from manifest metadata, including materialized parameter
    /// defaults.
    fn callable_signature_from_pub_function_export(
        &mut self,
        library: &str,
        function: &FunctionExport,
    ) -> FunctionSignature {
        FunctionSignature {
            params: function
                .params
                .iter()
                .map(|param| {
                    let base_ty = self.lower_pub_manifest_type_ref(library, &param.ty);
                    let kind = param_kind_from_manifest(param.kind);
                    FunctionParam {
                        name: param.name.clone(),
                        ty: Self::lower_param_container_type(kind, base_ty),
                        mutability: Mutability::Immutable,
                        is_self: false,
                        kind,
                        default: self.lower_pub_param_default(library, param),
                    }
                })
                .collect(),
            return_type: self.lower_pub_manifest_type_ref(library, &function.return_type),
        }
    }

    /// Lower one exported parameter default into IR so omitted public dependency arguments can be emitted at call
    /// sites.
    fn lower_pub_param_default(&mut self, library: &str, param: &ParamExport) -> Option<TypedExpr> {
        match param.default.as_ref() {
            Some(ParamDefaultExport::Unsupported) | None => None,
            Some(default) if default.is_materializable() => self.lower_pub_default_expr(library, default),
            Some(_) => None,
        }
    }

    /// Lower a metadata-safe exported default expression into the subset of IR that can be materialized by consumers.
    fn lower_pub_default_expr(&mut self, library: &str, default: &ParamDefaultExport) -> Option<TypedExpr> {
        match default {
            ParamDefaultExport::Int(value) => Some(TypedExpr::new(IrExprKind::Int(*value), IrType::Int)),
            ParamDefaultExport::Float(value) => value
                .parse::<f64>()
                .ok()
                .map(|value| TypedExpr::new(IrExprKind::Float(value), IrType::Float)),
            ParamDefaultExport::Bool(value) => Some(TypedExpr::new(IrExprKind::Bool(*value), IrType::Bool)),
            ParamDefaultExport::String(value) => Some(TypedExpr::new(
                IrExprKind::Literal(IrLiteral::StaticStr(value.clone())),
                IrType::StaticStr,
            )),
            ParamDefaultExport::Bytes(value) => Some(TypedExpr::new(IrExprKind::Bytes(value.clone()), IrType::Bytes)),
            ParamDefaultExport::None => Some(TypedExpr::new(IrExprKind::None, IrType::Unit)),
            ParamDefaultExport::List(values) => {
                let entries = values
                    .iter()
                    .map(|value| self.lower_pub_default_expr(library, value).map(IrListEntry::Element))
                    .collect::<Option<Vec<_>>>()?;
                Some(TypedExpr::new(
                    IrExprKind::List(entries),
                    IrType::List(Box::new(IrType::Unknown)),
                ))
            }
            ParamDefaultExport::Dict(entries) => {
                let entries = entries
                    .iter()
                    .map(|entry| {
                        Some(IrDictEntry::Pair(
                            self.lower_pub_default_expr(library, &entry.key)?,
                            Box::new(self.lower_pub_default_expr(library, &entry.value)?),
                        ))
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(TypedExpr::new(
                    IrExprKind::Dict(entries),
                    IrType::Dict(Box::new(IrType::Unknown), Box::new(IrType::Unknown)),
                ))
            }
            ParamDefaultExport::ConstRef(path) => self.lower_pub_default_const_ref(library, path),
            ParamDefaultExport::Call { path, args, signature } => {
                self.lower_pub_default_call(library, path, args, signature.as_ref())
            }
            ParamDefaultExport::Unsupported => None,
        }
    }

    /// Lower a default constant reference as a dependency-qualified value expression.
    fn lower_pub_default_const_ref(&mut self, library: &str, path: &[String]) -> Option<TypedExpr> {
        if path.is_empty() {
            return None;
        }
        let mut expr = TypedExpr::new(
            IrExprKind::Var {
                name: library.to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::ExternalName,
            },
            IrType::Unknown,
        );
        for segment in path {
            expr = TypedExpr::new(
                IrExprKind::Field {
                    object: Box::new(expr),
                    field: segment.clone(),
                },
                IrType::Unknown,
            );
        }
        Some(expr)
    }

    /// Lower an exported default call while preserving the public dependency canonical path for nested call planning.
    fn lower_pub_default_call(
        &mut self,
        library: &str,
        path: &[String],
        args: &[ParamDefaultCallArgExport],
        signature: Option<&ParamDefaultCallSignatureExport>,
    ) -> Option<TypedExpr> {
        let function_name = path.last()?.clone();
        let canonical_path = self.pub_default_canonical_path(library, path);
        let function = self.pub_function_export(library, &function_name);
        let callable_signature = signature
            .map(|signature| self.callable_signature_from_pub_default_call_signature(library, signature))
            .or_else(|| {
                function
                    .as_ref()
                    .map(|function| self.callable_signature_from_pub_function_export(library, function))
            });
        let return_type = signature
            .map(|signature| self.lower_pub_manifest_type_ref(library, &signature.return_type))
            .or_else(|| {
                function
                    .as_ref()
                    .map(|function| self.lower_pub_manifest_type_ref(library, &function.return_type))
            })
            .unwrap_or(IrType::Unknown);
        let args = args
            .iter()
            .map(|arg| {
                Some(IrCallArg {
                    name: arg.name.clone(),
                    kind: if arg.name.is_some() {
                        IrCallArgKind::Named
                    } else {
                        IrCallArgKind::Positional
                    },
                    expr: self.lower_pub_default_expr(library, &arg.value)?,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: function_name,
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Unknown,
                )),
                type_args: Vec::new(),
                args,
                callable_signature,
                canonical_path: Some(canonical_path),
            },
            self.pub_external_type(library, return_type),
        ))
    }

    /// Rebuild the source callable surface captured for a provider-owned default helper call.
    fn callable_signature_from_pub_default_call_signature(
        &mut self,
        library: &str,
        signature: &ParamDefaultCallSignatureExport,
    ) -> FunctionSignature {
        FunctionSignature {
            params: signature
                .params
                .iter()
                .map(|param| {
                    let base_ty = self.lower_pub_manifest_type_ref(library, &param.ty);
                    let kind = param_kind_from_manifest(param.kind);
                    FunctionParam {
                        name: param.name.clone(),
                        ty: Self::lower_param_container_type(kind, base_ty),
                        mutability: Mutability::Immutable,
                        is_self: false,
                        kind,
                        default: self.lower_pub_param_default(library, param),
                    }
                })
                .collect(),
            return_type: self.lower_pub_manifest_type_ref(library, &signature.return_type),
        }
    }

    /// Convert a default-expression path from manifest-local spelling into a public dependency canonical path.
    fn pub_default_canonical_path(&self, library: &str, path: &[String]) -> Vec<String> {
        let mut canonical = vec!["pub".to_string(), library.to_string()];
        canonical.extend(path.iter().cloned());
        canonical
    }

    /// Build the emitted function type for a public dependency callable without losing semantic call-planning metadata.
    fn pub_external_function_type(&self, library: &str, signature: &FunctionSignature) -> IrType {
        IrType::Function {
            params: signature
                .params
                .iter()
                .map(|param| self.pub_external_type(library, param.ty.clone()))
                .collect(),
            ret: Box::new(self.pub_external_type(library, signature.return_type.clone())),
        }
    }

    /// Resolve an imported stdlib type method signature by loading the owning stdlib stub AST.
    ///
    /// Function metadata already has a direct stdlib lookup path, but type-member calls such as `App.run()` arrive as
    /// method calls. The lightweight frontend import metadata only records `has_default`, so this path rehydrates the
    /// actual default expressions from the stdlib source declaration before IR emission fills omitted arguments.
    pub(in crate::backend::ir::lower) fn callable_signature_for_imported_stdlib_type_method_path(
        &mut self,
        path: &[String],
        method_name: &str,
    ) -> Result<Option<FunctionSignature>, LoweringError> {
        if path.len() < 3 || path.first().map(String::as_str) != Some(incan_core::lang::stdlib::STDLIB_ROOT) {
            return Ok(None);
        }
        let Some(type_name) = path.last() else {
            return Ok(None);
        };
        let module_path = &path[..path.len() - 1];
        if let Some(provider_crate) = self.sdk_provider_crate_for_module(module_path)
            && let Some(manifest) = self.sdk_provider_manifest_for_module(module_path)
            && let Some(method) = Self::api_method_export_for_pub_type(manifest, type_name, method_name)
        {
            let signature = self.callable_signature_from_compiled_provider_method_export(&provider_crate, &method);
            return Ok(Some(
                self.compiled_provider_external_signature(&provider_crate, signature),
            ));
        }
        let mut cache = crate::frontend::typechecker::stdlib_loader::StdlibAstCache::new();
        if let Some(method) = cache.lookup_type_method_decl(module_path, type_name, method_name) {
            return self.callable_signature_from_stdlib_method_decl(&method).map(Some);
        }
        Ok(None)
    }

    /// Return the checked SDK provider manifest that owns one exact canonical `std.*` module.
    fn sdk_provider_manifest_for_module(&self, module_path: &[String]) -> Option<&LibraryManifest> {
        self.provider_plan
            .as_deref()?
            .active_sdk_provider_for_module(module_path)?
            .manifest
            .as_deref()
    }

    /// Return the generated Rust crate that owns one compiled SDK module's nominal artifact types.
    fn sdk_provider_crate_for_module(&self, module_path: &[String]) -> Option<String> {
        let provider = self
            .provider_plan
            .as_deref()?
            .active_sdk_provider_for_module(module_path)?;
        Some(Self::sdk_provider_rust_dependency_key(provider))
    }

    /// Return the Rust-import-safe dependency key for one compiled or in-memory SDK provider.
    ///
    /// Installed providers always carry the exact generated crate key in artifact metadata. Source-backed compiler
    /// tests have no physical artifact, so their provider package spelling follows Cargo's hyphen normalization.
    fn sdk_provider_rust_dependency_key(provider: &ProviderRecord) -> String {
        provider
            .artifact
            .as_ref()
            .map(|artifact| artifact.dependency_key.clone())
            .unwrap_or_else(|| provider.identity.name.replace('-', "_"))
    }

    /// Return the unique active SDK provider manifest that declares one nominal type.
    fn sdk_provider_manifest_for_type(&self, type_name: &str) -> Option<&LibraryManifest> {
        let mut matches = self
            .provider_plan
            .as_deref()?
            .active_sdk_records()
            .filter_map(|provider| provider.manifest.as_deref())
            .filter(|manifest| {
                manifest.exports.models.iter().any(|model| model.name == type_name)
                    || manifest.exports.classes.iter().any(|class| class.name == type_name)
                    || manifest.exports.enums.iter().any(|enum_| enum_.name == type_name)
                    || manifest
                        .exports
                        .newtypes
                        .iter()
                        .any(|newtype| newtype.name == type_name)
                    || manifest.contract_metadata.api.iter().any(|api| {
                        api.modules.iter().flat_map(|module| &module.declarations).any(|declaration| {
                            matches!(declaration, ApiDeclaration::Model(model) if model.name == type_name)
                                || matches!(declaration, ApiDeclaration::Class(class) if class.name == type_name)
                                || matches!(declaration, ApiDeclaration::Enum(enum_) if enum_.name == type_name)
                                || matches!(declaration, ApiDeclaration::Newtype(newtype) if newtype.name == type_name)
                        })
                    })
            });
        let manifest = matches.next()?;
        matches.next().is_none().then_some(manifest)
    }

    /// Return the unique compiled SDK provider crate that owns one nominal type.
    pub(in crate::backend::ir::lower) fn sdk_provider_crate_for_type(&self, type_name: &str) -> Option<String> {
        if self.struct_names.contains_key(type_name)
            || self.enum_names.contains_key(type_name)
            || self.class_decls.contains_key(type_name)
            || self.trait_decls.contains_key(type_name)
            || self.newtype_construction.contains_key(type_name)
            || self.source_type_alias_targets.contains_key(type_name)
        {
            return None;
        }
        let mut matches = self
            .provider_plan
            .as_deref()?
            .active_sdk_records()
            .filter(|provider| {
                provider.manifest.as_deref().is_some_and(|manifest| {
                    manifest.exports.models.iter().any(|model| model.name == type_name)
                        || manifest.exports.classes.iter().any(|class| class.name == type_name)
                        || manifest.exports.enums.iter().any(|enum_| enum_.name == type_name)
                        || manifest.exports.newtypes.iter().any(|newtype| newtype.name == type_name)
                        || manifest.contract_metadata.api.iter().any(|api| {
                            api.modules.iter().flat_map(|module| &module.declarations).any(|declaration| {
                                matches!(declaration, ApiDeclaration::Model(model) if model.name == type_name)
                                    || matches!(declaration, ApiDeclaration::Class(class) if class.name == type_name)
                                    || matches!(declaration, ApiDeclaration::Enum(enum_) if enum_.name == type_name)
                                    || matches!(declaration, ApiDeclaration::Newtype(newtype) if newtype.name == type_name)
                            })
                        })
                })
            });
        let provider = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some(Self::sdk_provider_rust_dependency_key(provider))
    }

    /// Resolve a compiled-stdlib method signature from a typed receiver, including artifact-owned defaults.
    ///
    /// Calls on local values such as `path.open("rb")` no longer retain the import expression that introduced
    /// `Path`, so import-path-only lookup cannot recover its omitted arguments. The checked artifact metadata is the
    /// canonical source for these consumer calls.
    pub(in crate::backend::ir::lower) fn callable_signature_for_compiled_provider_type_method(
        &mut self,
        receiver_ty: &IrType,
        method_name: &str,
    ) -> Option<FunctionSignature> {
        let type_name = Self::nominal_receiver_type_name(receiver_ty)?;
        if self.struct_names.contains_key(type_name) || self.enum_names.contains_key(type_name) {
            return None;
        }
        let provider_crate = self.sdk_provider_crate_for_type(type_name)?;
        let manifest = self.sdk_provider_manifest_for_type(type_name)?;
        let method = Self::api_method_export_for_pub_type(manifest, type_name, method_name)?;
        let signature = self.callable_signature_from_compiled_provider_method_export(&provider_crate, &method);
        Some(self.compiled_provider_external_signature(&provider_crate, signature))
    }

    /// Resolve an imported public dependency model/class method signature from the provider manifest.
    pub(in crate::backend::ir::lower) fn callable_signature_for_imported_pub_type_method(
        &mut self,
        library: &str,
        receiver_ty: &IrType,
        method_name: &str,
    ) -> Option<FunctionSignature> {
        let type_name = Self::nominal_receiver_type_name(receiver_ty)?;
        let manifest_index = self.provider_plan.as_deref()?.library_manifest_index();
        let LibraryManifestIndexEntry::Loaded { manifest, .. } = manifest_index.get(library)? else {
            return None;
        };
        let method = manifest
            .exports
            .models
            .iter()
            .find(|model| model.name == type_name)
            .and_then(|model| model.methods.iter().find(|method| method.name == method_name))
            .cloned()
            .or_else(|| {
                manifest
                    .exports
                    .classes
                    .iter()
                    .find(|class| class.name == type_name)
                    .and_then(|class| class.methods.iter().find(|method| method.name == method_name))
                    .cloned()
            })
            .or_else(|| {
                manifest
                    .exports
                    .newtypes
                    .iter()
                    .find(|newtype| newtype.name == type_name)
                    .and_then(|newtype| newtype.methods.iter().find(|method| method.name == method_name))
                    .cloned()
            })
            .or_else(|| {
                manifest
                    .exports
                    .enums
                    .iter()
                    .find(|enum_| enum_.name == type_name)
                    .and_then(|enum_| enum_.methods.iter().find(|method| method.name == method_name))
                    .cloned()
            })
            .or_else(|| Self::api_method_export_for_pub_type(manifest, type_name, method_name))?;
        Some(self.callable_signature_from_pub_method_export(library, &method))
    }

    /// Resolve methods for public types that are exposed only through facade aliases.
    ///
    /// The compact export list may contain `Frame -> exprs.Frame` as an alias rather than a full class export, while
    /// checked API metadata still records the original class declaration and methods. Backend method lookup must use
    /// that same target-path metadata or call planning diverges between direct provider modules and public facades.
    fn api_method_export_for_pub_type(
        manifest: &crate::library_manifest::LibraryManifest,
        type_name: &str,
        method_name: &str,
    ) -> Option<MethodExport> {
        let api = manifest.contract_metadata.api.as_ref()?;
        for alias in manifest.exports.aliases.iter().filter(|alias| alias.name == type_name) {
            if let Some(method) = Self::api_method_export_for_target_path(api, &alias.target_path, method_name) {
                return Some(method);
            }
        }
        api.modules
            .iter()
            .flat_map(|module| module.declarations.iter())
            .find_map(|declaration| Self::api_method_export_for_declaration(declaration, type_name, method_name))
    }

    /// Resolve one checked API method from a module-qualified public type target path.
    fn api_method_export_for_target_path(
        api: &crate::frontend::api_metadata::CheckedApiMetadataPackage,
        target_path: &[String],
        method_name: &str,
    ) -> Option<MethodExport> {
        let type_name = target_path.last()?;
        let path = if target_path
            .first()
            .is_some_and(|segment| segment == API_CRATE_ROOT_SEGMENT)
        {
            &target_path[1..]
        } else {
            target_path
        };
        let module_path = path.get(..path.len().saturating_sub(1))?;
        let module = api.modules.iter().find(|module| module.module_path == module_path)?;
        module
            .declarations
            .iter()
            .find_map(|declaration| Self::api_method_export_for_declaration(declaration, type_name, method_name))
    }

    /// Convert one checked API declaration into the method export requested by backend call planning.
    fn api_method_export_for_declaration(
        declaration: &ApiDeclaration,
        type_name: &str,
        method_name: &str,
    ) -> Option<MethodExport> {
        let methods = match declaration {
            ApiDeclaration::Model(model) if model.name == type_name => model.methods.as_slice(),
            ApiDeclaration::Class(class) if class.name == type_name => class.methods.as_slice(),
            ApiDeclaration::Enum(enum_) if enum_.name == type_name => enum_.methods.as_slice(),
            ApiDeclaration::Newtype(newtype) if newtype.name == type_name => newtype.methods.as_slice(),
            _ => return None,
        };
        methods
            .iter()
            .find(|method| method.name == method_name)
            .map(method_export_from_api)
    }

    /// Return the nominal receiver type name used for manifest method lookup.
    pub(in crate::backend::ir::lower) fn nominal_receiver_type_name(receiver_ty: &IrType) -> Option<&str> {
        match receiver_ty {
            IrType::Struct(name) | IrType::Enum(name) | IrType::NamedGeneric(name, _) => {
                Some(name.rsplit("::").next().unwrap_or(name))
            }
            IrType::Ref(inner) | IrType::RefMut(inner) => Self::nominal_receiver_type_name(inner),
            _ => None,
        }
    }

    /// Rebuild a public dependency method signature from manifest metadata.
    fn callable_signature_from_pub_method_export(&mut self, library: &str, method: &MethodExport) -> FunctionSignature {
        FunctionSignature {
            params: method
                .params
                .iter()
                .map(|param| {
                    let base_ty = self.lower_pub_manifest_type_ref(library, &param.ty);
                    let kind = param_kind_from_manifest(param.kind);
                    FunctionParam {
                        name: param.name.clone(),
                        ty: Self::lower_param_container_type(kind, base_ty),
                        mutability: Mutability::Immutable,
                        is_self: false,
                        kind,
                        default: self.lower_pub_param_default(library, param),
                    }
                })
                .collect(),
            return_type: self.lower_pub_manifest_type_ref(library, &method.return_type),
        }
    }

    /// Rebuild an artifact-backed stdlib method signature without reopening provider source modules.
    fn callable_signature_from_compiled_provider_method_export(
        &mut self,
        provider_crate: &str,
        method: &MethodExport,
    ) -> FunctionSignature {
        FunctionSignature {
            params: method
                .params
                .iter()
                .map(|param| {
                    let kind = param_kind_from_manifest(param.kind);
                    FunctionParam {
                        name: param.name.clone(),
                        ty: Self::lower_param_container_type(
                            kind,
                            self.lower_resolved_type(&crate::library_manifest::resolved_type_from_manifest_type_ref(
                                &param.ty,
                            )),
                        ),
                        mutability: Mutability::Immutable,
                        is_self: false,
                        kind,
                        default: self.lower_compiled_provider_param_default(provider_crate, param),
                    }
                })
                .collect(),
            return_type: self.lower_resolved_type(&crate::library_manifest::resolved_type_from_manifest_type_ref(
                &method.return_type,
            )),
        }
    }

    /// Rebuild an artifact-backed stdlib function signature without reopening provider source modules.
    fn callable_signature_from_compiled_provider_function_export(
        &mut self,
        provider_crate: &str,
        function: &FunctionExport,
    ) -> FunctionSignature {
        FunctionSignature {
            params: function
                .params
                .iter()
                .map(|param| {
                    let kind = param_kind_from_manifest(param.kind);
                    FunctionParam {
                        name: param.name.clone(),
                        ty: Self::lower_param_container_type(
                            kind,
                            self.lower_resolved_type(&crate::library_manifest::resolved_type_from_manifest_type_ref(
                                &param.ty,
                            )),
                        ),
                        mutability: Mutability::Immutable,
                        is_self: false,
                        kind,
                        default: self.lower_compiled_provider_param_default(provider_crate, param),
                    }
                })
                .collect(),
            return_type: self.lower_resolved_type(&crate::library_manifest::resolved_type_from_manifest_type_ref(
                &function.return_type,
            )),
        }
    }

    /// Lower manifest-safe literal defaults for a compiled stdlib method call.
    fn lower_compiled_provider_param_default(
        &mut self,
        provider_crate: &str,
        param: &ParamExport,
    ) -> Option<TypedExpr> {
        match param.default.as_ref() {
            Some(ParamDefaultExport::Unsupported) | None => None,
            Some(default) if default.is_materializable() => {
                self.lower_compiled_provider_default_expr(provider_crate, default)
            }
            Some(_) => None,
        }
    }

    /// Keep artifact-owned defaults in consumer IR without loading the stdlib source AST.
    fn lower_compiled_provider_default_expr(
        &mut self,
        provider_crate: &str,
        default: &ParamDefaultExport,
    ) -> Option<TypedExpr> {
        match default {
            ParamDefaultExport::Int(value) => Some(TypedExpr::new(IrExprKind::Int(*value), IrType::Int)),
            ParamDefaultExport::Float(value) => value
                .parse::<f64>()
                .ok()
                .map(|value| TypedExpr::new(IrExprKind::Float(value), IrType::Float)),
            ParamDefaultExport::Bool(value) => Some(TypedExpr::new(IrExprKind::Bool(*value), IrType::Bool)),
            ParamDefaultExport::String(value) => Some(TypedExpr::new(
                IrExprKind::Literal(IrLiteral::StaticStr(value.clone())),
                IrType::StaticStr,
            )),
            ParamDefaultExport::Bytes(value) => Some(TypedExpr::new(IrExprKind::Bytes(value.clone()), IrType::Bytes)),
            ParamDefaultExport::None => Some(TypedExpr::new(IrExprKind::None, IrType::Unit)),
            ParamDefaultExport::List(values) => Some(TypedExpr::new(
                IrExprKind::List(
                    values
                        .iter()
                        .map(|value| {
                            self.lower_compiled_provider_default_expr(provider_crate, value)
                                .map(IrListEntry::Element)
                        })
                        .collect::<Option<Vec<_>>>()?,
                ),
                IrType::List(Box::new(IrType::Unknown)),
            )),
            ParamDefaultExport::Dict(entries) => Some(TypedExpr::new(
                IrExprKind::Dict(
                    entries
                        .iter()
                        .map(|entry| {
                            Some(IrDictEntry::Pair(
                                self.lower_compiled_provider_default_expr(provider_crate, &entry.key)?,
                                Box::new(self.lower_compiled_provider_default_expr(provider_crate, &entry.value)?),
                            ))
                        })
                        .collect::<Option<Vec<_>>>()?,
                ),
                IrType::Dict(Box::new(IrType::Unknown), Box::new(IrType::Unknown)),
            )),
            ParamDefaultExport::ConstRef(path) => self.compiled_provider_path_expr(provider_crate, path),
            ParamDefaultExport::Call { path, args, signature } => {
                self.lower_compiled_provider_default_call(provider_crate, path, args, signature.as_ref())
            }
            ParamDefaultExport::Unsupported => None,
        }
    }

    /// Build an artifact-qualified value path such as `provider::__incan_std::logging::Level::WARN`.
    fn compiled_provider_path_expr(&self, provider_crate: &str, path: &[String]) -> Option<TypedExpr> {
        if path.is_empty() {
            return None;
        }
        let mut expr = TypedExpr::new(
            IrExprKind::Var {
                name: provider_crate.to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::ExternalName,
            },
            IrType::Unknown,
        );
        for segment in std::iter::once(stdlib::INCAN_STD_NAMESPACE).chain(
            path.iter()
                .map(String::as_str)
                .skip_while(|segment| *segment == stdlib::STDLIB_ROOT),
        ) {
            expr = TypedExpr::new(
                IrExprKind::Field {
                    object: Box::new(expr),
                    field: segment.to_string(),
                },
                IrType::Unknown,
            );
        }
        Some(expr)
    }

    /// Lower an artifact-owned call default with its checked signature and provider-qualified callee path.
    fn lower_compiled_provider_default_call(
        &mut self,
        provider_crate: &str,
        path: &[String],
        args: &[ParamDefaultCallArgExport],
        signature: Option<&ParamDefaultCallSignatureExport>,
    ) -> Option<TypedExpr> {
        path.last()?;
        let callable_signature = signature.map(|signature| {
            let signature = FunctionSignature {
                params: signature
                    .params
                    .iter()
                    .map(|param| {
                        let kind = param_kind_from_manifest(param.kind);
                        FunctionParam {
                            name: param.name.clone(),
                            ty: Self::lower_param_container_type(
                                kind,
                                self.lower_resolved_type(
                                    &crate::library_manifest::resolved_type_from_manifest_type_ref(&param.ty),
                                ),
                            ),
                            mutability: Mutability::Immutable,
                            is_self: false,
                            kind,
                            default: self.lower_compiled_provider_param_default(provider_crate, param),
                        }
                    })
                    .collect(),
                return_type: self.lower_resolved_type(&crate::library_manifest::resolved_type_from_manifest_type_ref(
                    &signature.return_type,
                )),
            };
            self.compiled_provider_external_signature(provider_crate, signature)
        });
        let return_type = callable_signature
            .as_ref()
            .map(|signature| signature.return_type.clone())
            .unwrap_or(IrType::Unknown);
        let args = args
            .iter()
            .map(|arg| {
                Some(IrCallArg {
                    name: arg.name.clone(),
                    kind: if arg.name.is_some() {
                        IrCallArgKind::Named
                    } else {
                        IrCallArgKind::Positional
                    },
                    expr: self.lower_compiled_provider_default_expr(provider_crate, &arg.value)?,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let func = self.compiled_provider_path_expr(provider_crate, path)?;
        let mut canonical_path = vec![stdlib::STDLIB_ROOT.to_string()];
        canonical_path.extend(
            path.iter()
                .cloned()
                .skip_while(|segment| segment == stdlib::STDLIB_ROOT),
        );
        Some(TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(func),
                type_args: Vec::new(),
                args,
                callable_signature,
                canonical_path: Some(canonical_path),
            },
            return_type,
        ))
    }

    /// Resolve the signature for an imported stdlib function by its canonical import path.
    ///
    /// Lowered stdlib modules may import private helpers from sibling stdlib modules. Those helpers are not in the
    /// current module's IR function registry, but their `.incn` declarations are still available through the stdlib AST
    /// cache. Attaching the exact module-qualified signature here lets codegen apply normal Incan argument conversion
    /// rules without merging same-named helpers from unrelated stdlib modules.
    pub(in crate::backend::ir::lower) fn callable_signature_for_imported_stdlib_path(
        &mut self,
        path: &[String],
    ) -> Result<Option<FunctionSignature>, LoweringError> {
        if path.len() < 2 || path.first().map(String::as_str) != Some(incan_core::lang::stdlib::STDLIB_ROOT) {
            return Ok(None);
        }
        let Some(function_name) = path.last() else {
            return Ok(None);
        };
        let module_path = &path[..path.len() - 1];
        if let Some(provider_crate) = self.sdk_provider_crate_for_module(module_path)
            && let Some(manifest) = self.sdk_provider_manifest_for_module(module_path)
            && let Some(api) = manifest.contract_metadata.api.as_ref()
        {
            let provider_module_path = if module_path.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT) {
                &module_path[1..]
            } else {
                module_path
            };
            if let Some(module) = api
                .modules
                .iter()
                .find(|module| module.module_path == provider_module_path)
                && let Some(function) = module
                    .declarations
                    .iter()
                    .find_map(|declaration| Self::api_function_export_for_declaration(declaration, function_name))
            {
                let signature =
                    self.callable_signature_from_compiled_provider_function_export(&provider_crate, &function);
                return Ok(Some(
                    self.compiled_provider_external_signature(&provider_crate, signature),
                ));
            }
        }
        let mut cache = crate::frontend::typechecker::stdlib_loader::StdlibAstCache::new();
        let Some(func) = cache.lookup_function_decl(module_path, function_name) else {
            return Ok(None);
        };
        self.callable_signature_from_stdlib_function_decl(&func).map(Some)
    }

    /// Resolve a callable signature from the callee expression's type information.
    ///
    /// This covers values whose type is already known as `Function(...)`, which is separate from call-site metadata
    /// gathered for defaults, named arguments, and other invocation-specific details.
    fn callable_signature_for_callee_span(&self, span: ast::Span) -> Option<FunctionSignature> {
        let info = self.type_info.as_ref()?;
        match info.expr_type(span)? {
            ResolvedType::Function(params, ret) => Some(self.callable_signature_from_params(params, ret)),
            _ => None,
        }
    }

    /// Wrap an expression with any RFC 017 validated-newtype coercion selected by the typechecker.
    pub(in crate::backend::ir::lower) fn wrap_with_validated_newtype_coercion(
        &mut self,
        mut expr: TypedExpr,
        span: ast::Span,
    ) -> Result<TypedExpr, LoweringError> {
        let Some(coercion) = self
            .type_info
            .as_ref()
            .and_then(|info| info.validated_newtype_coercion(span).cloned())
        else {
            return Ok(expr);
        };
        if matches!(coercion.mode, ValidatedNewtypeCoercionMode::AggregateField { .. }) {
            return Ok(expr);
        }

        for step in coercion.steps {
            let struct_ty = self
                .struct_names
                .get(&step.newtype_name)
                .cloned()
                .unwrap_or_else(|| IrType::Struct(step.newtype_name.clone()));
            expr = if let Some(ctor) = step.ctor {
                Self::checked_newtype_match_expr(&step.newtype_name, &ctor, expr, struct_ty)
            } else if !step.constraints.is_empty() {
                Self::generated_constrained_newtype_expr(&step.newtype_name, &step.constraints, expr, struct_ty)
            } else {
                TypedExpr::new(
                    IrExprKind::Struct {
                        name: step.newtype_name,
                        fields: vec![(String::new(), expr)],
                    },
                    struct_ty,
                )
            };
        }
        Ok(expr)
    }

    /// Build the fail-fast `Result` match used by checked newtype construction and implicit coercion.
    fn checked_newtype_match_expr(name: &str, ctor: &str, lowered_value: TypedExpr, struct_ty: IrType) -> TypedExpr {
        let receiver = TypedExpr::new(
            IrExprKind::Var {
                name: name.to_string(),
                access: VarAccess::Copy,
                ref_kind: VarRefKind::TypeName,
            },
            struct_ty.clone(),
        );
        let from_underlying_call = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(receiver),
                method: ctor.to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: lowered_value,
                }],
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Result(Box::new(struct_ty.clone()), Box::new(IrType::Unknown)),
        );
        let value_name = "__incan_newtype_value".to_string();
        let ok_arm = MatchArm {
            pattern: Pattern::Enum {
                name: "Result".to_string(),
                variant: constructors::as_str(ConstructorId::Ok).to_string(),
                fields: vec![Pattern::Var(value_name.clone())],
            },
            bindings: Vec::new(),
            guard: None,
            body: TypedExpr::new(
                IrExprKind::Var {
                    name: value_name,
                    access: VarAccess::Move,
                    ref_kind: VarRefKind::Value,
                },
                struct_ty.clone(),
            ),
        };
        let err_name = "__incan_validation_error".to_string();
        let err_arm = MatchArm {
            pattern: Pattern::Enum {
                name: "Result".to_string(),
                variant: constructors::as_str(ConstructorId::Err).to_string(),
                fields: vec![Pattern::Var(err_name.clone())],
            },
            bindings: Vec::new(),
            guard: None,
            body: TypedExpr::new(
                IrExprKind::Call {
                    func: Box::new(TypedExpr::new(
                        IrExprKind::Var {
                            name: "raise_validation_error".to_string(),
                            access: VarAccess::Read,
                            ref_kind: VarRefKind::Value,
                        },
                        IrType::Unknown,
                    )),
                    type_args: Vec::new(),
                    args: vec![
                        IrCallArg {
                            name: None,
                            kind: IrCallArgKind::Positional,
                            expr: TypedExpr::new(
                                IrExprKind::Literal(IrLiteral::StaticStr(name.to_string())),
                                IrType::StaticStr,
                            ),
                        },
                        IrCallArg {
                            name: None,
                            kind: IrCallArgKind::Positional,
                            expr: TypedExpr::new(
                                IrExprKind::Literal(IrLiteral::StaticStr(ctor.to_string())),
                                IrType::StaticStr,
                            ),
                        },
                        IrCallArg {
                            name: None,
                            kind: IrCallArgKind::Positional,
                            expr: TypedExpr::new(
                                IrExprKind::Var {
                                    name: err_name,
                                    access: VarAccess::Move,
                                    ref_kind: VarRefKind::Value,
                                },
                                IrType::Struct("ValidationError".to_string()),
                            ),
                        },
                    ],
                    callable_signature: None,
                    canonical_path: Some(vec![
                        "incan_stdlib".to_string(),
                        "validation".to_string(),
                        "raise_validation_error".to_string(),
                    ]),
                },
                struct_ty.clone(),
            ),
        };
        TypedExpr::new(
            IrExprKind::Match {
                scrutinee: Box::new(from_underlying_call),
                arms: vec![ok_arm, err_arm],
            },
            struct_ty,
        )
    }

    /// Build the generated checked-construction expression for a constrained primitive newtype.
    fn generated_constrained_newtype_expr(
        name: &str,
        constraints: &[NewtypePrimitiveConstraint],
        lowered_value: TypedExpr,
        struct_ty: IrType,
    ) -> TypedExpr {
        let value_name = "__incan_newtype_input".to_string();
        let value_ty = lowered_value.ty.clone();
        let value_ref = || {
            TypedExpr::new(
                IrExprKind::Var {
                    name: value_name.clone(),
                    access: VarAccess::Copy,
                    ref_kind: VarRefKind::Value,
                },
                value_ty.clone(),
            )
        };
        let condition = constraints
            .iter()
            .map(|constraint| Self::constraint_condition(value_ref(), constraint))
            .reduce(|left, right| {
                TypedExpr::new(
                    IrExprKind::BinOp {
                        op: super::super::super::expr::BinOp::And,
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                    IrType::Bool,
                )
            })
            .unwrap_or_else(|| TypedExpr::new(IrExprKind::Bool(true), IrType::Bool));
        let success = TypedExpr::new(
            IrExprKind::Struct {
                name: name.to_string(),
                fields: vec![(String::new(), value_ref())],
            },
            struct_ty.clone(),
        );
        let failed_constraint = constraints
            .iter()
            .map(|constraint| format!("{}={}", Self::constraint_key_name(constraint.key), constraint.repr))
            .collect::<Vec<_>>()
            .join(", ");
        let failure = TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "raise_constraint_error".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Unknown,
                )),
                type_args: Vec::new(),
                args: vec![
                    IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: TypedExpr::new(
                            IrExprKind::Literal(IrLiteral::StaticStr(name.to_string())),
                            IrType::StaticStr,
                        ),
                    },
                    IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: TypedExpr::new(
                            IrExprKind::Literal(IrLiteral::StaticStr(failed_constraint)),
                            IrType::StaticStr,
                        ),
                    },
                ],
                callable_signature: None,
                canonical_path: Some(vec![
                    "incan_stdlib".to_string(),
                    "validation".to_string(),
                    "raise_constraint_error".to_string(),
                ]),
            },
            struct_ty.clone(),
        );
        TypedExpr::new(
            IrExprKind::Block {
                stmts: vec![IrStmt::new(IrStmtKind::Let {
                    name: value_name,
                    ty: value_ty,
                    type_annotation: None,
                    mutability: Mutability::Immutable,
                    value: lowered_value,
                })],
                value: Some(Box::new(TypedExpr::new(
                    IrExprKind::If {
                        condition: Box::new(condition),
                        then_branch: Box::new(success),
                        else_branch: Some(Box::new(failure)),
                    },
                    struct_ty.clone(),
                ))),
            },
            struct_ty,
        )
    }

    /// Lower one constrained-primitive predicate into a boolean IR expression.
    fn constraint_condition(value: TypedExpr, constraint: &NewtypePrimitiveConstraint) -> TypedExpr {
        let op = match constraint.key {
            TypeConstraintKey::Ge => super::super::super::expr::BinOp::Ge,
            TypeConstraintKey::Gt => super::super::super::expr::BinOp::Gt,
            TypeConstraintKey::Le => super::super::super::expr::BinOp::Le,
            TypeConstraintKey::Lt => super::super::super::expr::BinOp::Lt,
        };
        let literal = if matches!(value.ty, IrType::Float) {
            TypedExpr::new(IrExprKind::Float(constraint.value as f64), IrType::Float)
        } else {
            TypedExpr::new(IrExprKind::Int(constraint.value), IrType::Int)
        };
        TypedExpr::new(
            IrExprKind::BinOp {
                op,
                left: Box::new(value),
                right: Box::new(literal),
            },
            IrType::Bool,
        )
    }

    /// Return the source spelling for a constrained-primitive predicate key.
    fn constraint_key_name(key: TypeConstraintKey) -> &'static str {
        match key {
            TypeConstraintKey::Ge => "ge",
            TypeConstraintKey::Gt => "gt",
            TypeConstraintKey::Le => "le",
            TypeConstraintKey::Lt => "lt",
        }
    }

    /// Build a call to `ValidationErrorsBuilder::new` for aggregated constructor validation.
    fn validation_builder_new(target: &str) -> TypedExpr {
        TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "new".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Unknown,
                )),
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::Literal(IrLiteral::StaticStr(target.to_string())),
                        IrType::StaticStr,
                    ),
                }],
                callable_signature: None,
                canonical_path: Some(vec![
                    "incan_stdlib".to_string(),
                    "validation".to_string(),
                    "ValidationErrorsBuilder".to_string(),
                    "new".to_string(),
                ]),
            },
            IrType::Struct("ValidationErrorsBuilder".to_string()),
        )
    }

    /// Build an IR variable reference to the current validation-error builder.
    fn validation_builder_var(name: &str, access: VarAccess) -> TypedExpr {
        TypedExpr::new(
            IrExprKind::Var {
                name: name.to_string(),
                access,
                ref_kind: VarRefKind::Value,
            },
            IrType::Struct("ValidationErrorsBuilder".to_string()),
        )
    }

    /// Return the IR type used for runtime validation errors.
    fn validation_error_ty() -> IrType {
        IrType::Struct("ValidationError".to_string())
    }

    /// Build a receiver `.clone()` call for payloads that are intentionally reused by generated validation code.
    fn clone_expr(expr: TypedExpr) -> TypedExpr {
        TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(expr.clone()),
                method: "clone".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: Vec::new(),
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            expr.ty.clone(),
        )
    }

    /// Build an explicitly typed `Ok::<T, ValidationError>(value)` call.
    fn result_ok_expr(value: TypedExpr, ok_ty: IrType) -> TypedExpr {
        TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: constructors::as_str(ConstructorId::Ok).to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Unknown,
                )),
                type_args: vec![ok_ty.clone(), Self::validation_error_ty()],
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: value,
                }],
                callable_signature: None,
                canonical_path: None,
            },
            IrType::Result(Box::new(ok_ty), Box::new(Self::validation_error_ty())),
        )
    }

    /// Build an explicitly typed `Err::<T, ValidationError>(error)` call.
    fn result_err_expr(error: TypedExpr, ok_ty: IrType) -> TypedExpr {
        TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: constructors::as_str(ConstructorId::Err).to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Unknown,
                )),
                type_args: vec![ok_ty.clone(), Self::validation_error_ty()],
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: error,
                }],
                callable_signature: None,
                canonical_path: None,
            },
            IrType::Result(Box::new(ok_ty), Box::new(Self::validation_error_ty())),
        )
    }

    /// Build a typed result expression for one validated-newtype coercion step without panicking.
    fn validated_newtype_step_result_expr(
        name: &str,
        ctor: Option<&str>,
        constraints: &[NewtypePrimitiveConstraint],
        lowered_value: TypedExpr,
        struct_ty: IrType,
    ) -> TypedExpr {
        if let Some(ctor) = ctor {
            let receiver = TypedExpr::new(
                IrExprKind::Var {
                    name: name.to_string(),
                    access: VarAccess::Copy,
                    ref_kind: VarRefKind::TypeName,
                },
                struct_ty.clone(),
            );
            return TypedExpr::new(
                IrExprKind::MethodCall {
                    receiver: Box::new(receiver),
                    method: ctor.to_string(),
                    dispatch: None,
                    type_args: Vec::new(),
                    args: vec![IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: lowered_value,
                    }],
                    callable_signature: None,
                    arg_policy: MethodCallArgPolicy::Default,
                },
                IrType::Result(Box::new(struct_ty), Box::new(Self::validation_error_ty())),
            );
        }

        if !constraints.is_empty() {
            return Self::constrained_newtype_result_expr(name, constraints, lowered_value, struct_ty);
        }

        Self::result_ok_expr(
            TypedExpr::new(
                IrExprKind::Struct {
                    name: name.to_string(),
                    fields: vec![(String::new(), lowered_value)],
                },
                struct_ty.clone(),
            ),
            struct_ty,
        )
    }

    /// Build a generated constrained-newtype validation result without raising.
    fn constrained_newtype_result_expr(
        name: &str,
        constraints: &[NewtypePrimitiveConstraint],
        lowered_value: TypedExpr,
        struct_ty: IrType,
    ) -> TypedExpr {
        let condition = constraints
            .iter()
            .map(|constraint| Self::constraint_condition(lowered_value.clone(), constraint))
            .reduce(|left, right| {
                TypedExpr::new(
                    IrExprKind::BinOp {
                        op: super::super::super::expr::BinOp::And,
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                    IrType::Bool,
                )
            })
            .unwrap_or_else(|| TypedExpr::new(IrExprKind::Bool(true), IrType::Bool));
        let success = Self::result_ok_expr(
            TypedExpr::new(
                IrExprKind::Struct {
                    name: name.to_string(),
                    fields: vec![(String::new(), lowered_value)],
                },
                struct_ty.clone(),
            ),
            struct_ty.clone(),
        );
        let failed_constraint = constraints
            .iter()
            .map(|constraint| format!("{}={}", Self::constraint_key_name(constraint.key), constraint.repr))
            .collect::<Vec<_>>()
            .join(", ");
        let failure_error = TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "new".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Unknown,
                )),
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::Literal(IrLiteral::StaticStr(format!(
                            "{name} constraint {failed_constraint} failed"
                        ))),
                        IrType::StaticStr,
                    ),
                }],
                callable_signature: None,
                canonical_path: Some(vec![
                    "incan_stdlib".to_string(),
                    "validation".to_string(),
                    "ValidationError".to_string(),
                    "new".to_string(),
                ]),
            },
            Self::validation_error_ty(),
        );
        let failure = Self::result_err_expr(failure_error, struct_ty.clone());
        TypedExpr::new(
            IrExprKind::If {
                condition: Box::new(condition),
                then_branch: Box::new(success),
                else_branch: Some(Box::new(failure)),
            },
            IrType::Result(Box::new(struct_ty), Box::new(Self::validation_error_ty())),
        )
    }

    /// Feed an `Ok` value into the next newtype step while preserving an existing `Err`.
    fn chained_validated_newtype_result_expr(
        previous_result_name: &str,
        previous_ok_ty: IrType,
        next_name: &str,
        next_ctor: Option<&str>,
        next_constraints: &[NewtypePrimitiveConstraint],
        next_ty: IrType,
    ) -> TypedExpr {
        let value_name = "__incan_chained_newtype_value".to_string();
        let error_name = "__incan_chained_newtype_error".to_string();
        let ok_value = TypedExpr::new(
            IrExprKind::Var {
                name: value_name.clone(),
                access: VarAccess::Move,
                ref_kind: VarRefKind::Value,
            },
            previous_ok_ty.clone(),
        );
        let ok_arm = MatchArm {
            pattern: Pattern::Enum {
                name: "Result".to_string(),
                variant: constructors::as_str(ConstructorId::Ok).to_string(),
                fields: vec![Pattern::Var(value_name)],
            },
            bindings: Vec::new(),
            guard: None,
            body: Self::validated_newtype_step_result_expr(
                next_name,
                next_ctor,
                next_constraints,
                ok_value,
                next_ty.clone(),
            ),
        };
        let err_arm = MatchArm {
            pattern: Pattern::Enum {
                name: "Result".to_string(),
                variant: constructors::as_str(ConstructorId::Err).to_string(),
                fields: vec![Pattern::Var(error_name.clone())],
            },
            bindings: Vec::new(),
            guard: None,
            body: Self::result_err_expr(
                TypedExpr::new(
                    IrExprKind::Var {
                        name: error_name,
                        access: VarAccess::Move,
                        ref_kind: VarRefKind::Value,
                    },
                    Self::validation_error_ty(),
                ),
                next_ty.clone(),
            ),
        };
        TypedExpr::new(
            IrExprKind::Match {
                scrutinee: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: previous_result_name.to_string(),
                        access: VarAccess::Move,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Result(Box::new(previous_ok_ty), Box::new(Self::validation_error_ty())),
                )),
                arms: vec![ok_arm, err_arm],
            },
            IrType::Result(Box::new(next_ty), Box::new(Self::validation_error_ty())),
        )
    }

    /// Build a statement that appends one field validation error to the aggregate builder.
    fn push_field_error_stmt(builder_name: &str, field_name: &str, error_expr: TypedExpr) -> IrStmt {
        IrStmt::new(IrStmtKind::Expr(TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(Self::validation_builder_var(builder_name, VarAccess::Read)),
                method: "push_field_error".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: vec![
                    IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: TypedExpr::new(
                            IrExprKind::Literal(IrLiteral::StaticStr(field_name.to_string())),
                            IrType::StaticStr,
                        ),
                    },
                    IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: error_expr,
                    },
                ],
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Unit,
        )))
    }

    /// Build an expression that records `Err` and returns the same `Result` shape.
    fn record_result_error_expr(builder_name: &str, field_name: &str, result_name: &str, ok_ty: IrType) -> TypedExpr {
        let error_name = format!("__incan_{field_name}_validation_error");
        let value_name = format!("__incan_{field_name}_validation_value");
        let ok_arm = MatchArm {
            pattern: Pattern::Enum {
                name: "Result".to_string(),
                variant: constructors::as_str(ConstructorId::Ok).to_string(),
                fields: vec![Pattern::Var(value_name.clone())],
            },
            bindings: Vec::new(),
            guard: None,
            body: Self::result_ok_expr(
                TypedExpr::new(
                    IrExprKind::Var {
                        name: value_name,
                        access: VarAccess::Move,
                        ref_kind: VarRefKind::Value,
                    },
                    ok_ty.clone(),
                ),
                ok_ty.clone(),
            ),
        };
        let err_var = TypedExpr::new(
            IrExprKind::Var {
                name: error_name.clone(),
                access: VarAccess::Move,
                ref_kind: VarRefKind::Value,
            },
            Self::validation_error_ty(),
        );
        let err_arm = MatchArm {
            pattern: Pattern::Enum {
                name: "Result".to_string(),
                variant: constructors::as_str(ConstructorId::Err).to_string(),
                fields: vec![Pattern::Var(error_name.clone())],
            },
            bindings: Vec::new(),
            guard: None,
            body: TypedExpr::new(
                IrExprKind::Block {
                    stmts: vec![Self::push_field_error_stmt(
                        builder_name,
                        field_name,
                        Self::clone_expr(err_var.clone()),
                    )],
                    value: Some(Box::new(Self::result_err_expr(err_var, ok_ty.clone()))),
                },
                IrType::Result(Box::new(ok_ty.clone()), Box::new(Self::validation_error_ty())),
            ),
        };
        TypedExpr::new(
            IrExprKind::Match {
                scrutinee: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: result_name.to_string(),
                        access: VarAccess::Move,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Result(Box::new(ok_ty.clone()), Box::new(Self::validation_error_ty())),
                )),
                arms: vec![ok_arm, err_arm],
            },
            IrType::Result(Box::new(ok_ty), Box::new(Self::validation_error_ty())),
        )
    }

    /// Build the statement that raises the aggregate error after all fields are checked.
    fn validation_builder_raise_stmt(builder_name: &str) -> IrStmt {
        IrStmt::new(IrStmtKind::Expr(TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(Self::validation_builder_var(builder_name, VarAccess::Move)),
                method: "raise_if_any".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: Vec::new(),
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Unit,
        )))
    }

    /// Extract the `Ok` value from a checked-construction result after aggregate validation has run.
    fn result_value_match_expr(name: &str, ctor: &str, result_name: &str, struct_ty: IrType) -> TypedExpr {
        let value_name = "__incan_newtype_value".to_string();
        let err_name = "__incan_validation_error".to_string();
        let ok_arm = MatchArm {
            pattern: Pattern::Enum {
                name: "Result".to_string(),
                variant: constructors::as_str(ConstructorId::Ok).to_string(),
                fields: vec![Pattern::Var(value_name.clone())],
            },
            bindings: Vec::new(),
            guard: None,
            body: TypedExpr::new(
                IrExprKind::Var {
                    name: value_name,
                    access: VarAccess::Move,
                    ref_kind: VarRefKind::Value,
                },
                struct_ty.clone(),
            ),
        };
        let err_arm = MatchArm {
            pattern: Pattern::Enum {
                name: "Result".to_string(),
                variant: constructors::as_str(ConstructorId::Err).to_string(),
                fields: vec![Pattern::Var(err_name.clone())],
            },
            bindings: Vec::new(),
            guard: None,
            body: TypedExpr::new(
                IrExprKind::Call {
                    func: Box::new(TypedExpr::new(
                        IrExprKind::Var {
                            name: "raise_validation_error".to_string(),
                            access: VarAccess::Read,
                            ref_kind: VarRefKind::Value,
                        },
                        IrType::Unknown,
                    )),
                    type_args: Vec::new(),
                    args: vec![
                        IrCallArg {
                            name: None,
                            kind: IrCallArgKind::Positional,
                            expr: TypedExpr::new(
                                IrExprKind::Literal(IrLiteral::StaticStr(name.to_string())),
                                IrType::StaticStr,
                            ),
                        },
                        IrCallArg {
                            name: None,
                            kind: IrCallArgKind::Positional,
                            expr: TypedExpr::new(
                                IrExprKind::Literal(IrLiteral::StaticStr(ctor.to_string())),
                                IrType::StaticStr,
                            ),
                        },
                        IrCallArg {
                            name: None,
                            kind: IrCallArgKind::Positional,
                            expr: TypedExpr::new(
                                IrExprKind::Var {
                                    name: err_name,
                                    access: VarAccess::Move,
                                    ref_kind: VarRefKind::Value,
                                },
                                Self::validation_error_ty(),
                            ),
                        },
                    ],
                    callable_signature: None,
                    canonical_path: Some(vec![
                        "incan_stdlib".to_string(),
                        "validation".to_string(),
                        "raise_validation_error".to_string(),
                    ]),
                },
                struct_ty.clone(),
            ),
        };
        TypedExpr::new(
            IrExprKind::Match {
                scrutinee: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: result_name.to_string(),
                        access: VarAccess::Move,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Result(Box::new(struct_ty.clone()), Box::new(Self::validation_error_ty())),
                )),
                arms: vec![ok_arm, err_arm],
            },
            struct_ty,
        )
    }

    /// Return aggregate-mode coercion metadata for a constructor field expression span.
    fn aggregate_field_coercion(
        &self,
        span: ast::Span,
    ) -> Option<crate::frontend::typechecker::ValidatedNewtypeCoercionInfo> {
        self.type_info
            .as_ref()
            .and_then(|info| info.validated_newtype_coercion(span).cloned())
            .filter(|coercion| matches!(coercion.mode, ValidatedNewtypeCoercionMode::AggregateField { .. }))
    }

    /// Return whether a constructor call contains any fields needing aggregate validation.
    fn has_aggregate_constructor_fields(&self, args: &[ast::CallArg]) -> bool {
        args.iter().any(|arg| match arg {
            ast::CallArg::Named(_, expr) => self.aggregate_field_coercion(expr.span).is_some(),
            _ => false,
        })
    }

    /// Lower a model/class constructor call that must aggregate field validation errors.
    fn lower_aggregate_constructor_call(
        &mut self,
        name: &str,
        args: &[ast::CallArg],
        struct_ty: IrType,
    ) -> Result<(IrExprKind, IrType), LoweringError> {
        let builder_name = "__incan_validation_errors".to_string();
        let mut stmts = vec![IrStmt::new(IrStmtKind::Let {
            name: builder_name.clone(),
            ty: IrType::Struct("ValidationErrorsBuilder".to_string()),
            type_annotation: None,
            mutability: Mutability::Mutable,
            value: Self::validation_builder_new(name),
        })];
        let mut fields = Vec::new();

        for (idx, arg) in args.iter().enumerate() {
            let value = Self::call_arg_expr(arg);
            let lowered_value = self.lower_expr_spanned(value)?;
            let raw_name = format!("__incan_field_{idx}_raw");
            let raw_ty = lowered_value.ty.clone();
            stmts.push(IrStmt::new(IrStmtKind::Let {
                name: raw_name.clone(),
                ty: raw_ty.clone(),
                type_annotation: None,
                mutability: Mutability::Immutable,
                value: lowered_value,
            }));
            let raw_var = |access| {
                TypedExpr::new(
                    IrExprKind::Var {
                        name: raw_name.clone(),
                        access,
                        ref_kind: VarRefKind::Value,
                    },
                    raw_ty.clone(),
                )
            };
            match arg {
                ast::CallArg::Named(field_name, _) => {
                    let canonical = self.resolve_field_alias(name, field_name);
                    let Some(coercion) = self.aggregate_field_coercion(value.span) else {
                        fields.push((canonical, raw_var(VarAccess::Move)));
                        continue;
                    };

                    let mut current_result_name = None;
                    let mut current_ok_ty = raw_ty.clone();
                    let mut final_newtype_name = None;
                    let mut final_ctor_name = None;
                    for (step_idx, step) in coercion.steps.iter().enumerate() {
                        let step_ty = self
                            .struct_names
                            .get(&step.newtype_name)
                            .cloned()
                            .unwrap_or_else(|| IrType::Struct(step.newtype_name.clone()));
                        let result_name = format!("__incan_field_{idx}_{step_idx}_result");
                        let result_expr = if let Some(previous_name) = current_result_name.as_deref() {
                            Self::chained_validated_newtype_result_expr(
                                previous_name,
                                current_ok_ty.clone(),
                                &step.newtype_name,
                                step.ctor.as_deref(),
                                &step.constraints,
                                step_ty.clone(),
                            )
                        } else {
                            Self::validated_newtype_step_result_expr(
                                &step.newtype_name,
                                step.ctor.as_deref(),
                                &step.constraints,
                                raw_var(VarAccess::Copy),
                                step_ty,
                            )
                        };
                        let result_ty = result_expr.ty.clone();
                        stmts.push(IrStmt::new(IrStmtKind::Let {
                            name: result_name.clone(),
                            ty: result_ty,
                            type_annotation: None,
                            mutability: Mutability::Immutable,
                            value: result_expr,
                        }));
                        current_result_name = Some(result_name);
                        current_ok_ty = self
                            .struct_names
                            .get(&step.newtype_name)
                            .cloned()
                            .unwrap_or_else(|| IrType::Struct(step.newtype_name.clone()));
                        final_newtype_name = Some(step.newtype_name.clone());
                        final_ctor_name = step
                            .ctor
                            .clone()
                            .or_else(|| (!step.constraints.is_empty()).then(|| "constraint".to_string()))
                            .or_else(|| Some("constructor".to_string()));
                    }
                    let Some(result_name) = current_result_name else {
                        fields.push((canonical, raw_var(VarAccess::Move)));
                        continue;
                    };
                    let recorded_result_name = format!("__incan_field_{idx}_validated_result");
                    stmts.push(IrStmt::new(IrStmtKind::Let {
                        name: recorded_result_name.clone(),
                        ty: IrType::Result(Box::new(current_ok_ty.clone()), Box::new(Self::validation_error_ty())),
                        type_annotation: None,
                        mutability: Mutability::Immutable,
                        value: Self::record_result_error_expr(
                            &builder_name,
                            &canonical,
                            &result_name,
                            current_ok_ty.clone(),
                        ),
                    }));
                    fields.push((
                        canonical,
                        Self::result_value_match_expr(
                            final_newtype_name.as_deref().unwrap_or(name),
                            final_ctor_name.as_deref().unwrap_or("constructor"),
                            &recorded_result_name,
                            current_ok_ty,
                        ),
                    ));
                }
                ast::CallArg::Positional(_) => {
                    fields.push((String::new(), raw_var(VarAccess::Move)));
                }
                ast::CallArg::PositionalUnpack(_) | ast::CallArg::KeywordUnpack(_) => {
                    fields.push((String::new(), raw_var(VarAccess::Move)));
                }
            }
        }
        stmts.push(Self::validation_builder_raise_stmt(&builder_name));
        Ok((
            IrExprKind::Block {
                stmts,
                value: Some(Box::new(TypedExpr::new(
                    IrExprKind::Struct {
                        name: name.to_string(),
                        fields,
                    },
                    struct_ty.clone(),
                ))),
            },
            struct_ty,
        ))
    }

    /// Return the typechecker-proven callable signature for a full call expression span.
    pub(in crate::backend::ir::lower) fn callable_signature_for_call_span(
        &self,
        span: ast::Span,
    ) -> Option<FunctionSignature> {
        let info = self.type_info.as_ref()?;
        let params = info.call_site_callable_params(span)?;
        Some(FunctionSignature {
            params: self
                .callable_signature_from_params(params, &ResolvedType::Unknown)
                .params,
            return_type: IrType::Unknown,
        })
    }

    /// Prefer monomorphized call-site type args from the typechecker (RFC 054); otherwise lower AST types.
    pub(super) fn lower_call_site_type_args(
        &self,
        call_span: ast::Span,
        type_args: &[ast::Spanned<ast::Type>],
    ) -> Vec<IrType> {
        if let Some(info) = self.type_info.as_ref()
            && let Some(resolved) = info
                .calls
                .call_site_monomorph_type_args
                .get(&(call_span.start, call_span.end))
        {
            return resolved.iter().map(|t| self.lower_resolved_type(t)).collect();
        }
        type_args.iter().map(|ty| self.lower_type(&ty.node)).collect()
    }

    /// Return the expression carried by a call argument.
    fn call_arg_expr(arg: &ast::CallArg) -> &ast::Spanned<ast::Expr> {
        match arg {
            ast::CallArg::Positional(e)
            | ast::CallArg::Named(_, e)
            | ast::CallArg::PositionalUnpack(e)
            | ast::CallArg::KeywordUnpack(e) => e,
        }
    }

    /// Return whether passing `arg` to a callable parameter should refine that parameter to a shared borrow.
    fn callable_arg_needs_implicit_borrow(arg: &TypedExpr, target_ty: &IrType) -> bool {
        if arg.ty.is_copy() || matches!(target_ty, IrType::Ref(_) | IrType::RefMut(_)) {
            return false;
        }
        matches!(
            arg.kind,
            IrExprKind::Var {
                access: VarAccess::Read | VarAccess::Borrow,
                ..
            }
        )
    }

    /// Refine a function-typed local parameter call when borrowing preserves a non-`Copy` argument for later use.
    fn refine_function_typed_local_call(
        &mut self,
        func: &mut TypedExpr,
        args: &[IrCallArg],
        callable_signature: Option<FunctionSignature>,
    ) -> Option<FunctionSignature> {
        let IrExprKind::Var {
            name,
            ref_kind: VarRefKind::Value,
            ..
        } = &func.kind
        else {
            return callable_signature;
        };
        let local_name = name.clone();
        if !self.current_callable_param_scope_contains(&local_name) {
            return callable_signature;
        }

        let IrType::Function { params, ret } = &func.ty else {
            return callable_signature;
        };
        let mut signature =
            callable_signature.unwrap_or_else(|| FunctionSignature::from_function_type(params, ret.as_ref()));
        let mut changed = false;

        for (idx, arg) in args.iter().enumerate() {
            if !matches!(arg.kind, IrCallArgKind::Positional | IrCallArgKind::Named) {
                continue;
            }
            let Some(param) = signature.params.get_mut(idx) else {
                continue;
            };
            if Self::callable_arg_needs_implicit_borrow(&arg.expr, &param.ty) {
                param.ty = IrType::Ref(Box::new(param.ty.clone()));
                changed = true;
            }
        }

        if changed {
            let refined_ty = IrType::Function {
                params: signature.params.iter().map(|param| param.ty.clone()).collect(),
                ret: Box::new(signature.return_type.clone()),
            };
            func.ty = refined_ty.clone();
            self.update_local_binding(&local_name, refined_ty);
        }

        Some(signature)
    }

    fn lower_adapter_kind(adapter_kind: ast::InteropAdapterKind) -> super::super::super::decl::IrInteropAdapterKind {
        match adapter_kind {
            ast::InteropAdapterKind::Via => super::super::super::decl::IrInteropAdapterKind::Via,
            ast::InteropAdapterKind::Try => super::super::super::decl::IrInteropAdapterKind::Try,
        }
    }

    /// Lower a rusttype interop adapter into IR.
    fn lower_rusttype_interop_adapter(
        &mut self,
        arg_ty: &IrType,
        target_ty: &IrType,
    ) -> Result<Option<(TypedExpr, super::super::super::decl::IrInteropAdapterKind)>, LoweringError> {
        if let Some(type_name) = arg_ty.nominal_type_name()
            && let Some(edges) = self.rusttype_interop_edges.get(type_name).cloned()
        {
            for edge in edges {
                if !matches!(edge.direction, ast::InteropDirection::Into) {
                    continue;
                }
                let edge_ty = self.lower_type(&edge.ty.node);
                if edge_ty != *target_ty {
                    continue;
                }
                let adapter_expr = self.lower_expr_spanned(&edge.adapter)?;
                return Ok(Some((adapter_expr, Self::lower_adapter_kind(edge.adapter_kind))));
            }
        }

        if let Some(type_name) = target_ty.nominal_type_name()
            && let Some(edges) = self.rusttype_interop_edges.get(type_name).cloned()
        {
            for edge in edges {
                if !matches!(edge.direction, ast::InteropDirection::From) {
                    continue;
                }
                let edge_ty = self.lower_type(&edge.ty.node);
                if edge_ty != *arg_ty {
                    continue;
                }
                let adapter_expr = self.lower_expr_spanned(&edge.adapter)?;
                return Ok(Some((adapter_expr, Self::lower_adapter_kind(edge.adapter_kind))));
            }
        }

        Ok(None)
    }

    /// Wrap a Rust call result in an `InteropCoerce` node when the typechecker recorded a return coercion for the
    /// expression span.
    ///
    /// This handles metadata-backed Rust calls that surface borrowed scalar-like returns (`&str`, `&[u8]`) as owned
    /// Incan values. The typechecker records the mismatch; lowering inserts `.to_string()` or `.to_vec()` before the
    /// value reaches ordinary Incan storage and return sites.
    pub(in crate::backend::ir::lower) fn wrap_with_rust_return_coercion(
        &mut self,
        expr: TypedExpr,
        span: ast::Span,
    ) -> Result<TypedExpr, LoweringError> {
        let coercion = self
            .type_info
            .as_ref()
            .and_then(|info| info.rust_return_coercion(span).cloned());
        let Some(coercion) = coercion else {
            return Ok(expr);
        };
        // Return coercions are always Builtin; RustTypeUnwrap / RustTypeInterop do not apply here.
        let RustArgCoercionKind::Builtin(policy) = coercion.kind else {
            return Ok(expr);
        };
        let target_ty = self.lower_resolved_type(&coercion.target_type);
        let from_ty = expr.ty.clone();
        Ok(TypedExpr::new(
            IrExprKind::InteropCoerce {
                expr: Box::new(expr),
                from_ty,
                to_ty: target_ty.clone(),
                kind: IrInteropCoercionKind::Builtin {
                    policy,
                    rust_target: coercion.rust_target_type,
                },
            },
            target_ty,
        ))
    }

    /// Wrap one call argument in `InteropCoerce` when typechecking recorded a Rust boundary coercion.
    ///
    /// For `RustTypeInterop`, lowering first attempts to resolve a declared `interop:` adapter. If no
    /// adapter edge matches, lowering falls back to `RustTypeUnwrap` so the generated Rust call still
    /// receives the underlying Rust value.
    pub(in crate::backend::ir::lower) fn wrap_with_rust_arg_coercion(
        &mut self,
        arg_expr: TypedExpr,
        span: ast::Span,
    ) -> Result<TypedExpr, LoweringError> {
        let coercion = self
            .type_info
            .as_ref()
            .and_then(|info| info.rust_arg_coercion(span).cloned());
        let Some(coercion) = coercion else {
            return Ok(arg_expr);
        };
        let target_ty = self.lower_rust_boundary_target_type(&coercion.target_type);
        let from_ty = arg_expr.ty.clone();
        let kind = match coercion.kind {
            RustArgCoercionKind::Builtin(policy) => IrInteropCoercionKind::Builtin {
                policy,
                rust_target: coercion.rust_target_type,
            },
            RustArgCoercionKind::RustTypeUnwrap => IrInteropCoercionKind::RustTypeUnwrap,
            RustArgCoercionKind::RustTypeInterop => {
                if let Some((adapter, adapter_kind)) = self.lower_rusttype_interop_adapter(&from_ty, &target_ty)? {
                    IrInteropCoercionKind::AdapterCall {
                        adapter: Box::new(adapter),
                        adapter_kind,
                    }
                } else {
                    IrInteropCoercionKind::RustTypeUnwrap
                }
            }
            RustArgCoercionKind::TraitObjectBorrow { mutable } => IrInteropCoercionKind::TraitObjectBorrow { mutable },
        };
        Ok(TypedExpr::new(
            IrExprKind::InteropCoerce {
                expr: Box::new(arg_expr),
                from_ty,
                to_ty: target_ty.clone(),
                kind,
            },
            target_ty,
        ))
    }

    /// Lower the typechecker-selected Rust boundary target without collapsing borrowed Rust slices into owned values.
    ///
    /// General source-level references lower as `Ref<T>`, but Rust argument coercions use the target type as a backend
    /// contract. A `&str` parameter therefore lowers to `StrRef`, while `&String` remains a reference to the owned Rust
    /// string target recorded by the frontend.
    fn lower_rust_boundary_target_type(&self, target_ty: &ResolvedType) -> IrType {
        match target_ty {
            ResolvedType::Ref(inner) if matches!(inner.as_ref(), ResolvedType::Str) => IrType::StrRef,
            ResolvedType::Ref(inner) => IrType::Ref(Box::new(self.lower_rust_boundary_target_type(inner))),
            ResolvedType::RefMut(inner) => IrType::RefMut(Box::new(self.lower_rust_boundary_target_type(inner))),
            ResolvedType::TypeVar(_) => IrType::Unknown,
            ResolvedType::Tuple(items) => IrType::Tuple(
                items
                    .iter()
                    .map(|item| self.lower_rust_boundary_target_type(item))
                    .collect(),
            ),
            ResolvedType::FrozenList(inner) => IrType::NamedGeneric(
                collections::as_str(CollectionTypeId::FrozenList).to_string(),
                vec![self.lower_rust_boundary_target_type(inner)],
            ),
            ResolvedType::FrozenSet(inner) => IrType::NamedGeneric(
                collections::as_str(CollectionTypeId::FrozenSet).to_string(),
                vec![self.lower_rust_boundary_target_type(inner)],
            ),
            ResolvedType::FrozenDict(key, value) => IrType::NamedGeneric(
                collections::as_str(CollectionTypeId::FrozenDict).to_string(),
                vec![
                    self.lower_rust_boundary_target_type(key),
                    self.lower_rust_boundary_target_type(value),
                ],
            ),
            ResolvedType::Generic(name, args) => match collections::from_str(name.as_str()) {
                Some(CollectionTypeId::List) => IrType::List(Box::new(
                    args.first()
                        .map(|arg| self.lower_rust_boundary_target_type(arg))
                        .unwrap_or(IrType::Unknown),
                )),
                Some(CollectionTypeId::Dict) => IrType::Dict(
                    Box::new(
                        args.first()
                            .map(|arg| self.lower_rust_boundary_target_type(arg))
                            .unwrap_or(IrType::Unknown),
                    ),
                    Box::new(
                        args.get(1)
                            .map(|arg| self.lower_rust_boundary_target_type(arg))
                            .unwrap_or(IrType::Unknown),
                    ),
                ),
                Some(CollectionTypeId::Set) => IrType::Set(Box::new(
                    args.first()
                        .map(|arg| self.lower_rust_boundary_target_type(arg))
                        .unwrap_or(IrType::Unknown),
                )),
                Some(CollectionTypeId::Option) => IrType::Option(Box::new(
                    args.first()
                        .map(|arg| self.lower_rust_boundary_target_type(arg))
                        .unwrap_or(IrType::Unknown),
                )),
                Some(CollectionTypeId::Result) => IrType::Result(
                    Box::new(
                        args.first()
                            .map(|arg| self.lower_rust_boundary_target_type(arg))
                            .unwrap_or(IrType::Unknown),
                    ),
                    Box::new(
                        args.get(1)
                            .map(|arg| self.lower_rust_boundary_target_type(arg))
                            .unwrap_or(IrType::Unknown),
                    ),
                ),
                Some(CollectionTypeId::Tuple) => IrType::Tuple(
                    args.iter()
                        .map(|arg| self.lower_rust_boundary_target_type(arg))
                        .collect(),
                ),
                Some(
                    id @ (CollectionTypeId::FrozenList
                    | CollectionTypeId::FrozenSet
                    | CollectionTypeId::FrozenDict
                    | CollectionTypeId::Generator),
                ) => IrType::NamedGeneric(
                    collections::as_str(id).to_string(),
                    args.iter()
                        .map(|arg| self.lower_rust_boundary_target_type(arg))
                        .collect(),
                ),
                None => IrType::NamedGeneric(
                    name.clone(),
                    args.iter()
                        .map(|arg| self.lower_rust_boundary_target_type(arg))
                        .collect(),
                ),
            },
            _ => self.lower_resolved_type(target_ty),
        }
    }

    /// Lower a function/constructor call expression.
    ///
    /// Handles struct constructors, builtin functions, newtype checked construction, and regular function calls.
    pub(in crate::backend::ir::lower) fn lower_call_expr(
        &mut self,
        f: &ast::Spanned<ast::Expr>,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        call_span: ast::Span,
    ) -> Result<(IrExprKind, IrType), LoweringError> {
        if let Some(name) = Self::explicit_builtin_member_name(f)
            && let Some(builtin) = BuiltinFn::from_name(name)
        {
            let args_ir = self.lower_call_args(args)?.into_iter().map(|a| a.expr).collect();
            return Ok((
                IrExprKind::BuiltinCall {
                    func: builtin,
                    args: args_ir,
                },
                IrType::Unknown,
            ));
        }

        // Check if this is a struct/model/class constructor call
        if let ast::Expr::Ident(name) = &f.node {
            let constructor_name = self.symbol_aliases.get(name).cloned().unwrap_or_else(|| name.clone());
            if stdlib::is_graph_constructor_type(&constructor_name) && args.is_empty() {
                let lowered_type_args = self.lower_call_site_type_args(call_span, type_args);
                let receiver_ty = if lowered_type_args.is_empty() {
                    IrType::Struct(constructor_name.clone())
                } else {
                    IrType::NamedGeneric(constructor_name.clone(), lowered_type_args.clone())
                };
                return Ok((
                    IrExprKind::MethodCall {
                        receiver: Box::new(TypedExpr::new(
                            IrExprKind::Var {
                                name: constructor_name,
                                access: VarAccess::Read,
                                ref_kind: VarRefKind::TypeName,
                            },
                            receiver_ty.clone(),
                        )),
                        method: "__incan_new".to_string(),
                        dispatch: None,
                        type_args: Vec::new(),
                        args: Vec::new(),
                        callable_signature: None,
                        arg_policy: MethodCallArgPolicy::Default,
                    },
                    receiver_ty,
                ));
            }
            if keywords::from_str(name.as_str()) == Some(KeywordId::Cls)
                && matches!(self.lookup_var(name), IrType::Unknown)
                && let Some(owner_name) = self.current_classmethod_constructor.clone()
            {
                return self.lower_constructor_call(&owner_name, type_args, args, call_span);
            }

            if let Some(field_names) = self
                .type_info
                .as_ref()
                .and_then(|info| info.rust_named_field_constructor_fields(call_span))
                .map(|fields| fields.to_vec())
            {
                let lowered_args = self.lower_call_args(args)?;
                let fields = field_names
                    .into_iter()
                    .zip(lowered_args.into_iter().zip(args.iter()))
                    .map(|(field_name, (arg, ast_arg))| {
                        let span = Self::call_arg_expr(ast_arg).span;
                        let expr = self.wrap_with_rust_arg_coercion(arg.expr, span)?;
                        Ok((field_name, expr))
                    })
                    .collect::<Result<Vec<_>, LoweringError>>()?;
                let expr_ty = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.expr_type(call_span))
                    .map(|ty| self.lower_resolved_type(ty))
                    .unwrap_or(IrType::Unknown);
                return Ok((
                    IrExprKind::Struct {
                        name: name.clone(),
                        fields,
                    },
                    expr_ty,
                ));
            }

            // Constructor lowering must follow typechecker resolution, not identifier casing. Local declarations are
            // still available through `struct_names`; imported constructors are marked as `TypeName` on the callee
            // span by the typechecker.
            let is_known_struct = self.struct_names.contains_key(&constructor_name);
            let is_resolved_type_name = self
                .type_info
                .as_ref()
                .is_some_and(|info| matches!(info.ident_kind(f.span), Some(IdentKind::TypeName)));

            if is_known_struct || is_resolved_type_name {
                return self.lower_constructor_call(&constructor_name, type_args, args, call_span);
            }
        }

        let expanded_partial_args = self.partial_projection_call_args(f, args, call_span);
        let args = expanded_partial_args.as_deref().unwrap_or(args);

        let selected_emitted_name = self
            .type_info
            .as_ref()
            .and_then(|info| info.selected_function_emitted_name(call_span))
            .map(str::to_string);
        let mut imported_callee_path = self.imported_callee_path_for_expr(&f.node);
        if let (Some(path), Some(emitted_name)) = (&mut imported_callee_path, &selected_emitted_name)
            && let Some(last) = path.last_mut()
        {
            *last = emitted_name.clone();
        }
        let mut func = self.lower_expr_spanned(f)?;
        if let (ast::Expr::Ident(_), Some(emitted_name), IrExprKind::Var { name, .. }) =
            (&f.node, selected_emitted_name.as_deref(), &mut func.kind)
        {
            *name = emitted_name.to_string();
            func.ty = self.lookup_var(emitted_name);
        }
        if let Some(resolved_operator) = self
            .type_info
            .as_ref()
            .and_then(|info| info.resolved_operator_call(call_span).cloned())
            && resolved_operator.kind == ResolvedOperatorKind::Len
        {
            let Some(first_arg) = args.first() else {
                return Ok((
                    IrExprKind::BuiltinCall {
                        func: BuiltinFn::Len,
                        args: Vec::new(),
                    },
                    IrType::Int,
                ));
            };
            let receiver = self.lower_expr_spanned(Self::call_arg_expr(first_arg))?;
            return Ok((
                IrExprKind::MethodCall {
                    receiver: Box::new(receiver),
                    method: resolved_operator.method,
                    dispatch: None,
                    type_args: Vec::new(),
                    args: Vec::new(),
                    callable_signature: self.callable_signature_for_call_span(call_span),
                    arg_policy: MethodCallArgPolicy::Default,
                },
                IrType::Int,
            ));
        }
        if let ast::Expr::Ident(name) = &f.node
            && let Some(builtin) = BuiltinFn::from_name(name)
            && imported_callee_path.is_none()
            && self
                .type_info
                .as_ref()
                .is_none_or(|info| info.ident_kind(f.span).is_none())
            && self.callable_signature_for_call_span(call_span).is_none()
            && !matches!(func.ty, IrType::Function { .. })
        {
            let args_ir = self.lower_call_args(args)?.into_iter().map(|a| a.expr).collect();
            return Ok((
                IrExprKind::BuiltinCall {
                    func: builtin,
                    args: args_ir,
                },
                IrType::Unknown, // Return type depends on the builtin
            ));
        }

        // Regular function call (user-defined or unknown)
        let mut args_ir = self.lower_call_args(args)?;
        if args_ir.is_empty()
            && imported_callee_path
                .as_ref()
                .is_some_and(|path| path.as_slice() == ["std", "logging", "get_logger"])
        {
            let logger_name = self.current_default_logger_name();
            args_ir.push(IrCallArg {
                name: None,
                kind: IrCallArgKind::Positional,
                expr: TypedExpr::new(
                    IrExprKind::Literal(IrLiteral::StaticStr(logger_name)),
                    IrType::StaticStr,
                ),
            });
        }
        let lowered_type_args = self.lower_call_site_type_args(call_span, type_args);
        for (arg_ir, arg_ast) in args_ir.iter_mut().zip(args.iter()) {
            let arg_span = Self::call_arg_expr(arg_ast).span;
            arg_ir.expr = self.wrap_with_rust_arg_coercion(arg_ir.expr.clone(), arg_span)?;
        }
        if imported_callee_path
            .as_ref()
            .is_some_and(|path| testing::is_assert_helper_std_path(path, TestingAssertHelperId::AssertRaises))
            && args_ir
                .get(1)
                .is_none_or(|arg| !matches!(arg.expr.kind, IrExprKind::Literal(IrLiteral::StaticStr(_))))
        {
            let Some(error_type) = type_args.first() else {
                return Err(LoweringError {
                    message: "std.testing.assert_raises requires an error type argument".to_string(),
                    span: call_span.into(),
                });
            };
            args_ir.insert(
                1,
                IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::Literal(IrLiteral::StaticStr(error_type.node.to_string())),
                        IrType::StaticStr,
                    ),
                },
            );
        }
        if let Some(resolved_operator) = self
            .type_info
            .as_ref()
            .and_then(|info| info.resolved_operator_call(call_span).cloned())
            && resolved_operator.kind == ResolvedOperatorKind::Call
            && imported_callee_path.is_none()
        {
            let ret_ty = self
                .type_info
                .as_ref()
                .and_then(|info| info.expr_type(call_span))
                .map(|ty| self.lower_resolved_type(ty))
                .unwrap_or(IrType::Unknown);
            return Ok((
                IrExprKind::MethodCall {
                    receiver: Box::new(func),
                    method: resolved_operator.method,
                    dispatch: None,
                    type_args: Vec::new(),
                    args: args_ir,
                    callable_signature: self.callable_signature_for_call_span(call_span),
                    arg_policy: MethodCallArgPolicy::Default,
                },
                ret_ty,
            ));
        }
        let call_site_signature = self.callable_signature_for_call_span(call_span);
        let local_callable_signature = match &f.node {
            ast::Expr::Ident(name) => selected_emitted_name
                .as_deref()
                .and_then(|emitted_name| self.lookup_local_callable_signature(emitted_name))
                .or_else(|| self.lookup_local_callable_signature(name)),
            ast::Expr::Partial(_) => self.partial_expr_signature_for_span(f.span),
            _ => None,
        };
        let callable_signature = imported_callee_path
            .as_deref()
            .map(|path| {
                Ok(self
                    .callable_signature_for_imported_stdlib_path(path)?
                    .or_else(|| self.callable_signature_for_imported_pub_path(path)))
            })
            .transpose()?
            .flatten()
            .or(call_site_signature)
            .or(local_callable_signature)
            .or_else(|| self.callable_signature_for_callee_span(f.span));
        let callable_signature = self.refine_function_typed_local_call(&mut func, &args_ir, callable_signature);
        let imported_pub_library = imported_callee_path.as_deref().and_then(|path| {
            if path.first().is_some_and(|segment| segment == "pub") {
                path.get(1)
            } else {
                None
            }
        });
        let imported_sdk_provider_crate = imported_callee_path.as_deref().and_then(|path| {
            (path.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT) && path.len() >= 2)
                .then(|| self.sdk_provider_crate_for_module(&path[..path.len() - 1]))
                .flatten()
        });
        let callable_signature = match (
            imported_pub_library,
            imported_sdk_provider_crate.as_deref(),
            callable_signature,
        ) {
            (Some(library), _, Some(signature)) => Some(self.pub_external_signature(library, signature)),
            (None, Some(provider_crate), Some(signature)) => {
                Some(self.compiled_provider_external_signature(provider_crate, signature))
            }
            (_, _, signature) => signature,
        };
        if let (Some(library), Some(signature)) = (imported_pub_library, callable_signature.as_ref()) {
            func.ty = self.pub_external_function_type(library, signature);
        } else if let (Some(provider_crate), Some(signature)) =
            (imported_sdk_provider_crate.as_deref(), callable_signature.as_ref())
        {
            func.ty = IrType::Function {
                params: signature.params.iter().map(|param| param.ty.clone()).collect(),
                ret: Box::new(self.pub_external_type(provider_crate, signature.return_type.clone())),
            };
        }

        let ret_ty = if let IrType::Function { ret, .. } = &func.ty {
            let ret_ty = (**ret).clone();
            match (imported_pub_library, imported_sdk_provider_crate.as_deref()) {
                (Some(library), _) => self.pub_external_type(library, ret_ty),
                (None, Some(provider_crate)) => self.pub_external_type(provider_crate, ret_ty),
                (None, None) => ret_ty,
            }
        } else {
            IrType::Unknown
        };
        Ok((
            IrExprKind::Call {
                func: Box::new(func),
                type_args: lowered_type_args,
                args: args_ir,
                callable_signature,
                canonical_path: imported_callee_path,
            },
            ret_ty,
        ))
    }

    /// Expand a call through a known partial projection into the full wrapper surface.
    ///
    /// Generated Rust functions do not have source-level default parameters. Local and imported partial calls therefore
    /// need to materialize preset keyword arguments before ordinary call lowering so every boundary sees the same
    /// callable shape.
    fn partial_projection_call_args(
        &self,
        callee: &ast::Spanned<ast::Expr>,
        args: &[ast::CallArg],
        call_span: ast::Span,
    ) -> Option<Vec<ast::CallArg>> {
        let ast::Expr::Ident(callee_name) = &callee.node else {
            return None;
        };
        let info = self.type_info.as_ref()?;
        let projection = info.partial_projection(callee_name)?;
        let merged = merge_named_partial_args(
            projection.presets.iter().map(|preset| PartialPresetRef {
                name: preset.name.as_str(),
                value: &preset.value,
            }),
            args,
        )?;

        let params = info
            .call_site_callable_params(call_span)
            .or_else(|| match info.expr_type(callee.span)? {
                ResolvedType::Function(params, _) => Some(params.as_slice()),
                _ => None,
            })?;
        let mut by_name = merged
            .into_iter()
            .filter_map(|arg| match arg {
                ast::CallArg::Named(name, value) => Some((name, value)),
                _ => None,
            })
            .collect::<HashMap<_, _>>();
        let mut ordered = Vec::with_capacity(by_name.len());
        for param in params.iter().filter(|param| param.kind == ast::ParamKind::Normal) {
            let Some(name) = param.name.as_deref() else {
                continue;
            };
            if let Some(value) = by_name.remove(name) {
                ordered.push(ast::CallArg::Named(name.to_string(), value));
            }
        }
        ordered.extend(
            by_name
                .into_iter()
                .map(|(name, value)| ast::CallArg::Named(name, value)),
        );
        Some(ordered)
    }

    /// Lower a struct/model/class/newtype constructor call.
    pub(super) fn lower_constructor_call(
        &mut self,
        name: &str,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        call_span: ast::Span,
    ) -> Result<(IrExprKind, IrType), LoweringError> {
        if let Some(hook_call) = self.lower_type_constructor_hook_call(name, type_args, args, call_span)? {
            return Ok(hook_call);
        }

        if name == surface_types::as_str(surface_types::SurfaceTypeId::ValidationError) {
            let mut message = None;
            let mut code = None;
            for arg in args {
                match arg {
                    ast::CallArg::Positional(expr) => {
                        message = Some(self.lower_expr_spanned(expr)?);
                    }
                    ast::CallArg::Named(field, expr) if field == "message" => {
                        message = Some(self.lower_expr_spanned(expr)?);
                    }
                    ast::CallArg::Named(field, expr) if field == "code" => {
                        code = Some(self.lower_expr_spanned(expr)?);
                    }
                    ast::CallArg::Named(_, expr)
                    | ast::CallArg::PositionalUnpack(expr)
                    | ast::CallArg::KeywordUnpack(expr) => {
                        message.get_or_insert(self.lower_expr_spanned(expr)?);
                    }
                }
            }
            let mut lowered_args = Vec::new();
            if let Some(message) = message {
                lowered_args.push(IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: message,
                });
            }
            let method = if let Some(code) = code {
                lowered_args.push(IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: code,
                });
                "with_code"
            } else {
                "new"
            };
            return Ok((
                IrExprKind::Call {
                    func: Box::new(TypedExpr::new(
                        IrExprKind::Var {
                            name: method.to_string(),
                            access: VarAccess::Read,
                            ref_kind: VarRefKind::Value,
                        },
                        IrType::Unknown,
                    )),
                    type_args: Vec::new(),
                    args: lowered_args,
                    callable_signature: None,
                    canonical_path: Some(vec![
                        "incan_stdlib".to_string(),
                        "validation".to_string(),
                        "ValidationError".to_string(),
                        method.to_string(),
                    ]),
                },
                IrType::Struct(surface_types::as_str(surface_types::SurfaceTypeId::ValidationError).to_string()),
            ));
        }

        // Get type if known, otherwise Unknown (will be inferred at emit time)
        let struct_ty = self.struct_names.get(name).cloned().unwrap_or(IrType::Unknown);
        if self.has_aggregate_constructor_fields(args) {
            return self.lower_aggregate_constructor_call(name, args, struct_ty);
        }

        // Apply the canonical checked-construction hook before ordinary tuple construction.
        if let Some(ctor) = self
            .newtype_construction
            .get(name)
            .and_then(|plan| plan.checked_constructor.clone())
            && args.len() == 1
            && matches!(args[0], ast::CallArg::Positional(_))
            && self.current_impl_type.as_deref() != Some(name)
        {
            let ast::CallArg::Positional(value) = &args[0] else {
                unreachable!("checked by matches! above")
            };
            let lowered_value = self.lower_expr_spanned(value)?;
            // Keep the failure path local to generated code: the Err branch still panics, but we no longer emit an
            // `.expect()` extraction in the generated Rust.
            let checked = Self::checked_newtype_match_expr(name, &ctor, lowered_value, struct_ty.clone());
            return Ok((checked.kind, struct_ty));
        }
        if let Some(constraints) = self
            .newtype_construction
            .get(name)
            .filter(|plan| plan.checked_constructor.is_none() && !plan.constraints.is_empty())
            .map(|plan| plan.constraints.clone())
            && args.len() == 1
            && matches!(args[0], ast::CallArg::Positional(_))
            && self.current_impl_type.as_deref() != Some(name)
        {
            let ast::CallArg::Positional(value) = &args[0] else {
                unreachable!("checked by matches! above")
            };
            let lowered_value = self.lower_expr_spanned(value)?;
            let checked =
                Self::generated_constrained_newtype_expr(name, &constraints, lowered_value, struct_ty.clone());
            return Ok((checked.kind, struct_ty));
        }

        // This is a constructor call - lower as struct instantiation
        // RFC 021: resolve field aliases to canonical names
        let struct_name = name.to_string();
        let fields: Vec<(String, TypedExpr)> = args
            .iter()
            .map(|arg| match arg {
                ast::CallArg::Named(field_name, value) => {
                    let lowered_value = self.lower_expr_spanned(value)?;
                    // RFC 021: map alias → canonical field name
                    let canonical = self.resolve_field_alias(&struct_name, field_name);
                    Ok((canonical, lowered_value))
                }
                ast::CallArg::Positional(value) => {
                    // Positional args - use empty string for field name
                    // (emitter will detect this and use tuple-style construction)
                    let lowered_value = self.lower_expr_spanned(value)?;
                    Ok((String::new(), lowered_value))
                }
                ast::CallArg::PositionalUnpack(value) | ast::CallArg::KeywordUnpack(value) => {
                    let lowered_value = self.lower_expr_spanned(value)?;
                    Ok((String::new(), lowered_value))
                }
            })
            .collect::<Result<Vec<_>, LoweringError>>()?;
        Ok((
            IrExprKind::Struct {
                name: name.to_string(),
                fields,
            },
            struct_ty,
        ))
    }

    /// Lower imported stdlib type construction through a source-defined static `__incan_new` method when present.
    fn lower_type_constructor_hook_call(
        &mut self,
        name: &str,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        call_span: ast::Span,
    ) -> Result<Option<(IrExprKind, IrType)>, LoweringError> {
        let Some(type_path) = self.import_aliases.get(name).cloned() else {
            return Ok(None);
        };
        if type_path.len() < 2 {
            return Ok(None);
        }
        let Some(type_name) = type_path.last().cloned() else {
            return Ok(None);
        };
        let module_path = &type_path[..type_path.len() - 1];
        let Some(type_info) = self.stdlib_cache.lookup_type(module_path, &type_name) else {
            return Ok(None);
        };
        if Self::is_named_field_constructor_call(&type_info, args) {
            return Ok(None);
        }
        let Some(hook) = self
            .stdlib_cache
            .lookup_type_method_decl(module_path, &type_name, TYPE_CONSTRUCTOR_HOOK)
        else {
            return Ok(None);
        };
        if hook.receiver.is_some() {
            return Ok(None);
        }

        let args_ir = self.lower_call_args(args)?;
        let lowered_type_args = self.lower_call_site_type_args(call_span, type_args);
        let receiver_ty = if lowered_type_args.is_empty() {
            self.struct_names
                .get(name)
                .cloned()
                .unwrap_or_else(|| IrType::Struct(name.to_string()))
        } else {
            IrType::NamedGeneric(name.to_string(), lowered_type_args)
        };
        let ret_ty = self.lower_type(&hook.return_type.node);
        Ok(Some((
            IrExprKind::MethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: name.to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::TypeName,
                    },
                    receiver_ty,
                )),
                method: TYPE_CONSTRUCTOR_HOOK.to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: args_ir,
                callable_signature: self
                    .callable_signature_for_imported_stdlib_type_method_path(&type_path, TYPE_CONSTRUCTOR_HOOK)?,
                arg_policy: MethodCallArgPolicy::Default,
            },
            ret_ty,
        )))
    }

    /// Return whether a constructor call is an ordinary named-field model/class construction.
    fn is_named_field_constructor_call(type_info: &crate::frontend::symbols::TypeInfo, args: &[ast::CallArg]) -> bool {
        let fields = match type_info {
            crate::frontend::symbols::TypeInfo::Model(info) => &info.fields,
            crate::frontend::symbols::TypeInfo::Class(info) => &info.fields,
            _ => return false,
        };
        !args.is_empty()
            && args.iter().all(|arg| match arg {
                ast::CallArg::Named(field, _) => fields.contains_key(field),
                _ => false,
            })
    }

    /// Lower call arguments to IR expressions.
    ///
    /// Handles positional, named, and unpack arguments.
    pub(in crate::backend::ir::lower) fn lower_call_args(
        &mut self,
        args: &[ast::CallArg],
    ) -> Result<Vec<IrCallArg>, LoweringError> {
        let mut lowered = Vec::new();
        for arg in args {
            match arg {
                ast::CallArg::Positional(e) => lowered.push(IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: self.lower_expr_spanned(e)?,
                }),
                ast::CallArg::Named(name, e) => lowered.push(IrCallArg {
                    name: Some(name.clone()),
                    kind: IrCallArgKind::Named,
                    expr: self.lower_expr_spanned(e)?,
                }),
                ast::CallArg::PositionalUnpack(e) => {
                    let expr = self.lower_expr_spanned(e)?;
                    if let Some(FixedUnpackPlan::Positional(item_types)) =
                        self.type_info.as_ref().and_then(|info| info.fixed_unpack_plan(e.span))
                    {
                        lowered.extend(self.lower_fixed_positional_unpack_args(&expr, item_types));
                    } else {
                        lowered.push(IrCallArg {
                            name: None,
                            kind: IrCallArgKind::PositionalUnpack,
                            expr,
                        });
                    }
                }
                ast::CallArg::KeywordUnpack(e) => {
                    let expr = self.lower_expr_spanned(e)?;
                    if let Some(FixedUnpackPlan::Keyword(keys)) =
                        self.type_info.as_ref().and_then(|info| info.fixed_unpack_plan(e.span))
                    {
                        lowered.extend(self.lower_fixed_keyword_unpack_args(&expr, keys));
                    } else {
                        lowered.push(IrCallArg {
                            name: None,
                            kind: IrCallArgKind::KeywordUnpack,
                            expr,
                        });
                    }
                }
            }
        }
        Ok(lowered)
    }

    /// Expand a typechecker-proven `*expr` shape into ordinary positional IR arguments.
    fn lower_fixed_positional_unpack_args(&self, expr: &TypedExpr, item_types: &[ResolvedType]) -> Vec<IrCallArg> {
        let items = match &expr.kind {
            IrExprKind::Tuple(items) => items.clone(),
            IrExprKind::List(items) => items
                .iter()
                .filter_map(|item| match item {
                    IrListEntry::Element(value) => Some(value.clone()),
                    IrListEntry::Spread(_) => None,
                })
                .collect(),
            _ => item_types
                .iter()
                .enumerate()
                .map(|(idx, ty)| {
                    TypedExpr::new(
                        IrExprKind::Field {
                            object: Box::new(expr.clone()),
                            field: idx.to_string(),
                        },
                        self.lower_resolved_type(ty),
                    )
                    .with_span(expr.span)
                })
                .collect(),
        };

        items
            .into_iter()
            .map(|expr| IrCallArg {
                name: None,
                kind: IrCallArgKind::Positional,
                expr,
            })
            .collect()
    }

    /// Expand a typechecker-proven `**expr` key set into ordinary named IR arguments.
    fn lower_fixed_keyword_unpack_args(&self, expr: &TypedExpr, keys: &[String]) -> Vec<IrCallArg> {
        let IrExprKind::Dict(entries) = &expr.kind else {
            return vec![IrCallArg {
                name: None,
                kind: IrCallArgKind::KeywordUnpack,
                expr: expr.clone(),
            }];
        };

        entries
            .iter()
            .zip(keys.iter())
            .filter_map(|(entry, name)| match entry {
                IrDictEntry::Pair(_, value) => Some(IrCallArg {
                    name: Some(name.clone()),
                    kind: IrCallArgKind::Named,
                    expr: value.as_ref().clone(),
                }),
                IrDictEntry::Spread(_) => None,
            })
            .collect()
    }
}

/// Convert manifest parameter kind metadata back to the frontend enum used by IR call signatures.
fn param_kind_from_manifest(kind: ParamKindExport) -> ast::ParamKind {
    match kind {
        ParamKindExport::Normal => ast::ParamKind::Normal,
        ParamKindExport::RestPositional => ast::ParamKind::RestPositional,
        ParamKindExport::RestKeyword => ast::ParamKind::RestKeyword,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};
    use std::sync::Arc;

    use super::AstLowering;
    use crate::backend::ir::decl::IrDeclKind;
    use crate::backend::ir::expr::{IrExprKind, MethodCallArgPolicy, VarRefKind};
    use crate::backend::ir::stmt::IrStmtKind;
    use crate::backend::ir::types::IrType;
    use crate::frontend::api_metadata::{
        ApiDeclaration, ApiFunction, CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadata,
        CheckedApiMetadataPackage, SourceAnchor, SourceSpan,
    };
    use crate::frontend::ast::{
        CallArg, Expr, InteropAdapterKind, InteropDirection, InteropEdgeDecl, Literal, Span, Spanned, Type,
    };
    use crate::frontend::library_manifest_index::{
        LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
    };
    use crate::frontend::symbols::ResolvedType;
    use crate::frontend::typechecker::{RustArgCoercionInfo, RustArgCoercionKind, TypeCheckInfo};
    use crate::library_manifest::{
        AliasExport, CompiledProviderMetadata, ExportIdentity, ExportIdentityKind, ExportIdentityProjection,
        FunctionExport, LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION, LibraryExports, LibraryIdentityGraph, LibraryManifest,
        ParamDefaultExport, ParamExport, ParamKindExport, ProviderModuleClaim, TypeRef,
    };
    use crate::provider::ProviderPlan;
    use incan_core::interop::CoercionPolicy;
    use incan_core::lang::surface::constructors::{self, ConstructorId};

    fn mk_edge(
        direction: InteropDirection,
        ty: Type,
        adapter_kind: InteropAdapterKind,
        adapter_name: &str,
    ) -> InteropEdgeDecl {
        InteropEdgeDecl {
            direction,
            ty: Spanned::new(ty, Span::new(0, 0)),
            adapter_kind,
            adapter: Spanned::new(Expr::Ident(adapter_name.to_string()), Span::new(0, 0)),
        }
    }

    fn exported_fn(name: &str, param: &str, ret: &str) -> FunctionExport {
        FunctionExport {
            name: name.to_string(),
            emitted_name: None,
            type_params: Vec::new(),
            params: vec![ParamExport {
                name: "value".to_string(),
                ty: TypeRef::Named {
                    name: param.to_string(),
                },
                kind: ParamKindExport::Normal,
                has_default: false,
                default: None,
            }],
            return_type: TypeRef::Named { name: ret.to_string() },
            is_async: false,
        }
    }

    #[test]
    fn imported_pub_callable_signature_uses_identity_graph_before_short_name_lookup() -> Result<(), String> {
        let mut manifest = LibraryManifest::new("mylib", "0.1.0");
        manifest.exports = LibraryExports {
            functions: vec![exported_fn("cast", "int", "int")],
            aliases: vec![AliasExport {
                name: "safe_cast".to_string(),
                target_path: vec!["helpers".to_string(), "cast".to_string()],
                projected_function: None,
            }],
            ..LibraryExports::default()
        };
        manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            package: None,
            modules: vec![CheckedApiMetadata {
                schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
                module_path: vec!["helpers".to_string()],
                declarations: vec![ApiDeclaration::Function(ApiFunction {
                    name: "cast".to_string(),
                    anchor: SourceAnchor {
                        id: "helpers.cast".to_string(),
                        span: SourceSpan { start: 0, end: 0 },
                    },
                    docstring: None,
                    docstring_sections: None,
                    decorators: Vec::new(),
                    type_params: Vec::new(),
                    params: exported_fn("cast", "str", "str").params,
                    return_type: TypeRef::Named {
                        name: "str".to_string(),
                    },
                    is_async: false,
                })],
            }],
        });
        manifest.contract_metadata.identity_graph = LibraryIdentityGraph {
            schema_version: LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION,
            exports: vec![ExportIdentity {
                public_name: "safe_cast".to_string(),
                public_path: vec!["mylib".to_string(), "safe_cast".to_string()],
                source_path: vec!["facade".to_string(), "safe_cast".to_string()],
                kind: ExportIdentityKind::Alias,
                projection: ExportIdentityProjection::Alias {
                    target_path: vec!["helpers".to_string(), "cast".to_string()],
                },
            }],
        };

        let index = LibraryManifestIndex::from_entries(HashMap::from([(
            "mylib".to_string(),
            LibraryManifestIndexEntry::Loaded {
                manifest: Box::new(manifest),
                metadata: LibraryArtifactMetadata::from_crate_root(
                    "mylib",
                    "mylib",
                    std::env::temp_dir().join("incan_identity_graph_backend_test"),
                ),
            },
        )]));
        let mut lowering = AstLowering::new();
        lowering.set_provider_plan(Some(Arc::new(ProviderPlan::for_library_index(index))));

        let signature = lowering
            .callable_signature_for_imported_pub_path(&[
                "pub".to_string(),
                "mylib".to_string(),
                "safe_cast".to_string(),
            ])
            .ok_or_else(|| "expected identity graph to resolve safe_cast through helpers.cast".to_string())?;

        assert_eq!(signature.params[0].ty, IrType::String);
        assert_eq!(signature.return_type, IrType::String);
        Ok(())
    }

    #[test]
    fn compiled_provider_signature_uses_artifact_api_and_preserves_union_owner() -> Result<(), String> {
        let provider_name = "artifact_only_provider";
        let mut manifest = LibraryManifest::new(provider_name, "0.5.0");
        manifest.contract_metadata.provider = CompiledProviderMetadata {
            namespace_claims: vec![ProviderModuleClaim {
                module_path: vec!["artifact_only".to_string()],
                required_features: BTreeSet::new(),
            }],
            ..CompiledProviderMetadata::default()
        };
        manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            package: None,
            modules: vec![CheckedApiMetadata {
                schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
                module_path: vec!["artifact_only".to_string()],
                declarations: vec![ApiDeclaration::Function(ApiFunction {
                    name: "consume".to_string(),
                    anchor: SourceAnchor {
                        id: "artifact_only.consume".to_string(),
                        span: SourceSpan { start: 0, end: 0 },
                    },
                    docstring: None,
                    docstring_sections: None,
                    decorators: Vec::new(),
                    type_params: Vec::new(),
                    params: vec![ParamExport {
                        name: "value".to_string(),
                        ty: TypeRef::Applied {
                            name: "Union".to_string(),
                            args: vec![
                                TypeRef::Named {
                                    name: "int".to_string(),
                                },
                                TypeRef::Named {
                                    name: "str".to_string(),
                                },
                            ],
                        },
                        kind: ParamKindExport::Normal,
                        has_default: true,
                        default: Some(ParamDefaultExport::ConstRef(vec![
                            "artifact_only".to_string(),
                            "Defaults".to_string(),
                            "VALUE".to_string(),
                        ])),
                    }],
                    return_type: TypeRef::Named {
                        name: constructors::as_str(ConstructorId::None).to_string(),
                    },
                    is_async: false,
                })],
            }],
        });
        let mut lowering = AstLowering::new();
        lowering.set_provider_plan(Some(Arc::new(ProviderPlan::for_in_memory_sdk_manifest(
            LibraryManifestIndex::default(),
            manifest,
        ))));

        let signature = lowering
            .callable_signature_for_imported_stdlib_path(&[
                "std".to_string(),
                "artifact_only".to_string(),
                "consume".to_string(),
            ])
            .map_err(|error| error.message)?
            .ok_or_else(|| "expected compiled provider API signature".to_string())?;

        assert!(matches!(
            &signature.params[0].ty,
            IrType::ExternalUnion { library, .. } if library == provider_name
        ));
        assert!(
            signature.params[0].default.is_some(),
            "artifact-owned const defaults must survive without provider source"
        );

        lowering.import_aliases.insert(
            "artifact".to_string(),
            vec!["std".to_string(), "artifact_only".to_string()],
        );
        assert_eq!(
            lowering.imported_module_function_callee_path(&Expr::Ident("artifact".to_string()), "consume"),
            Some(vec![
                "std".to_string(),
                "artifact_only".to_string(),
                "consume".to_string()
            ]),
            "module-qualified calls must use provider claims rather than the compiler's legacy stdlib registry"
        );
        Ok(())
    }

    #[test]
    fn lower_rusttype_interop_adapter_uses_into_edge_for_rusttype_argument() -> Result<(), String> {
        let mut lowering = AstLowering::new();
        lowering.rusttype_interop_edges.insert(
            "Email".to_string(),
            vec![mk_edge(
                InteropDirection::Into,
                Type::Simple("str".to_string()),
                InteropAdapterKind::Via,
                "email_into_str",
            )],
        );

        let adapter = lowering
            .lower_rusttype_interop_adapter(&IrType::Struct("Email".to_string()), &IrType::String)
            .map_err(|err| format!("expected successful adapter lowering, got {err:?}"))?;

        assert!(adapter.is_some(), "expected into edge adapter to resolve");
        Ok(())
    }

    #[test]
    fn lower_rusttype_interop_adapter_uses_from_edge_for_rusttype_target() -> Result<(), String> {
        let mut lowering = AstLowering::new();
        lowering.rusttype_interop_edges.insert(
            "Email".to_string(),
            vec![mk_edge(
                InteropDirection::From,
                Type::Simple("str".to_string()),
                InteropAdapterKind::Try,
                "email_parse",
            )],
        );

        let adapter = lowering
            .lower_rusttype_interop_adapter(&IrType::String, &IrType::Struct("Email".to_string()))
            .map_err(|err| format!("expected successful adapter lowering, got {err:?}"))?;

        assert!(adapter.is_some(), "expected from edge adapter to resolve");
        Ok(())
    }

    #[test]
    fn lower_method_call_wraps_args_with_rust_arg_coercion() -> Result<(), String> {
        let arg_span = Span::new(10, 20);
        let mut type_info = TypeCheckInfo::default();
        type_info.rust.arg_coercions.insert(
            (arg_span.start, arg_span.end),
            RustArgCoercionInfo {
                rust_target_type: "&str".to_string(),
                target_type: ResolvedType::Ref(Box::new(ResolvedType::Str)),
                kind: RustArgCoercionKind::Builtin(CoercionPolicy::Borrow),
            },
        );

        let mut lowering = AstLowering::new_with_type_info(type_info);
        let expr = Expr::MethodCall(
            Box::new(Spanned::new(Expr::Ident("value".to_string()), Span::new(0, 5))),
            "coerce_me".to_string(),
            Vec::new(),
            vec![CallArg::Positional(Spanned::new(
                Expr::Literal(Literal::String("hello".to_string())),
                arg_span,
            ))],
        );

        let lowered = lowering
            .lower_expr(&expr, Span::new(0, 100))
            .map_err(|err| format!("expected successful lowering, got {err:?}"))?;

        match lowered.kind {
            IrExprKind::MethodCall { args, .. } => {
                let Some(first_arg) = args.first() else {
                    return Err("expected lowered method arg".to_string());
                };
                match &first_arg.expr.kind {
                    IrExprKind::InteropCoerce { to_ty, .. } => {
                        assert_eq!(
                            *to_ty,
                            IrType::StrRef,
                            "expected borrowed str target to lower to StrRef"
                        );
                    }
                    other => {
                        return Err(format!(
                            "expected first method arg to be wrapped in InteropCoerce, got {other:?}"
                        ));
                    }
                }
            }
            other => return Err(format!("expected MethodCall lowering, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn lower_rust_named_field_constructor_wraps_fields_with_rust_arg_coercion() -> Result<(), String> {
        let call_span = Span::new(0, 40);
        let callee_span = Span::new(0, 14);
        let arg_span = Span::new(20, 31);
        let mut type_info = TypeCheckInfo::default();
        type_info.expressions.ident_kinds.insert(
            (callee_span.start, callee_span.end),
            crate::frontend::typechecker::IdentKind::TypeName,
        );
        type_info.expressions.expr_types.insert(
            (call_span.start, call_span.end),
            ResolvedType::RustPath("demo::FunctionOption".to_string()),
        );
        type_info.record_rust_named_field_constructor_fields(call_span, vec!["name".to_string()]);
        type_info.rust.arg_coercions.insert(
            (arg_span.start, arg_span.end),
            RustArgCoercionInfo {
                rust_target_type: "String".to_string(),
                target_type: ResolvedType::Str,
                kind: RustArgCoercionKind::Builtin(CoercionPolicy::Exact),
            },
        );

        let mut lowering = AstLowering::new_with_type_info(type_info);
        let expr = Expr::Call(
            Box::new(Spanned::new(Expr::Ident("FunctionOption".to_string()), callee_span)),
            Vec::new(),
            vec![CallArg::Named(
                "name".to_string(),
                Spanned::new(Expr::Ident("OPTION_NAME".to_string()), arg_span),
            )],
        );

        let lowered = lowering
            .lower_expr(&expr, call_span)
            .map_err(|err| format!("expected successful lowering, got {err:?}"))?;

        match lowered.kind {
            IrExprKind::Struct { fields, .. } => {
                let Some((field_name, field_expr)) = fields.first() else {
                    return Err("expected one lowered Rust constructor field".to_string());
                };
                assert_eq!(field_name, "name");
                if !matches!(field_expr.kind, IrExprKind::InteropCoerce { .. }) {
                    return Err(format!(
                        "expected Rust constructor field to be wrapped in InteropCoerce, got {:?}",
                        field_expr.kind
                    ));
                }
            }
            other => return Err(format!("expected Rust Struct lowering, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn lower_rust_boundary_target_preserves_nested_borrowed_str_refs() {
        let lowering = AstLowering::new();
        let target = ResolvedType::Generic("List".to_string(), vec![ResolvedType::Ref(Box::new(ResolvedType::Str))]);

        assert_eq!(
            lowering.lower_rust_boundary_target_type(&target),
            IrType::List(Box::new(IrType::StrRef)),
        );
    }

    #[test]
    fn lower_method_call_threads_arg_shape_hint_from_typechecker() -> Result<(), String> {
        let receiver_span = Span::new(0, 5);
        let arg_span = Span::new(10, 17);
        let mut type_info = TypeCheckInfo::default();
        type_info.record_regular_method_arg_shape(receiver_span, "get");
        type_info.rust.arg_coercions.insert(
            (arg_span.start, arg_span.end),
            RustArgCoercionInfo {
                rust_target_type: "&Q".to_string(),
                target_type: ResolvedType::Ref(Box::new(ResolvedType::RustPath("Q".to_string()))),
                kind: RustArgCoercionKind::Builtin(CoercionPolicy::Borrow),
            },
        );

        let mut lowering = AstLowering::new_with_type_info(type_info);
        let expr = Expr::MethodCall(
            Box::new(Spanned::new(Expr::Ident("value".to_string()), receiver_span)),
            "get".to_string(),
            Vec::new(),
            vec![CallArg::Positional(Spanned::new(
                Expr::Literal(Literal::String("hello".to_string())),
                arg_span,
            ))],
        );

        let lowered = lowering
            .lower_expr(&expr, Span::new(0, 100))
            .map_err(|err| format!("expected successful lowering, got {err:?}"))?;

        match lowered.kind {
            IrExprKind::MethodCall { arg_policy, args, .. } => {
                assert_eq!(arg_policy, MethodCallArgPolicy::PreserveShape);
                assert!(
                    !matches!(
                        args.first().map(|arg| &arg.expr.kind),
                        Some(IrExprKind::InteropCoerce { .. })
                    ),
                    "expected preserved lookup method args to skip rust arg coercion wrapping, got {args:?}"
                );
            }
            other => return Err(format!("expected MethodCall lowering, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn lower_rust_import_associated_method_keeps_type_like_receiver() -> Result<(), String> {
        use crate::frontend::{lexer, parser, typechecker::TypeChecker};

        let source = r#"
from rust::datafusion::dataframe import DataFrameWriteOptions

def f() -> None:
  _ = DataFrameWriteOptions.new()
"#;
        let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;

        let mut checker = TypeChecker::new();
        checker
            .check_program(&ast)
            .map_err(|errs| format!("typecheck failed: {errs:?}"))?;

        let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
        let program = lowering
            .lower_program(&ast)
            .map_err(|err| format!("lowering failed: {err:?}"))?;

        let function = program
            .declarations
            .iter()
            .find_map(|decl| match &decl.kind {
                IrDeclKind::Function(function) if function.name == "f" => Some(function),
                _ => None,
            })
            .ok_or_else(|| "expected lowered function `f`".to_string())?;
        let Some(stmt) = function.body.first() else {
            return Err("expected expression statement body".to_string());
        };
        let IrStmtKind::Let { value: expr, .. } = &stmt.kind else {
            return Err(format!("expected expression statement body, got {:?}", function.body));
        };

        match &expr.kind {
            IrExprKind::MethodCall { receiver, method, .. } => {
                assert_eq!(method, "new");
                match &receiver.kind {
                    IrExprKind::Var { name, ref_kind, .. } => {
                        assert_eq!(name, "DataFrameWriteOptions");
                        assert_eq!(*ref_kind, VarRefKind::ExternalRustName);
                    }
                    other => return Err(format!("expected variable receiver, got {other:?}")),
                }
            }
            other => return Err(format!("expected MethodCall lowering, got {other:?}")),
        }

        Ok(())
    }

    #[test]
    fn lower_nested_rust_associated_method_arg_keeps_type_like_receiver() -> Result<(), String> {
        use crate::frontend::{lexer, parser, typechecker::TypeChecker};

        let source = r#"
from rust::datafusion::execution::context import SessionContext
from rust::datafusion::dataframe import DataFrameWriteOptions

def f(uri: str) -> None:
  ctx = SessionContext.new()
  _ = ctx.write_csv(uri, DataFrameWriteOptions.new(), None)
"#;
        let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;

        let mut checker = TypeChecker::new();
        checker
            .check_program(&ast)
            .map_err(|errs| format!("typecheck failed: {errs:?}"))?;

        let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
        let program = lowering
            .lower_program(&ast)
            .map_err(|err| format!("lowering failed: {err:?}"))?;

        let function = program
            .declarations
            .iter()
            .find_map(|decl| match &decl.kind {
                IrDeclKind::Function(function) if function.name == "f" => Some(function),
                _ => None,
            })
            .ok_or_else(|| "expected lowered function `f`".to_string())?;
        let Some(stmt) = function.body.get(1) else {
            return Err(format!("expected nested write_csv statement, got {:?}", function.body));
        };
        let IrStmtKind::Let { value: expr, .. } = &stmt.kind else {
            return Err(format!("expected let statement, got {:?}", function.body));
        };

        let IrExprKind::MethodCall { args, .. } = &expr.kind else {
            return Err(format!("expected outer MethodCall, got {:?}", expr.kind));
        };
        let nested = args
            .get(1)
            .ok_or_else(|| format!("expected second method arg, got {:?}", args))?;

        match &nested.expr.kind {
            IrExprKind::MethodCall { receiver, method, .. } => {
                assert_eq!(method, "new");
                match &receiver.kind {
                    IrExprKind::Var { name, ref_kind, .. } => {
                        assert_eq!(name, "DataFrameWriteOptions");
                        assert_eq!(*ref_kind, VarRefKind::ExternalRustName);
                    }
                    other => return Err(format!("expected variable receiver, got {other:?}")),
                }
            }
            IrExprKind::InteropCoerce { expr, .. } => match &expr.kind {
                IrExprKind::MethodCall { receiver, method, .. } => {
                    assert_eq!(method, "new");
                    match &receiver.kind {
                        IrExprKind::Var { name, ref_kind, .. } => {
                            assert_eq!(name, "DataFrameWriteOptions");
                            assert_eq!(*ref_kind, VarRefKind::ExternalRustName);
                        }
                        other => return Err(format!("expected variable receiver, got {other:?}")),
                    }
                }
                other => return Err(format!("expected nested MethodCall in InteropCoerce, got {other:?}")),
            },
            other => return Err(format!("expected nested MethodCall arg, got {other:?}")),
        }

        Ok(())
    }

    #[test]
    fn lower_rust_constant_method_receiver_as_value_not_type_like() -> Result<(), String> {
        use crate::frontend::{lexer, parser, typechecker::TypeChecker};

        let source = r#"
from rust::std::time import Duration, UNIX_EPOCH

def f() -> None:
  duration = Duration.from_secs(1)
  _ = UNIX_EPOCH.saturating_add(duration)
"#;
        let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;

        let mut checker = TypeChecker::new();
        checker
            .check_program(&ast)
            .map_err(|errs| format!("typecheck failed: {errs:?}"))?;

        let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
        let program = lowering
            .lower_program(&ast)
            .map_err(|err| format!("lowering failed: {err:?}"))?;

        let function = program
            .declarations
            .iter()
            .find_map(|decl| match &decl.kind {
                IrDeclKind::Function(function) if function.name == "f" => Some(function),
                _ => None,
            })
            .ok_or_else(|| "expected lowered function `f`".to_string())?;
        let Some(stmt) = function.body.get(1) else {
            return Err(format!("expected UNIX_EPOCH method statement, got {:?}", function.body));
        };
        let IrStmtKind::Let { value: expr, .. } = &stmt.kind else {
            return Err(format!("expected let statement, got {:?}", function.body));
        };

        match &expr.kind {
            IrExprKind::MethodCall { receiver, method, .. } => {
                assert_eq!(method, "saturating_add");
                match &receiver.kind {
                    IrExprKind::Var { name, ref_kind, .. } => {
                        assert_eq!(name, "UNIX_EPOCH");
                        assert_eq!(*ref_kind, VarRefKind::Value);
                    }
                    other => return Err(format!("expected variable receiver, got {other:?}")),
                }
            }
            other => return Err(format!("expected MethodCall lowering, got {other:?}")),
        }

        Ok(())
    }

    #[test]
    fn lower_generic_box_as_ref_preserves_nominal_generic_receiver_args() -> Result<(), String> {
        use crate::backend::ir::decl::IrDeclKind;
        use crate::backend::ir::stmt::IrStmtKind;
        use crate::frontend::{lexer, parser, typechecker::TypeChecker};

        let source = r#"
from rust::std::boxed import Box

@derive(Clone)
class Node[T]:
  pub value: T

def take[T](node: Node[T]) -> T:
  return node.value

def from_box[T](child: Box[Node[T]]) -> T:
  return take(child.as_ref())
"#;
        let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;

        let mut checker = TypeChecker::new();
        checker
            .check_program(&ast)
            .map_err(|errs| format!("typecheck failed: {errs:?}"))?;

        let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
        let program = lowering
            .lower_program(&ast)
            .map_err(|err| format!("lowering failed: {err:?}"))?;

        let function = program
            .declarations
            .iter()
            .find_map(|decl| match &decl.kind {
                IrDeclKind::Function(function) if function.name == "from_box" => Some(function),
                _ => None,
            })
            .ok_or_else(|| "expected lowered function `from_box`".to_string())?;
        let Some(stmt) = function.body.first() else {
            return Err("expected return statement body".to_string());
        };
        let IrStmtKind::Return(Some(expr)) = &stmt.kind else {
            return Err(format!("expected return statement body, got {:?}", function.body));
        };
        let IrExprKind::Call { args, .. } = &expr.kind else {
            return Err(format!("expected call expression, got {:?}", expr.kind));
        };
        let arg = args.first().ok_or_else(|| "expected call arg".to_string())?;

        match &arg.expr.kind {
            IrExprKind::MethodCall { receiver, method, .. } => {
                assert_eq!(method, "as_ref");
                assert_eq!(
                    receiver.ty,
                    IrType::NamedGeneric(
                        "Box".to_string(),
                        vec![IrType::NamedGeneric(
                            "Node".to_string(),
                            vec![IrType::Generic("T".to_string())]
                        )]
                    )
                );
            }
            other => return Err(format!("expected nested MethodCall arg, got {other:?}")),
        }

        Ok(())
    }
}
