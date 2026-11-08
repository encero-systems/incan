//! Collection of checked public exports, ownership contracts, export identities, and import and alias exports.

use super::*;

/// Collect private nominal declarations whose compiler-owned decorator preserves migration guidance for consumers.
pub fn collect_checked_made_private_types(
    program: &Program,
    module_path: &[String],
) -> Vec<crate::library_manifest::MadePrivateTypeExport> {
    let mut exports = program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let (visibility, decorators, name) = match &declaration.node {
                Declaration::Model(value) => (value.visibility, &value.decorators, &value.name),
                Declaration::Class(value) => (value.visibility, &value.decorators, &value.name),
                Declaration::Trait(value) => (value.visibility, &value.decorators, &value.name),
                Declaration::Enum(value) => (value.visibility, &value.decorators, &value.name),
                Declaration::Newtype(value) => (value.visibility, &value.decorators, &value.name),
                _ => return None,
            };
            if visibility != Visibility::Private {
                return None;
            }
            let facts = crate::symbols::made_private_from_decorators(decorators)?;
            Some(crate::library_manifest::MadePrivateTypeExport {
                module_path: module_path.to_vec(),
                name: name.clone(),
                facts: (&facts).into(),
            })
        })
        .collect::<Vec<_>>();
    exports.sort_by(|left, right| {
        left.module_path
            .cmp(&right.module_path)
            .then_with(|| left.name.cmp(&right.name))
    });
    exports
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
                        identity: checked_direct_identity_at_span(checker, &export.name, decl.span),
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
                        )
                        .with_canonical(checked_binding_canonical(checker, &export.name)),
                        kind: CheckedExportKind::Partial(export),
                    });
                }
            }
            _ => {}
        }
    }

    let type_origins = checker.checked_nominal_type_origins();
    for export in &mut exports {
        export.identity.type_origins.clone_from(&type_origins);
        attach_ownership_contracts(export, checker);
    }
    exports.sort_by(|a, b| a.name.cmp(&b.name));
    exports
}

/// Attach the declaration-owned contract after resolving aliases and members, never by guessing a displayed name.
pub(super) fn attach_ownership_contracts(export: &mut CheckedNamedExport, checker: &TypeChecker) {
    let contracts = &checker.type_info.declarations.ownership_contracts;
    let direct = export
        .identity
        .canonical
        .as_ref()
        .and_then(|identity| contracts.get(identity))
        .cloned();
    let methods = match &mut export.kind {
        CheckedExportKind::Function(function) => {
            function.ownership = direct;
            return;
        }
        CheckedExportKind::Alias(alias) => {
            if let Some(function) = &mut alias.projected_function {
                function.ownership = direct;
            }
            return;
        }
        CheckedExportKind::Model(owner) => &mut owner.methods,
        CheckedExportKind::Class(owner) => &mut owner.methods,
        CheckedExportKind::Trait(owner) => &mut owner.methods,
        CheckedExportKind::Enum(owner) => &mut owner.methods,
        CheckedExportKind::Newtype(owner) => &mut owner.methods,
        _ => return,
    };
    for method in methods {
        method.ownership = method
            .canonical
            .as_ref()
            .and_then(|identity| contracts.get(identity))
            .cloned();
    }
}

/// Return the source identity path for one public binding in the currently exported module.
pub(super) fn checked_export_source_path(checker: &TypeChecker, name: &str) -> Vec<String> {
    let mut path = checker.current_module_path.clone().unwrap_or_default();
    path.push(name.to_string());
    path
}

/// Return direct identity metadata for an export that is not projecting another declaration.
pub(super) fn checked_direct_identity(checker: &TypeChecker, name: &str) -> CheckedExportIdentity {
    CheckedExportIdentity::direct(checked_export_source_path(checker, name))
        .with_canonical(checked_binding_canonical(checker, name))
}

/// Return direct identity metadata for a function declaration whose spelling may name an overload set.
pub(super) fn checked_direct_identity_at_span(checker: &TypeChecker, name: &str, span: Span) -> CheckedExportIdentity {
    let canonical = checker
        .type_info()
        .declarations
        .function_bindings_by_span
        .get(&span)
        .and_then(|binding| binding.identity.clone())
        .or_else(|| checked_binding_canonical(checker, name));
    CheckedExportIdentity::direct(checked_export_source_path(checker, name)).with_canonical(canonical)
}

/// Return the symbol table's authoritative identity for one active public binding.
pub(super) fn checked_binding_canonical(checker: &TypeChecker, name: &str) -> Option<CanonicalSymbolId> {
    let symbol_id = checker.symbols.lookup(name)?;
    checker.symbols.identity_of(symbol_id).cloned()
}

