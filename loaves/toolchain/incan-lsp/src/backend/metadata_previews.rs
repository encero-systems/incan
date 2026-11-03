//! Checked API metadata, registry and C binding previews at a source offset.

use super::*;

/// Build hover-ready checked API snippets for public metadata declarations in an analyzed document.
pub(super) fn api_metadata_previews(ast: &Program, metadata: &CheckedApiMetadata) -> Vec<ApiMetadataPreview> {
    let mut previews = Vec::new();
    for declaration in &metadata.declarations {
        match declaration {
            ApiDeclaration::Function(function) => {
                previews.push(preview_for_anchor(&function.anchor, api_function_markdown(function)))
            }
            ApiDeclaration::Model(model) => {
                previews.push(preview_for_anchor(&model.anchor, api_model_markdown(model)));
                if let Some(Declaration::Model(ast_model)) = find_top_level_decl(ast, &model.name) {
                    push_field_previews(&mut previews, &model.name, &model.fields, &ast_model.fields);
                }
                push_method_previews(&mut previews, &model.name, &model.methods);
            }
            ApiDeclaration::Class(class) => {
                previews.push(preview_for_anchor(&class.anchor, api_class_markdown(class)));
                if let Some(Declaration::Class(ast_class)) = find_top_level_decl(ast, &class.name) {
                    push_field_previews(&mut previews, &class.name, &class.fields, &ast_class.fields);
                }
                push_method_previews(&mut previews, &class.name, &class.methods);
            }
            ApiDeclaration::Trait(trait_decl) => {
                previews.push(preview_for_anchor(&trait_decl.anchor, api_trait_markdown(trait_decl)));
                push_method_previews(&mut previews, &trait_decl.name, &trait_decl.methods);
            }
            ApiDeclaration::Enum(enum_decl) => {
                previews.push(preview_for_anchor(&enum_decl.anchor, api_enum_markdown(enum_decl)));
                if let Some(Declaration::Enum(ast_enum)) = find_top_level_decl(ast, &enum_decl.name) {
                    push_enum_variant_previews(
                        &mut previews,
                        &enum_decl.name,
                        enum_decl.value_type,
                        &enum_decl.variants,
                        &ast_enum.variants,
                    );
                }
            }
            ApiDeclaration::Newtype(newtype) => {
                previews.push(preview_for_anchor(&newtype.anchor, api_newtype_markdown(newtype)));
                push_method_previews(&mut previews, &newtype.name, &newtype.methods);
            }
            ApiDeclaration::TypeAlias(alias) => {
                previews.push(preview_for_anchor(&alias.anchor, api_type_alias_markdown(alias)))
            }
            ApiDeclaration::Const(konst) => previews.push(preview_for_anchor(&konst.anchor, api_const_markdown(konst))),
            ApiDeclaration::Static(static_decl) => previews.push(preview_for_anchor(
                &static_decl.anchor,
                api_static_markdown(static_decl),
            )),
            ApiDeclaration::Alias(alias) => previews.push(ApiMetadataPreview {
                span: alias.anchor.span,
                markdown: checked_api_markdown(
                    format!("pub alias {} = {}", alias.name, alias.target_path.join("::")),
                    "public import alias",
                    Vec::new(),
                ),
            }),
            ApiDeclaration::Partial(partial) => {
                previews.push(preview_for_anchor(&partial.anchor, api_partial_markdown(partial)))
            }
        }
    }
    previews
}

/// Return the most precise checked API preview containing `offset`.
pub(super) fn api_metadata_preview_at_offset(
    previews: &[ApiMetadataPreview],
    offset: usize,
) -> Option<&ApiMetadataPreview> {
    previews
        .iter()
        .filter(|preview| preview.span.start <= offset && offset < preview.span.end)
        .min_by_key(|preview| preview.span.end.saturating_sub(preview.span.start))
}

/// Build registry-membership hover data from the successful typecheck artifact. The LSP deliberately consumes this
/// shared checked graph projection rather than resolving `@describe` syntax a second time.
pub(super) fn registry_previews(artifacts: &typechecker::RegistryArtifacts) -> Vec<RegistryPreview> {
    artifacts
        .descriptions
        .iter()
        .map(|description| RegistryPreview {
            registration_span: description.decorator_span,
            subject_span: description.declaration_span,
            markdown: format!(
                "```incan\n@describe({}, {}, {})\n```\n\n*checked registry membership*\n\n- Registry: `{}`\n- Subject: `{}`\n- Key: `{}`\n- Descriptor: `{}`",
                description.registry_name,
                description.key,
                description.descriptor,
                description.registry_name,
                description.subject_kind,
                description.key,
                description.descriptor,
            ),
        })
        .collect()
}

