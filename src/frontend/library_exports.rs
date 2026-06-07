//! Checked library-export extraction helpers.
//!
//! These types represent the public API surface extracted from typechecked symbols, so later stages (like `.incnlib`
//! manifest writing) can consume a stable, semantic model.

use std::collections::HashMap;

use crate::frontend::ast::{
    AliasDecl, ClassDecl, Declaration, DictEntry, EnumDecl, Expr, FunctionDecl, ImportDecl, ImportItem, ImportKind,
    ListEntry, Literal, ModelDecl, NewtypeDecl, PartialDecl, Program, Span, Spanned, TraitBound, TraitDecl,
    TypeAliasDecl, TypeParam, Visibility,
};
use crate::frontend::decorator_resolution;
use crate::frontend::module::canonicalize_source_module_segments;
use crate::frontend::symbols::{
    CallableParam, ClassInfo, FieldInfo, FunctionInfo, MethodInfo, ModelInfo, NewtypeInfo, ResolvedType, SymbolKind,
    TraitInfo, TypeBoundInfo, TypeInfo, ValueEnumBacking, ValueEnumValue, VariableInfo, resolve_type,
};
use crate::frontend::typechecker::{PartialProjectionTargetKind, TypeChecker};

#[derive(Clone, Copy)]
struct DefaultPathContext<'a> {
    checker: &'a TypeChecker,
    owner_module_path: Option<&'a [String]>,
}

impl<'a> DefaultPathContext<'a> {
    /// Build the default-expression path context for the checker currently exporting one source module.
    fn for_checker(checker: &'a TypeChecker) -> Self {
        Self {
            checker,
            owner_module_path: non_root_module_path(checker.current_module_path.as_deref()),
        }
    }

    /// Resolve a default-expression value path to the module that owns it.
    fn canonical_value_path(self, path: Vec<String>) -> Vec<String> {
        let Some(first) = path.first() else {
            return path;
        };
        if let Some(imported_path) = self.checker.import_aliases.get(first) {
            let mut canonical = imported_path.clone();
            canonical.extend(path.into_iter().skip(1));
            return canonical;
        }
        if path_is_already_absolute(&path) {
            return path;
        }
        let Some(owner_module_path) = self.owner_module_path else {
            return path;
        };
        if path.starts_with(owner_module_path) {
            return path;
        }
        let mut canonical = owner_module_path.to_vec();
        canonical.extend(path);
        canonical
    }
}

/// Return the non-root source module path that should qualify provider-owned default expressions.
fn non_root_module_path(path: Option<&[String]>) -> Option<&[String]> {
    let path = path?;
    if matches!(path, [segment] if segment == "main" || segment == "lib") {
        None
    } else {
        Some(path)
    }
}

/// Return whether a default-expression path is already rooted in a compiler-known namespace.
fn path_is_already_absolute(path: &[String]) -> bool {
    matches!(path.first().map(String::as_str), Some("std" | "rust" | "pub"))
        || path.first().map(String::as_str) == Some(incan_core::lang::stdlib::INCAN_STD_NAMESPACE)
}

#[derive(Debug, Clone)]
pub struct CheckedTypeParam {
    pub name: String,
    pub bounds: Vec<CheckedTypeBound>,
}

