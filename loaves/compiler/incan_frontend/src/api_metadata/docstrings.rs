//! Docstring parsing and validation against checked API declarations.

use super::*;

/// Validate parsed API docstrings across a checked metadata package.
pub fn validate_checked_api_docstrings(package: &[CheckedApiMetadata]) -> Vec<ApiDocstringDiagnostic> {
    let aliases = package_aliases(package);
    let mut diagnostics = Vec::new();
    for module in package {
        validate_module_docstrings(module, &aliases, &mut diagnostics);
    }
    diagnostics
}

/// Parse a source docstring into structured API documentation.
pub(super) fn parse_docstring(docstring: Option<&str>) -> Option<ApiDocstring> {
    let docstring = docstring?;
    let lines = normalized_docstring_lines(docstring);
    if lines.is_empty() {
        return None;
    }

    let mut parsed = DocstringBuilder::default();
    let mut section = DocstringSection::Summary;
    for line in lines {
        if let Some(next_section) = DocstringSection::from_heading(&line) {
            section = next_section;
            continue;
        }
        parsed.push_line(section, &line);
    }
    Some(parsed.finish())
}

/// Return normalized docstring body lines.
pub(super) fn normalized_docstring_lines(docstring: &str) -> Vec<String> {
    docstring
        .lines()
        .map(str::trim)
        .skip_while(|line| line.is_empty())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .skip_while(|line| line.is_empty())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(str::to_string)
        .collect()
}

/// Append a normalized prose line to a docstring section.
pub(super) fn push_prose_line(lines: &mut Vec<String>, line: &str) {
    if line.is_empty() {
        if !lines.last().is_some_and(String::is_empty) {
            lines.push(String::new());
        }
        return;
    }
    lines.push(line.to_string());
}

/// Append a normalized entry line to a docstring section.
pub(super) fn push_entry_line(entries: &mut Vec<ApiDocstringEntry>, line: &str) {
    if line.is_empty() {
        return;
    }
    if let Some((name, description)) = line.split_once(':') {
        let name = name.trim();
        if !name.is_empty() {
            entries.push(ApiDocstringEntry {
                name: name.to_string(),
                description: description.trim().to_string(),
            });
            return;
        }
    }
    if let Some(last) = entries.last_mut() {
        if !last.description.is_empty() {
            last.description.push(' ');
        }
        last.description.push_str(line.trim());
    }
}

/// Parse a docstring return section into structured API documentation.
pub(super) fn parse_return_section(lines: Vec<String>) -> Option<ApiDocstringReturn> {
    let description = joined_non_empty(lines)?;
    if let Some((ty, rest)) = description.split_once(':') {
        let ty = ty.trim();
        if looks_like_type_spelling(ty) {
            return Some(ApiDocstringReturn {
                ty: Some(ty.to_string()),
                description: rest.trim().to_string(),
            });
        }
    }
    Some(ApiDocstringReturn { ty: None, description })
}

/// Join non-empty docstring lines into a single paragraph.
pub(super) fn joined_non_empty(lines: Vec<String>) -> Option<String> {
    let joined = lines.join("\n").trim().to_string();
    if joined.is_empty() { None } else { Some(joined) }
}

/// Return whether a docstring fragment looks like a type spelling.
pub(super) fn looks_like_type_spelling(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':' | '[' | ']' | ',' | ' ' | '&'))
}