/// Build checked public export entries for a module-level alias.
pub(super) fn checked_alias_exports(alias: &AliasDecl, checker: &TypeChecker) -> Vec<CheckedNamedExport> {
    let Some(symbol) = checker.lookup_symbol(alias.name.as_str()) else {
        return Vec::new();
    };
    let target_path = DefaultPathContext::for_checker(checker).canonical_value_path(alias.target.segments.clone());
    let identity = CheckedExportIdentity::alias(checked_export_source_path(checker, &alias.name), target_path.clone())
        .with_canonical(checked_binding_canonical(checker, &alias.name));
    if let SymbolKind::FunctionOverloads(overloads) = &symbol.kind {
        return checked_overload_function_exports(alias.name.clone(), overloads, identity);
    }
    let projected_function = checked_projected_function_export(&alias.name, &symbol.kind);
    let projected_type = match &symbol.kind {
        SymbolKind::Type(_) => Some(ResolvedType::Named(alias.name.clone())),
        SymbolKind::Trait(_) => identity.canonical.clone().map(ResolvedType::Trait),
        _ => None,
    };
    vec![CheckedNamedExport {
        name: alias.name.clone(),
        identity,
        kind: CheckedExportKind::Alias(CheckedAliasExport {
            name: alias.name.clone(),
            target_path,
            projected_type,
            projected_function,
        }),
    }]
}

/// Return checked exports introduced by an import declaration.
pub(super) fn checked_import_exports(import: &ImportDecl, checker: &TypeChecker) -> Vec<CheckedNamedExport> {
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
        ImportKind::PubFrom { library, path, items } => {
            let mut base_path = vec!["pub".to_string(), library.clone()];
            base_path.extend(path.iter().cloned());
            checked_import_item_exports(items, base_path, checker)
        }
        _ => Vec::new(),
    }
}

/// Return checked exports introduced by source-module import items.
pub(super) fn checked_source_import_item_exports(
    items: &[ImportItem],
    module: &crate::ast::ImportPath,
    base_path: Vec<String>,
    checker: &TypeChecker,
) -> Vec<CheckedNamedExport> {
    items
        .iter()
        .flat_map(|item| {
            let exported_name = item.alias.as_ref().unwrap_or(&item.name).clone();
            let mut target_path = base_path.clone();
            target_path.push(item.name.clone());
            let identity = CheckedExportIdentity::reexport(target_path.clone(), target_path.clone())
                .with_canonical(checked_binding_canonical(checker, &exported_name));
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
            checked_import_export_from_symbol_kind(exported_name, target_path, symbol_kind.as_ref(), identity, checker)
        })
        .collect()
}

/// Return checked exports introduced by one imported item.
pub(super) fn checked_import_item_exports(
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
            let identity = CheckedExportIdentity::reexport(target_path.clone(), target_path.clone())
                .with_canonical(checked_binding_canonical(checker, &exported_name));
            checked_import_export_from_symbol_kind(exported_name, target_path, symbol_kind, identity, checker)
        })
        .collect()
}

/// Convert one imported symbol kind into checked public export entries.
pub(super) fn checked_import_export_from_symbol_kind(
    exported_name: String,
    target_path: Vec<String>,
    symbol_kind: Option<&SymbolKind>,
    identity: CheckedExportIdentity,
    checker: &TypeChecker,
) -> Vec<CheckedNamedExport> {
    if let Some(SymbolKind::FunctionOverloads(overloads)) = symbol_kind {
        return checked_overload_function_exports(exported_name, overloads, identity);
    }
    let projected_function = symbol_kind.and_then(|kind| checked_projected_function_export(&exported_name, kind));
    let projected_type = match symbol_kind {
        Some(SymbolKind::Trait(_)) => identity.canonical.clone().map(ResolvedType::Trait),
        _ => checker
            .checked_nominal_type_origins()
            .contains_key(&exported_name)
            .then(|| ResolvedType::Named(exported_name.clone())),
    };
    vec![CheckedNamedExport {
        name: exported_name.clone(),
        identity,
        kind: CheckedExportKind::Alias(CheckedAliasExport {
            name: exported_name,
            target_path,
            projected_type,
            projected_function,
        }),
    }]
}

/// Convert a same-name overload group into separate function exports under one source name.
pub(super) fn checked_overload_function_exports(
    exported_name: String,
    overloads: &[crate::symbols::FunctionOverloadInfo],
    identity: CheckedExportIdentity,
) -> Vec<CheckedNamedExport> {
    overloads
        .iter()
        .map(|overload| {
            let export = checked_alias_function_export(&exported_name, &overload.info);
            CheckedNamedExport {
                name: export.name.clone(),
                identity: identity
                    .clone()
                    .with_canonical(overload.identity.clone().or_else(|| identity.canonical.clone())),
                kind: CheckedExportKind::Function(export),
            }
        })
        .collect()
}

/// Build manifest-ready callable metadata for an alias that projects a function.
pub(super) fn checked_alias_function_export(name: &str, info: &FunctionInfo) -> CheckedFunctionExport {
    CheckedFunctionExport {
        ownership: None,
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
pub(super) fn checked_projected_function_export(name: &str, kind: &SymbolKind) -> Option<CheckedFunctionExport> {
    match kind {
        SymbolKind::Function(info) => Some(checked_alias_function_export(name, info)),
        SymbolKind::Variable(VariableInfo {
            ty: ResolvedType::Function(params, return_type),
            ..
        }) => Some(CheckedFunctionExport {
            ownership: None,
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