/// Return the most specific registry membership attached at an editor offset.
pub(super) fn registry_preview_at_offset(previews: &[RegistryPreview], offset: usize) -> Option<&RegistryPreview> {
    previews
        .iter()
        .filter(|preview| preview.registration_span.start <= offset && offset < preview.registration_span.end)
        .min_by_key(|preview| {
            preview
                .registration_span
                .end
                .saturating_sub(preview.registration_span.start)
        })
}

/// Build editor previews from the checked C descriptor and direct-call artifacts produced by the typechecker.
///
/// This keeps the editor on the declaration-level contract: it does not inspect generated Rust, linker commands, or
/// a future Oven receipt. A raw-call preview points back to its exact checked declaration so navigation does not have
/// to infer the binding from source spelling.
pub(super) fn c_binding_previews(artifacts: &CAbiInteropArtifacts, module_path: &[String]) -> Vec<CBindingPreview> {
    let mut descriptors = artifacts.bindings.values().collect::<Vec<_>>();
    descriptors.sort_by(|left, right| {
        left.span
            .start
            .cmp(&right.span.start)
            .then_with(|| left.class_name.cmp(&right.class_name))
    });

    let declaration_spans = descriptors
        .iter()
        .map(|descriptor| (descriptor.class_name.as_str(), descriptor.span))
        .collect::<HashMap<_, _>>();
    let mut previews = descriptors
        .into_iter()
        .map(|descriptor| CBindingPreview {
            span: descriptor.span,
            declaration_span: descriptor.span,
            markdown: c_binding_declaration_markdown(descriptor, module_path),
        })
        .collect::<Vec<_>>();

    let mut raw_calls = artifacts.raw_calls.iter().collect::<Vec<_>>();
    raw_calls.sort_by(|left, right| {
        left.span
            .start
            .cmp(&right.span.start)
            .then_with(|| left.span.end.cmp(&right.span.end))
            .then_with(|| left.binding.cmp(&right.binding))
            .then_with(|| left.symbol.cmp(&right.symbol))
    });
    for raw_call in raw_calls {
        let Some(declaration_span) = declaration_spans.get(raw_call.binding.as_str()) else {
            continue;
        };
        previews.push(CBindingPreview {
            span: raw_call.span,
            declaration_span: *declaration_span,
            markdown: format!(
                "```incan\n{}.{}(...)\n```\n\n*checked raw C call*\n\n- Declaration: `{}.{}`\n- Owning callable: {}\n- Safety: acknowledged by the enclosing `unsafe:` block\n- Navigation: go to definition opens the checked binding declaration",
                raw_call.binding,
                raw_call.symbol,
                raw_call.binding,
                raw_call.symbol,
                raw_call
                    .owner
                    .as_ref()
                    .map(|owner| format!("`{}` ({})", owner.name, if owner.visibility == incan_frontend::ast::Visibility::Public { "public" } else { "private" }))
                    .unwrap_or_else(|| "not a named callable".to_string()),
            ),
        });
    }

    previews
}

