//! Collection of checked API metadata and alias metadata for one module.

use super::*;

/// Collect checked public API metadata for one parsed and typechecked module.
pub fn collect_checked_api_metadata(
    program: &Program,
    checker: &TypeChecker,
    module_path: Vec<String>,
) -> CheckedApiMetadata {
    let checked_exports = collect_checked_public_exports(program, checker);
    let checked_by_name: HashMap<String, CheckedNamedExport> = checked_exports
        .iter()
        .cloned()
        .map(|export| (export.name.clone(), export))
        .collect();

    let mut declarations = Vec::new();
    for decl in &program.declarations {
        if let Some(nominal) = checked_nominal_api_declaration(decl, &checked_by_name, checker, &module_path) {
            declarations.push(nominal);
            continue;
        }
        match &decl.node {
            Declaration::Function(function) if public(function.visibility) => {
                if let Some(export) = checked_source_function(&checked_exports, checker, decl.span) {
                    declarations.push(ApiDeclaration::Function(api_function(
                        function,
                        decl.span,
                        export,
                        checker,
                        &module_path,
                    )));
                }
            }
            Declaration::TypeAlias(alias) if public(alias.visibility) => {
                if let Some(CheckedExportKind::TypeAlias(export)) = checked_kind(&checked_by_name, &alias.name) {
                    declarations.push(ApiDeclaration::TypeAlias(api_type_alias(
                        alias,
                        decl.span,
                        export,
                        &module_path,
                    )));
                }
            }
            Declaration::Const(konst) if public(konst.visibility) => {
                if let Some(CheckedExportKind::Const(export)) = checked_kind(&checked_by_name, &konst.name) {
                    declarations.push(ApiDeclaration::Const(api_const(
                        &konst.name,
                        decl.span,
                        export,
                        checker,
                        &module_path,
                    )));
                }
            }
            Declaration::Static(static_decl) if public(static_decl.visibility) => {
                if let Some(CheckedExportKind::Static(export)) = checked_kind(&checked_by_name, &static_decl.name) {
                    declarations.push(ApiDeclaration::Static(ApiStatic {
                        name: export.name.clone(),
                        anchor: anchor(&module_path, &export.name, decl.span),
                        ty: type_ref_from_resolved(&export.ty),
                    }));
                }
            }
            Declaration::Partial(partial) if public(partial.visibility) => {
                if let Some(CheckedExportKind::Partial(export)) = checked_kind(&checked_by_name, &partial.name) {
                    declarations.push(ApiDeclaration::Partial(api_partial(export, decl.span, &module_path)));
                }
            }
            // Module-level `from ... import ...` bindings are part of a module's public surface, including facade
            // modules such as `std.web` whose `prelude.incn` consists entirely of re-exports. Rust imports remain an
            // implementation detail unless explicitly declared `pub`.
            Declaration::Alias(_) | Declaration::Import(_) => declarations.extend(
                checked_api_aliases_for_declaration(&decl.node, decl.span, &module_path)
                    .into_iter()
                    .map(ApiDeclaration::Alias),
            ),
            _ => {}
        }
    }

    for declaration in &mut declarations {
        if let ApiDeclaration::Alias(alias) = declaration
            && let Some(export) = checked_by_name.get(&alias.name)
            && let CheckedExportKind::Alias(checked) = &export.kind
        {
            alias.projected_type = checked.projected_type.as_ref().map(type_ref_from_resolved);
            alias.projected_function = checked
                .projected_function
                .as_ref()
                .map(|function| {
                    let projected = crate::library_manifest::function_export_from_checked(
                        module_path.first().map_or("", String::as_str),
                        function,
                    );
                    ApiProjectedFunction {
                        source_path: alias.target_path.clone(),
                        callable: ApiCallableMetadata {
                            ownership: projected.ownership,
                            name: alias.name.clone(),
                            anchor: alias.anchor.clone(),
                            type_params: projected.type_params,
                            receiver: None,
                            params: projected.params,
                            return_type: projected.return_type,
                            is_async: projected.is_async,
                        },
                        decorators: Vec::new(),
                    }
                })
                .or_else(|| checked_provider_alias_projection(alias, export, checker));
        }
    }

    crate::library_manifest::with_checked_type_origins(
        CheckedApiMetadata {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            derivable_traits: TypeChecker::derivable_traits_from_program(program),
            module_path,
            declarations,
        },
        &checker.checked_nominal_type_origins(),
    )
}

/// Select a source overload by its checked declaration identity, including any expansion ancestry. Name lookup
/// selects an overload set and cannot supply one declaration's defaults or ownership contract. A public alias shares
/// its target's canonical identity but must never rename the authored declaration in checked API metadata.
pub(super) fn checked_source_function<'a>(
    exports: &'a [CheckedNamedExport],
    checker: &TypeChecker,
    span: Span,
) -> Option<&'a CheckedFunctionExport> {
    let identity = checker
        .type_info()
        .declarations
        .function_bindings_by_span
        .get(&span)?
        .identity
        .as_ref()?;
    exports.iter().find_map(|export| {
        if export.identity.projection != CheckedExportProjection::Direct
            || export.identity.canonical.as_ref() != Some(identity)
        {
            return None;
        }
        match &export.kind {
            CheckedExportKind::Function(function) => Some(function),
            _ => None,
        }
    })
}

