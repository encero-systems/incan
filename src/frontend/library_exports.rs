//! Checked library-export extraction helpers.
//!
//! These types represent the public API surface extracted from typechecked symbols, so later stages (like `.incnlib`
//! manifest writing) can consume a stable, semantic model.

use std::collections::HashMap;

use crate::frontend::ast::{
    AliasDecl, ClassDecl, Declaration, EnumDecl, FunctionDecl, ModelDecl, NewtypeDecl, Program, TraitBound, TraitDecl,
    TypeAliasDecl, TypeParam, Visibility,
};
use crate::frontend::symbols::{
    CallableParam, ClassInfo, FieldInfo, FunctionInfo, MethodInfo, ModelInfo, NewtypeInfo, ResolvedType, SymbolKind,
    TraitInfo, TypeBoundInfo, TypeInfo, ValueEnumBacking, ValueEnumValue, VariableInfo, resolve_type,
};
use crate::frontend::typechecker::TypeChecker;

#[derive(Debug, Clone)]
pub struct CheckedTypeParam {
    pub name: String,
    pub bounds: Vec<CheckedTypeBound>,
}

#[derive(Debug, Clone)]
pub struct CheckedTypeBound {
    pub name: String,
    pub type_args: Vec<ResolvedType>,
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
    pub return_type: ResolvedType,
    pub is_async: bool,
    pub has_body: bool,
}