/// Render one checked C declaration as source-level contract facts suitable for hover.
pub(super) fn c_binding_declaration_markdown(
    descriptor: &incan_frontend::typechecker::CBindingDescriptor,
    module_path: &[String],
) -> String {
    let resources = if descriptor.resources.is_empty() {
        "none".to_string()
    } else {
        descriptor
            .resources
            .iter()
            .map(|resource| format!("{} → {}", resource.name, resource.release))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let symbols = if descriptor.symbols.is_empty() {
        "none".to_string()
    } else {
        descriptor
            .symbols
            .iter()
            .map(c_binding_symbol_signature)
            .collect::<Vec<_>>()
            .join("; ")
    };
    let enums = if descriptor.enums.is_empty() {
        "none".to_string()
    } else {
        descriptor
            .enums
            .iter()
            .map(|enumeration| format!("{}: {}", enumeration.name, scalar_type_as_str(enumeration.carrier)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let structures = if descriptor.structs.is_empty() {
        "none".to_string()
    } else {
        descriptor
            .structs
            .iter()
            .map(|structure| structure.name.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };

    format!(
        "```incan\nbinding {}:\n```\n\n*checked C binding*\n\n- Identity: `{}`\n- Header: `{}`\n- Link: `c.{}(\"{}\")`\n- Resources: {}\n- Symbols: {}\n- Enums: {}\n- Structures: {}\n\nThis is the compiler-checked declaration contract. Its direct calls require `unsafe:`; an ordinary Incan façade should own public domain behavior.",
        descriptor.class_name,
        c_binding_descriptor_identity(module_path, descriptor),
        descriptor.header,
        link_capability_as_str(descriptor.link_capability),
        descriptor.system_library,
        resources,
        symbols,
        enums,
        structures,
    )
}

/// Render one checked symbol signature without exposing generated-Rust carrier types.
pub(super) fn c_binding_symbol_signature(symbol: &incan_frontend::typechecker::CBindingSymbol) -> String {
    let parameters = symbol
        .parameters
        .iter()
        .map(|parameter| format!("{}: {}", parameter.name, c_binding_type_spelling(&parameter.ty)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{}({parameters}) -> {}",
        symbol.name,
        c_binding_type_spelling(&symbol.return_type)
    )
}

/// Render the checked source vocabulary for a structural C type.
pub(super) fn c_binding_type_spelling(ty: &CBindingType) -> String {
    match ty {
        CBindingType::Scalar(scalar) => scalar_type_as_str(*scalar).to_string(),
        CBindingType::Pointer { mutable, pointee } => format!(
            "{}[{}]",
            if *mutable { "c.MutPtr" } else { "c.ConstPtr" },
            c_binding_type_spelling(pointee)
        ),
        CBindingType::Struct(name) => name.clone(),
        CBindingType::Resource { access, resource } => format!(
            "{}[{resource}]",
            match access {
                CResourceAccess::Owned => "c.Owned",
                CResourceAccess::Borrowed => "c.Borrowed",
                CResourceAccess::BorrowedMut => "c.BorrowedMut",
            }
        ),
        CBindingType::Output { mode, value } => format!(
            "{}[{}]",
            match mode {
                COutputMode::Out => "c.Out",
                COutputMode::InOut => "c.InOut",
            },
            c_binding_type_spelling(value)
        ),
        CBindingType::Nullable(value) => format!("Option[{}]", c_binding_type_spelling(value)),
        CBindingType::Void => constructors::as_str(constructors::ConstructorId::None).to_string(),
    }
}

/// Return the most precise checked C preview containing an editor offset.
pub(super) fn c_binding_preview_at_offset(previews: &[CBindingPreview], offset: usize) -> Option<&CBindingPreview> {
    previews
        .iter()
        .filter(|preview| preview.span.start <= offset && offset < preview.span.end)
        .min_by_key(|preview| preview.span.end.saturating_sub(preview.span.start))
}

/// Convert a metadata anchor to an LSP preview entry.
pub(super) fn preview_for_anchor(anchor: &SourceAnchor, markdown: String) -> ApiMetadataPreview {
    ApiMetadataPreview {
        span: anchor.span,
        markdown,
    }
}

/// Find a top-level declaration by its exported source name.
pub(super) fn find_top_level_decl<'a>(ast: &'a Program, name: &str) -> Option<&'a Declaration> {
    ast.declarations.iter().find_map(|decl| match &decl.node {
        Declaration::Function(function) if function.name == name => Some(&decl.node),
        Declaration::Model(model) if model.name == name => Some(&decl.node),
        Declaration::Class(class) if class.name == name => Some(&decl.node),
        Declaration::Trait(trait_decl) if trait_decl.name == name => Some(&decl.node),
        Declaration::Enum(enum_decl) if enum_decl.name == name => Some(&decl.node),
        Declaration::TypeAlias(alias) if alias.name == name => Some(&decl.node),
        Declaration::Newtype(newtype) if newtype.name == name => Some(&decl.node),
        Declaration::Const(konst) if konst.name == name => Some(&decl.node),
        Declaration::Static(static_decl) if static_decl.name == name => Some(&decl.node),
        Declaration::Partial(partial) if partial.name == name => Some(&decl.node),
        _ => None,
    })
}

/// Add variant-level previews for checked public enum variants using AST source spans.
pub(super) fn push_enum_variant_previews(
    previews: &mut Vec<ApiMetadataPreview>,
    owner: &str,
    value_type: Option<EnumValueTypeExport>,
    checked_variants: &[incan_frontend::api_metadata::ApiEnumVariant],
    ast_variants: &[incan_frontend::ast::Spanned<incan_frontend::ast::VariantDecl>],
) {
    for variant in ast_variants {
        let Some(checked) = checked_variants
            .iter()
            .find(|checked| checked.name == variant.node.name)
        else {
            continue;
        };
        previews.push(ApiMetadataPreview {
            span: variant.span,
            markdown: api_enum_variant_markdown(owner, value_type, checked),
        });
    }
}

/// Add field-level previews for checked public model/class fields using AST source spans.
pub(super) fn push_field_previews(
    previews: &mut Vec<ApiMetadataPreview>,
    owner: &str,
    checked_fields: &[FieldExport],
    ast_fields: &[incan_frontend::ast::Spanned<incan_frontend::ast::FieldDecl>],
) {
    for field in ast_fields {
        let Some(checked) = checked_fields.iter().find(|checked| checked.name == field.node.name) else {
            continue;
        };
        previews.push(ApiMetadataPreview {
            span: field.span,
            markdown: api_field_markdown(owner, checked),
        });
    }
}

/// Add method-level previews using checked API metadata method anchors.
pub(super) fn push_method_previews(previews: &mut Vec<ApiMetadataPreview>, owner: &str, methods: &[ApiMethod]) {
    for method in methods {
        previews.push(preview_for_anchor(&method.anchor, api_method_markdown(owner, method)));
    }
}