/// Validate docstring coverage for declarations in one API metadata module.
pub(super) fn validate_module_docstrings(
    module: &CheckedApiMetadata,
    aliases: &[ApiAlias],
    diagnostics: &mut Vec<ApiDocstringDiagnostic>,
) {
    for declaration in &module.declarations {
        match declaration {
            ApiDeclaration::Function(function) => validate_callable_docstring(
                &module.module_path,
                &function.name,
                &function.anchor,
                function.docstring_sections.as_ref(),
                CallableDocFacts {
                    params: &function.params,
                    return_type: &function.return_type,
                    decorators: &function.decorators,
                    aliases: aliases_for_declaration(aliases, &module.module_path, &function.name),
                },
                diagnostics,
            ),
            ApiDeclaration::Model(model) => validate_type_docstring(
                &module.module_path,
                &model.name,
                &model.anchor,
                model.docstring_sections.as_ref(),
                TypeDocFacts {
                    fields: &model.fields,
                    decorators: &model.decorators,
                    aliases: aliases_for_declaration(aliases, &module.module_path, &model.name),
                },
                diagnostics,
            ),
            ApiDeclaration::Class(class) => validate_type_docstring(
                &module.module_path,
                &class.name,
                &class.anchor,
                class.docstring_sections.as_ref(),
                TypeDocFacts {
                    fields: &class.fields,
                    decorators: &class.decorators,
                    aliases: aliases_for_declaration(aliases, &module.module_path, &class.name),
                },
                diagnostics,
            ),
            ApiDeclaration::Trait(trait_decl) => validate_type_docstring(
                &module.module_path,
                &trait_decl.name,
                &trait_decl.anchor,
                trait_decl.docstring_sections.as_ref(),
                TypeDocFacts {
                    fields: &trait_decl.requires,
                    decorators: &trait_decl.decorators,
                    aliases: aliases_for_declaration(aliases, &module.module_path, &trait_decl.name),
                },
                diagnostics,
            ),
            ApiDeclaration::Enum(enum_decl) => validate_declaration_docstring(
                &module.module_path,
                &enum_decl.name,
                &enum_decl.anchor,
                enum_decl.docstring_sections.as_ref(),
                DeclarationDocFacts {
                    decorators: &enum_decl.decorators,
                    aliases: aliases_for_declaration(aliases, &module.module_path, &enum_decl.name),
                },
                diagnostics,
            ),
            ApiDeclaration::Newtype(newtype) => validate_declaration_docstring(
                &module.module_path,
                &newtype.name,
                &newtype.anchor,
                newtype.docstring_sections.as_ref(),
                DeclarationDocFacts {
                    decorators: &newtype.decorators,
                    aliases: aliases_for_declaration(aliases, &module.module_path, &newtype.name),
                },
                diagnostics,
            ),
            _ => {}
        }

        for method in declaration_methods(declaration) {
            validate_callable_docstring(
                &module.module_path,
                &method.name,
                &method.anchor,
                method.docstring_sections.as_ref(),
                CallableDocFacts {
                    params: &method.params,
                    return_type: &method.return_type,
                    decorators: &method.decorators,
                    aliases: Vec::new(),
                },
                diagnostics,
            );
        }
    }
}

/// Validate a callable docstring against its exported API shape.
pub(super) fn validate_callable_docstring(
    module_path: &[String],
    declaration_name: &str,
    anchor: &SourceAnchor,
    docstring: Option<&ApiDocstring>,
    facts: CallableDocFacts<'_>,
    diagnostics: &mut Vec<ApiDocstringDiagnostic>,
) {
    let Some(docstring) = docstring else {
        return;
    };
    validate_named_entries(
        module_path,
        anchor,
        declaration_name,
        "parameter",
        &docstring.params,
        facts.params.iter().map(|param| param.name.as_str()).collect(),
        diagnostics,
    );
    validate_return_docstring(
        module_path,
        anchor,
        declaration_name,
        docstring,
        facts.return_type,
        diagnostics,
    );
    validate_decorator_entries(
        module_path,
        anchor,
        declaration_name,
        &docstring.decorators,
        facts.decorators,
        diagnostics,
    );
    validate_alias_entries(
        module_path,
        anchor,
        declaration_name,
        &docstring.aliases,
        facts.aliases,
        diagnostics,
    );
}

/// Validate a type docstring against its exported API shape.
pub(super) fn validate_type_docstring(
    module_path: &[String],
    declaration_name: &str,
    anchor: &SourceAnchor,
    docstring: Option<&ApiDocstring>,
    facts: TypeDocFacts<'_>,
    diagnostics: &mut Vec<ApiDocstringDiagnostic>,
) {
    let Some(docstring) = docstring else {
        return;
    };
    validate_named_entries(
        module_path,
        anchor,
        declaration_name,
        "field",
        &docstring.fields,
        facts.fields.iter().map(|field| field.name.as_str()).collect(),
        diagnostics,
    );
    validate_decorator_entries(
        module_path,
        anchor,
        declaration_name,
        &docstring.decorators,
        facts.decorators,
        diagnostics,
    );
    validate_alias_entries(
        module_path,
        anchor,
        declaration_name,
        &docstring.aliases,
        facts.aliases,
        diagnostics,
    );
}

/// Validate one exported declaration docstring.
pub(super) fn validate_declaration_docstring(
    module_path: &[String],
    declaration_name: &str,
    anchor: &SourceAnchor,
    docstring: Option<&ApiDocstring>,
    facts: DeclarationDocFacts<'_>,
    diagnostics: &mut Vec<ApiDocstringDiagnostic>,
) {
    let Some(docstring) = docstring else {
        return;
    };
    validate_decorator_entries(
        module_path,
        anchor,
        declaration_name,
        &docstring.decorators,
        facts.decorators,
        diagnostics,
    );
    validate_alias_entries(
        module_path,
        anchor,
        declaration_name,
        &docstring.aliases,
        facts.aliases,
        diagnostics,
    );
}