#[derive(Debug, Clone)]
pub struct CheckedFunctionExport {
    pub name: String,
    pub type_params: Vec<CheckedTypeParam>,
    pub params: Vec<CallableParam>,
    pub return_type: ResolvedType,
    pub is_async: bool,
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
    pub type_params: Vec<CheckedTypeParam>,
    /// Direct supertraits `(trait_name, type_arguments)` after typechecking (RFC 042).
    pub supertraits: Vec<(String, Vec<ResolvedType>)>,
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
pub struct CheckedEnumExport {
    pub name: String,
    pub type_params: Vec<CheckedTypeParam>,
    pub traits: Vec<String>,
    pub trait_adoptions: Vec<CheckedTypeBound>,
    pub value_type: Option<ValueEnumBacking>,
    pub variants: Vec<CheckedEnumVariant>,
    pub methods: Vec<CheckedMethod>,
    pub derives: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CheckedNewtypeExport {
    pub name: String,
    pub type_params: Vec<CheckedTypeParam>,
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
    pub kind: CheckedExportKind,
}

/// Collect checked public exports from one program while preserving alias identity.
pub fn collect_checked_public_exports(program: &Program, checker: &TypeChecker) -> Vec<CheckedNamedExport> {
    let mut exports = Vec::new();

    for decl in &program.declarations {
        match &decl.node {
            Declaration::Function(function) if matches!(function.visibility, Visibility::Public) => {
                if let Some(export) = checked_function_export(function, checker) {
                    exports.push(CheckedNamedExport {
                        name: export.name.clone(),
                        kind: CheckedExportKind::Function(export),
                    });
                }
            }
            Declaration::TypeAlias(alias) if matches!(alias.visibility, Visibility::Public) => {
                exports.push(CheckedNamedExport {
                    name: alias.name.clone(),
                    kind: CheckedExportKind::TypeAlias(checked_type_alias_export(alias, checker)),
                });
            }
            Declaration::Model(model) if matches!(model.visibility, Visibility::Public) => {
                if let Some(export) = checked_model_export(model, checker) {
                    exports.push(CheckedNamedExport {
                        name: export.name.clone(),
                        kind: CheckedExportKind::Model(export),
                    });
                }
            }
            Declaration::Class(class) if matches!(class.visibility, Visibility::Public) => {
                if let Some(export) = checked_class_export(class, checker) {
                    exports.push(CheckedNamedExport {
                        name: export.name.clone(),
                        kind: CheckedExportKind::Class(export),
                    });
                }
            }
            Declaration::Trait(trait_decl) if matches!(trait_decl.visibility, Visibility::Public) => {
                if let Some(export) = checked_trait_export(trait_decl, checker) {
                    exports.push(CheckedNamedExport {
                        name: export.name.clone(),
                        kind: CheckedExportKind::Trait(export),
                    });
                }
            }
            Declaration::Enum(enum_decl) if matches!(enum_decl.visibility, Visibility::Public) => {
                if let Some(export) = checked_enum_export(enum_decl, checker) {
                    exports.push(CheckedNamedExport {
                        name: export.name.clone(),
                        kind: CheckedExportKind::Enum(export),
                    });
                }
            }
            Declaration::Newtype(newtype_decl) if matches!(newtype_decl.visibility, Visibility::Public) => {
                if let Some(export) = checked_newtype_export(newtype_decl, checker) {
                    exports.push(CheckedNamedExport {
                        name: export.name.clone(),
                        kind: CheckedExportKind::Newtype(export),
                    });
                }
            }
            Declaration::Const(konst) if matches!(konst.visibility, Visibility::Public) => {
                if let Some(export) = checked_const_export(konst.name.as_str(), checker) {
                    exports.push(CheckedNamedExport {
                        name: export.name.clone(),
                        kind: CheckedExportKind::Const(export),
                    });
                }
            }
            Declaration::Static(static_decl) if matches!(static_decl.visibility, Visibility::Public) => {
                if let Some(export) = checked_static_export(static_decl.name.as_str(), checker) {
                    exports.push(CheckedNamedExport {
                        name: export.name.clone(),
                        kind: CheckedExportKind::Static(export),
                    });
                }
            }
            Declaration::Alias(alias) if matches!(alias.visibility, Visibility::Public) => {
                if let Some(export) = checked_alias_export(alias, checker) {
                    exports.push(export);
                }
            }
            _ => {}
        }
    }

    exports.sort_by(|a, b| a.name.cmp(&b.name));
    exports
}

/// Build a checked public export entry for a module-level alias.
fn checked_alias_export(alias: &AliasDecl, checker: &TypeChecker) -> Option<CheckedNamedExport> {
    checker.lookup_symbol(alias.name.as_str())?;
    Some(CheckedNamedExport {
        name: alias.name.clone(),
        kind: CheckedExportKind::Alias(CheckedAliasExport {
            name: alias.name.clone(),
            target_path: alias.target.segments.clone(),
        }),
    })
}

/// Build checked export metadata for a function or callable-valued decorated function binding.
fn checked_function_export(function: &FunctionDecl, checker: &TypeChecker) -> Option<CheckedFunctionExport> {
    let symbol = checker.lookup_symbol(function.name.as_str())?;
    let (params, return_type, is_async) = match &symbol.kind {
        SymbolKind::Function(FunctionInfo {
            params,
            return_type,
            is_async,
            ..
        }) => (params.clone(), return_type.clone(), *is_async),
        SymbolKind::Variable(VariableInfo {
            ty: ResolvedType::Function(params, return_type),
            ..
        }) => {
            // Callable values do not yet carry asyncness, so preserve the source declaration marker until decorator
            // typing records async callable metadata explicitly.
            (params.clone(), return_type.as_ref().clone(), function.is_async())
        }
        _ => return None,
    };

    Some(CheckedFunctionExport {
        name: function.name.clone(),
        type_params: checked_type_params(&function.type_params, checker),
        params,
        return_type,
        is_async,
    })
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
        fields: map_fields(fields),
        methods: map_method_overloads(method_overloads),
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
        fields: map_fields(fields),
        methods: map_method_overloads(method_overloads),
    })
}