#[derive(Debug, Clone)]
pub struct CheckedTypeBound {
    pub name: String,
    pub source_name: Option<String>,
    pub type_args: Vec<ResolvedType>,
    pub module_path: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct CheckedField {
    pub name: String,
    pub ty: ResolvedType,
    pub has_default: bool,
    pub alias: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CheckedMethod {
    pub name: String,
    pub alias_of: Option<String>,
    pub type_params: Vec<CheckedTypeParam>,
    pub receiver: Option<crate::frontend::ast::Receiver>,
    pub params: Vec<CallableParam>,
    pub param_defaults: Vec<Option<CheckedParamDefault>>,
    pub return_type: ResolvedType,
    pub is_async: bool,
    pub has_body: bool,
}

#[derive(Debug, Clone)]
pub struct CheckedFunctionExport {
    pub name: String,
    pub emitted_name: Option<String>,
    pub type_params: Vec<CheckedTypeParam>,
    pub params: Vec<CallableParam>,
    pub param_defaults: Vec<Option<CheckedParamDefault>>,
    pub return_type: ResolvedType,
    pub is_async: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedExportIdentity {
    pub source_path: Vec<String>,
    pub projection: CheckedExportProjection,
}

impl CheckedExportIdentity {
    /// Build identity metadata for a public export that exposes its own source declaration directly.
    pub fn direct(source_path: Vec<String>) -> Self {
        Self {
            source_path,
            projection: CheckedExportProjection::Direct,
        }
    }

    /// Build identity metadata for a public alias that projects another source declaration or overload set.
    pub fn alias(source_path: Vec<String>, target_path: Vec<String>) -> Self {
        Self {
            source_path,
            projection: CheckedExportProjection::Alias { target_path },
        }
    }

    /// Build identity metadata for a public reexport that preserves the target declaration identity through a facade.
    pub fn reexport(source_path: Vec<String>, target_path: Vec<String>) -> Self {
        Self {
            source_path,
            projection: CheckedExportProjection::Reexport { target_path },
        }
    }

    /// Build identity metadata for a public partial preset that projects a callable or constructor target.
    pub fn partial(source_path: Vec<String>, target_path: Vec<String>, target_kind: CheckedPartialTargetKind) -> Self {
        Self {
            source_path,
            projection: CheckedExportProjection::Partial {
                target_path,
                target_kind,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckedExportProjection {
    Direct,
    Alias {
        target_path: Vec<String>,
    },
    Reexport {
        target_path: Vec<String>,
    },
    Partial {
        target_path: Vec<String>,
        target_kind: CheckedPartialTargetKind,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum CheckedParamDefault {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Bytes(Vec<u8>),
    None,
    List(Vec<CheckedParamDefault>),
    Dict(Vec<(CheckedParamDefault, CheckedParamDefault)>),
    ConstRef(Vec<String>),
    Call {
        path: Vec<String>,
        args: Vec<CheckedParamDefaultArg>,
        signature: Option<CheckedParamDefaultCallSignature>,
    },
    Unsupported,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckedParamDefaultArg {
    pub name: Option<String>,
    pub value: CheckedParamDefault,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckedParamDefaultCallSignature {
    pub params: Vec<CallableParam>,
    pub return_type: ResolvedType,
}

#[derive(Debug, Clone)]
pub struct CheckedPartialExport {
    pub name: String,
    pub target_path: Vec<String>,
    pub target_kind: CheckedPartialTargetKind,
    pub presets: Vec<CheckedPartialPreset>,
    pub type_params: Vec<CheckedTypeParam>,
    pub params: Vec<CallableParam>,
    pub return_type: ResolvedType,
    pub is_async: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckedPartialTargetKind {
    Function,
    ModelConstructor,
    ClassConstructor,
    NewtypeConstructor,
    Partial,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct CheckedPartialPreset {
    pub name: String,
    pub ty: ResolvedType,
    pub value: CheckedPresetValue,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CheckedPresetValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Bytes(Vec<u8>),
    None,
    List(Vec<CheckedPresetValue>),
    Dict(Vec<(CheckedPresetValue, CheckedPresetValue)>),
    ConstRef(Vec<String>),
    ModelLiteral {
        name: String,
        fields: Vec<(String, CheckedPresetValue)>,
    },
    Unsupported,
}

#[derive(Debug, Clone)]
pub struct CheckedTypeAliasExport {
    pub name: String,
    pub type_params: Vec<CheckedTypeParam>,
    pub target: ResolvedType,
}

#[derive(Debug, Clone)]
pub struct CheckedAliasExport {
    pub name: String,
    pub target_path: Vec<String>,
    pub projected_function: Option<CheckedFunctionExport>,
}

#[derive(Debug, Clone)]
pub struct CheckedModelExport {
    pub name: String,
    pub type_params: Vec<CheckedTypeParam>,
    pub traits: Vec<String>,
    pub trait_adoptions: Vec<CheckedTypeBound>,
    /// `@derive(...)` names that must remain available to `pub::` consumers.
    pub derives: Vec<String>,
    pub fields: Vec<CheckedField>,
    pub methods: Vec<CheckedMethod>,
}

#[derive(Debug, Clone)]
pub struct CheckedClassExport {
    pub name: String,
    pub type_params: Vec<CheckedTypeParam>,
    pub extends: Option<String>,
    pub traits: Vec<String>,
    pub trait_adoptions: Vec<CheckedTypeBound>,
    /// `@derive(...)` names that must remain available to `pub::` consumers.
    pub derives: Vec<String>,
    pub fields: Vec<CheckedField>,
    pub methods: Vec<CheckedMethod>,
}

#[derive(Debug, Clone)]
pub struct CheckedTraitExport {
    pub name: String,
    pub source_name: String,
    pub type_params: Vec<CheckedTypeParam>,
    /// Direct supertraits after typechecking (RFC 042), including resolved owner module metadata.
    pub supertraits: Vec<CheckedTypeBound>,
    pub requires: Vec<(String, ResolvedType)>,
    pub methods: Vec<CheckedMethod>,
}

#[derive(Debug, Clone)]
pub struct CheckedEnumVariant {
    pub name: String,
    pub fields: Vec<ResolvedType>,
    pub value: Option<ValueEnumValue>,
}

#[derive(Debug, Clone)]
pub struct CheckedEnumVariantAlias {
    pub name: String,
    pub target: String,
}

#[derive(Debug, Clone)]
pub struct CheckedEnumExport {
    pub name: String,
    pub type_params: Vec<CheckedTypeParam>,
    pub traits: Vec<String>,
    pub trait_adoptions: Vec<CheckedTypeBound>,
    pub value_type: Option<ValueEnumBacking>,
    pub variants: Vec<CheckedEnumVariant>,
    pub variant_aliases: Vec<CheckedEnumVariantAlias>,
    pub methods: Vec<CheckedMethod>,
    pub derives: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CheckedNewtypeExport {
    pub name: String,
    pub type_params: Vec<CheckedTypeParam>,
    pub traits: Vec<String>,
    pub trait_adoptions: Vec<CheckedTypeBound>,
    pub is_rusttype: bool,
    pub underlying: ResolvedType,
    pub methods: Vec<CheckedMethod>,
}

#[derive(Debug, Clone)]
pub struct CheckedConstExport {
    pub name: String,
    pub ty: ResolvedType,
}

#[derive(Debug, Clone)]
pub struct CheckedStaticExport {
    pub name: String,
    pub ty: ResolvedType,
}

#[derive(Debug, Clone)]
pub enum CheckedExportKind {
    Function(CheckedFunctionExport),
    Partial(CheckedPartialExport),
    Alias(CheckedAliasExport),
    TypeAlias(CheckedTypeAliasExport),
    Model(CheckedModelExport),
    Class(CheckedClassExport),
    Trait(CheckedTraitExport),
    Enum(CheckedEnumExport),
    Newtype(CheckedNewtypeExport),
    Const(CheckedConstExport),
    Static(CheckedStaticExport),
}

#[derive(Debug, Clone)]
pub struct CheckedNamedExport {
    pub name: String,
    pub identity: CheckedExportIdentity,
    pub kind: CheckedExportKind,
}

/// Collect checked public exports from one program while preserving alias identity.
pub fn collect_checked_public_exports(program: &Program, checker: &TypeChecker) -> Vec<CheckedNamedExport> {
    let mut exports = Vec::new();

    for decl in &program.declarations {
        match &decl.node {
            Declaration::Function(function) if matches!(function.visibility, Visibility::Public) => {
                if let Some(export) = checked_function_export(function, checker, decl.span) {
                    exports.push(CheckedNamedExport {
                        name: export.name.clone(),
                        identity: checked_direct_identity(checker, &export.name),
                        kind: CheckedExportKind::Function(export),
                    });
                }
            }
            Declaration::TypeAlias(alias) if matches!(alias.visibility, Visibility::Public) => {
                exports.push(CheckedNamedExport {
                    name: alias.name.clone(),
                    identity: checked_direct_identity(checker, &alias.name),
                    kind: CheckedExportKind::TypeAlias(checked_type_alias_export(alias, checker)),
                });
            }
            Declaration::Model(model) if matches!(model.visibility, Visibility::Public) => {
                if let Some(export) = checked_model_export(model, checker) {
                    exports.push(CheckedNamedExport {
                        name: export.name.clone(),
                        identity: checked_direct_identity(checker, &export.name),
                        kind: CheckedExportKind::Model(export),
                    });
                }
            }
            Declaration::Class(class) if matches!(class.visibility, Visibility::Public) => {
                if let Some(export) = checked_class_export(class, checker) {
                    exports.push(CheckedNamedExport {
                        name: export.name.clone(),
                        identity: checked_direct_identity(checker, &export.name),
                        kind: CheckedExportKind::Class(export),
                    });
                }
            }
            Declaration::Trait(trait_decl) if matches!(trait_decl.visibility, Visibility::Public) => {
                if let Some(export) = checked_trait_export(trait_decl, checker) {
                    exports.push(CheckedNamedExport {
                        name: export.name.clone(),
                        identity: checked_direct_identity(checker, &export.name),
                        kind: CheckedExportKind::Trait(export),
                    });
                }
            }
            Declaration::Enum(enum_decl) if matches!(enum_decl.visibility, Visibility::Public) => {
                if let Some(export) = checked_enum_export(enum_decl, checker) {
                    exports.push(CheckedNamedExport {
                        name: export.name.clone(),
                        identity: checked_direct_identity(checker, &export.name),
                        kind: CheckedExportKind::Enum(export),
                    });
                }
            }
            Declaration::Newtype(newtype_decl) if matches!(newtype_decl.visibility, Visibility::Public) => {
                if let Some(export) = checked_newtype_export(newtype_decl, checker) {
                    exports.push(CheckedNamedExport {
                        name: export.name.clone(),
                        identity: checked_direct_identity(checker, &export.name),
                        kind: CheckedExportKind::Newtype(export),
                    });
                }
            }
            Declaration::Const(konst) if matches!(konst.visibility, Visibility::Public) => {
                if let Some(export) = checked_const_export(konst.name.as_str(), checker) {
                    exports.push(CheckedNamedExport {
                        name: export.name.clone(),
                        identity: checked_direct_identity(checker, &export.name),
                        kind: CheckedExportKind::Const(export),
                    });
                }
            }
            Declaration::Static(static_decl) if matches!(static_decl.visibility, Visibility::Public) => {
                if let Some(export) = checked_static_export(static_decl.name.as_str(), checker) {
                    exports.push(CheckedNamedExport {
                        name: export.name.clone(),
                        identity: checked_direct_identity(checker, &export.name),
                        kind: CheckedExportKind::Static(export),
                    });
                }
            }
            Declaration::Alias(alias) if matches!(alias.visibility, Visibility::Public) => {
                exports.extend(checked_alias_exports(alias, checker));
            }
            Declaration::Import(import) if matches!(import.visibility, Visibility::Public) => {
                exports.extend(checked_import_exports(import, checker));
            }
            Declaration::Partial(partial) if matches!(partial.visibility, Visibility::Public) => {
                if let Some(export) = checked_partial_export(partial, checker) {
                    exports.push(CheckedNamedExport {
                        name: export.name.clone(),
                        identity: CheckedExportIdentity::partial(
                            checked_export_source_path(checker, &export.name),
                            export.target_path.clone(),
                            export.target_kind,
                        ),
                        kind: CheckedExportKind::Partial(export),
                    });
                }
            }
            _ => {}
        }
    }

    exports.sort_by(|a, b| a.name.cmp(&b.name));
    exports
}

/// Return the source identity path for one public binding in the currently exported module.
fn checked_export_source_path(checker: &TypeChecker, name: &str) -> Vec<String> {
    let mut path = checker.current_module_path.clone().unwrap_or_default();
    path.push(name.to_string());
    path
}

/// Return direct identity metadata for an export that is not projecting another declaration.
fn checked_direct_identity(checker: &TypeChecker, name: &str) -> CheckedExportIdentity {
    CheckedExportIdentity::direct(checked_export_source_path(checker, name))
}

/// Build checked public export entries for a module-level alias.
fn checked_alias_exports(alias: &AliasDecl, checker: &TypeChecker) -> Vec<CheckedNamedExport> {
    let Some(symbol) = checker.lookup_symbol(alias.name.as_str()) else {
        return Vec::new();
    };
    let target_path = DefaultPathContext::for_checker(checker).canonical_value_path(alias.target.segments.clone());
    let identity = CheckedExportIdentity::alias(checked_export_source_path(checker, &alias.name), target_path.clone());
    if let SymbolKind::FunctionOverloads(overloads) = &symbol.kind {
        return checked_overload_function_exports(alias.name.clone(), overloads, identity);
    }
    let projected_function = checked_projected_function_export(&alias.name, &symbol.kind);
    vec![CheckedNamedExport {
        name: alias.name.clone(),
        identity,
        kind: CheckedExportKind::Alias(CheckedAliasExport {
            name: alias.name.clone(),
            target_path,
            projected_function,
        }),
    }]
}

/// Return checked exports introduced by an import declaration.
fn checked_import_exports(import: &ImportDecl, checker: &TypeChecker) -> Vec<CheckedNamedExport> {
    match &import.kind {
        ImportKind::From { module, items } => {
            let base_path =
                canonicalize_source_module_segments(&decorator_resolution::path_segments_with_prefix(module));
            checked_source_import_item_exports(items, module, base_path, checker)
        }
        ImportKind::RustFrom {
            crate_name,
            path,
            items,
            ..
        } => {
            let mut base_path = vec!["rust".to_string(), crate_name.clone()];
            base_path.extend(path.iter().cloned());
            checked_import_item_exports(items, base_path, checker)
        }
        ImportKind::PubFrom { library, items } => {
            let base_path = vec!["pub".to_string(), library.clone()];
            checked_import_item_exports(items, base_path, checker)
        }
        _ => Vec::new(),
    }
}

/// Return checked exports introduced by source-module import items.
fn checked_source_import_item_exports(
    items: &[ImportItem],
    module: &crate::frontend::ast::ImportPath,
    base_path: Vec<String>,
    checker: &TypeChecker,
) -> Vec<CheckedNamedExport> {
    items
        .iter()
        .flat_map(|item| {
            let exported_name = item.alias.as_ref().unwrap_or(&item.name).clone();
            let mut target_path = base_path.clone();
            target_path.push(item.name.clone());
            let identity = CheckedExportIdentity::reexport(target_path.clone(), target_path.clone());
            if let Some(overloads) = checker.type_info().function_overloads(&exported_name) {
                return checked_overload_function_exports(exported_name, overloads, identity);
            }
            let symbol_kind = checker
                .dependency_member_symbol_for_path(module, &item.name)
                .or_else(|| {
                    checker
                        .lookup_symbol(exported_name.as_str())
                        .map(|symbol| symbol.kind.clone())
                });
            checked_import_export_from_symbol_kind(exported_name, target_path, symbol_kind.as_ref(), identity)
        })
        .collect()
}

/// Return checked exports introduced by one imported item.
fn checked_import_item_exports(
    items: &[ImportItem],
    base_path: Vec<String>,
    checker: &TypeChecker,
) -> Vec<CheckedNamedExport> {
    items
        .iter()
        .flat_map(|item| {
            let exported_name = item.alias.as_ref().unwrap_or(&item.name).clone();
            let mut target_path = base_path.clone();
            target_path.push(item.name.clone());
            let symbol_kind = checker.lookup_symbol(exported_name.as_str()).map(|symbol| &symbol.kind);
            let identity = CheckedExportIdentity::reexport(target_path.clone(), target_path.clone());
            checked_import_export_from_symbol_kind(exported_name, target_path, symbol_kind, identity)
        })
        .collect()
}

/// Convert one imported symbol kind into checked public export entries.
fn checked_import_export_from_symbol_kind(
    exported_name: String,
    target_path: Vec<String>,
    symbol_kind: Option<&SymbolKind>,
    identity: CheckedExportIdentity,
) -> Vec<CheckedNamedExport> {
    if let Some(SymbolKind::FunctionOverloads(overloads)) = symbol_kind {
        return checked_overload_function_exports(exported_name, overloads, identity);
    }
    let projected_function = symbol_kind.and_then(|kind| checked_projected_function_export(&exported_name, kind));
    vec![CheckedNamedExport {
        name: exported_name.clone(),
        identity,
        kind: CheckedExportKind::Alias(CheckedAliasExport {
            name: exported_name,
            target_path,
            projected_function,
        }),
    }]
}

/// Convert a same-name overload group into separate function exports under one source name.
fn checked_overload_function_exports(
    exported_name: String,
    overloads: &[crate::frontend::symbols::FunctionOverloadInfo],
    identity: CheckedExportIdentity,
) -> Vec<CheckedNamedExport> {
    overloads
        .iter()
        .map(|overload| {
            let export = checked_alias_function_export(&exported_name, &overload.info);
            CheckedNamedExport {
                name: export.name.clone(),
                identity: identity.clone(),
                kind: CheckedExportKind::Function(export),
            }
        })
        .collect()
}

/// Build manifest-ready callable metadata for an alias that projects a function.
fn checked_alias_function_export(name: &str, info: &FunctionInfo) -> CheckedFunctionExport {
    CheckedFunctionExport {
        name: name.to_string(),
        emitted_name: info.emitted_name.clone(),
        type_params: checked_function_type_params(info),
        params: info.params.clone(),
        param_defaults: vec![None; info.params.len()],
        return_type: info.return_type.clone(),
        is_async: info.is_async,
    }
}

/// Return the checked export for a projected callable alias.
fn checked_projected_function_export(name: &str, kind: &SymbolKind) -> Option<CheckedFunctionExport> {
    match kind {
        SymbolKind::Function(info) => Some(checked_alias_function_export(name, info)),
        SymbolKind::Variable(VariableInfo {
            ty: ResolvedType::Function(params, return_type),
            ..
        }) => Some(CheckedFunctionExport {
            name: name.to_string(),
            emitted_name: None,
            type_params: Vec::new(),
            params: params.clone(),
            param_defaults: vec![None; params.len()],
            return_type: return_type.as_ref().clone(),
            is_async: false,
        }),
        _ => None,
    }
}

/// Build checked export metadata for a public partial callable preset.
fn checked_partial_export(partial: &PartialDecl, checker: &TypeChecker) -> Option<CheckedPartialExport> {
    let symbol = checker.lookup_symbol(partial.name.as_str())?;
    let SymbolKind::Function(info) = &symbol.kind else {
        return None;
    };
    let projection = checker.type_info.partial_projection(&partial.name);
    let default_context = DefaultPathContext::for_checker(checker);
    let presets = partial
        .args
        .iter()
        .map(|arg| CheckedPartialPreset {
            name: arg.name.clone(),
            ty: info
                .params
                .iter()
                .find(|param| param.name() == Some(arg.name.as_str()))
                .map(|param| param.ty.clone())
                .unwrap_or(ResolvedType::Unknown),
            value: checked_preset_value(&arg.value.node, default_context),
        })
        .collect();
    Some(CheckedPartialExport {
        name: partial.name.clone(),
        target_path: projection.map_or_else(
            || partial.target.segments.clone(),
            |projection| projection.target_path.clone(),
        ),
        target_kind: projection.map_or_else(
            || checked_partial_target_kind(partial, checker),
            |projection| checked_partial_target_kind_from_projection(projection.target_kind),
        ),
        presets,
        type_params: info
            .type_params
            .iter()
            .map(|name| CheckedTypeParam {
                name: name.clone(),
                bounds: info
                    .type_param_bound_details
                    .get(name)
                    .map_or_else(Vec::new, |bounds| map_type_bound_infos(bounds)),
            })
            .collect(),
        params: info.params.clone(),
        return_type: info.return_type.clone(),
        is_async: info.is_async,
    })
}

/// Convert collection-time partial projection metadata into public export metadata.
fn checked_partial_target_kind_from_projection(kind: PartialProjectionTargetKind) -> CheckedPartialTargetKind {
    match kind {
        PartialProjectionTargetKind::Function => CheckedPartialTargetKind::Function,
        PartialProjectionTargetKind::ModelConstructor => CheckedPartialTargetKind::ModelConstructor,
        PartialProjectionTargetKind::ClassConstructor => CheckedPartialTargetKind::ClassConstructor,
        PartialProjectionTargetKind::NewtypeConstructor => CheckedPartialTargetKind::NewtypeConstructor,
        PartialProjectionTargetKind::Unknown => CheckedPartialTargetKind::Unknown,
    }
}

/// Classify the direct target kind for manifest/API partial provenance.
fn checked_partial_target_kind(partial: &PartialDecl, checker: &TypeChecker) -> CheckedPartialTargetKind {
    let [target] = partial.target.segments.as_slice() else {
        return CheckedPartialTargetKind::Unknown;
    };
    match checker.lookup_symbol(target).map(|symbol| &symbol.kind) {
        Some(SymbolKind::Function(_) | SymbolKind::FunctionOverloads(_)) => CheckedPartialTargetKind::Function,
        Some(SymbolKind::Type(TypeInfo::Model(_))) => CheckedPartialTargetKind::ModelConstructor,
        Some(SymbolKind::Type(TypeInfo::Class(_))) => CheckedPartialTargetKind::ClassConstructor,
        Some(SymbolKind::Type(TypeInfo::Newtype(_))) => CheckedPartialTargetKind::NewtypeConstructor,
        Some(SymbolKind::Type(_) | SymbolKind::Trait(_) | SymbolKind::Variable(_) | SymbolKind::Static(_))
        | Some(SymbolKind::Field(_))
        | Some(SymbolKind::Property(_))
        | Some(SymbolKind::Module(_))
        | Some(SymbolKind::Variant(_))
        | Some(SymbolKind::RustItem(_))
        | None => CheckedPartialTargetKind::Unknown,
    }
}

/// Convert a preset expression into the metadata-safe subset used by public partial provenance.
fn checked_preset_value(expr: &Expr, context: DefaultPathContext<'_>) -> CheckedPresetValue {
    match expr {
        Expr::Literal(literal) => checked_preset_literal(literal),
        Expr::Ident(name) => CheckedPresetValue::ConstRef(context.canonical_value_path(vec![name.clone()])),
        Expr::Field(base, field) => {
            let mut path = checked_preset_path(&base.node);
            if path.is_empty() {
                CheckedPresetValue::Unsupported
            } else {
                path.push(field.clone());
                CheckedPresetValue::ConstRef(context.canonical_value_path(path))
            }
        }
        Expr::List(entries) => CheckedPresetValue::List(
            entries
                .iter()
                .map(|entry| match entry {
                    ListEntry::Element(value) => checked_preset_value(&value.node, context),
                    ListEntry::Spread(_) => CheckedPresetValue::Unsupported,
                })
                .collect(),
        ),
        Expr::Dict(entries) => {
            let pairs = entries
                .iter()
                .map(|entry| match entry {
                    DictEntry::Pair(key, value) => (
                        checked_preset_value(&key.node, context),
                        checked_preset_value(&value.node, context),
                    ),
                    DictEntry::Spread(_) => (CheckedPresetValue::Unsupported, CheckedPresetValue::Unsupported),
                })
                .collect();
            CheckedPresetValue::Dict(pairs)
        }
        Expr::Call(callee, _type_args, args) => {
            let path = checked_preset_path(&callee.node);
            let [name] = path.as_slice() else {
                return CheckedPresetValue::Unsupported;
            };
            let mut fields = Vec::new();
            for arg in args {
                let crate::frontend::ast::CallArg::Named(field, value) = arg else {
                    return CheckedPresetValue::Unsupported;
                };
                fields.push((field.clone(), checked_preset_value(&value.node, context)));
            }
            CheckedPresetValue::ModelLiteral {
                name: name.clone(),
                fields,
            }
        }
        Expr::Constructor(name, args) => {
            let mut fields = Vec::new();
            for arg in args {
                let crate::frontend::ast::CallArg::Named(field, value) = arg else {
                    return CheckedPresetValue::Unsupported;
                };
                fields.push((field.clone(), checked_preset_value(&value.node, context)));
            }
            CheckedPresetValue::ModelLiteral {
                name: name.clone(),
                fields,
            }
        }
        _ => CheckedPresetValue::Unsupported,
    }
}

/// Convert a source parameter default into the metadata-safe subset that public library consumers can materialize.
fn checked_param_default(expr: &Spanned<Expr>, context: DefaultPathContext<'_>) -> CheckedParamDefault {
    match &expr.node {
        Expr::Literal(literal) => checked_param_default_literal(literal),
        Expr::Ident(name) => CheckedParamDefault::ConstRef(context.canonical_value_path(vec![name.clone()])),
        Expr::Field(base, field) => {
            let mut path = checked_preset_path(&base.node);
            if path.is_empty() {
                CheckedParamDefault::Unsupported
            } else {
                path.push(field.clone());
                CheckedParamDefault::ConstRef(context.canonical_value_path(path))
            }
        }
        Expr::List(entries) => CheckedParamDefault::List(
            entries
                .iter()
                .map(|entry| match entry {
                    ListEntry::Element(value) => checked_param_default(value, context),
                    ListEntry::Spread(_) => CheckedParamDefault::Unsupported,
                })
                .collect(),
        ),
        Expr::Dict(entries) => CheckedParamDefault::Dict(
            entries
                .iter()
                .map(|entry| match entry {
                    DictEntry::Pair(key, value) => (
                        checked_param_default(key, context),
                        checked_param_default(value, context),
                    ),
                    DictEntry::Spread(_) => (CheckedParamDefault::Unsupported, CheckedParamDefault::Unsupported),
                })
                .collect(),
        ),
        Expr::Call(callee, _type_args, args) => {
            let path = context.canonical_value_path(checked_preset_path(&callee.node));
            if path.is_empty() {
                return CheckedParamDefault::Unsupported;
            }
            let args = args
                .iter()
                .map(|arg| match arg {
                    crate::frontend::ast::CallArg::Positional(value) => CheckedParamDefaultArg {
                        name: None,
                        value: checked_param_default(value, context),
                    },
                    crate::frontend::ast::CallArg::Named(name, value) => CheckedParamDefaultArg {
                        name: Some(name.clone()),
                        value: checked_param_default(value, context),
                    },
                    crate::frontend::ast::CallArg::PositionalUnpack(_)
                    | crate::frontend::ast::CallArg::KeywordUnpack(_) => CheckedParamDefaultArg {
                        name: None,
                        value: CheckedParamDefault::Unsupported,
                    },
                })
                .collect();
            CheckedParamDefault::Call {
                path,
                args,
                signature: checked_param_default_call_signature(callee, context.checker),
            }
        }
        _ => CheckedParamDefault::Unsupported,
    }
}

/// Capture the checked callable surface for a default-expression helper call.
fn checked_param_default_call_signature(
    callee: &Spanned<Expr>,
    checker: &TypeChecker,
) -> Option<CheckedParamDefaultCallSignature> {
    let callee_ty = checker
        .type_info()
        .expr_type(callee.span)
        .cloned()
        .or_else(|| checked_param_default_callee_symbol_type(&callee.node, checker))?;
    let ResolvedType::Function(params, return_type) = callee_ty else {
        return None;
    };
    Some(CheckedParamDefaultCallSignature {
        params,
        return_type: return_type.as_ref().clone(),
    })
}

/// Fallback callable-surface lookup for default calls whose callee span has no recorded type.
fn checked_param_default_callee_symbol_type(callee: &Expr, checker: &TypeChecker) -> Option<ResolvedType> {
    let path = checked_preset_path(callee);
    if path.is_empty() {
        return None;
    }
    let symbol_names = [(path.len() > 1).then(|| path.join(".")), path.last().cloned()];
    for name in symbol_names.into_iter().flatten() {
        let Some(symbol) = checker.lookup_symbol(&name) else {
            continue;
        };
        match &symbol.kind {
            SymbolKind::Function(info) => {
                return Some(ResolvedType::Function(
                    info.params.clone(),
                    Box::new(info.return_type.clone()),
                ));
            }
            SymbolKind::Variable(VariableInfo {
                ty: ResolvedType::Function(params, return_type),
                ..
            }) => return Some(ResolvedType::Function(params.clone(), return_type.clone())),
            _ => {}
        }
    }
    None
}

/// Convert a literal parameter default into checked public metadata.
fn checked_param_default_literal(literal: &Literal) -> CheckedParamDefault {
    match literal {
        Literal::Int(value) => CheckedParamDefault::Int(value.value),
        Literal::Float(value) => CheckedParamDefault::Float(value.value),
        Literal::Decimal(value) => CheckedParamDefault::String(value.repr.clone()),
        Literal::String(value) => CheckedParamDefault::String(value.clone()),
        Literal::Bytes(value) => CheckedParamDefault::Bytes(value.clone()),
        Literal::Bool(value) => CheckedParamDefault::Bool(*value),
        Literal::None => CheckedParamDefault::None,
    }
}

/// Convert a literal preset expression into checked partial export metadata.
fn checked_preset_literal(literal: &Literal) -> CheckedPresetValue {
    match literal {
        Literal::Int(value) => CheckedPresetValue::Int(value.value),
        Literal::Float(value) => CheckedPresetValue::Float(value.value),
        Literal::Decimal(value) => CheckedPresetValue::String(value.repr.clone()),
        Literal::String(value) => CheckedPresetValue::String(value.clone()),
        Literal::Bytes(value) => CheckedPresetValue::Bytes(value.clone()),
        Literal::Bool(value) => CheckedPresetValue::Bool(*value),
        Literal::None => CheckedPresetValue::None,
    }
}

/// Extract a dotted constant/model path from a metadata-safe preset expression.
fn checked_preset_path(expr: &Expr) -> Vec<String> {
    match expr {
        Expr::Ident(name) => vec![name.clone()],
        Expr::Field(base, field) => {
            let mut path = checked_preset_path(&base.node);
            if path.is_empty() {
                return Vec::new();
            }
            path.push(field.clone());
            path
        }
        _ => Vec::new(),
    }
}

/// Build checked export metadata for a function or callable-valued decorated function binding.
fn checked_function_export(
    function: &FunctionDecl,
    checker: &TypeChecker,
    span: Span,
) -> Option<CheckedFunctionExport> {
    let symbol = checker.lookup_symbol(function.name.as_str())?;
    let (params, return_type, is_async, emitted_name) = match &symbol.kind {
        SymbolKind::Function(FunctionInfo {
            params,
            return_type,
            is_async,
            emitted_name,
            ..
        }) => (params.clone(), return_type.clone(), *is_async, emitted_name.clone()),
        SymbolKind::FunctionOverloads(overloads) => {
            let overload = overloads.iter().find(|overload| overload.span == span)?;
            (
                overload.info.params.clone(),
                overload.info.return_type.clone(),
                overload.info.is_async,
                overload.info.emitted_name.clone(),
            )
        }
        SymbolKind::Variable(VariableInfo {
            ty: ResolvedType::Function(params, return_type),
            ..
        }) => {
            // Callable values do not yet carry asyncness, so preserve the source declaration marker until decorator
            // typing records async callable metadata explicitly.
            (params.clone(), return_type.as_ref().clone(), function.is_async(), None)
        }
        _ => return None,
    };

    let default_context = DefaultPathContext::for_checker(checker);
    Some(CheckedFunctionExport {
        name: function.name.clone(),
        emitted_name,
        type_params: checked_type_params(&function.type_params, checker),
        param_defaults: function
            .params
            .iter()
            .map(|param| {
                param
                    .node
                    .default
                    .as_ref()
                    .map(|default| checked_param_default(default, default_context))
            })
            .collect(),
        params,
        return_type,
        is_async,
    })
}

/// Convert checked function metadata type parameters into export metadata type parameters.
fn checked_function_type_params(info: &FunctionInfo) -> Vec<CheckedTypeParam> {
    info.type_params
        .iter()
        .map(|name| CheckedTypeParam {
            name: name.clone(),
            bounds: info
                .type_param_bound_details
                .get(name)
                .map_or_else(Vec::new, |bounds| map_type_bound_infos(bounds)),
        })
        .collect()
}

fn checked_type_alias_export(alias: &TypeAliasDecl, checker: &TypeChecker) -> CheckedTypeAliasExport {
    let target = resolve_type(&alias.target.node, &checker.symbols);
    CheckedTypeAliasExport {
        name: alias.name.clone(),
        type_params: checked_type_params(&alias.type_params, checker),
        target,
    }
}

/// Extract a checked public model export from collected model symbol metadata.
fn checked_model_export(model: &ModelDecl, checker: &TypeChecker) -> Option<CheckedModelExport> {
    let symbol = checker.lookup_symbol(model.name.as_str())?;
    let SymbolKind::Type(TypeInfo::Model(ModelInfo {
        traits,
        trait_adoptions,
        derives,
        fields,
        field_order,
        method_overloads,
        ..
    })) = &symbol.kind
    else {
        return None;
    };

    Some(CheckedModelExport {
        name: model.name.clone(),
        type_params: checked_type_params(&model.type_params, checker),
        traits: sorted_vec(traits.to_vec()),
        trait_adoptions: sorted_type_bounds(map_type_bound_infos(trait_adoptions)),
        derives: sorted_vec(derives.to_vec()),
        fields: map_fields(fields, field_order),
        methods: map_method_overloads_with_defaults(method_overloads, &model.methods, checker),
    })
}

/// Extract a checked public class export from collected class symbol metadata.
fn checked_class_export(class: &ClassDecl, checker: &TypeChecker) -> Option<CheckedClassExport> {
    let symbol = checker.lookup_symbol(class.name.as_str())?;
    let SymbolKind::Type(TypeInfo::Class(ClassInfo {
        extends,
        traits,
        trait_adoptions,
        derives,
        fields,
        field_order,
        method_overloads,
        ..
    })) = &symbol.kind
    else {
        return None;
    };

    Some(CheckedClassExport {
        name: class.name.clone(),
        type_params: checked_type_params(&class.type_params, checker),
        extends: extends.clone(),
        traits: sorted_vec(traits.to_vec()),
        trait_adoptions: sorted_type_bounds(map_type_bound_infos(trait_adoptions)),
        derives: sorted_vec(derives.to_vec()),
        fields: map_fields(fields, field_order),
        methods: map_method_overloads_with_defaults(method_overloads, &class.methods, checker),
    })
}

/// Extract the checked public trait contract, preserving resolved supertrait identities for package consumers.
fn checked_trait_export(trait_decl: &TraitDecl, checker: &TypeChecker) -> Option<CheckedTraitExport> {
    let symbol = checker.lookup_symbol(trait_decl.name.as_str())?;
    let SymbolKind::Trait(TraitInfo { requires, methods, .. }) = &symbol.kind else {
        return None;
    };

    let mut sorted_requires = requires.clone();
    sorted_requires.sort_by(|(left, _), (right, _)| left.cmp(right));

    Some(CheckedTraitExport {
        name: trait_decl.name.clone(),
        source_name: trait_decl.name.clone(),
        type_params: checked_type_params(&trait_decl.type_params, checker),
        supertraits: sorted_type_bounds(checked_spanned_trait_bounds(&trait_decl.traits, checker)),
        requires: sorted_requires,
        methods: map_methods_with_defaults(methods, &trait_decl.methods, checker),
    })
}

/// Extract the checked public enum contract, including value-enum metadata when present.
fn checked_enum_export(enum_decl: &EnumDecl, checker: &TypeChecker) -> Option<CheckedEnumExport> {
    let symbol = checker.lookup_symbol(enum_decl.name.as_str())?;
    let SymbolKind::Type(TypeInfo::Enum(enum_info)) = &symbol.kind else {
        return None;
    };

    let mut variants = Vec::new();
    for variant in &enum_decl.variants {
        let fields = checker
            .lookup_symbol(variant.node.name.as_str())
            .and_then(|symbol| match &symbol.kind {
                SymbolKind::Variant(info) => Some(info.fields.clone()),
                _ => None,
            })
            .unwrap_or_else(|| {
                variant
                    .node
                    .fields
                    .iter()
                    .map(|field| resolve_type(&field.node, &checker.symbols))
                    .collect()
            });

        variants.push(CheckedEnumVariant {
            name: variant.node.name.clone(),
            fields,
            value: enum_info
                .value_enum
                .as_ref()
                .and_then(|value_enum| value_enum.values.get(&variant.node.name).cloned()),
        });
    }

    Some(CheckedEnumExport {
        name: enum_decl.name.clone(),
        type_params: checked_type_params(&enum_decl.type_params, checker),
        traits: sorted_vec(enum_info.traits.clone()),
        trait_adoptions: sorted_type_bounds(map_type_bound_infos(&enum_info.trait_adoptions)),
        value_type: enum_info.value_enum.as_ref().map(|value_enum| value_enum.value_type),
        variants,
        variant_aliases: enum_decl
            .variant_aliases
            .iter()
            .map(|alias| CheckedEnumVariantAlias {
                name: alias.node.name.clone(),
                target: alias.node.target.clone(),
            })
            .collect(),
        methods: map_method_overloads_with_defaults(&enum_info.method_overloads, &enum_decl.methods, checker),
        derives: sorted_vec(enum_info.derives.clone()),
    })
}

/// Build a manifest-ready newtype export from the checked symbol metadata.
fn checked_newtype_export(newtype_decl: &NewtypeDecl, checker: &TypeChecker) -> Option<CheckedNewtypeExport> {
    let symbol = checker.lookup_symbol(newtype_decl.name.as_str())?;
    let SymbolKind::Type(TypeInfo::Newtype(NewtypeInfo {
        is_rusttype,
        underlying,
        traits,
        trait_adoptions,
        method_overloads,
        ..
    })) = &symbol.kind
    else {
        return None;
    };

    Some(CheckedNewtypeExport {
        name: newtype_decl.name.clone(),
        type_params: checked_type_params(&newtype_decl.type_params, checker),
        traits: sorted_vec(traits.clone()),
        trait_adoptions: sorted_type_bounds(map_type_bound_infos(trait_adoptions)),
        is_rusttype: *is_rusttype,
        underlying: underlying.clone(),
        methods: map_method_overloads_with_defaults(method_overloads, &newtype_decl.methods, checker),
    })
}

fn checked_const_export(const_name: &str, checker: &TypeChecker) -> Option<CheckedConstExport> {
    let symbol = checker.lookup_symbol(const_name)?;
    let SymbolKind::Variable(variable_info) = &symbol.kind else {
        return None;
    };
    Some(CheckedConstExport {
        name: const_name.to_string(),
        ty: variable_info.ty.clone(),
    })
}

fn checked_static_export(static_name: &str, checker: &TypeChecker) -> Option<CheckedStaticExport> {
    let symbol = checker.lookup_symbol(static_name)?;
    let SymbolKind::Static(static_info) = &symbol.kind else {
        return None;
    };
    Some(CheckedStaticExport {
        name: static_name.to_string(),
        ty: static_info.ty.clone(),
    })
}

fn checked_type_params(type_params: &[TypeParam], checker: &TypeChecker) -> Vec<CheckedTypeParam> {
    type_params
        .iter()
        .map(|type_param| CheckedTypeParam {
            name: type_param.name.clone(),
            bounds: checked_trait_bounds(&type_param.bounds, checker),
        })
        .collect()
}

/// Resolve source trait bounds into checked export metadata.
fn checked_trait_bounds(bounds: &[TraitBound], checker: &TypeChecker) -> Vec<CheckedTypeBound> {
    bounds.iter().map(|bound| checked_trait_bound(bound, checker)).collect()
}

/// Resolve source trait bounds that still carry their parse spans.
fn checked_spanned_trait_bounds(bounds: &[Spanned<TraitBound>], checker: &TypeChecker) -> Vec<CheckedTypeBound> {
    bounds
        .iter()
        .map(|bound| checked_trait_bound(&bound.node, checker))
        .collect()
}

/// Resolve one source trait bound into checked export metadata.
fn checked_trait_bound(bound: &TraitBound, checker: &TypeChecker) -> CheckedTypeBound {
    CheckedTypeBound {
        name: bound.name.clone(),
        source_name: checker.trait_bound_source_name(&bound.name),
        type_args: bound
            .type_args
            .iter()
            .map(|type_arg| resolve_type(&type_arg.node, &checker.symbols))
            .collect(),
        module_path: checker.trait_bound_module_path(&bound.name),
    }
}

/// Convert collected trait adoption metadata into the manifest-ready checked bound shape.
fn map_type_bound_infos(bounds: &[TypeBoundInfo]) -> Vec<CheckedTypeBound> {
    bounds
        .iter()
        .map(|bound| CheckedTypeBound {
            name: bound.name.clone(),
            source_name: bound.source_name.clone(),
            type_args: bound.type_args.clone(),
            module_path: bound.module_path.clone(),
        })
        .collect()
}

/// Sort generic trait adoptions deterministically for stable library manifests.
fn sorted_type_bounds(mut bounds: Vec<CheckedTypeBound>) -> Vec<CheckedTypeBound> {
    bounds.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| type_args_sort_key(&left.type_args).cmp(&type_args_sort_key(&right.type_args)))
    });
    bounds
}

/// Render type arguments into a deterministic key for sorting trait adoptions.
fn type_args_sort_key(args: &[ResolvedType]) -> String {
    args.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")
}

/// Map checked field metadata into manifest order, preserving source declaration order when it is available.
///
/// Dependency manifests use this order to reconstruct constructor and partial surfaces. Any fields missing from the
/// recorded source order are appended lexically so generated metadata remains deterministic.
fn map_fields(fields: &HashMap<String, FieldInfo>, field_order: &[String]) -> Vec<CheckedField> {
    let mut used = std::collections::HashSet::new();
    let mut entries = Vec::with_capacity(fields.len());
    for name in field_order {
        if let Some(info) = fields.get(name) {
            used.insert(name.as_str());
            entries.push(CheckedField {
                name: name.clone(),
                ty: info.ty.clone(),
                has_default: info.has_default,
                alias: info.alias.clone(),
                description: info.description.clone(),
            });
        }
    }
    let mut remaining = fields
        .iter()
        .filter(|(name, _)| !used.contains(name.as_str()))
        .collect::<Vec<_>>();
    remaining.sort_by_key(|(name, _)| *name);
    entries.extend(remaining.into_iter().map(|(name, info)| CheckedField {
        name: name.clone(),
        ty: info.ty.clone(),
        has_default: info.has_default,
        alias: info.alias.clone(),
        description: info.description.clone(),
    }));
    entries
}

/// Convert legacy one-method-per-name metadata into checked export methods.
fn map_methods_with_defaults(
    methods: &HashMap<String, MethodInfo>,
    ast_methods: &[Spanned<crate::frontend::ast::MethodDecl>],
    checker: &TypeChecker,
) -> Vec<CheckedMethod> {
    let mut entries: Vec<_> = methods
        .iter()
        .filter(|(name, _)| is_exported_method_name(name))
        .map(|(name, info)| checked_method_from_info(name, info))
        .collect();
    attach_method_defaults(&mut entries, ast_methods, checker);
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    entries
}

/// Flatten same-name overload groups into manifest-ready checked methods without losing RFC 025 trait implementations.
fn map_method_overloads_with_defaults(
    method_overloads: &HashMap<String, Vec<MethodInfo>>,
    ast_methods: &[Spanned<crate::frontend::ast::MethodDecl>],
    checker: &TypeChecker,
) -> Vec<CheckedMethod> {
    let mut entries: Vec<_> = method_overloads
        .iter()
        .filter(|(name, _)| is_exported_method_name(name))
        .flat_map(|(name, overloads)| overloads.iter().map(|info| checked_method_from_info(name, info)))
        .collect();
    attach_method_defaults(&mut entries, ast_methods, checker);
    entries.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| method_signature_sort_key(left).cmp(&method_signature_sort_key(right)))
    });
    entries
}

/// Return whether a method belongs in public API metadata.
fn is_exported_method_name(name: &str) -> bool {
    !name.starts_with('_') || name.starts_with("__")
}

/// Convert one semantic method entry into the checked export shape.
fn checked_method_from_info(name: &str, info: &MethodInfo) -> CheckedMethod {
    CheckedMethod {
        name: name.to_string(),
        alias_of: info.alias_of.clone(),
        type_params: info
            .type_params
            .iter()
            .map(|type_param| CheckedTypeParam {
                name: type_param.clone(),
                bounds: info
                    .type_param_bound_details
                    .get(type_param)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|bound| CheckedTypeBound {
                        name: bound.name,
                        source_name: bound.source_name,
                        type_args: bound.type_args,
                        module_path: bound.module_path,
                    })
                    .collect(),
            })
            .collect(),
        receiver: info.receiver,
        params: info.params.clone(),
        param_defaults: vec![None; info.params.len()],
        return_type: info.return_type.clone(),
        is_async: info.is_async,
        has_body: info.has_body,
    }
}

/// Attach source-level method defaults to checked semantic method entries.
fn attach_method_defaults(
    entries: &mut [CheckedMethod],
    ast_methods: &[Spanned<crate::frontend::ast::MethodDecl>],
    checker: &TypeChecker,
) {
    let mut used = vec![false; ast_methods.len()];
    let default_context = DefaultPathContext::for_checker(checker);
    for entry in entries {
        let Some(idx) = matching_ast_method_index(entry, ast_methods, &used, checker) else {
            continue;
        };
        used[idx] = true;
        entry.param_defaults = ast_methods[idx]
            .node
            .params
            .iter()
            .map(|param| {
                param
                    .node
                    .default
                    .as_ref()
                    .map(|default| checked_param_default(default, default_context))
            })
            .collect();
    }
}

/// Find the source method that corresponds to one checked method entry.
fn matching_ast_method_index(
    checked: &CheckedMethod,
    ast_methods: &[Spanned<crate::frontend::ast::MethodDecl>],
    used: &[bool],
    checker: &TypeChecker,
) -> Option<usize> {
    ast_methods
        .iter()
        .enumerate()
        .find(|(idx, method)| {
            !used.get(*idx).copied().unwrap_or(false) && ast_method_shape_matches_checked(method, checked, checker)
        })
        .map(|(idx, _)| idx)
        .or_else(|| {
            ast_methods
                .iter()
                .enumerate()
                .find(|(idx, method)| {
                    !used.get(*idx).copied().unwrap_or(false) && ast_method_param_names_match_checked(method, checked)
                })
                .map(|(idx, _)| idx)
        })
}

/// Return whether an AST method and checked method have the same parameter names.
fn ast_method_param_names_match_checked(
    ast_method: &Spanned<crate::frontend::ast::MethodDecl>,
    checked: &CheckedMethod,
) -> bool {
    ast_method.node.name == checked.name
        && ast_method.node.params.len() == checked.params.len()
        && ast_method
            .node
            .params
            .iter()
            .zip(checked.params.iter())
            .all(|(ast_param, checked_param)| checked_param.name() == Some(ast_param.node.name.as_str()))
}

/// Return whether an AST method and checked method have the same callable shape.
fn ast_method_shape_matches_checked(
    ast_method: &Spanned<crate::frontend::ast::MethodDecl>,
    checked: &CheckedMethod,
    checker: &TypeChecker,
) -> bool {
    let ast_type_params = ast_method
        .node
        .type_params
        .iter()
        .map(|type_param| type_param.name.as_str())
        .collect::<Vec<_>>();
    ast_method.node.name == checked.name
        && ast_method.node.params.len() == checked.params.len()
        && ast_method.node.receiver == checked.receiver
        && ast_method.node.is_async() == checked.is_async
        && ast_type_params
            == checked
                .type_params
                .iter()
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>()
        && ast_method
            .node
            .params
            .iter()
            .zip(checked.params.iter())
            .all(|(ast_param, checked_param)| {
                checked_param.name() == Some(ast_param.node.name.as_str())
                    && checked_param.kind == ast_param.node.kind
                    && checked_param.has_default == ast_param.node.default.is_some()
                    && checked_param.ty == resolve_type(&ast_param.node.ty.node, &checker.symbols)
            })
}

/// Render a deterministic method-signature sort key for overload groups.
fn method_signature_sort_key(method: &CheckedMethod) -> String {
    let params = method
        .params
        .iter()
        .map(|param| {
            let name = param.name().unwrap_or("_");
            format!("{name}:{}", param.ty)
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("({params})->{}", method.return_type)
}

fn sorted_vec(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::ast::{Span, Spanned};

    fn spanned(expr: Expr) -> Spanned<Expr> {
        Spanned::new(expr, Span::default())
    }

    #[test]
    fn checked_preset_value_rejects_non_symbolic_field_paths() {
        let value = Expr::Field(
            Box::new(spanned(Expr::Call(
                Box::new(spanned(Expr::Ident("defaults".to_string()))),
                Vec::new(),
                Vec::new(),
            ))),
            "method".to_string(),
        );

        let checker = TypeChecker::new();
        assert_eq!(
            checked_preset_value(&value, DefaultPathContext::for_checker(&checker)),
            CheckedPresetValue::Unsupported
        );
    }
}