/// Validate the return section for a callable docstring.
pub(super) fn validate_return_docstring(
    module_path: &[String],
    anchor: &SourceAnchor,
    declaration_name: &str,
    docstring: &ApiDocstring,
    return_type: &TypeRef,
    diagnostics: &mut Vec<ApiDocstringDiagnostic>,
) {
    let Some(returns) = &docstring.returns else {
        return;
    };
    let Some(documented_type) = &returns.ty else {
        return;
    };
    let checked_type = type_ref_doc_name(return_type);
    if documented_type != &checked_type {
        push_docstring_diagnostic(
            diagnostics,
            module_path,
            anchor,
            format!(
                "API docstring drift for `{declaration_name}`: documented return type `{documented_type}` does not match checked return type `{checked_type}`"
            ),
            "Update the `Returns:` section or the checked function signature.",
        );
    }
}

/// Validate decorator documentation entries for an exported callable.
pub(super) fn validate_decorator_entries(
    module_path: &[String],
    anchor: &SourceAnchor,
    declaration_name: &str,
    documented: &[ApiDocstringEntry],
    checked: &[DecoratorMetadata],
    diagnostics: &mut Vec<ApiDocstringDiagnostic>,
) {
    let names = checked
        .iter()
        .flat_map(|decorator| {
            [
                decorator.source_name.as_str(),
                decorator.path.last().map(String::as_str).unwrap_or_default(),
            ]
        })
        .collect::<Vec<_>>();
    validate_named_entries(
        module_path,
        anchor,
        declaration_name,
        "decorator",
        documented,
        names,
        diagnostics,
    );
}

/// Validate alias documentation entries for exported declarations.
pub(super) fn validate_alias_entries(
    module_path: &[String],
    anchor: &SourceAnchor,
    declaration_name: &str,
    documented: &[ApiDocstringEntry],
    checked_aliases: Vec<&str>,
    diagnostics: &mut Vec<ApiDocstringDiagnostic>,
) {
    validate_named_entries(
        module_path,
        anchor,
        declaration_name,
        "alias",
        documented,
        checked_aliases,
        diagnostics,
    );
}

/// Validate named docstring entries against checked metadata names and report stale or missing entries.
pub(super) fn validate_named_entries(
    module_path: &[String],
    anchor: &SourceAnchor,
    declaration_name: &str,
    noun: &str,
    documented: &[ApiDocstringEntry],
    checked_names: Vec<&str>,
    diagnostics: &mut Vec<ApiDocstringDiagnostic>,
) {
    if documented.is_empty() {
        return;
    }

    let checked: HashSet<&str> = checked_names.into_iter().filter(|name| !name.is_empty()).collect();
    let mut seen = HashSet::new();
    for entry in documented {
        if !seen.insert(entry.name.as_str()) {
            push_docstring_diagnostic(
                diagnostics,
                module_path,
                anchor,
                format!(
                    "API docstring drift for `{declaration_name}`: docstring documents {noun} `{}` more than once",
                    entry.name
                ),
                format!(
                    "Keep one `{}` entry in the `{}` section.",
                    entry.name,
                    section_name_for_noun(noun)
                ),
            );
            continue;
        }
        if !checked.contains(entry.name.as_str()) {
            push_docstring_diagnostic(
                diagnostics,
                module_path,
                anchor,
                format!(
                    "API docstring drift for `{declaration_name}`: documented {noun} `{}` does not exist in checked metadata",
                    entry.name
                ),
                format!(
                    "Remove `{}` from the `{}` section or update the checked declaration.",
                    entry.name,
                    section_name_for_noun(noun)
                ),
            );
        }
    }

    if matches!(noun, "parameter" | "field") {
        for checked_name in checked {
            if !seen.contains(checked_name) {
                push_docstring_diagnostic(
                    diagnostics,
                    module_path,
                    anchor,
                    format!(
                        "API docstring drift for `{declaration_name}`: checked {noun} `{checked_name}` is missing from the docstring"
                    ),
                    format!(
                        "Add `{checked_name}: ...` to the `{}` section or remove the stale section.",
                        section_name_for_noun(noun)
                    ),
                );
            }
        }
    }
}

