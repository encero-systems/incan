//! The Loaves a document's compilation reaches, as the editor sees them: module and member completion in
//! `from pub::<key>…` and `from std.…` imports, and hover over imported modules and items.
//!
//! Every Loaf takes this one route (DD-0007): a standard package is a Loaf like a dependency, and its manifest's
//! checked API is the only source either has.

use super::*;

/// Complete the public modules and declarations of the Loaf module a `from <module> import …` line names.
pub(super) fn import_item_completions(line_prefix: &str, loaves: &LoafIndex) -> Option<Vec<CompletionItem>> {
    let after_from = line_prefix.trim_start().strip_prefix("from ")?;
    let (module_text, _after_import) = after_from.split_once(" import ")?;
    let module = module_segments(module_text);
    let entry = loaves.owner_of(&module)?;
    let path = &module[entry.binding_root().len()..];
    let manifest = &entry.artifact.manifest;
    let api = manifest.contract_metadata.api.as_ref()?;

    let mut completions = BTreeMap::new();
    if path.is_empty() {
        for root in checked_api_public_module_paths(api)
            .into_iter()
            .filter(|candidate| candidate.len() == 1)
        {
            completions.insert(
                root[0].clone(),
                completion(root[0].clone(), CompletionItemKind::MODULE, "public module"),
            );
        }
        insert_flat_export_completions(&mut completions, manifest);
    } else {
        let mut declarations: BTreeMap<String, Vec<(Vec<String>, CompletionItemKind)>> = BTreeMap::new();
        for source_module in checked_api_modules_for_public_namespace(api, path) {
            for declaration in &source_module.declarations {
                if !checked_api_declaration_is_public_namespace_member(declaration) {
                    continue;
                }
                let Some((name, kind)) = declaration_completion(declaration) else {
                    continue;
                };
                declarations
                    .entry(name.to_string())
                    .or_default()
                    .push((source_module.module_path.clone(), kind));
            }
        }
        for (name, mut candidates) in declarations {
            candidates.sort_by(|left, right| left.0.cmp(&right.0));
            candidates.dedup_by(|left, right| left.0 == right.0);
            let (kind, detail) = if candidates.len() == 1 {
                (
                    candidates[0].1,
                    format!("public declaration of {}", module_display(&module)),
                )
            } else {
                (
                    CompletionItemKind::REFERENCE,
                    format!(
                        "ambiguous public name; qualify one of: {}",
                        candidates
                            .iter()
                            .map(|(path, _)| path.join("."))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
            };
            completions.insert(name.clone(), completion(name, kind, &detail));
        }
        for child in child_modules(api, path) {
            completions
                .entry(child.clone())
                .or_insert_with(|| completion(child, CompletionItemKind::MODULE, "public module"));
        }
    }
    (!completions.is_empty()).then(|| completions.into_values().collect())
}

/// Complete the module path of a `from`/`import` line that names a Loaf: dependency keys after `pub::`, standard
/// roots after `std.`, and each Loaf's public modules below them.
pub(super) fn module_completions(line_prefix: &str, loaves: &LoafIndex) -> Option<Vec<CompletionItem>> {
    let trimmed = line_prefix.trim_start();
    let remainder = trimmed
        .strip_prefix("from ")
        .or_else(|| trimmed.strip_prefix("import "))?;
    if remainder.contains(" import ") {
        return None;
    }
    // The segments before the one being typed.
    let typed = module_segments(remainder);
    let completed = if remainder.ends_with('.') || remainder.ends_with("::") {
        typed
    } else {
        typed[..typed.len().saturating_sub(1)].to_vec()
    };
    let names = match completed.as_slice() {
        [] => return None,
        [root] if root == "pub" => loaves
            .entries()
            .filter(|entry| matches!(entry.namespace, LoafNamespace::Dependency { .. }))
            .map(|entry| entry.key.clone())
            .collect::<BTreeSet<_>>(),
        [root] if root == stdlib::STDLIB_ROOT => loaves
            .entries()
            .filter_map(|entry| match &entry.namespace {
                LoafNamespace::Standard { policy } => Some(policy.namespace_roots),
                LoafNamespace::Dependency { .. } => None,
            })
            .flatten()
            .map(|root| root.to_string())
            .collect(),
        _ => {
            let entry = loaves.owner_of(&completed)?;
            let api = entry.artifact.manifest.contract_metadata.api.as_ref()?;
            child_modules(api, &completed[entry.binding_root().len()..])
        }
    };
    let items = names
        .into_iter()
        .map(|name| completion(name, CompletionItemKind::MODULE, "Loaf module"))
        .collect::<Vec<_>>();
    (!items.is_empty()).then_some(items)
}

/// Hover over an import of a Loaf module: the module and the Loaf that owns it.
pub(super) fn import_module_hover(ast: &Program, offset: usize, loaves: &LoafIndex) -> Option<String> {
    let module = import_at(ast, offset)?.0;
    let entry = loaves.owner_of(&module)?;
    Some(format!(
        "```incan\n{}\n```\n\n*module of the Loaf `{}` {}*",
        module_display(&module),
        entry.artifact.identity.name,
        entry.artifact.identity.version
    ))
}

/// Hover over a name a `from <Loaf module> import …` line imports: its checked public API.
pub(super) fn import_item_hover(
    ast: &Program,
    source: &str,
    offset: usize,
    loaves: &LoafIndex,
) -> Option<(String, Span)> {
    let (ident, ident_span) = identifier_at_offset(source, offset)?;
    let (module, items) = import_at(ast, ident_span.start)?;
    let item = items
        .iter()
        .find(|item| item.alias.as_deref().unwrap_or(item.name.as_str()) == ident || item.name == ident)?;
    let entry = loaves.owner_of(&module)?;
    let api = entry.artifact.manifest.contract_metadata.api.as_ref()?;
    let path = &module[entry.binding_root().len()..];
    let declaration = checked_api_modules_for_public_namespace(api, path)
        .into_iter()
        .flat_map(|source_module| source_module.declarations.iter())
        .find(|declaration| {
            checked_api_declaration_is_public_namespace_member(declaration)
                && declaration_completion(declaration).is_some_and(|(name, _)| name == item.name)
        })?;
    Some((declaration_markdown(declaration), ident_span))
}

/// The import declaration containing `offset` whose module is a Loaf's (`pub::…` or `std.…`), with the items a
/// `from` import names.
fn import_at(ast: &Program, offset: usize) -> Option<(Vec<String>, &[incan_frontend::ast::ImportItem])> {
    ast.declarations.iter().find_map(|declaration| {
        let Declaration::Import(import) = &declaration.node else {
            return None;
        };
        if !(declaration.span.start <= offset && offset < declaration.span.end) {
            return None;
        }
        let (segments, items): (&[String], &[incan_frontend::ast::ImportItem]) = match &import.kind {
            incan_frontend::ast::ImportKind::Module(path) => (&path.segments, &[]),
            incan_frontend::ast::ImportKind::From { module, items } => (&module.segments, items),
            _ => return None,
        };
        matches!(
            segments.first().map(String::as_str),
            Some("pub") | Some(stdlib::STDLIB_ROOT)
        )
        .then(|| (segments.to_vec(), items))
    })
}

/// Split a typed module path (`pub::key.a`, `std.json`, `std::json`) into its segments.
fn module_segments(text: &str) -> Vec<String> {
    text.trim()
        .replace("::", ".")
        .split('.')
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect()
}

/// A module path as source spells it: `pub::key.a.b` or `std.a.b`.
fn module_display(module: &[String]) -> String {
    match module {
        [root, rest @ ..] if root == "pub" => format!("pub::{}", rest.join(".")),
        _ => module.join("."),
    }
}

/// The public modules directly below `path` in a Loaf's checked API.
fn child_modules(api: &CheckedApiMetadataPackage, path: &[String]) -> BTreeSet<String> {
    checked_api_public_module_paths(api)
        .into_iter()
        .filter(|candidate| candidate.starts_with(path) && candidate.len() == path.len() + 1)
        .map(|candidate| candidate[path.len()].clone())
        .collect()
}

/// Add a Loaf's flat export facade to the completions of its root.
fn insert_flat_export_completions(
    completions: &mut BTreeMap<String, CompletionItem>,
    manifest: &incan_frontend::library_manifest::LibraryManifest,
) {
    let exports = &manifest.exports;
    let groups: [(Vec<&str>, CompletionItemKind); 11] = [
        (
            exports.models.iter().map(|item| item.name.as_str()).collect(),
            CompletionItemKind::CLASS,
        ),
        (
            exports.classes.iter().map(|item| item.name.as_str()).collect(),
            CompletionItemKind::CLASS,
        ),
        (
            exports.functions.iter().map(|item| item.name.as_str()).collect(),
            CompletionItemKind::FUNCTION,
        ),
        (
            exports.partials.iter().map(|item| item.name.as_str()).collect(),
            CompletionItemKind::FUNCTION,
        ),
        (
            exports.traits.iter().map(|item| item.name.as_str()).collect(),
            CompletionItemKind::INTERFACE,
        ),
        (
            exports.enums.iter().map(|item| item.name.as_str()).collect(),
            CompletionItemKind::ENUM,
        ),
        (
            exports.newtypes.iter().map(|item| item.name.as_str()).collect(),
            CompletionItemKind::CLASS,
        ),
        (
            exports.type_aliases.iter().map(|item| item.name.as_str()).collect(),
            CompletionItemKind::TYPE_PARAMETER,
        ),
        (
            exports.consts.iter().map(|item| item.name.as_str()).collect(),
            CompletionItemKind::CONSTANT,
        ),
        (
            exports.statics.iter().map(|item| item.name.as_str()).collect(),
            CompletionItemKind::CONSTANT,
        ),
        (
            exports.aliases.iter().map(|item| item.name.as_str()).collect(),
            CompletionItemKind::REFERENCE,
        ),
    ];
    for (names, kind) in groups {
        for name in names {
            completions
                .entry(name.to_string())
                .or_insert_with(|| completion(name.to_string(), kind, "public export"));
        }
    }
}

/// The completion label and kind of one checked public API declaration.
fn declaration_completion(declaration: &ApiDeclaration) -> Option<(&str, CompletionItemKind)> {
    match declaration {
        ApiDeclaration::Function(item) => Some((&item.name, CompletionItemKind::FUNCTION)),
        ApiDeclaration::Model(item) => Some((&item.name, CompletionItemKind::CLASS)),
        ApiDeclaration::Class(item) => Some((&item.name, CompletionItemKind::CLASS)),
        ApiDeclaration::Trait(item) => Some((&item.name, CompletionItemKind::INTERFACE)),
        ApiDeclaration::Enum(item) => Some((&item.name, CompletionItemKind::ENUM)),
        ApiDeclaration::Newtype(item) => Some((&item.name, CompletionItemKind::CLASS)),
        ApiDeclaration::TypeAlias(item) => Some((&item.name, CompletionItemKind::TYPE_PARAMETER)),
        ApiDeclaration::Const(item) => Some((&item.name, CompletionItemKind::CONSTANT)),
        ApiDeclaration::Static(item) => Some((&item.name, CompletionItemKind::CONSTANT)),
        ApiDeclaration::Alias(item) => Some((
            &item.name,
            if item.projected_function.is_some() {
                CompletionItemKind::FUNCTION
            } else {
                CompletionItemKind::REFERENCE
            },
        )),
        ApiDeclaration::Partial(item) => Some((&item.name, CompletionItemKind::FUNCTION)),
    }
}

/// The hover markdown of one checked public API declaration.
fn declaration_markdown(declaration: &ApiDeclaration) -> String {
    match declaration {
        ApiDeclaration::Function(function) => api_function_markdown(function),
        ApiDeclaration::Model(model) => api_model_markdown(model),
        ApiDeclaration::Class(class) => api_class_markdown(class),
        ApiDeclaration::Trait(trait_decl) => api_trait_markdown(trait_decl),
        ApiDeclaration::Enum(enum_decl) => api_enum_markdown(enum_decl),
        ApiDeclaration::Newtype(newtype) => api_newtype_markdown(newtype),
        ApiDeclaration::TypeAlias(alias) => api_type_alias_markdown(alias),
        ApiDeclaration::Const(konst) => api_const_markdown(konst),
        ApiDeclaration::Static(static_decl) => api_static_markdown(static_decl),
        ApiDeclaration::Alias(alias) => checked_api_markdown(
            format!("pub alias {} = {}", alias.name, alias.target_path.join("::")),
            "public import alias",
            Vec::new(),
        ),
        ApiDeclaration::Partial(partial) => api_partial_markdown(partial),
    }
}

/// One compact completion item for a Loaf's public surface.
fn completion(label: String, kind: CompletionItemKind, detail: &str) -> CompletionItem {
    CompletionItem {
        label,
        kind: Some(kind),
        detail: Some(detail.to_string()),
        ..Default::default()
    }
}