/// Project a public nominal declaration only when the checked export inventory retains its matching kind. Source
/// visibility and declaration order remain with the module collector; this boundary does not infer missing exports.
pub(super) fn checked_nominal_api_declaration(
    decl: &Spanned<Declaration>,
    checked: &HashMap<String, CheckedNamedExport>,
    checker: &TypeChecker,
    module_path: &[String],
) -> Option<ApiDeclaration> {
    match &decl.node {
        Declaration::Model(model) if public(model.visibility) => {
            let Some(CheckedExportKind::Model(export)) = checked_kind(checked, &model.name) else {
                return None;
            };
            Some(ApiDeclaration::Model(api_model(
                model,
                decl.span,
                export,
                checker,
                module_path,
            )))
        }
        Declaration::Class(class) if public(class.visibility) => {
            let Some(CheckedExportKind::Class(export)) = checked_kind(checked, &class.name) else {
                return None;
            };
            Some(ApiDeclaration::Class(api_class(
                class,
                decl.span,
                export,
                checker,
                module_path,
            )))
        }
        Declaration::Trait(trait_decl) if public(trait_decl.visibility) => {
            let Some(CheckedExportKind::Trait(export)) = checked_kind(checked, &trait_decl.name) else {
                return None;
            };
            Some(ApiDeclaration::Trait(api_trait(
                trait_decl,
                decl.span,
                export,
                checker,
                module_path,
            )))
        }
        Declaration::Enum(enum_decl) if public(enum_decl.visibility) => {
            let Some(CheckedExportKind::Enum(export)) = checked_kind(checked, &enum_decl.name) else {
                return None;
            };
            Some(ApiDeclaration::Enum(api_enum(
                enum_decl,
                decl.span,
                export,
                checker,
                module_path,
            )))
        }
        Declaration::Newtype(newtype) if public(newtype.visibility) => {
            let Some(CheckedExportKind::Newtype(export)) = checked_kind(checked, &newtype.name) else {
                return None;
            };
            Some(ApiDeclaration::Newtype(api_newtype(
                newtype,
                decl.span,
                export,
                checker,
                module_path,
            )))
        }
        _ => None,
    }
}

/// Retain an imported callable's admitted API facts on its public alias before local projection materialization.
///
/// A provider's source declarations are deliberately absent from the consumer's API module list. Follow the checked
/// canonical identity to the declaring Loaf instead of attempting to resolve `std` or `pub` as a local source module.
/// The public hop remains the alias's target path; callable defaults and decorators come from the actual declaration.
pub(super) fn checked_provider_alias_projection(
    alias: &ApiAlias,
    export: &CheckedNamedExport,
    checker: &TypeChecker,
) -> Option<ApiProjectedFunction> {
    if !alias
        .target_path
        .first()
        .is_some_and(|root| matches!(root.as_str(), "pub" | "std"))
    {
        return None;
    }
    let canonical = export.identity.canonical.as_ref()?;
    let provider = checker.loaves.declaring_loaf(canonical)?;
    let api = provider.artifact.manifest.contract_metadata.api.as_ref()?;
    let SymbolOrigin::Package { module_path, .. } = &canonical.origin else {
        return None;
    };
    let module = api.modules.iter().find(|module| module.module_path == *module_path)?;
    let function = module.declarations.iter().find_map(|declaration| match declaration {
        ApiDeclaration::Function(function) if function.name == canonical.declaration_name => Some(function),
        _ => None,
    })?;
    let mut callable = callable_from_function(function);
    callable.name = alias.name.clone();
    callable.anchor = alias.anchor.clone();
    Some(ApiProjectedFunction {
        source_path: alias.target_path.clone(),
        callable,
        decorators: function.decorators.clone(),
    })
}

/// Collect only the checked public alias projection for one parsed module.
///
/// Callers use this after the owning compilation session has successfully checked the module. Keeping this projection
/// separate lets registry inspection attach facade paths without constructing a second typechecker merely to recover
/// import spellings that are already present in the checked source program.
pub fn collect_checked_api_alias_metadata(program: &Program, module_path: Vec<String>) -> CheckedApiMetadata {
    let declarations = program
        .declarations
        .iter()
        .flat_map(|declaration| {
            checked_api_aliases_for_declaration(&declaration.node, declaration.span, &module_path)
                .into_iter()
                .map(ApiDeclaration::Alias)
        })
        .collect();
    CheckedApiMetadata {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        derivable_traits: TypeChecker::derivable_traits_from_program(program),
        module_path,
        declarations,
    }
}