fn checked_trait_export(trait_decl: &TraitDecl, checker: &TypeChecker) -> Option<CheckedTraitExport> {
    let symbol = checker.lookup_symbol(trait_decl.name.as_str())?;
    let SymbolKind::Trait(TraitInfo {
        requires,
        methods,
        supertraits,
        ..
    }) = &symbol.kind
    else {
        return None;
    };

    let mut sorted_requires = requires.clone();
    sorted_requires.sort_by(|(left, _), (right, _)| left.cmp(right));

    let mut sorted_supertraits = supertraits.clone();
    sorted_supertraits.sort_by(|(left, _), (right, _)| left.cmp(right));

    Some(CheckedTraitExport {
        name: trait_decl.name.clone(),
        type_params: checked_type_params(&trait_decl.type_params, checker),
        supertraits: sorted_supertraits,
        requires: sorted_requires,
        methods: map_methods(methods),
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
        methods: map_method_overloads(&enum_info.method_overloads),
        derives: sorted_vec(enum_info.derives.clone()),
    })
}

/// Build a manifest-ready newtype export from the checked symbol metadata.
fn checked_newtype_export(newtype_decl: &NewtypeDecl, checker: &TypeChecker) -> Option<CheckedNewtypeExport> {
    let symbol = checker.lookup_symbol(newtype_decl.name.as_str())?;
    let SymbolKind::Type(TypeInfo::Newtype(NewtypeInfo {
        is_rusttype,
        underlying,
        methods,
        ..
    })) = &symbol.kind
    else {
        return None;
    };

    Some(CheckedNewtypeExport {
        name: newtype_decl.name.clone(),
        type_params: checked_type_params(&newtype_decl.type_params, checker),
        is_rusttype: *is_rusttype,
        underlying: underlying.clone(),
        methods: map_methods(methods),
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

fn checked_trait_bounds(bounds: &[TraitBound], checker: &TypeChecker) -> Vec<CheckedTypeBound> {
    bounds
        .iter()
        .map(|bound| CheckedTypeBound {
            name: bound.name.clone(),
            type_args: bound
                .type_args
                .iter()
                .map(|type_arg| resolve_type(&type_arg.node, &checker.symbols))
                .collect(),
        })
        .collect()
}

/// Convert collected trait adoption metadata into the manifest-ready checked bound shape.
fn map_type_bound_infos(bounds: &[TypeBoundInfo]) -> Vec<CheckedTypeBound> {
    bounds
        .iter()
        .map(|bound| CheckedTypeBound {
            name: bound.name.clone(),
            type_args: bound.type_args.clone(),
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

fn map_fields(fields: &HashMap<String, FieldInfo>) -> Vec<CheckedField> {
    let mut entries: Vec<_> = fields
        .iter()
        .map(|(name, info)| CheckedField {
            name: name.clone(),
            ty: info.ty.clone(),
            has_default: info.has_default,
            alias: info.alias.clone(),
            description: info.description.clone(),
        })
        .collect();
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    entries
}

/// Convert legacy one-method-per-name metadata into checked export methods.
fn map_methods(methods: &HashMap<String, MethodInfo>) -> Vec<CheckedMethod> {
    let mut entries: Vec<_> = methods
        .iter()
        .map(|(name, info)| checked_method_from_info(name, info))
        .collect();
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    entries
}

/// Flatten same-name overload groups into manifest-ready checked methods without losing RFC 025 trait implementations.
fn map_method_overloads(method_overloads: &HashMap<String, Vec<MethodInfo>>) -> Vec<CheckedMethod> {
    let mut entries: Vec<_> = method_overloads
        .iter()
        .flat_map(|(name, overloads)| overloads.iter().map(|info| checked_method_from_info(name, info)))
        .collect();
    entries.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| method_signature_sort_key(left).cmp(&method_signature_sort_key(right)))
    });
    entries
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
                        type_args: bound.type_args,
                    })
                    .collect(),
            })
            .collect(),
        receiver: info.receiver,
        params: info.params.clone(),
        return_type: info.return_type.clone(),
        is_async: info.is_async,
        has_body: info.has_body,
    }
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