/// Return the expected docstring section name for a documented noun.
pub(super) fn section_name_for_noun(noun: &str) -> &'static str {
    match noun {
        "parameter" => "Args:",
        "field" => "Fields:",
        "alias" => "Aliases:",
        "decorator" => "Decorators:",
        _ => "docstring",
    }
}

/// Record an API docstring diagnostic anchored to a source span.
pub(super) fn push_docstring_diagnostic(
    diagnostics: &mut Vec<ApiDocstringDiagnostic>,
    module_path: &[String],
    anchor: &SourceAnchor,
    message: String,
    hint: impl Into<String>,
) {
    diagnostics.push(ApiDocstringDiagnostic {
        module_path: module_path.to_vec(),
        error: CompileError::new(message, anchor.span).with_hint(hint),
    });
}

/// Return method metadata attached to a class-like declaration.
pub(super) fn declaration_methods(declaration: &ApiDeclaration) -> &[ApiMethod] {
    match declaration {
        ApiDeclaration::Model(model) => &model.methods,
        ApiDeclaration::Class(class) => &class.methods,
        ApiDeclaration::Trait(trait_decl) => &trait_decl.methods,
        ApiDeclaration::Newtype(newtype) => &newtype.methods,
        _ => &[],
    }
}

/// Return alias metadata exported by a checked package.
pub(super) fn package_aliases(package: &[CheckedApiMetadata]) -> Vec<ApiAlias> {
    package
        .iter()
        .flat_map(|module| module.declarations.iter())
        .filter_map(|declaration| match declaration {
            ApiDeclaration::Alias(alias) => Some(alias.clone()),
            _ => None,
        })
        .collect()
}

/// Return aliases that target a specific exported declaration.
pub(super) fn aliases_for_declaration<'a>(aliases: &'a [ApiAlias], module_path: &[String], name: &str) -> Vec<&'a str> {
    aliases
        .iter()
        .filter(|alias| alias_targets_declaration(alias, module_path, name))
        .map(|alias| alias.name.as_str())
        .collect()
}

/// Return whether an alias path names a specific exported declaration.
pub(super) fn alias_targets_declaration(alias: &ApiAlias, module_path: &[String], name: &str) -> bool {
    let mut declaration_path = module_path.to_vec();
    declaration_path.push(name.to_string());
    if alias.target_path == declaration_path {
        return true;
    }
    if alias.target_path.first().is_some_and(|segment| segment == "crate") && alias.target_path[1..] == declaration_path
    {
        return true;
    }
    false
}

/// Render a type reference as a docstring-facing type name.
pub(super) fn type_ref_doc_name(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Named { name, .. } => name.clone(),
        TypeRef::Applied { name, args, .. } => {
            let args = args.iter().map(type_ref_doc_name).collect::<Vec<_>>().join(", ");
            format!("{name}[{args}]")
        }
        TypeRef::Function { params, return_type } => {
            let params = params.iter().map(type_ref_doc_name).collect::<Vec<_>>().join(", ");
            format!("({params}) -> {}", type_ref_doc_name(return_type))
        }
        TypeRef::TypeToken { inner } => format!("Type[{}]", type_ref_doc_name(inner)),
        TypeRef::Tuple { elements } => {
            let elements = elements.iter().map(type_ref_doc_name).collect::<Vec<_>>().join(", ");
            format!("({elements})")
        }
        TypeRef::TypeParam { name } => name.clone(),
        TypeRef::SelfType => "Self".to_string(),
        TypeRef::Ref { inner } => format!("&{}", type_ref_doc_name(inner)),
        TypeRef::MutParam { inner } => format!("mut {}", type_ref_doc_name(inner)),
        TypeRef::Ownership { mode, inner } => format!("{}[{}]", mode.as_str(), type_ref_doc_name(inner)),
        TypeRef::RustPath { path } => path.clone(),
        TypeRef::NativeUnion(native) => native
            .members
            .iter()
            .map(type_ref_doc_name)
            .collect::<Vec<_>>()
            .join(" | "),
        TypeRef::Unknown => "Unknown".to_string(),
    }
}

/// Build a source anchor for an API metadata span.
pub(super) fn anchor(module_path: &[String], name: &str, span: Span) -> SourceAnchor {
    let mut parts = module_path.to_vec();
    parts.push(name.to_string());
    SourceAnchor {
        id: parts.join("::"),
        span: source_span(span),
    }
}

/// Convert a concrete span into an API metadata source span.
pub(super) fn source_span(span: Span) -> SourceSpan {
    span
}
