//! Local toolchain inspection commands.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::ValueEnum;
use incan_codegraph::{
    CodegraphDocument, CodegraphEdge, CodegraphEdgeKind, CodegraphNode, CodegraphNodeKind, CodegraphPackage,
    CodegraphSpan,
};
use serde_json::json;

use crate::cli::prelude::ParsedModule;
use crate::cli::{CliError, CliResult, ExitCode};
use crate::frontend::api_metadata::{
    ApiDeclaration, ApiFunction, ApiMethod, ApiPartial, CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadata,
    CheckedApiMetadataPackage, CheckedApiPackageIdentity, SourceAnchor, collect_checked_api_metadata,
    validate_checked_api_docstrings,
};
use crate::frontend::ast::{
    AliasDecl, AssertKind, CallArg, ClassDecl, ComprehensionClause, Condition, ConstDecl, Declaration, DecoratorArg,
    DecoratorArgValue, DictEntry, EnumDecl, Expr, FStringPart, FunctionDecl, ImportDecl, ImportItem, ImportKind,
    ImportPath, Literal, MatchBody, ModelDecl, NewtypeDecl, PartialDecl, Pattern, RaceForBody, Span, Spanned,
    Statement, StaticDecl, TestModuleDecl, TraitDecl, TypeAliasDecl, Visibility,
};
use crate::frontend::contract_metadata::{
    CanonicalModelBundle, read_model_bundles_from_json, read_project_model_bundles,
};
use crate::frontend::diagnostics;
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::typechecker;
use crate::library_manifest::{FieldExport, LibraryManifest, ParamExport, ParamKindExport, ReceiverExport, TypeRef};
use crate::manifest::ProjectManifest;

use super::common::{collect_modules, imported_module_deps_for_with_index, module_key_index, resolve_project_root};

/// Output format for `incan tools doctor`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolsDoctorFormat {
    /// Human-readable diagnostic report.
    Text,
    /// Machine-readable JSON report for editor integrations and issue templates.
    Json,
}

/// Output format for `incan tools codegraph export`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolsCodegraphFormat {
    /// Stable pretty JSON document.
    Json,
    /// Newline-delimited graph records.
    Jsonl,
}

/// Output format for `incan tools metadata api`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolsMetadataFormat {
    /// Stable checked API metadata JSON.
    Json,
    /// Generated Markdown reference from checked API metadata.
    Markdown,
}

/// Output format for `incan tools metadata model`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolsModelMetadataFormat {
    /// Formatted Incan model source.
    Incan,
    /// Canonical model bundle JSON.
    Json,
}

/// Run local toolchain diagnostics for CLI and editor setup.
pub fn tools_doctor(format: ToolsDoctorFormat) -> CliResult<ExitCode> {
    let report = DoctorReport::collect();
    match format {
        ToolsDoctorFormat::Text => report.print_text(),
        ToolsDoctorFormat::Json => report.print_json()?,
    }
    Ok(ExitCode::SUCCESS)
}

/// Emit compiler-backed codegraph facts for a source file or project directory.
pub fn tools_codegraph_export(path: &Path, format: ToolsCodegraphFormat, allow_errors: bool) -> CliResult<ExitCode> {
    let document = collect_codegraph_document(path, allow_errors)?;
    match format {
        ToolsCodegraphFormat::Json => {
            let output = document
                .to_pretty_json()
                .map_err(|error| CliError::failure(format!("failed to serialize codegraph document: {error}")))?;
            println!("{output}");
        }
        ToolsCodegraphFormat::Jsonl => {
            let output = document
                .to_jsonl()
                .map_err(|error| CliError::failure(format!("failed to serialize codegraph records: {error}")))?;
            print!("{output}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Emit checked public API metadata for a source file or project directory.
pub fn tools_metadata_api(path: &Path, format: ToolsMetadataFormat) -> CliResult<ExitCode> {
    let package = collect_api_metadata_package(path)?;
    match format {
        ToolsMetadataFormat::Json => {
            let output = serde_json::to_string_pretty(&package)
                .map_err(|error| CliError::failure(format!("failed to serialize API metadata: {error}")))?;
            println!("{output}");
        }
        ToolsMetadataFormat::Markdown => {
            print!("{}", render_api_metadata_markdown(&package));
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Render a compact Markdown API reference from checked API metadata.
fn render_api_metadata_markdown(package: &CheckedApiMetadataPackage) -> String {
    let title = package
        .package
        .as_ref()
        .map(|identity| identity.name.as_str())
        .unwrap_or("Checked API");
    let mut output = format!("# {title} API\n\n");
    if let Some(identity) = &package.package
        && let Some(version) = &identity.version
    {
        output.push_str(&format!("Version: `{version}`\n\n"));
    }

    for module in &package.modules {
        output.push_str(&format!("## Module `{}`\n\n", module.module_path.join("::")));
        for declaration in &module.declarations {
            match declaration {
                ApiDeclaration::Function(function) => render_api_function_markdown(&mut output, function),
                ApiDeclaration::Partial(partial) => render_api_partial_markdown(&mut output, partial),
                _ => render_api_declaration_summary_markdown(&mut output, declaration),
            }
        }
    }
    output
}

/// Render one public function declaration into the generated Markdown reference.
fn render_api_function_markdown(output: &mut String, function: &ApiFunction) {
    output.push_str(&format!("### `{}`\n\n", function.name));
    output.push_str("```incan\n");
    output.push_str(&format!(
        "pub def {}({}) -> {}\n",
        function.name,
        format_api_params(&function.params),
        format_api_type_ref(&function.return_type)
    ));
    output.push_str("```\n\n");
    if let Some(docstring) = function
        .docstring
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        output.push_str(docstring);
        output.push_str("\n\n");
    }
}

/// Render one public partial declaration into the generated Markdown reference.
fn render_api_partial_markdown(output: &mut String, partial: &ApiPartial) {
    output.push_str(&format!("### `{}`\n\n", partial.name));
    output.push_str("```incan\n");
    output.push_str(&format!(
        "pub {} = partial {}({}) -> {}\n",
        partial.name,
        partial.target_path.join("::"),
        format_api_params(&partial.params),
        format_api_type_ref(&partial.return_type)
    ));
    output.push_str("```\n\n");
    output.push_str(&format!("- Target: `{}`\n", partial.target_path.join("::")));
    if !partial.presets.is_empty() {
        let presets = partial
            .presets
            .iter()
            .map(|preset| format!("`{}`", preset.name))
            .collect::<Vec<_>>()
            .join(", ");
        output.push_str(&format!("- Presets: {presets}\n"));
    }
    output.push('\n');
}

/// Render a concise declaration summary for checked API declaration kinds without a specialized Markdown section.
fn render_api_declaration_summary_markdown(output: &mut String, declaration: &ApiDeclaration) {
    let Some((name, signature)) = api_declaration_summary_signature(declaration) else {
        return;
    };
    output.push_str(&format!("### `{name}`\n\n"));
    output.push_str("```incan\n");
    output.push_str(&signature);
    output.push('\n');
    output.push_str("```\n\n");
}

/// Return a compact checked declaration signature for generated Markdown.
fn api_declaration_summary_signature(declaration: &ApiDeclaration) -> Option<(String, String)> {
    match declaration {
        ApiDeclaration::Model(model) => Some((model.name.clone(), format!("pub model {}", model.name))),
        ApiDeclaration::Class(class) => Some((class.name.clone(), format!("pub class {}", class.name))),
        ApiDeclaration::Trait(trait_decl) => Some((trait_decl.name.clone(), format!("pub trait {}", trait_decl.name))),
        ApiDeclaration::Enum(enum_decl) => Some((enum_decl.name.clone(), format!("pub enum {}", enum_decl.name))),
        ApiDeclaration::Newtype(newtype) => {
            let keyword = if newtype.is_rusttype { "rusttype" } else { "newtype" };
            Some((
                newtype.name.clone(),
                format!(
                    "pub {keyword} {} = {}",
                    newtype.name,
                    format_api_type_ref(&newtype.underlying)
                ),
            ))
        }
        ApiDeclaration::TypeAlias(alias) => Some((
            alias.name.clone(),
            format!(
                "pub type {} = {}",
                alias.name,
                format_api_type_ref(&alias.type_alias.target)
            ),
        )),
        ApiDeclaration::Const(konst) => Some((
            konst.name.clone(),
            format!("pub const {}: {}", konst.name, format_api_type_ref(&konst.ty)),
        )),
        ApiDeclaration::Static(static_decl) => Some((
            static_decl.name.clone(),
            format!(
                "pub static {}: {}",
                static_decl.name,
                format_api_type_ref(&static_decl.ty)
            ),
        )),
        ApiDeclaration::Alias(alias) => Some((
            alias.name.clone(),
            format!("pub {} = alias {}", alias.name, alias.target_path.join("::")),
        )),
        ApiDeclaration::Function(_) | ApiDeclaration::Partial(_) => None,
    }
}

/// Format checked API callable parameters for generated Markdown signatures.
fn format_api_params(params: &[ParamExport]) -> String {
    params.iter().map(format_api_param).collect::<Vec<_>>().join(", ")
}

/// Format one checked API callable parameter for generated Markdown signatures.
fn format_api_param(param: &ParamExport) -> String {
    let prefix = match param.kind {
        ParamKindExport::Normal => "",
        ParamKindExport::RestPositional => "*",
        ParamKindExport::RestKeyword => "**",
    };
    let default = if param.has_default { " = ..." } else { "" };
    format!("{prefix}{}: {}{default}", param.name, format_api_type_ref(&param.ty))
}

/// Format a checked API type reference for generated Markdown signatures.
fn format_api_type_ref(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Named { name } | TypeRef::TypeParam { name } => name.clone(),
        TypeRef::Applied { name, args } => format!(
            "{}[{}]",
            name,
            args.iter().map(format_api_type_ref).collect::<Vec<_>>().join(", ")
        ),
        TypeRef::Function { params, return_type } => format!(
            "Callable[[{}], {}]",
            params.iter().map(format_api_type_ref).collect::<Vec<_>>().join(", "),
            format_api_type_ref(return_type)
        ),
        TypeRef::Tuple { elements } => {
            format!(
                "({})",
                elements.iter().map(format_api_type_ref).collect::<Vec<_>>().join(", ")
            )
        }
        TypeRef::SelfType => "Self".to_string(),
        TypeRef::Ref { inner } => format!("&{}", format_api_type_ref(inner)),
        TypeRef::RustPath { path } => format!("rust::{path}"),
        TypeRef::Unknown => "unknown".to_string(),
    }
}

/// Emit one canonical model bundle from a project, bundle file, or `.incnlib` artifact.
pub fn tools_metadata_model(path: &Path, model: &str, format: ToolsModelMetadataFormat) -> CliResult<ExitCode> {
    let bundle = find_model_bundle(path, model)?;
    match format {
        ToolsModelMetadataFormat::Incan => {
            print!(
                "{}",
                bundle
                    .emit_incan_model_source()
                    .map_err(|error| CliError::failure(error.to_string()))?
            );
        }
        ToolsModelMetadataFormat::Json => {
            let output = serde_json::to_string_pretty(&bundle)
                .map_err(|error| CliError::failure(format!("failed to serialize model bundle: {error}")))?;
            println!("{output}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Locate one model bundle by logical type name or stable model id and include available names when lookup fails.
fn find_model_bundle(path: &Path, model: &str) -> CliResult<CanonicalModelBundle> {
    let bundles = collect_model_bundles_for_path(path)?;
    bundles
        .into_iter()
        .find(|bundle| bundle.logical_type_name == model || bundle.stable_model_id.as_deref() == Some(model))
        .ok_or_else(|| {
            let available = collect_available_model_names(path).unwrap_or_default();
            let available = if available.is_empty() {
                "none".to_string()
            } else {
                available.join(", ")
            };
            CliError::failure(format!(
                "model `{model}` was not found in checked model metadata for {} (available: {available})",
                path.display()
            ))
        })
}

/// Collect validated model bundles from a project directory, source path, JSON bundle file, or library artifact.
fn collect_model_bundles_for_path(path: &Path) -> CliResult<Vec<CanonicalModelBundle>> {
    let absolute = absolute_path(path)?;
    if absolute.is_file()
        && absolute
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension == "incnlib")
    {
        let manifest =
            LibraryManifest::read_from_path(&absolute).map_err(|error| CliError::failure(error.to_string()))?;
        let bundles = manifest.contract_metadata.models.model_bundles;
        if bundles.is_empty() {
            return Err(CliError::failure(format!(
                "artifact {} does not carry checked model metadata",
                absolute.display()
            )));
        }
        return Ok(bundles);
    }
    if absolute.is_file()
        && absolute
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension == "json")
    {
        return read_model_bundles_from_json(&absolute).map_err(|error| CliError::failure(error.to_string()));
    }

    let project_root = if absolute.is_dir() {
        absolute
    } else {
        resolve_project_root(&absolute)
    };
    let Some(manifest) =
        ProjectManifest::discover(&project_root).map_err(|error| CliError::failure(error.to_string()))?
    else {
        return Err(CliError::failure(format!(
            "model metadata lookup requires a project manifest, bundle JSON, or `.incnlib` artifact: {}",
            path.display()
        )));
    };
    read_project_model_bundles(manifest.project_root(), &manifest.contract_model_bundle_paths())
        .map_err(|error| CliError::failure(error.to_string()))
}

/// Return sorted logical model names available at the given metadata path.
fn collect_available_model_names(path: &Path) -> CliResult<Vec<String>> {
    let mut names: Vec<String> = collect_model_bundles_for_path(path)?
        .into_iter()
        .map(|bundle| bundle.logical_type_name)
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

/// Resolve a CLI path relative to the current working directory without requiring the path to exist.
fn absolute_path(path: &Path) -> CliResult<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(env::current_dir()
            .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))?
            .join(path))
    }
}

/// Type-check a codegraph input path and collect deterministic source graph facts.
pub(crate) fn collect_codegraph_document(path: &Path, allow_errors: bool) -> CliResult<CodegraphDocument> {
    collect_codegraph_document_with_diagnostics(path, allow_errors, true)
}

/// Collect codegraph facts without printing type-check diagnostics when unchecked source is allowed.
pub(crate) fn collect_codegraph_document_suppressing_diagnostics(
    path: &Path,
    allow_errors: bool,
) -> CliResult<CodegraphDocument> {
    collect_codegraph_document_with_diagnostics(path, allow_errors, false)
}

/// Type-check a codegraph input path and collect deterministic source graph facts.
fn collect_codegraph_document_with_diagnostics(
    path: &Path,
    allow_errors: bool,
    emit_diagnostics: bool,
) -> CliResult<CodegraphDocument> {
    let input = collect_codegraph_input(path)?;
    let manifest =
        ProjectManifest::discover(&input.project_root).map_err(|error| CliError::failure(error.to_string()))?;
    let declared = manifest.as_ref().map(ProjectManifest::declared_rust_crate_names);
    let library_manifest_index = manifest
        .as_ref()
        .map(LibraryManifestIndex::from_project_manifest)
        .unwrap_or_default();
    let module_idx_by_key = module_key_index(&input.modules);
    let mut all_errors = String::new();
    let mut metadata_modules = Vec::new();

    for (idx, module) in input.modules.iter().enumerate() {
        let deps_for_module = imported_module_deps_for_with_index(&input.modules, idx, &module_idx_by_key);
        let mut checker = typechecker::TypeChecker::new();
        if let Some(names) = declared.clone() {
            checker.set_declared_crate_names(names);
        }
        checker.set_library_manifest_index(library_manifest_index.clone());

        match checker.check_with_imports(&module.ast, &deps_for_module) {
            Ok(()) => {
                if emit_diagnostics {
                    for warn in checker.warnings() {
                        eprint!(
                            "{}",
                            diagnostics::format_error(
                                module.file_path.to_string_lossy().as_ref(),
                                &module.source,
                                warn
                            )
                        );
                    }
                }
                metadata_modules.push(collect_checked_api_metadata(
                    &module.ast,
                    &checker,
                    codegraph_module_path(module, input.entry_path.as_deref()),
                ));
            }
            Err(errs) => {
                for err in &errs {
                    all_errors.push_str(&diagnostics::format_error(
                        module.file_path.to_string_lossy().as_ref(),
                        &module.source,
                        err,
                    ));
                }
            }
        }
    }

    if !all_errors.is_empty() && !allow_errors {
        return Err(CliError::failure(all_errors.trim_end()));
    }
    if !all_errors.is_empty() && emit_diagnostics {
        eprintln!("warning: codegraph export continuing with unchecked source graph after type-check errors");
        eprint!("{all_errors}");
    }

    Ok(build_codegraph_document(
        &input.modules,
        manifest.as_ref(),
        &input.project_root,
        input.entry_path.as_deref(),
        &metadata_modules,
    ))
}

/// Collected codegraph input modules plus the path context used to render stable facts.
struct CodegraphInput {
    modules: Vec<ParsedModule>,
    project_root: PathBuf,
    entry_path: Option<PathBuf>,
}

/// Collect codegraph modules from a file, project directory, or arbitrary source directory.
fn collect_codegraph_input(path: &Path) -> CliResult<CodegraphInput> {
    let absolute = absolute_path(path)?;
    if absolute.is_file() {
        let modules = collect_modules(&absolute.to_string_lossy())?;
        let project_root = resolve_project_root(&absolute);
        return Ok(CodegraphInput {
            modules,
            project_root,
            entry_path: Some(absolute),
        });
    }

    if absolute.is_dir() {
        let project_root = fs::canonicalize(&absolute).unwrap_or(absolute);
        let entry_paths = collect_codegraph_directory_entries(&project_root)?;
        let modules = collect_codegraph_directory_modules(&project_root, &entry_paths)?;
        return Ok(CodegraphInput {
            modules,
            project_root,
            entry_path: None,
        });
    }

    Err(CliError::failure(format!(
        "codegraph export path does not exist: {}",
        absolute.display()
    )))
}

/// Return every `.incn` source file under a directory in deterministic order.
fn collect_codegraph_directory_entries(root: &Path) -> CliResult<Vec<PathBuf>> {
    let mut entries = Vec::new();
    collect_codegraph_directory_entries_into(root, &mut entries)?;
    entries.sort();
    if entries.is_empty() {
        return Err(CliError::failure(format!(
            "codegraph export found no `.incn` files under directory: {}",
            root.display()
        )));
    }
    Ok(entries)
}

/// Recursively collect `.incn` source files under a directory.
fn collect_codegraph_directory_entries_into(dir: &Path, entries: &mut Vec<PathBuf>) -> CliResult<()> {
    for entry in fs::read_dir(dir).map_err(|error| {
        CliError::failure(format!(
            "failed to read codegraph export directory {}: {error}",
            dir.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            CliError::failure(format!(
                "failed to read codegraph export directory entry under {}: {error}",
                dir.display()
            ))
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            CliError::failure(format!(
                "failed to inspect codegraph export path {}: {error}",
                path.display()
            ))
        })?;
        if file_type.is_dir() {
            collect_codegraph_directory_entries_into(&path, entries)?;
        } else if file_type.is_file() && path.extension().is_some_and(|extension| extension == "incn") {
            entries.push(path);
        }
    }
    Ok(())
}

/// Collect and deduplicate modules reached by every directory entry source.
fn collect_codegraph_directory_modules(root: &Path, entry_paths: &[PathBuf]) -> CliResult<Vec<ParsedModule>> {
    let mut modules_by_path: BTreeMap<PathBuf, ParsedModule> = BTreeMap::new();
    for entry_path in entry_paths {
        let collected = collect_modules(&entry_path.to_string_lossy())?;
        for mut module in collected {
            module.file_path = fs::canonicalize(&module.file_path).unwrap_or(module.file_path);
            if module.file_path.starts_with(root) {
                module.path_segments = codegraph_module_segments_for_path(&module.file_path, root)?;
                module.name = module.path_segments.join("_");
            }
            modules_by_path.entry(module.file_path.clone()).or_insert(module);
        }
    }
    Ok(modules_by_path.into_values().collect())
}

/// Derive a module path from a source file's path relative to a globbed codegraph root.
fn codegraph_module_segments_for_path(path: &Path, root: &Path) -> CliResult<Vec<String>> {
    let relative = path.strip_prefix(root).map_err(|error| {
        CliError::failure(format!(
            "failed to derive codegraph module path for {} relative to {}: {error}",
            path.display(),
            root.display()
        ))
    })?;
    let mut segments = Vec::new();
    for component in relative.components() {
        let value = component.as_os_str().to_string_lossy();
        if value.ends_with(".incn") {
            segments.push(value.trim_end_matches(".incn").to_string());
        } else {
            segments.push(value.to_string());
        }
    }
    Ok(segments)
}

/// Build a codegraph document from parsed and checked modules.
fn build_codegraph_document(
    modules: &[ParsedModule],
    manifest: Option<&ProjectManifest>,
    project_root: &Path,
    entry_path: Option<&Path>,
    metadata_modules: &[CheckedApiMetadata],
) -> CodegraphDocument {
    let package = Some(CodegraphPackage {
        name: manifest
            .and_then(|manifest| manifest.project.as_ref())
            .and_then(|project| project.name.clone()),
        version: manifest
            .and_then(|manifest| manifest.project.as_ref())
            .and_then(|project| project.version.clone()),
        root_path: Some(".".to_string()),
    });
    let mut document = CodegraphDocument::new(package);
    let package_id = "package:root".to_string();
    document.push_node(CodegraphNode {
        id: package_id.clone(),
        kind: CodegraphNodeKind::Package,
        label: manifest
            .and_then(|manifest| manifest.project.as_ref())
            .and_then(|project| project.name.clone())
            .unwrap_or_else(|| "package".to_string()),
        file_path: None,
        module_path: Vec::new(),
        span: None,
        facts: BTreeMap::new(),
    });

    for module in modules {
        push_module_codegraph(
            &mut document,
            module,
            project_root,
            entry_path,
            &package_id,
            metadata_modules,
        );
    }
    document
}

/// Add file, module, declaration, and import facts for one parsed module.
fn push_module_codegraph(
    document: &mut CodegraphDocument,
    module: &ParsedModule,
    project_root: &Path,
    entry_path: Option<&Path>,
    package_id: &str,
    metadata_modules: &[CheckedApiMetadata],
) {
    let rel_path = codegraph_path(&module.file_path, project_root);
    let file_id = format!("file:{rel_path}");
    document.push_node(CodegraphNode {
        id: file_id.clone(),
        kind: CodegraphNodeKind::File,
        label: rel_path.clone(),
        file_path: Some(rel_path.clone()),
        module_path: Vec::new(),
        span: None,
        facts: BTreeMap::new(),
    });
    document.push_edge(codegraph_edge(
        package_id,
        &file_id,
        CodegraphEdgeKind::Contains,
        None,
        "package_contains_file",
    ));

    let module_path = codegraph_module_path(module, entry_path);
    let metadata = metadata_modules
        .iter()
        .find(|metadata| metadata.module_path == module_path);
    let module_id = format!("module:{}", module_path.join("::"));
    document.push_node(CodegraphNode {
        id: module_id.clone(),
        kind: CodegraphNodeKind::Module,
        label: module_path.join("::"),
        file_path: Some(rel_path.clone()),
        module_path: module_path.clone(),
        span: Some(CodegraphSpan {
            start: 0,
            end: module.source.len(),
        }),
        facts: BTreeMap::new(),
    });
    document.push_edge(codegraph_edge(
        &file_id,
        &module_id,
        CodegraphEdgeKind::Contains,
        None,
        "file_contains_module",
    ));

    for declaration in &module.ast.declarations {
        match &declaration.node {
            Declaration::Import(import) => {
                push_import_codegraph(document, import, declaration.span, &module_id, &module_path, &rel_path);
            }
            Declaration::Docstring(_) => {}
            other => {
                if let Some((kind, name, visibility)) = declaration_codegraph_info(other) {
                    let decl_id = format!("decl:{}::{name}:{}", module_path.join("::"), declaration.span.start);
                    let mut facts = BTreeMap::new();
                    facts.insert("declaration_kind".to_string(), kind.to_string());
                    facts.insert("visibility".to_string(), visibility.to_string());
                    let checked_declaration =
                        metadata.and_then(|metadata| find_api_metadata_declaration(metadata, &name, declaration.span));
                    if let Some(api_declaration) = checked_declaration {
                        facts.extend(api_declaration_facts(api_declaration));
                    }
                    document.push_node(CodegraphNode {
                        id: decl_id.clone(),
                        kind: CodegraphNodeKind::Declaration,
                        label: name,
                        file_path: Some(rel_path.clone()),
                        module_path: module_path.clone(),
                        span: Some(CodegraphSpan {
                            start: declaration.span.start,
                            end: declaration.span.end,
                        }),
                        facts,
                    });
                    document.push_edge(codegraph_edge(
                        &module_id,
                        &decl_id,
                        CodegraphEdgeKind::Contains,
                        Some(CodegraphSpan {
                            start: declaration.span.start,
                            end: declaration.span.end,
                        }),
                        "module_contains_declaration",
                    ));
                    document.push_edge(codegraph_edge(
                        &module_id,
                        &decl_id,
                        CodegraphEdgeKind::Defines,
                        Some(CodegraphSpan {
                            start: declaration.span.start,
                            end: declaration.span.end,
                        }),
                        "module_defines_declaration",
                    ));
                    if let Some(api_declaration) = checked_declaration {
                        push_api_member_codegraph(document, api_declaration, &decl_id, &rel_path, &module_path);
                    }
                    push_declaration_body_facts(
                        document,
                        declaration,
                        &decl_id,
                        &rel_path,
                        &module_path,
                        &module.source,
                    );
                }
            }
        }
    }
}

/// Context needed to turn parsed body syntax into source-backed graph facts.
#[derive(Clone)]
struct BodyFactContext<'a> {
    parent_id: &'a str,
    rel_path: &'a str,
    module_path: &'a [String],
    source: &'a str,
    owner: String,
}

impl<'a> BodyFactContext<'a> {
    fn with_owner(&self, owner: impl Into<String>) -> Self {
        Self {
            parent_id: self.parent_id,
            rel_path: self.rel_path,
            module_path: self.module_path,
            source: self.source,
            owner: owner.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PatternLabel {
    family: PatternFamily,
    label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PatternFamily {
    Constructor,
    StringLiteral,
    Literal,
}

impl PatternFamily {
    fn as_fact(self) -> &'static str {
        match self {
            Self::Constructor => "constructor",
            Self::StringLiteral => "string_literal",
            Self::Literal => "literal",
        }
    }
}

/// Add deterministic body-level graph facts contained by one declaration.
fn push_declaration_body_facts(
    document: &mut CodegraphDocument,
    declaration: &Spanned<Declaration>,
    declaration_id: &str,
    rel_path: &str,
    module_path: &[String],
    source: &str,
) {
    collect_declaration_body_facts(
        document,
        declaration,
        BodyFactContext {
            parent_id: declaration_id,
            rel_path,
            module_path,
            source,
            owner: "<module>".to_string(),
        },
    );
}

fn collect_declaration_body_facts(
    document: &mut CodegraphDocument,
    declaration: &Spanned<Declaration>,
    context: BodyFactContext<'_>,
) {
    match &declaration.node {
        Declaration::Import(_)
        | Declaration::Alias(_)
        | Declaration::Partial(_)
        | Declaration::TypeAlias(_)
        | Declaration::Docstring(_) => {}
        Declaration::Const(const_decl) => {
            collect_expr_body_facts(document, &const_decl.value, context.with_owner(&const_decl.name));
        }
        Declaration::Static(static_decl) => {
            collect_expr_body_facts(document, &static_decl.value, context.with_owner(&static_decl.name));
        }
        Declaration::Function(function) => {
            let context = context.with_owner(&function.name);
            collect_decorator_body_facts(document, &function.decorators, context.clone());
            collect_param_body_facts(document, &function.params, context.clone());
            collect_body_facts(document, &function.body, context);
        }
        Declaration::Model(model) => {
            collect_type_body_facts(
                document,
                &model.name,
                &model.decorators,
                &model.fields,
                &model.properties,
                &model.methods,
                context,
            );
        }
        Declaration::Class(class) => {
            collect_type_body_facts(
                document,
                &class.name,
                &class.decorators,
                &class.fields,
                &class.properties,
                &class.methods,
                context,
            );
        }
        Declaration::Trait(trait_decl) => {
            collect_decorator_body_facts(document, &trait_decl.decorators, context.with_owner(&trait_decl.name));
            for property in &trait_decl.properties {
                let owner = format!("{}.{}", trait_decl.name, property.node.name);
                if let Some(body) = &property.node.body {
                    collect_body_facts(document, body, context.with_owner(owner));
                }
            }
            for method in &trait_decl.methods {
                collect_method_body_facts(document, &trait_decl.name, method, context.clone());
            }
        }
        Declaration::Newtype(newtype_decl) => {
            collect_decorator_body_facts(
                document,
                &newtype_decl.decorators,
                context.with_owner(&newtype_decl.name),
            );
            for rebinding in &newtype_decl.rebindings {
                let owner = format!("{}.{}", newtype_decl.name, rebinding.node.name);
                collect_expr_body_facts(document, &rebinding.node.target, context.with_owner(owner));
            }
            for interop_edge in &newtype_decl.interop_edges {
                let owner = format!("{}.interop", newtype_decl.name);
                collect_expr_body_facts(document, &interop_edge.node.adapter, context.with_owner(owner));
            }
            for method in &newtype_decl.methods {
                collect_method_body_facts(document, &newtype_decl.name, method, context.clone());
            }
        }
        Declaration::Enum(enum_decl) => {
            collect_decorator_body_facts(document, &enum_decl.decorators, context.with_owner(&enum_decl.name));
            for method in &enum_decl.methods {
                collect_method_body_facts(document, &enum_decl.name, method, context.clone());
            }
        }
        Declaration::TestModule(test_module) => {
            let context = context.with_owner(format!("module {}", test_module.name));
            for nested in &test_module.body {
                collect_declaration_body_facts(document, nested, context.clone());
            }
        }
    }
}

fn collect_type_body_facts(
    document: &mut CodegraphDocument,
    type_name: &str,
    decorators: &[Spanned<crate::frontend::ast::Decorator>],
    fields: &[Spanned<crate::frontend::ast::FieldDecl>],
    properties: &[Spanned<crate::frontend::ast::PropertyDecl>],
    methods: &[Spanned<crate::frontend::ast::MethodDecl>],
    context: BodyFactContext<'_>,
) {
    collect_decorator_body_facts(document, decorators, context.with_owner(type_name));
    for field in fields {
        if let Some(default) = &field.node.default {
            let owner = format!("{type_name}.{}", field.node.name);
            collect_expr_body_facts(document, default, context.with_owner(owner));
        }
    }
    for property in properties {
        let owner = format!("{type_name}.{}", property.node.name);
        if let Some(body) = &property.node.body {
            collect_body_facts(document, body, context.with_owner(owner));
        }
    }
    for method in methods {
        collect_method_body_facts(document, type_name, method, context.clone());
    }
}

fn collect_method_body_facts(
    document: &mut CodegraphDocument,
    type_name: &str,
    method: &Spanned<crate::frontend::ast::MethodDecl>,
    context: BodyFactContext<'_>,
) {
    let context = context.with_owner(format!("{type_name}.{}", method.node.name));
    collect_decorator_body_facts(document, &method.node.decorators, context.clone());
    collect_param_body_facts(document, &method.node.params, context.clone());
    if let Some(body) = &method.node.body {
        collect_body_facts(document, body, context);
    }
}

fn collect_decorator_body_facts(
    document: &mut CodegraphDocument,
    decorators: &[Spanned<crate::frontend::ast::Decorator>],
    context: BodyFactContext<'_>,
) {
    for decorator in decorators {
        for arg in &decorator.node.args {
            match arg {
                DecoratorArg::Positional(value) => collect_expr_body_facts(document, value, context.clone()),
                DecoratorArg::Named(_, DecoratorArgValue::Expr(value)) => {
                    collect_expr_body_facts(document, value, context.clone());
                }
                DecoratorArg::Named(_, DecoratorArgValue::Type(_)) => {}
            }
        }
    }
}

fn collect_param_body_facts(
    document: &mut CodegraphDocument,
    params: &[Spanned<crate::frontend::ast::Param>],
    context: BodyFactContext<'_>,
) {
    for param in params {
        if let Some(default) = &param.node.default {
            collect_expr_body_facts(document, default, context.clone());
        }
    }
}

fn collect_body_facts(document: &mut CodegraphDocument, body: &[Spanned<Statement>], context: BodyFactContext<'_>) {
    for statement in body {
        collect_statement_body_facts(document, statement, context.clone());
    }
}

fn collect_statement_body_facts(
    document: &mut CodegraphDocument,
    statement: &Spanned<Statement>,
    context: BodyFactContext<'_>,
) {
    match &statement.node {
        Statement::Assignment(stmt) => collect_expr_body_facts(document, &stmt.value, context),
        Statement::FieldAssignment(stmt) => {
            collect_expr_body_facts(document, &stmt.object, context.clone());
            collect_expr_body_facts(document, &stmt.value, context);
        }
        Statement::IndexAssignment(stmt) => {
            collect_expr_body_facts(document, &stmt.object, context.clone());
            collect_expr_body_facts(document, &stmt.index, context.clone());
            collect_expr_body_facts(document, &stmt.value, context);
        }
        Statement::Return(Some(expr)) | Statement::Expr(expr) | Statement::Break(Some(expr)) => {
            collect_expr_body_facts(document, expr, context);
        }
        Statement::CompoundAssignment(stmt) => collect_expr_body_facts(document, &stmt.value, context),
        Statement::TupleUnpack(stmt) => collect_expr_body_facts(document, &stmt.value, context),
        Statement::TupleAssign(stmt) => {
            for target in &stmt.targets {
                collect_expr_body_facts(document, target, context.clone());
            }
            collect_expr_body_facts(document, &stmt.value, context);
        }
        Statement::ChainedAssignment(stmt) => collect_expr_body_facts(document, &stmt.value, context),
        Statement::Assert(assert_stmt) => collect_assert_body_facts(document, assert_stmt, context),
        Statement::Surface(surface_stmt) => match &surface_stmt.payload {
            crate::frontend::ast::SurfaceStmtPayload::KeywordArgs(args) => {
                for arg in args {
                    collect_expr_body_facts(document, arg, context.clone());
                }
            }
        },
        Statement::VocabBlock(block) => {
            collect_decorator_body_facts(document, &block.decorators, context.clone());
            for arg in &block.header_args {
                collect_expr_body_facts(document, arg, context.clone());
            }
            collect_body_facts(document, &block.body, context);
        }
        Statement::If(stmt) => {
            collect_condition_body_facts(document, &stmt.condition, context.clone());
            collect_body_facts(document, &stmt.then_body, context.clone());
            for (condition, body) in &stmt.elif_branches {
                collect_expr_body_facts(document, condition, context.clone());
                collect_body_facts(document, body, context.clone());
            }
            if let Some(else_body) = &stmt.else_body {
                collect_body_facts(document, else_body, context);
            }
        }
        Statement::Loop(stmt) => collect_body_facts(document, &stmt.body, context),
        Statement::While(stmt) => {
            collect_condition_body_facts(document, &stmt.condition, context.clone());
            collect_body_facts(document, &stmt.body, context);
        }
        Statement::For(stmt) => {
            collect_expr_body_facts(document, &stmt.iter, context.clone());
            collect_body_facts(document, &stmt.body, context);
        }
        Statement::Return(None) | Statement::Break(None) | Statement::Pass | Statement::Continue => {}
    }
}

fn collect_assert_body_facts(
    document: &mut CodegraphDocument,
    assert_stmt: &crate::frontend::ast::AssertStmt,
    context: BodyFactContext<'_>,
) {
    match &assert_stmt.kind {
        AssertKind::Condition(condition) => collect_expr_body_facts(document, condition, context.clone()),
        AssertKind::IsPattern { value, .. } => collect_expr_body_facts(document, value, context.clone()),
        AssertKind::Raises { call, .. } => collect_expr_body_facts(document, call, context.clone()),
    }
    if let Some(message) = &assert_stmt.message {
        collect_expr_body_facts(document, message, context);
    }
}

fn collect_condition_body_facts(document: &mut CodegraphDocument, condition: &Condition, context: BodyFactContext<'_>) {
    match condition {
        Condition::Expr(expr) | Condition::Let { value: expr, .. } => collect_expr_body_facts(document, expr, context),
    }
}

fn collect_expr_body_facts(document: &mut CodegraphDocument, expr: &Spanned<Expr>, context: BodyFactContext<'_>) {
    match &expr.node {
        Expr::Ident(_) | Expr::SelfExpr => push_reference_fact(document, expr.span, &expr.node, context),
        Expr::Literal(_) => {}
        Expr::Binary(left, _, right) | Expr::Index(left, right) => {
            collect_expr_body_facts(document, left, context.clone());
            collect_expr_body_facts(document, right, context);
        }
        Expr::Unary(_, operand) | Expr::Try(operand) | Expr::Paren(operand) => {
            collect_expr_body_facts(document, operand, context);
        }
        Expr::Field(operand, _) => {
            collect_expr_body_facts(document, operand, context.clone());
            push_reference_fact(document, expr.span, &expr.node, context);
        }
        Expr::Call(callee, _type_args, args) => {
            push_call_fact(document, expr.span, &expr.node, context.clone());
            collect_expr_body_facts(document, callee, context.clone());
            collect_call_arg_body_facts(document, args, context);
        }
        Expr::MethodCall(base, _, _type_args, args) => {
            push_call_fact(document, expr.span, &expr.node, context.clone());
            collect_expr_body_facts(document, base, context.clone());
            collect_call_arg_body_facts(document, args, context);
        }
        Expr::Constructor(_, args) => {
            push_call_fact(document, expr.span, &expr.node, context.clone());
            collect_call_arg_body_facts(document, args, context);
        }
        Expr::Partial(partial) => {
            collect_expr_body_facts(document, &partial.target, context.clone());
            for arg in &partial.args {
                collect_expr_body_facts(document, &arg.value, context.clone());
            }
        }
        Expr::Slice(base, slice) => {
            collect_expr_body_facts(document, base, context.clone());
            if let Some(start) = &slice.start {
                collect_expr_body_facts(document, start, context.clone());
            }
            if let Some(end) = &slice.end {
                collect_expr_body_facts(document, end, context.clone());
            }
            if let Some(step) = &slice.step {
                collect_expr_body_facts(document, step, context);
            }
        }
        Expr::Match(scrutinee, arms) => {
            push_match_dispatch_fact(document, expr.span, scrutinee, arms, context.clone());
            collect_expr_body_facts(document, scrutinee, context.clone());
            for arm in arms {
                if let Some(guard) = &arm.node.guard {
                    collect_expr_body_facts(document, guard, context.clone());
                }
                match &arm.node.body {
                    MatchBody::Expr(value) => collect_expr_body_facts(document, value, context.clone()),
                    MatchBody::Block(body) => collect_body_facts(document, body, context.clone()),
                }
            }
        }
        Expr::If(if_expr) => {
            collect_expr_body_facts(document, &if_expr.condition, context.clone());
            collect_body_facts(document, &if_expr.then_body, context.clone());
            if let Some(else_body) = &if_expr.else_body {
                collect_body_facts(document, else_body, context);
            }
        }
        Expr::Loop(loop_expr) => collect_body_facts(document, &loop_expr.body, context),
        Expr::ListComp(comp) => {
            collect_expr_body_facts(document, &comp.expr, context.clone());
            collect_expr_body_facts(document, &comp.iter, context.clone());
            if let Some(filter) = &comp.filter {
                collect_expr_body_facts(document, filter, context.clone());
            }
            collect_comprehension_clause_body_facts(document, &comp.clauses, context);
        }
        Expr::DictComp(comp) => {
            collect_expr_body_facts(document, &comp.key, context.clone());
            collect_expr_body_facts(document, &comp.value, context.clone());
            collect_expr_body_facts(document, &comp.iter, context.clone());
            if let Some(filter) = &comp.filter {
                collect_expr_body_facts(document, filter, context.clone());
            }
            collect_comprehension_clause_body_facts(document, &comp.clauses, context);
        }
        Expr::Generator(generator) => {
            collect_expr_body_facts(document, &generator.expr, context.clone());
            collect_comprehension_clause_body_facts(document, &generator.clauses, context);
        }
        Expr::Closure(params, body) => {
            collect_param_body_facts(document, params, context.clone());
            collect_expr_body_facts(document, body, context);
        }
        Expr::Tuple(items) | Expr::Set(items) => {
            for item in items {
                collect_expr_body_facts(document, item, context.clone());
            }
        }
        Expr::List(entries) => {
            for entry in entries {
                match entry {
                    crate::frontend::ast::ListEntry::Element(value)
                    | crate::frontend::ast::ListEntry::Spread(value) => {
                        collect_expr_body_facts(document, value, context.clone());
                    }
                }
            }
        }
        Expr::Dict(entries) => {
            for entry in entries {
                match entry {
                    DictEntry::Pair(key, value) => {
                        collect_expr_body_facts(document, key, context.clone());
                        collect_expr_body_facts(document, value, context.clone());
                    }
                    DictEntry::Spread(value) => collect_expr_body_facts(document, value, context.clone()),
                }
            }
        }
        Expr::FString(parts) => {
            for part in parts {
                if let FStringPart::Expr { expr, .. } = part {
                    collect_expr_body_facts(document, expr, context.clone());
                }
            }
        }
        Expr::Yield(Some(value)) => collect_expr_body_facts(document, value, context),
        Expr::Yield(None) => {}
        Expr::Range { start, end, .. } => {
            collect_expr_body_facts(document, start, context.clone());
            collect_expr_body_facts(document, end, context);
        }
        Expr::Surface(surface_expr) => match &surface_expr.payload {
            crate::frontend::ast::SurfaceExprPayload::PrefixUnary(value) => {
                collect_expr_body_facts(document, value, context);
            }
            crate::frontend::ast::SurfaceExprPayload::RaceFor(race) => {
                for arm in &race.arms {
                    collect_expr_body_facts(document, &arm.awaitable, context.clone());
                    match &arm.body {
                        RaceForBody::Expr(value) => collect_expr_body_facts(document, value, context.clone()),
                        RaceForBody::Block(body) => collect_body_facts(document, body, context.clone()),
                    }
                }
            }
            crate::frontend::ast::SurfaceExprPayload::LeadingDotPath { .. } => {}
            crate::frontend::ast::SurfaceExprPayload::ScopedGlyph { left, right, .. } => {
                collect_expr_body_facts(document, left, context.clone());
                collect_expr_body_facts(document, right, context);
            }
            crate::frontend::ast::SurfaceExprPayload::ScopedSymbolCall { args, .. } => {
                push_call_fact(document, expr.span, &expr.node, context.clone());
                collect_call_arg_body_facts(document, args, context);
            }
        },
    }
}

fn collect_comprehension_clause_body_facts(
    document: &mut CodegraphDocument,
    clauses: &[ComprehensionClause],
    context: BodyFactContext<'_>,
) {
    for clause in clauses {
        match clause {
            ComprehensionClause::For { iter, .. } | ComprehensionClause::If(iter) => {
                collect_expr_body_facts(document, iter, context.clone());
            }
        }
    }
}

fn collect_call_arg_body_facts(document: &mut CodegraphDocument, args: &[CallArg], context: BodyFactContext<'_>) {
    for arg in args {
        match arg {
            CallArg::Positional(value)
            | CallArg::Named(_, value)
            | CallArg::PositionalUnpack(value)
            | CallArg::KeywordUnpack(value) => collect_expr_body_facts(document, value, context.clone()),
        }
    }
}

fn push_match_dispatch_fact(
    document: &mut CodegraphDocument,
    span: Span,
    scrutinee: &Spanned<Expr>,
    arms: &[Spanned<crate::frontend::ast::MatchArm>],
    context: BodyFactContext<'_>,
) {
    let patterns = collect_match_patterns(arms);
    if patterns.len() < 2 {
        return;
    }
    let Some((domain_key, domain_label)) = domain_key_and_label(&scrutinee.node, &context.owner) else {
        return;
    };
    let pattern_labels = patterns.iter().map(|pattern| pattern.label.clone()).collect::<Vec<_>>();
    let pattern_families = patterns
        .iter()
        .map(|pattern| pattern.family.as_fact().to_string())
        .collect::<Vec<_>>();
    let mut facts = BTreeMap::new();
    facts.insert("domain_key".to_string(), domain_key);
    facts.insert("domain_label".to_string(), domain_label.clone());
    facts.insert("arm_count".to_string(), arms.len().to_string());
    facts.insert("explicit_pattern_count".to_string(), patterns.len().to_string());
    facts.insert("has_default_arm".to_string(), match_has_default_arm(arms).to_string());
    facts.insert("pattern_labels".to_string(), json_string_list(&pattern_labels));
    facts.insert("pattern_families".to_string(), json_string_list(&pattern_families));

    push_body_fact_node(
        document,
        BodyFactNodeInput {
            context,
            span,
            kind: CodegraphNodeKind::MatchDispatch,
            fact_kind: "match_dispatch",
            label: format!("match {domain_label}"),
            facts,
            contains_relation: "declaration_contains_match_dispatch",
        },
    );
}

fn push_call_fact(document: &mut CodegraphDocument, span: Span, expr: &Expr, context: BodyFactContext<'_>) {
    let Some((callee_key, callee_label)) = call_site_key_and_label(expr, &context.owner) else {
        return;
    };
    let mut facts = BTreeMap::new();
    facts.insert("callee_key".to_string(), callee_key.clone());
    facts.insert("callee_label".to_string(), callee_label.clone());
    let node_id = push_body_fact_node(
        document,
        BodyFactNodeInput {
            context,
            span,
            kind: CodegraphNodeKind::CallSite,
            fact_kind: "call_site",
            label: format!("call {callee_label}"),
            facts,
            contains_relation: "declaration_contains_call_site",
        },
    );
    push_body_external_target(
        document,
        BodyExternalTargetInput {
            source_id: &node_id,
            span,
            edge_kind: CodegraphEdgeKind::Calls,
            relation: "call_site_targets_callee",
            target_kind: "call_target",
            target_key: &callee_key,
            target_label: &callee_label,
        },
    );
}

fn push_reference_fact(document: &mut CodegraphDocument, span: Span, expr: &Expr, context: BodyFactContext<'_>) {
    let Some((reference_key, reference_label, reference_kind)) = reference_key_and_label(expr, &context.owner) else {
        return;
    };
    let mut facts = BTreeMap::new();
    facts.insert("reference_key".to_string(), reference_key.clone());
    facts.insert("reference_label".to_string(), reference_label.clone());
    facts.insert("reference_kind".to_string(), reference_kind.to_string());
    let node_id = push_body_fact_node(
        document,
        BodyFactNodeInput {
            context,
            span,
            kind: CodegraphNodeKind::Reference,
            fact_kind: "reference",
            label: reference_label.clone(),
            facts,
            contains_relation: "declaration_contains_reference",
        },
    );
    push_body_external_target(
        document,
        BodyExternalTargetInput {
            source_id: &node_id,
            span,
            edge_kind: CodegraphEdgeKind::References,
            relation: "reference_targets_symbol",
            target_kind: "reference_target",
            target_key: &reference_key,
            target_label: &reference_label,
        },
    );
}

struct BodyFactNodeInput<'a> {
    context: BodyFactContext<'a>,
    span: Span,
    kind: CodegraphNodeKind,
    fact_kind: &'static str,
    label: String,
    facts: BTreeMap<String, String>,
    contains_relation: &'static str,
}

fn push_body_fact_node(document: &mut CodegraphDocument, input: BodyFactNodeInput<'_>) -> String {
    let mut facts = input.facts;
    let (line, column) = line_column(input.context.source, input.span);
    facts.insert("body_fact_kind".to_string(), input.fact_kind.to_string());
    facts.insert("owner".to_string(), input.context.owner.clone());
    facts.insert("line".to_string(), line.to_string());
    facts.insert("column".to_string(), column.to_string());

    let node_id = format!(
        "body-fact:{}:{}:{}:{}-{}",
        input.fact_kind,
        stable_id_piece(&input.context.module_path.join("::")),
        stable_id_piece(&input.context.owner),
        input.span.start,
        input.span.end
    );
    let span = Some(CodegraphSpan {
        start: input.span.start,
        end: input.span.end,
    });
    document.push_node(CodegraphNode {
        id: node_id.clone(),
        kind: input.kind,
        label: input.label,
        file_path: Some(input.context.rel_path.to_string()),
        module_path: input.context.module_path.to_vec(),
        span,
        facts,
    });
    document.push_edge(codegraph_edge(
        input.context.parent_id,
        &node_id,
        CodegraphEdgeKind::Contains,
        span,
        input.contains_relation,
    ));
    node_id
}

struct BodyExternalTargetInput<'a> {
    source_id: &'a str,
    span: Span,
    edge_kind: CodegraphEdgeKind,
    relation: &'a str,
    target_kind: &'a str,
    target_key: &'a str,
    target_label: &'a str,
}

fn push_body_external_target(document: &mut CodegraphDocument, input: BodyExternalTargetInput<'_>) {
    let target_id = format!("external:{}:{}", input.target_kind, stable_id_piece(input.target_key));
    if !codegraph_document_has_node(document, &target_id) {
        let mut facts = BTreeMap::new();
        facts.insert("target_kind".to_string(), input.target_kind.to_string());
        facts.insert("target_key".to_string(), input.target_key.to_string());
        document.push_node(CodegraphNode {
            id: target_id.clone(),
            kind: CodegraphNodeKind::External,
            label: input.target_label.to_string(),
            file_path: None,
            module_path: Vec::new(),
            span: None,
            facts,
        });
    }
    document.push_edge(codegraph_edge(
        input.source_id,
        &target_id,
        input.edge_kind,
        Some(CodegraphSpan {
            start: input.span.start,
            end: input.span.end,
        }),
        input.relation,
    ));
}

fn collect_match_patterns(arms: &[Spanned<crate::frontend::ast::MatchArm>]) -> BTreeSet<PatternLabel> {
    let mut labels = BTreeSet::new();
    for arm in arms {
        collect_pattern_labels(&arm.node.pattern.node, &mut labels);
    }
    labels
}

fn collect_pattern_labels(pattern: &Pattern, labels: &mut BTreeSet<PatternLabel>) {
    match pattern {
        Pattern::Wildcard | Pattern::Binding(_) => {}
        Pattern::Literal(literal) => {
            let (family, label) = literal_label(literal);
            labels.insert(PatternLabel { family, label });
        }
        Pattern::Constructor(name, _args) => {
            labels.insert(PatternLabel {
                family: PatternFamily::Constructor,
                label: format!("{}(...)", incan_source_path_label(name)),
            });
        }
        Pattern::Tuple(items) | Pattern::Or(items) => {
            for item in items {
                collect_pattern_labels(&item.node, labels);
            }
        }
        Pattern::Group(inner) => collect_pattern_labels(&inner.node, labels),
    }
}

fn match_has_default_arm(arms: &[Spanned<crate::frontend::ast::MatchArm>]) -> bool {
    arms.iter().any(|arm| pattern_is_default(&arm.node.pattern.node))
}

fn pattern_is_default(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Wildcard | Pattern::Binding(_) => true,
        Pattern::Group(inner) => pattern_is_default(&inner.node),
        Pattern::Or(items) => items.iter().any(|item| pattern_is_default(&item.node)),
        Pattern::Literal(_) | Pattern::Constructor(_, _) | Pattern::Tuple(_) => false,
    }
}

fn literal_label(literal: &Literal) -> (PatternFamily, String) {
    match literal {
        Literal::String(value) => (PatternFamily::StringLiteral, format!("\"{}\"", compact(value))),
        Literal::Int(value) => (PatternFamily::Literal, value.repr.clone()),
        Literal::Float(value) => (PatternFamily::Literal, value.repr.clone()),
        Literal::Decimal(value) => (PatternFamily::Literal, value.repr.clone()),
        Literal::Bool(value) => (PatternFamily::Literal, value.to_string()),
        Literal::None => (PatternFamily::Literal, "None".to_string()),
        Literal::Bytes(value) => (PatternFamily::Literal, format!("bytes[{}]", value.len())),
    }
}

fn call_site_key_and_label(expr: &Expr, owner: &str) -> Option<(String, String)> {
    match expr {
        Expr::Call(callee, _, _) => {
            let (callee_key, callee_label) = domain_key_and_label(&callee.node, owner)?;
            Some((format!("call:{callee_key}"), format!("{callee_label}(...)")))
        }
        Expr::MethodCall(base, method, _, _) => {
            let key = match &base.node {
                Expr::SelfExpr => owner_type(owner)
                    .map(|type_name| format!("method:self:{type_name}:{method}"))
                    .unwrap_or_else(|| format!("method:self:{method}")),
                _ => format!("method:{method}"),
            };
            let label = match &base.node {
                Expr::SelfExpr => format!("self.{method}()"),
                _ => format!(".{method}()"),
            };
            Some((key, label))
        }
        Expr::Constructor(name, _) => Some((
            format!("constructor:{name}"),
            format!("{}(...)", incan_source_path_label(name)),
        )),
        Expr::Surface(surface_expr) => match &surface_expr.payload {
            crate::frontend::ast::SurfaceExprPayload::ScopedSymbolCall { symbol, .. } => {
                Some((format!("surface_symbol:{symbol}"), format!("{symbol}(...)")))
            }
            _ => None,
        },
        _ => None,
    }
}

fn incan_source_path_label(name: &str) -> String {
    name.replace("::", ".")
}

fn reference_key_and_label(expr: &Expr, owner: &str) -> Option<(String, String, &'static str)> {
    match expr {
        Expr::Ident(name) => Some((format!("ident:{name}"), name.clone(), "identifier")),
        Expr::SelfExpr => {
            let key = owner_type(owner)
                .map(|type_name| format!("self:{type_name}"))
                .unwrap_or_else(|| "self".to_string());
            let label = owner_type(owner)
                .map(|type_name| format!("self ({type_name})"))
                .unwrap_or_else(|| "self".to_string());
            Some((key, label, "self"))
        }
        Expr::Field(base, field) => {
            let key = match &base.node {
                Expr::SelfExpr => owner_type(owner)
                    .map(|type_name| format!("field:self:{type_name}:{field}"))
                    .unwrap_or_else(|| format!("field:self:{field}")),
                _ => format!("field:{field}"),
            };
            let label = match &base.node {
                Expr::SelfExpr => format!("self.{field}"),
                _ => format!(".{field}"),
            };
            Some((key, label, "field"))
        }
        _ => None,
    }
}

fn domain_key_and_label(expr: &Expr, owner: &str) -> Option<(String, String)> {
    match expr {
        Expr::SelfExpr => {
            let key = owner_type(owner)
                .map(|type_name| format!("self:{type_name}"))
                .unwrap_or_else(|| "self".to_string());
            let label = owner_type(owner)
                .map(|type_name| format!("self ({type_name})"))
                .unwrap_or_else(|| "self".to_string());
            Some((key, label))
        }
        Expr::Ident(name) => Some((format!("ident:{name}"), name.clone())),
        Expr::Field(base, field) => {
            let key = match &base.node {
                Expr::SelfExpr => owner_type(owner)
                    .map(|type_name| format!("field:self:{type_name}:{field}"))
                    .unwrap_or_else(|| format!("field:self:{field}")),
                _ => format!("field:{field}"),
            };
            let label = match &base.node {
                Expr::SelfExpr => format!("self.{field}"),
                _ => format!(".{field}"),
            };
            Some((key, label))
        }
        Expr::MethodCall(_, method, _, _) => Some((format!("method:{method}"), format!(".{method}()"))),
        Expr::Call(callee, _, _) => {
            let (callee_key, callee_label) = domain_key_and_label(&callee.node, owner)?;
            Some((format!("call:{callee_key}"), format!("{callee_label}(...)")))
        }
        _ => None,
    }
}

fn owner_type(owner: &str) -> Option<&str> {
    let (type_name, method_name) = owner.split_once('.')?;
    if type_name.is_empty() || method_name.is_empty() {
        None
    } else {
        Some(type_name)
    }
}

fn json_string_list(values: &[String]) -> String {
    serde_json::to_string(values).unwrap_or_else(|_| "[]".to_string())
}

fn line_column(source: &str, span: Span) -> (usize, usize) {
    let offset = span.start.min(source.len());
    let mut line = 1usize;
    let mut column = 1usize;
    for ch in source[..offset].chars() {
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

fn compact(value: &str) -> String {
    const LIMIT: usize = 48;
    if value.chars().count() <= LIMIT {
        return value.to_string();
    }
    let mut compacted = value.chars().take(LIMIT - 3).collect::<String>();
    compacted.push_str("...");
    compacted
}

/// Find a checked API declaration by exported name and exact source anchor.
fn find_api_metadata_declaration<'a>(
    metadata: &'a CheckedApiMetadata,
    name: &str,
    span: crate::frontend::ast::Span,
) -> Option<&'a ApiDeclaration> {
    metadata.declarations.iter().find(|declaration| {
        let anchor = api_declaration_anchor(declaration);
        api_declaration_name(declaration) == name && anchor.span.start == span.start && anchor.span.end == span.end
    })
}

/// Add RFC 048 API metadata facts to a source declaration node.
fn api_declaration_facts(declaration: &ApiDeclaration) -> BTreeMap<String, String> {
    let mut facts = BTreeMap::new();
    let anchor = api_declaration_anchor(declaration);
    facts.insert("checked_api_anchor_id".to_string(), anchor.id.clone());
    facts.insert(
        "checked_api_schema_version".to_string(),
        CHECKED_API_METADATA_SCHEMA_VERSION.to_string(),
    );
    facts.insert(
        "checked_api_kind".to_string(),
        api_declaration_kind(declaration).to_string(),
    );
    facts.insert(
        "checked_api_signature".to_string(),
        api_declaration_signature(declaration),
    );
    if let Some(summary) = api_declaration_doc_summary(declaration) {
        facts.insert("checked_api_doc_summary".to_string(), summary);
    }
    facts
}

/// Add checked API child nodes that are useful for package exploration but are not top-level declarations.
fn push_api_member_codegraph(
    document: &mut CodegraphDocument,
    declaration: &ApiDeclaration,
    declaration_id: &str,
    rel_path: &str,
    module_path: &[String],
) {
    match declaration {
        ApiDeclaration::Model(model) => {
            for field in &model.fields {
                push_api_field_member(document, declaration_id, &model.anchor, rel_path, module_path, field);
            }
            for method in &model.methods {
                push_api_method_member(document, declaration_id, &model.anchor, rel_path, module_path, method);
            }
        }
        ApiDeclaration::Class(class) => {
            for field in &class.fields {
                push_api_field_member(document, declaration_id, &class.anchor, rel_path, module_path, field);
            }
            for method in &class.methods {
                push_api_method_member(document, declaration_id, &class.anchor, rel_path, module_path, method);
            }
        }
        ApiDeclaration::Trait(trait_decl) => {
            for field in &trait_decl.requires {
                push_api_field_member(
                    document,
                    declaration_id,
                    &trait_decl.anchor,
                    rel_path,
                    module_path,
                    field,
                );
            }
            for method in &trait_decl.methods {
                push_api_method_member(
                    document,
                    declaration_id,
                    &trait_decl.anchor,
                    rel_path,
                    module_path,
                    method,
                );
            }
        }
        ApiDeclaration::Enum(enum_decl) => {
            for variant in &enum_decl.variants {
                let mut facts = BTreeMap::new();
                facts.insert("member_kind".to_string(), "enum_variant".to_string());
                facts.insert("checked_api_parent_anchor_id".to_string(), enum_decl.anchor.id.clone());
                if !variant.fields.is_empty() {
                    facts.insert(
                        "checked_api_signature".to_string(),
                        format!(
                            "{}({})",
                            variant.name,
                            variant
                                .fields
                                .iter()
                                .map(format_api_type_ref)
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    );
                }
                push_api_member_node(
                    document,
                    ApiMemberNodeInput {
                        declaration_id,
                        parent_anchor: &enum_decl.anchor,
                        rel_path,
                        module_path,
                        member_kind: "enum_variant",
                        label: &variant.name,
                        span: None,
                        facts,
                    },
                );
            }
            for alias in &enum_decl.variant_aliases {
                let mut facts = BTreeMap::new();
                facts.insert("member_kind".to_string(), "enum_variant_alias".to_string());
                facts.insert("checked_api_parent_anchor_id".to_string(), enum_decl.anchor.id.clone());
                facts.insert("target".to_string(), alias.target.clone());
                push_api_member_node(
                    document,
                    ApiMemberNodeInput {
                        declaration_id,
                        parent_anchor: &enum_decl.anchor,
                        rel_path,
                        module_path,
                        member_kind: "enum_variant_alias",
                        label: &alias.name,
                        span: None,
                        facts,
                    },
                );
            }
        }
        ApiDeclaration::Newtype(newtype) => {
            for method in &newtype.methods {
                push_api_method_member(document, declaration_id, &newtype.anchor, rel_path, module_path, method);
            }
        }
        ApiDeclaration::Function(_)
        | ApiDeclaration::TypeAlias(_)
        | ApiDeclaration::Const(_)
        | ApiDeclaration::Static(_)
        | ApiDeclaration::Alias(_)
        | ApiDeclaration::Partial(_) => {}
    }
}

/// Add a field-like API member node.
fn push_api_field_member(
    document: &mut CodegraphDocument,
    declaration_id: &str,
    parent_anchor: &SourceAnchor,
    rel_path: &str,
    module_path: &[String],
    field: &FieldExport,
) {
    let mut facts = BTreeMap::new();
    facts.insert("member_kind".to_string(), "field".to_string());
    facts.insert("checked_api_parent_anchor_id".to_string(), parent_anchor.id.clone());
    facts.insert("checked_api_type".to_string(), format_api_type_ref(&field.ty));
    facts.insert("has_default".to_string(), field.has_default.to_string());
    if let Some(alias) = &field.alias {
        facts.insert("alias".to_string(), alias.clone());
    }
    if let Some(description) = &field.description {
        facts.insert("description".to_string(), description.clone());
    }
    push_api_member_node(
        document,
        ApiMemberNodeInput {
            declaration_id,
            parent_anchor,
            rel_path,
            module_path,
            member_kind: "field",
            label: &field.name,
            span: None,
            facts,
        },
    );
}

/// Add a method API member node.
fn push_api_method_member(
    document: &mut CodegraphDocument,
    declaration_id: &str,
    parent_anchor: &SourceAnchor,
    rel_path: &str,
    module_path: &[String],
    method: &ApiMethod,
) {
    let mut facts = BTreeMap::new();
    facts.insert("member_kind".to_string(), "method".to_string());
    facts.insert("checked_api_anchor_id".to_string(), method.anchor.id.clone());
    facts.insert("checked_api_parent_anchor_id".to_string(), parent_anchor.id.clone());
    facts.insert("checked_api_signature".to_string(), api_method_signature(method));
    if let Some(summary) = compact_api_doc_summary(
        method
            .docstring_sections
            .as_ref()
            .and_then(|docstring| docstring.summary.as_deref()),
    ) {
        facts.insert("checked_api_doc_summary".to_string(), summary);
    }
    push_api_member_node(
        document,
        ApiMemberNodeInput {
            declaration_id,
            parent_anchor,
            rel_path,
            module_path,
            member_kind: "method",
            label: &method.name,
            span: Some(CodegraphSpan {
                start: method.anchor.span.start,
                end: method.anchor.span.end,
            }),
            facts,
        },
    );
}

/// Inputs for adding one checked API member graph node.
struct ApiMemberNodeInput<'a> {
    declaration_id: &'a str,
    parent_anchor: &'a SourceAnchor,
    rel_path: &'a str,
    module_path: &'a [String],
    member_kind: &'a str,
    label: &'a str,
    span: Option<CodegraphSpan>,
    facts: BTreeMap<String, String>,
}

/// Add one checked API member node and containment edge.
fn push_api_member_node(document: &mut CodegraphDocument, input: ApiMemberNodeInput<'_>) {
    let member_id = format!(
        "api-member:{}:{}:{}",
        stable_id_piece(&input.parent_anchor.id),
        input.member_kind,
        stable_id_piece(input.label)
    );
    document.push_node(CodegraphNode {
        id: member_id.clone(),
        kind: CodegraphNodeKind::ApiMember,
        label: input.label.to_string(),
        file_path: Some(input.rel_path.to_string()),
        module_path: input.module_path.to_vec(),
        span: input.span,
        facts: input.facts,
    });
    document.push_edge(codegraph_edge(
        input.declaration_id,
        &member_id,
        CodegraphEdgeKind::Contains,
        input.span,
        "declaration_contains_api_member",
    ));
}

/// Return the source anchor attached to a checked API declaration.
fn api_declaration_anchor(declaration: &ApiDeclaration) -> &SourceAnchor {
    match declaration {
        ApiDeclaration::Function(function) => &function.anchor,
        ApiDeclaration::Model(model) => &model.anchor,
        ApiDeclaration::Class(class) => &class.anchor,
        ApiDeclaration::Trait(trait_decl) => &trait_decl.anchor,
        ApiDeclaration::Enum(enum_decl) => &enum_decl.anchor,
        ApiDeclaration::Newtype(newtype) => &newtype.anchor,
        ApiDeclaration::TypeAlias(alias) => &alias.anchor,
        ApiDeclaration::Const(konst) => &konst.anchor,
        ApiDeclaration::Static(static_decl) => &static_decl.anchor,
        ApiDeclaration::Alias(alias) => &alias.anchor,
        ApiDeclaration::Partial(partial) => &partial.anchor,
    }
}

/// Return the exported source name attached to a checked API declaration.
fn api_declaration_name(declaration: &ApiDeclaration) -> &str {
    match declaration {
        ApiDeclaration::Function(function) => &function.name,
        ApiDeclaration::Model(model) => &model.name,
        ApiDeclaration::Class(class) => &class.name,
        ApiDeclaration::Trait(trait_decl) => &trait_decl.name,
        ApiDeclaration::Enum(enum_decl) => &enum_decl.name,
        ApiDeclaration::Newtype(newtype) => &newtype.name,
        ApiDeclaration::TypeAlias(alias) => &alias.name,
        ApiDeclaration::Const(konst) => &konst.name,
        ApiDeclaration::Static(static_decl) => &static_decl.name,
        ApiDeclaration::Alias(alias) => &alias.name,
        ApiDeclaration::Partial(partial) => &partial.name,
    }
}

/// Return the RFC 048 declaration kind label used by graph facts.
fn api_declaration_kind(declaration: &ApiDeclaration) -> &'static str {
    match declaration {
        ApiDeclaration::Function(_) => "function",
        ApiDeclaration::Model(_) => "model",
        ApiDeclaration::Class(_) => "class",
        ApiDeclaration::Trait(_) => "trait",
        ApiDeclaration::Enum(_) => "enum",
        ApiDeclaration::Newtype(newtype) if newtype.is_rusttype => "rusttype",
        ApiDeclaration::Newtype(_) => "newtype",
        ApiDeclaration::TypeAlias(_) => "type_alias",
        ApiDeclaration::Const(_) => "const",
        ApiDeclaration::Static(_) => "static",
        ApiDeclaration::Alias(_) => "alias",
        ApiDeclaration::Partial(_) => "partial",
    }
}

/// Return a compact source-like signature for checked API graph facts.
fn api_declaration_signature(declaration: &ApiDeclaration) -> String {
    match declaration {
        ApiDeclaration::Function(function) => {
            let prefix = if function.is_async { "pub async def" } else { "pub def" };
            format!(
                "{prefix} {}({}) -> {}",
                function.name,
                format_api_params(&function.params),
                format_api_type_ref(&function.return_type)
            )
        }
        ApiDeclaration::Model(model) => format!("pub model {}", model.name),
        ApiDeclaration::Class(class) => format!("pub class {}", class.name),
        ApiDeclaration::Trait(trait_decl) => format!("pub trait {}", trait_decl.name),
        ApiDeclaration::Enum(enum_decl) => format!("pub enum {}", enum_decl.name),
        ApiDeclaration::Newtype(newtype) => {
            let keyword = if newtype.is_rusttype { "rusttype" } else { "newtype" };
            format!(
                "pub {keyword} {} = {}",
                newtype.name,
                format_api_type_ref(&newtype.underlying)
            )
        }
        ApiDeclaration::TypeAlias(alias) => {
            format!(
                "pub type {} = {}",
                alias.name,
                format_api_type_ref(&alias.type_alias.target)
            )
        }
        ApiDeclaration::Const(konst) => format!("pub const {}: {}", konst.name, format_api_type_ref(&konst.ty)),
        ApiDeclaration::Static(static_decl) => {
            format!(
                "pub static {}: {}",
                static_decl.name,
                format_api_type_ref(&static_decl.ty)
            )
        }
        ApiDeclaration::Alias(alias) => format!("pub {} = alias {}", alias.name, alias.target_path.join("::")),
        ApiDeclaration::Partial(partial) => {
            let prefix = if partial.is_async {
                "pub async partial"
            } else {
                "pub partial"
            };
            format!(
                "{prefix} {}({}) -> {}",
                partial.name,
                format_api_params(&partial.params),
                format_api_type_ref(&partial.return_type)
            )
        }
    }
}

/// Return a compact source-like signature for a checked API method.
fn api_method_signature(method: &ApiMethod) -> String {
    let prefix = if method.is_async { "async def" } else { "def" };
    let mut params = Vec::new();
    if let Some(receiver) = &method.receiver {
        params.push(match receiver {
            ReceiverExport::Immutable => "self".to_string(),
            ReceiverExport::Mutable => "mut self".to_string(),
        });
    }
    params.extend(method.params.iter().map(format_api_param));
    format!(
        "{prefix} {}({}) -> {}",
        method.name,
        params.join(", "),
        format_api_type_ref(&method.return_type)
    )
}

/// Return the parsed docstring summary attached to a checked API declaration.
fn api_declaration_doc_summary(declaration: &ApiDeclaration) -> Option<String> {
    let summary = match declaration {
        ApiDeclaration::Function(function) => function
            .docstring_sections
            .as_ref()
            .and_then(|docstring| docstring.summary.as_deref()),
        ApiDeclaration::Model(model) => model
            .docstring_sections
            .as_ref()
            .and_then(|docstring| docstring.summary.as_deref()),
        ApiDeclaration::Class(class) => class
            .docstring_sections
            .as_ref()
            .and_then(|docstring| docstring.summary.as_deref()),
        ApiDeclaration::Trait(trait_decl) => trait_decl
            .docstring_sections
            .as_ref()
            .and_then(|docstring| docstring.summary.as_deref()),
        ApiDeclaration::Enum(enum_decl) => enum_decl
            .docstring_sections
            .as_ref()
            .and_then(|docstring| docstring.summary.as_deref()),
        ApiDeclaration::Newtype(newtype) => newtype
            .docstring_sections
            .as_ref()
            .and_then(|docstring| docstring.summary.as_deref()),
        ApiDeclaration::TypeAlias(_)
        | ApiDeclaration::Const(_)
        | ApiDeclaration::Static(_)
        | ApiDeclaration::Alias(_)
        | ApiDeclaration::Partial(_) => None,
    };
    compact_api_doc_summary(summary)
}

/// Keep graph doc facts small enough for indexing and previews.
fn compact_api_doc_summary(summary: Option<&str>) -> Option<String> {
    let trimmed = summary?.trim();
    if trimmed.is_empty() {
        return None;
    }
    let paragraph = trimmed.split("\n\n").next().unwrap_or(trimmed);
    let mut compact = paragraph.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() > 240 {
        compact = compact.chars().take(237).collect::<String>();
        compact.push_str("...");
    }
    Some(compact)
}

/// Add an import declaration and its textual target as graph facts.
fn push_import_codegraph(
    document: &mut CodegraphDocument,
    import: &ImportDecl,
    span: crate::frontend::ast::Span,
    module_id: &str,
    module_path: &[String],
    rel_path: &str,
) {
    let import_label = import_label(import);
    let import_id = format!("import:{}:{}:{}", module_path.join("::"), span.start, span.end);
    let mut facts = BTreeMap::new();
    facts.insert(
        "visibility".to_string(),
        visibility_label(import.visibility).to_string(),
    );
    facts.insert("target".to_string(), import_label.clone());
    document.push_node(CodegraphNode {
        id: import_id.clone(),
        kind: CodegraphNodeKind::Import,
        label: import_label.clone(),
        file_path: Some(rel_path.to_string()),
        module_path: module_path.to_vec(),
        span: Some(CodegraphSpan {
            start: span.start,
            end: span.end,
        }),
        facts,
    });
    document.push_edge(codegraph_edge(
        module_id,
        &import_id,
        CodegraphEdgeKind::Contains,
        Some(CodegraphSpan {
            start: span.start,
            end: span.end,
        }),
        "module_contains_import",
    ));

    let external_id = format!("external:{}", stable_id_piece(&import_label));
    if !codegraph_document_has_node(document, &external_id) {
        document.push_node(CodegraphNode {
            id: external_id.clone(),
            kind: CodegraphNodeKind::External,
            label: import_label,
            file_path: None,
            module_path: Vec::new(),
            span: None,
            facts: BTreeMap::new(),
        });
    }
    document.push_edge(codegraph_edge(
        &import_id,
        &external_id,
        CodegraphEdgeKind::Imports,
        Some(CodegraphSpan {
            start: span.start,
            end: span.end,
        }),
        "import_targets_external",
    ));
}

/// Return the codegraph kind, name, and visibility for declarations with stable source identity.
fn declaration_codegraph_info(declaration: &Declaration) -> Option<(&'static str, String, &'static str)> {
    match declaration {
        Declaration::Const(ConstDecl { visibility, name, .. }) => {
            Some(("const", name.clone(), visibility_label(*visibility)))
        }
        Declaration::Static(StaticDecl { visibility, name, .. }) => {
            Some(("static", name.clone(), visibility_label(*visibility)))
        }
        Declaration::Model(ModelDecl { visibility, name, .. }) => {
            Some(("model", name.clone(), visibility_label(*visibility)))
        }
        Declaration::Class(ClassDecl { visibility, name, .. }) => {
            Some(("class", name.clone(), visibility_label(*visibility)))
        }
        Declaration::Trait(TraitDecl { visibility, name, .. }) => {
            Some(("trait", name.clone(), visibility_label(*visibility)))
        }
        Declaration::Alias(AliasDecl { visibility, name, .. }) => {
            Some(("alias", name.clone(), visibility_label(*visibility)))
        }
        Declaration::Partial(PartialDecl { visibility, name, .. }) => {
            Some(("partial", name.clone(), visibility_label(*visibility)))
        }
        Declaration::TypeAlias(TypeAliasDecl { visibility, name, .. }) => {
            Some(("type_alias", name.clone(), visibility_label(*visibility)))
        }
        Declaration::Newtype(NewtypeDecl {
            visibility,
            name,
            is_rusttype,
            ..
        }) => {
            let kind = if *is_rusttype { "rusttype" } else { "newtype" };
            Some((kind, name.clone(), visibility_label(*visibility)))
        }
        Declaration::Enum(EnumDecl { visibility, name, .. }) => {
            Some(("enum", name.clone(), visibility_label(*visibility)))
        }
        Declaration::Function(FunctionDecl { visibility, name, .. }) => {
            Some(("function", name.clone(), visibility_label(*visibility)))
        }
        Declaration::TestModule(TestModuleDecl { name, .. }) => Some(("test_module", name.clone(), "private")),
        Declaration::Import(_) | Declaration::Docstring(_) => None,
    }
}

/// Format one import declaration as a stable target label.
fn import_label(import: &ImportDecl) -> String {
    match &import.kind {
        ImportKind::Module(path) => {
            let mut label = format_import_path(path);
            if let Some(alias) = &import.alias {
                label.push_str(" as ");
                label.push_str(alias);
            }
            label
        }
        ImportKind::From { module, items } => {
            format!(
                "from {} import {}",
                format_import_path(module),
                format_import_items(items)
            )
        }
        ImportKind::PubLibrary { library } => format!("pub::{library}"),
        ImportKind::PubFrom { library, items } => {
            format!("from pub::{library} import {}", format_import_items(items))
        }
        ImportKind::Python(module) => format!("python:{module}"),
        ImportKind::RustCrate {
            crate_name,
            path,
            version,
            features,
        } => format_rust_import(crate_name, path, version.as_deref(), features),
        ImportKind::RustFrom {
            crate_name,
            path,
            version,
            features,
            items,
        } => format!(
            "from {} import {}",
            format_rust_import(crate_name, path, version.as_deref(), features),
            format_import_items(items)
        ),
    }
}

/// Format one source import path.
fn format_import_path(path: &ImportPath) -> String {
    path.to_rust_path()
}

/// Format one imported item list.
fn format_import_items(items: &[ImportItem]) -> String {
    items
        .iter()
        .map(|item| {
            if let Some(alias) = &item.alias {
                format!("{} as {}", item.name, alias)
            } else {
                item.name.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Format one Rust-origin import path with optional dependency metadata.
fn format_rust_import(crate_name: &str, path: &[String], version: Option<&str>, features: &[String]) -> String {
    let mut parts = vec!["rust".to_string(), crate_name.to_string()];
    parts.extend(path.iter().cloned());
    let mut label = parts.join("::");
    if let Some(version) = version {
        label.push_str(&format!("@{version}"));
    }
    if !features.is_empty() {
        label.push_str("[features=");
        label.push_str(&features.join(","));
        label.push(']');
    }
    label
}

/// Return a stable visibility label.
fn visibility_label(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Private => "private",
        Visibility::Public => "public",
    }
}

/// Build one edge with a deterministic id.
fn codegraph_edge(
    source_id: &str,
    target_id: &str,
    kind: CodegraphEdgeKind,
    span: Option<CodegraphSpan>,
    relation: &str,
) -> CodegraphEdge {
    let mut facts = BTreeMap::new();
    facts.insert("relation".to_string(), relation.to_string());
    CodegraphEdge {
        id: format!(
            "edge:{}:{relation}:{}",
            stable_id_piece(source_id),
            stable_id_piece(target_id)
        ),
        kind,
        source_id: source_id.to_string(),
        target_id: target_id.to_string(),
        span,
        facts,
    }
}

/// Render a path relative to the project root when possible so exported facts are shareable.
fn codegraph_path(path: &Path, project_root: &Path) -> String {
    path.strip_prefix(project_root).unwrap_or(path).display().to_string()
}

/// Sanitize a string for use inside stable ids.
fn stable_id_piece(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' | '.' => ch,
            _ => '_',
        })
        .collect()
}

/// Return whether a document already contains a node id.
fn codegraph_document_has_node(document: &CodegraphDocument, id: &str) -> bool {
    document.nodes.iter().any(|node| node.id == id)
}

/// Type-check a metadata entry path and collect checked API metadata for all local modules.
fn collect_api_metadata_package(path: &Path) -> CliResult<CheckedApiMetadataPackage> {
    let entry_path = resolve_metadata_entry_path(path)?;
    let entry_path_string = entry_path.to_string_lossy();
    let modules = collect_modules(&entry_path_string)?;
    let project_root = resolve_project_root(&entry_path);
    let manifest = ProjectManifest::discover(&project_root).map_err(|error| CliError::failure(error.to_string()))?;
    let declared = manifest.as_ref().map(ProjectManifest::declared_rust_crate_names);
    let library_manifest_index = manifest
        .as_ref()
        .map(LibraryManifestIndex::from_project_manifest)
        .unwrap_or_default();
    let module_idx_by_key = module_key_index(&modules);
    let mut all_errors = String::new();
    let mut metadata_modules = Vec::new();

    for (idx, module) in modules.iter().enumerate() {
        let deps_for_module = imported_module_deps_for_with_index(&modules, idx, &module_idx_by_key);
        let mut checker = typechecker::TypeChecker::new();
        if let Some(names) = declared.clone() {
            checker.set_declared_crate_names(names);
        }
        checker.set_library_manifest_index(library_manifest_index.clone());

        match checker.check_with_imports(&module.ast, &deps_for_module) {
            Ok(()) => {
                for warn in checker.warnings() {
                    eprint!(
                        "{}",
                        diagnostics::format_error(module.file_path.to_string_lossy().as_ref(), &module.source, warn)
                    );
                }
                metadata_modules.push(collect_checked_api_metadata(
                    &module.ast,
                    &checker,
                    metadata_module_path(module, &entry_path),
                ));
            }
            Err(errs) => {
                for err in &errs {
                    all_errors.push_str(&diagnostics::format_error(
                        module.file_path.to_string_lossy().as_ref(),
                        &module.source,
                        err,
                    ));
                }
            }
        }
    }

    if !all_errors.is_empty() {
        return Err(CliError::failure(all_errors.trim_end()));
    }

    for diagnostic in validate_checked_api_docstrings(&metadata_modules) {
        if let Some((module, _)) = modules
            .iter()
            .zip(metadata_modules.iter())
            .find(|(_, metadata)| metadata.module_path == diagnostic.module_path)
        {
            all_errors.push_str(&diagnostics::format_error(
                module.file_path.to_string_lossy().as_ref(),
                &module.source,
                &diagnostic.error,
            ));
        } else {
            all_errors.push_str(&diagnostic.error.message);
            all_errors.push('\n');
        }
    }

    if !all_errors.is_empty() {
        return Err(CliError::failure(all_errors.trim_end()));
    }

    Ok(CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: manifest.as_ref().and_then(checked_api_package_identity),
        modules: metadata_modules,
    })
}

/// Extract checked API package identity from the project manifest when the manifest declares a non-empty name.
fn checked_api_package_identity(manifest: &ProjectManifest) -> Option<CheckedApiPackageIdentity> {
    let project = manifest.project.as_ref()?;
    let name = project.name.as_ref()?.trim();
    if name.is_empty() {
        return None;
    }
    Some(CheckedApiPackageIdentity {
        name: name.to_string(),
        version: project
            .version
            .as_ref()
            .map(|version| version.trim())
            .filter(|version| !version.is_empty())
            .map(str::to_string),
    })
}

/// Return the logical module path used in metadata for one parsed module.
fn metadata_module_path(module: &ParsedModule, entry_path: &Path) -> Vec<String> {
    if module.file_path == entry_path
        && let Some(stem) = entry_path.file_stem().and_then(|stem| stem.to_str())
    {
        return vec![stem.to_string()];
    }
    module.path_segments.clone()
}

/// Return the logical module path used in codegraph facts for one parsed module.
fn codegraph_module_path(module: &ParsedModule, entry_path: Option<&Path>) -> Vec<String> {
    if let Some(entry_path) = entry_path
        && module.file_path == entry_path
        && let Some(stem) = entry_path.file_stem().and_then(|stem| stem.to_str())
    {
        return vec![stem.to_string()];
    }
    module.path_segments.clone()
}

/// Resolve a file or project directory to the source file used as the metadata entry point.
fn resolve_metadata_entry_path(path: &Path) -> CliResult<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))?
            .join(path)
    };

    if absolute.is_file() {
        return Ok(absolute);
    }
    if absolute.is_dir() {
        let lib = absolute.join("src").join("lib.incn");
        if lib.is_file() {
            return Ok(lib);
        }
        let main = absolute.join("src").join("main.incn");
        if main.is_file() {
            return Ok(main);
        }
        return Err(CliError::failure(format!(
            "tool export requires an Incan source file, or a project directory with `src/lib.incn` or `src/main.incn`: {}",
            absolute.display()
        )));
    }

    Err(CliError::failure(format!(
        "tool export path does not exist: {}",
        absolute.display()
    )))
}

#[derive(Debug)]
struct DoctorReport {
    version: &'static str,
    current_exe: Option<PathBuf>,
    cwd: Option<PathBuf>,
    path_incan: ToolPath,
    path_incan_lsp: ToolPath,
    cargo_bin_incan: CargoBinEntry,
    cargo_bin_incan_lsp: CargoBinEntry,
    offline_readiness: OfflineReadiness,
}

impl DoctorReport {
    /// Collect local process, PATH, and cargo-bin state for the doctor report.
    fn collect() -> Self {
        let cwd = env::current_dir().ok();
        Self {
            version: crate::version::INCAN_VERSION,
            current_exe: env::current_exe().ok(),
            cwd: cwd.clone(),
            path_incan: ToolPath::resolve("incan"),
            path_incan_lsp: ToolPath::resolve("incan-lsp"),
            cargo_bin_incan: CargoBinEntry::from_home("incan"),
            cargo_bin_incan_lsp: CargoBinEntry::from_home("incan-lsp"),
            offline_readiness: OfflineReadiness::collect(cwd.as_deref()),
        }
    }

    /// Print the doctor report as stable, human-readable text.
    fn print_text(&self) {
        println!("Incan tools doctor");
        println!("version: {}", self.version);
        println!("current_exe: {}", display_option_path(&self.current_exe));
        println!("cwd: {}", display_option_path(&self.cwd));
        println!();
        self.path_incan.print_text("PATH incan");
        self.path_incan_lsp.print_text("PATH incan-lsp");
        println!();
        self.cargo_bin_incan.print_text("~/.cargo/bin/incan");
        self.cargo_bin_incan_lsp.print_text("~/.cargo/bin/incan-lsp");
        println!();
        println!("editor setup:");
        println!("  leave incan.lsp.path and incan.compiler.path empty to use workspace discovery or PATH");
        println!(
            "  if either setting is explicit, use a literal executable path; shell syntax like $HOME or ~ is not expanded"
        );
        println!("  after rebuilding or changing paths, reload VS Code/Cursor so it starts a fresh incan-lsp process");
        println!();
        self.offline_readiness.print_text();
    }

    /// Print the doctor report as pretty JSON for editor integrations and issue templates.
    fn print_json(&self) -> CliResult<()> {
        let value = json!({
            "version": self.version,
            "current_exe": self.current_exe.as_deref().map(path_to_string),
            "cwd": self.cwd.as_deref().map(path_to_string),
            "path": {
                "incan": self.path_incan.as_json(),
                "incan_lsp": self.path_incan_lsp.as_json(),
            },
            "cargo_bin": {
                "incan": self.cargo_bin_incan.as_json(),
                "incan_lsp": self.cargo_bin_incan_lsp.as_json(),
            },
            "editor_setup": {
                "recommended_lsp_path": "",
                "recommended_compiler_path": "",
                "literal_path_settings": true,
                "reload_after_rebuild": true
            },
            "offline_readiness": self.offline_readiness.as_json()
        });
        let output = serde_json::to_string_pretty(&value)
            .map_err(|error| CliError::failure(format!("failed to serialize doctor report: {error}")))?;
        println!("{output}");
        Ok(())
    }
}

#[derive(Debug)]
struct ToolPath {
    command: String,
    resolved: Option<PathBuf>,
    executable: bool,
}

impl ToolPath {
    /// Resolve one command name through the current process PATH.
    fn resolve(command: &str) -> Self {
        let resolved = find_on_path(command);
        let executable = resolved.as_deref().is_some_and(is_executable_file);
        Self {
            command: command.to_string(),
            resolved,
            executable,
        }
    }

    /// Print one PATH resolution entry.
    fn print_text(&self, label: &str) {
        println!("{label}:");
        println!("  command: {}", self.command);
        println!("  resolved: {}", display_option_path(&self.resolved));
        println!("  executable: {}", self.executable);
    }

    /// Convert one PATH resolution entry into JSON.
    fn as_json(&self) -> serde_json::Value {
        json!({
            "command": self.command,
            "resolved": self.resolved.as_deref().map(path_to_string),
            "executable": self.executable,
        })
    }
}

#[derive(Debug)]
struct CargoBinEntry {
    path: Option<PathBuf>,
    exists: bool,
    symlink_target: Option<PathBuf>,
    executable: bool,
}

impl CargoBinEntry {
    /// Inspect one expected `~/.cargo/bin` tool entry.
    fn from_home(binary: &str) -> Self {
        let path = home_dir().map(|home| home.join(".cargo").join("bin").join(binary));
        let exists = path.as_deref().is_some_and(Path::exists);
        let symlink_target = path.as_deref().and_then(|path| fs::read_link(path).ok());
        let executable = path.as_deref().is_some_and(is_executable_file);
        Self {
            path,
            exists,
            symlink_target,
            executable,
        }
    }

    /// Print one cargo-bin entry.
    fn print_text(&self, label: &str) {
        println!("{label}:");
        println!("  path: {}", display_option_path(&self.path));
        println!("  exists: {}", self.exists);
        println!("  symlink_target: {}", display_option_path(&self.symlink_target));
        println!("  executable: {}", self.executable);
    }

    /// Convert one cargo-bin entry into JSON.
    fn as_json(&self) -> serde_json::Value {
        json!({
            "path": self.path.as_deref().map(path_to_string),
            "exists": self.exists,
            "symlink_target": self.symlink_target.as_deref().map(path_to_string),
            "executable": self.executable,
        })
    }
}

#[derive(Debug)]
struct OfflineReadiness {
    advisory_only: bool,
    status: OfflineReadinessStatus,
    cargo: CargoCommandInfo,
    cargo_home: CargoHomeInfo,
    registry_cache: CachePathHint,
    registry_index: CachePathHint,
    registry_src: CachePathHint,
    git_checkouts: CachePathHint,
    git_db: CachePathHint,
    cargo_config: CargoConfigHints,
    next_steps: Vec<String>,
}

impl OfflineReadiness {
    /// Collect advisory local signals without network access, resolution, or builds.
    fn collect(cwd: Option<&Path>) -> Self {
        let cargo = CargoCommandInfo::collect();
        let cargo_home = CargoHomeInfo::collect();
        let registry_cache =
            CachePathHint::from_optional_path(cargo_home.path.as_deref().map(|path| path.join("registry/cache")));
        let registry_index =
            CachePathHint::from_optional_path(cargo_home.path.as_deref().map(|path| path.join("registry/index")));
        let registry_src =
            CachePathHint::from_optional_path(cargo_home.path.as_deref().map(|path| path.join("registry/src")));
        let git_checkouts =
            CachePathHint::from_optional_path(cargo_home.path.as_deref().map(|path| path.join("git/checkouts")));
        let git_db = CachePathHint::from_optional_path(cargo_home.path.as_deref().map(|path| path.join("git/db")));
        let cargo_config = CargoConfigHints::collect(cwd, cargo_home.path.as_deref());
        let status = OfflineReadinessStatus::from_signals(
            &cargo,
            &cargo_home,
            [&registry_cache, &registry_index, &registry_src, &git_checkouts, &git_db],
            &cargo_config,
        );
        let next_steps = build_offline_next_steps(
            &cargo,
            &cargo_home,
            [&registry_cache, &registry_index, &registry_src, &git_checkouts, &git_db],
            &cargo_config,
        );

        Self {
            advisory_only: true,
            status,
            cargo,
            cargo_home,
            registry_cache,
            registry_index,
            registry_src,
            git_checkouts,
            git_db,
            cargo_config,
            next_steps,
        }
    }

    /// Print the advisory offline-readiness section.
    fn print_text(&self) {
        println!("offline readiness:");
        println!("  status: {}", self.status.as_str());
        println!("  advisory_only: {}", self.advisory_only);
        println!("  note: advisory local signals only; Cargo and RFC 020 policy flags remain authoritative");
        println!("  cargo:");
        println!("    command: {}", self.cargo.command);
        println!("    available: {}", self.cargo.available);
        println!("    version: {}", self.cargo.version.as_deref().unwrap_or("(unknown)"));
        println!("    error: {}", self.cargo.error.as_deref().unwrap_or("(none)"));
        println!("  cargo_home:");
        println!("    source: {}", self.cargo_home.source.as_str());
        println!("    path: {}", display_option_path(&self.cargo_home.path));
        println!("    exists: {}", self.cargo_home.exists);
        self.registry_cache.print_text("registry_cache");
        self.registry_index.print_text("registry_index");
        self.registry_src.print_text("registry_src");
        self.git_checkouts.print_text("git_checkouts");
        self.git_db.print_text("git_db");
        println!("  cargo_config:");
        println!("    files_checked: {}", self.cargo_config.files.len());
        println!(
            "    source_replacement_detected: {}",
            self.cargo_config.source_replacement_detected
        );
        println!(
            "    vendor_source_detected: {}",
            self.cargo_config.vendor_source_detected
        );
        println!("    net_offline_detected: {}", self.cargo_config.net_offline_detected);
        for file in &self.cargo_config.files {
            println!("    file: {}", file.path.display());
            println!("      readable: {}", file.readable);
            println!("      source_replacement: {}", file.source_replacement);
            println!("      vendor_source: {}", file.vendor_source);
            println!("      net_offline: {}", file.net_offline);
            println!("      parse_error: {}", file.parse_error.as_deref().unwrap_or("(none)"));
        }
        println!("  next_steps:");
        for step in &self.next_steps {
            println!("    - {step}");
        }
    }

    /// Convert advisory offline-readiness into stable JSON.
    fn as_json(&self) -> serde_json::Value {
        json!({
            "advisory_only": self.advisory_only,
            "status": self.status.as_str(),
            "source_of_truth": "Cargo and RFC 020 policy flags",
            "cargo": self.cargo.as_json(),
            "cargo_home": self.cargo_home.as_json(),
            "caches": {
                "registry_cache": self.registry_cache.as_json(),
                "registry_index": self.registry_index.as_json(),
                "registry_src": self.registry_src.as_json(),
                "git_checkouts": self.git_checkouts.as_json(),
                "git_db": self.git_db.as_json(),
            },
            "cargo_config": self.cargo_config.as_json(),
            "next_steps": self.next_steps,
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum OfflineReadinessStatus {
    Present,
    Missing,
    Unknown,
}

impl OfflineReadinessStatus {
    /// Classify whether local offline-readiness signals are present, missing, or unknown.
    fn from_signals(
        cargo: &CargoCommandInfo,
        cargo_home: &CargoHomeInfo,
        caches: [&CachePathHint; 5],
        cargo_config: &CargoConfigHints,
    ) -> Self {
        if !cargo.available || cargo_home.path.is_none() {
            return Self::Missing;
        }
        if caches.iter().any(|cache| cache.exists && cache.has_entries)
            || cargo_config.source_replacement_detected
            || cargo_config.vendor_source_detected
        {
            return Self::Present;
        }
        if cargo_home.exists {
            Self::Unknown
        } else {
            Self::Missing
        }
    }

    /// Return the stable JSON/text spelling for this advisory status.
    fn as_str(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Missing => "missing",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug)]
struct CargoCommandInfo {
    command: &'static str,
    available: bool,
    version: Option<String>,
    error: Option<String>,
}

impl CargoCommandInfo {
    /// Run only `cargo --version`; this does not resolve packages or access the network.
    fn collect() -> Self {
        match Command::new("cargo").arg("--version").output() {
            Ok(output) if output.status.success() => {
                let version = String::from_utf8(output.stdout)
                    .ok()
                    .map(|text| text.trim().to_string())
                    .filter(|text| !text.is_empty());
                Self {
                    command: "cargo",
                    available: true,
                    version,
                    error: None,
                }
            }
            Ok(output) => {
                let error = String::from_utf8(output.stderr)
                    .ok()
                    .map(|text| text.trim().to_string())
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| format!("cargo --version exited with {}", output.status));
                Self {
                    command: "cargo",
                    available: false,
                    version: None,
                    error: Some(error),
                }
            }
            Err(error) => Self {
                command: "cargo",
                available: false,
                version: None,
                error: Some(error.to_string()),
            },
        }
    }

    /// Convert Cargo command availability into JSON.
    fn as_json(&self) -> serde_json::Value {
        json!({
            "command": self.command,
            "available": self.available,
            "version": self.version,
            "error": self.error,
        })
    }
}

#[derive(Debug)]
struct CargoHomeInfo {
    source: CargoHomeSource,
    path: Option<PathBuf>,
    exists: bool,
}

impl CargoHomeInfo {
    /// Resolve the effective Cargo home from `CARGO_HOME` or the default home directory.
    fn collect() -> Self {
        let (source, path) = if let Some(path) = env::var_os("CARGO_HOME").map(PathBuf::from) {
            (CargoHomeSource::CargoHomeEnv, Some(path))
        } else if let Some(home) = home_dir() {
            (CargoHomeSource::HomeDefault, Some(home.join(".cargo")))
        } else {
            (CargoHomeSource::Unknown, None)
        };
        let exists = path.as_deref().is_some_and(Path::exists);
        Self { source, path, exists }
    }

    /// Convert the effective Cargo home into JSON.
    fn as_json(&self) -> serde_json::Value {
        json!({
            "source": self.source.as_str(),
            "path": self.path.as_deref().map(path_to_string),
            "exists": self.exists,
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum CargoHomeSource {
    CargoHomeEnv,
    HomeDefault,
    Unknown,
}

impl CargoHomeSource {
    /// Return the stable JSON/text spelling for the Cargo home source.
    fn as_str(self) -> &'static str {
        match self {
            Self::CargoHomeEnv => "CARGO_HOME",
            Self::HomeDefault => "HOME/.cargo",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug)]
struct CachePathHint {
    path: Option<PathBuf>,
    exists: bool,
    has_entries: bool,
}

impl CachePathHint {
    /// Inspect whether one optional cache path exists and contains entries.
    fn from_optional_path(path: Option<PathBuf>) -> Self {
        let exists = path.as_deref().is_some_and(Path::exists);
        let has_entries = path.as_deref().is_some_and(path_has_entries);
        Self {
            path,
            exists,
            has_entries,
        }
    }

    /// Print one cache path hint in the doctor text report.
    fn print_text(&self, label: &str) {
        println!("  {label}:");
        println!("    path: {}", display_option_path(&self.path));
        println!("    exists: {}", self.exists);
        println!("    has_entries: {}", self.has_entries);
    }

    /// Convert one cache path hint into JSON.
    fn as_json(&self) -> serde_json::Value {
        json!({
            "path": self.path.as_deref().map(path_to_string),
            "exists": self.exists,
            "has_entries": self.has_entries,
        })
    }
}

#[derive(Debug)]
struct CargoConfigHints {
    files: Vec<CargoConfigFileHint>,
    source_replacement_detected: bool,
    vendor_source_detected: bool,
    net_offline_detected: bool,
}

impl CargoConfigHints {
    /// Collect local Cargo config files that may affect offline or vendored builds.
    fn collect(cwd: Option<&Path>, cargo_home: Option<&Path>) -> Self {
        let files = cargo_config_candidates(cwd, cargo_home)
            .into_iter()
            .filter(|path| path.is_file())
            .map(CargoConfigFileHint::from_path)
            .collect::<Vec<_>>();
        let source_replacement_detected = files.iter().any(|file| file.source_replacement);
        let vendor_source_detected = files.iter().any(|file| file.vendor_source);
        let net_offline_detected = files.iter().any(|file| file.net_offline);
        Self {
            files,
            source_replacement_detected,
            vendor_source_detected,
            net_offline_detected,
        }
    }

    /// Convert Cargo config hints into JSON.
    fn as_json(&self) -> serde_json::Value {
        json!({
            "files": self.files.iter().map(CargoConfigFileHint::as_json).collect::<Vec<_>>(),
            "source_replacement_detected": self.source_replacement_detected,
            "vendor_source_detected": self.vendor_source_detected,
            "net_offline_detected": self.net_offline_detected,
        })
    }
}

#[derive(Debug)]
struct CargoConfigFileHint {
    path: PathBuf,
    readable: bool,
    source_replacement: bool,
    vendor_source: bool,
    net_offline: bool,
    parse_error: Option<String>,
}

impl CargoConfigFileHint {
    /// Parse one Cargo config file and extract offline/source replacement hints.
    fn from_path(path: PathBuf) -> Self {
        let Ok(content) = fs::read_to_string(&path) else {
            return Self {
                path,
                readable: false,
                source_replacement: false,
                vendor_source: false,
                net_offline: false,
                parse_error: Some("failed to read Cargo config".to_string()),
            };
        };
        let parsed = toml::from_str::<toml::Value>(&content);
        match parsed {
            Ok(value) => Self {
                path,
                readable: true,
                source_replacement: cargo_config_has_source_replacement(&value),
                vendor_source: cargo_config_has_vendor_source(&value),
                net_offline: cargo_config_has_net_offline(&value),
                parse_error: None,
            },
            Err(error) => Self {
                path,
                readable: true,
                source_replacement: content.contains("replace-with"),
                vendor_source: content.contains("directory") || content.contains("vendor"),
                net_offline: content.contains("offline"),
                parse_error: Some(error.to_string()),
            },
        }
    }

    /// Convert one Cargo config file hint into JSON.
    fn as_json(&self) -> serde_json::Value {
        json!({
            "path": path_to_string(&self.path),
            "readable": self.readable,
            "source_replacement": self.source_replacement,
            "vendor_source": self.vendor_source,
            "net_offline": self.net_offline,
            "parse_error": self.parse_error,
        })
    }
}

/// Build the ordered list of Cargo config paths that can influence the current directory.
fn cargo_config_candidates(cwd: Option<&Path>, cargo_home: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(cwd) = cwd {
        for ancestor in cwd.ancestors() {
            candidates.push(ancestor.join(".cargo").join("config.toml"));
            candidates.push(ancestor.join(".cargo").join("config"));
        }
    }
    if let Some(cargo_home) = cargo_home {
        candidates.push(cargo_home.join("config.toml"));
        candidates.push(cargo_home.join("config"));
    }

    let mut seen = BTreeSet::new();
    candidates
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

/// Return whether a parsed Cargo config defines any source replacement.
fn cargo_config_has_source_replacement(value: &toml::Value) -> bool {
    value
        .get("source")
        .and_then(toml::Value::as_table)
        .is_some_and(|sources| {
            sources.values().any(|source| {
                source
                    .as_table()
                    .is_some_and(|table| table.get("replace-with").and_then(toml::Value::as_str).is_some())
            })
        })
}

/// Return whether a parsed Cargo config points at a vendored or local registry source.
fn cargo_config_has_vendor_source(value: &toml::Value) -> bool {
    value
        .get("source")
        .and_then(toml::Value::as_table)
        .is_some_and(|sources| {
            sources.iter().any(|(name, source)| {
                name.contains("vendor")
                    || source.as_table().is_some_and(|table| {
                        table.get("directory").and_then(toml::Value::as_str).is_some()
                            || table
                                .get("local-registry")
                                .and_then(toml::Value::as_str)
                                .is_some_and(|path| path.contains("vendor"))
                    })
            })
        })
}

/// Return whether a parsed Cargo config enables Cargo's offline mode by default.
fn cargo_config_has_net_offline(value: &toml::Value) -> bool {
    value
        .get("net")
        .and_then(toml::Value::as_table)
        .and_then(|net| net.get("offline"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

/// Build concrete next steps for missing or incomplete offline-readiness signals.
fn build_offline_next_steps(
    cargo: &CargoCommandInfo,
    cargo_home: &CargoHomeInfo,
    caches: [&CachePathHint; 5],
    cargo_config: &CargoConfigHints,
) -> Vec<String> {
    let mut steps = Vec::new();
    if !cargo.available {
        steps.push("Install Cargo or put the cargo executable on PATH.".to_string());
    }
    if cargo_home.path.is_none() {
        steps.push("Set CARGO_HOME or HOME so Cargo cache locations can be inspected.".to_string());
    } else if !cargo_home.exists {
        steps.push("Run an online Cargo command once, or restore a prepared CARGO_HOME cache.".to_string());
    }
    if !caches.iter().any(|cache| cache.exists && cache.has_entries) {
        steps.push("Populate Cargo registry/git caches before relying on offline builds.".to_string());
    }
    if !cargo_config.source_replacement_detected && !cargo_config.vendor_source_detected {
        steps.push(
            "For vendor-based offline builds, add Cargo source replacement config such as a vendored source directory."
                .to_string(),
        );
    }
    if !cargo_config.net_offline_detected {
        steps.push("Use Incan RFC 020 policy flags, or Cargo offline/frozen policy, for enforcement; this report is advisory only.".to_string());
    }
    if steps.is_empty() {
        steps.push("Local offline-readiness signals are present, but run the intended Incan command with the desired RFC 020 policy flags for authoritative validation.".to_string());
    }
    steps
}

/// Return whether a directory can be read and contains at least one entry.
fn path_has_entries(path: &Path) -> bool {
    fs::read_dir(path)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}

/// Resolve the current user's home directory from platform-standard environment variables.
fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(PathBuf::from))
}

/// Find an executable command in the current process PATH.
fn find_on_path(command: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    for dir in env::split_paths(&paths) {
        for candidate in executable_candidates(&dir, command) {
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// Build platform-specific executable candidates for one PATH directory.
fn executable_candidates(dir: &Path, command: &str) -> Vec<PathBuf> {
    if cfg!(windows) {
        let extensions = env::var_os("PATHEXT")
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".EXE;.CMD;.BAT;.COM".to_string());
        extensions
            .split(';')
            .map(|extension| dir.join(format!("{command}{extension}")))
            .collect()
    } else {
        vec![dir.join(command)]
    }
}

#[cfg(unix)]
/// Return whether a path is a regular executable file on Unix.
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
/// Return whether a path is an executable-like file on non-Unix platforms.
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

/// Render a path for plain text or JSON output.
fn path_to_string(path: &Path) -> String {
    path.display().to_string()
}

/// Render an optional path, using a consistent placeholder when absent.
fn display_option_path(path: &Option<PathBuf>) -> String {
    path.as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "(not found)".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::api_metadata::ApiDeclaration;
    use incan_codegraph::{CODEGRAPH_SCHEMA_VERSION, CodegraphEdgeKind, CodegraphNodeKind};

    #[test]
    fn collect_api_metadata_package_extracts_project_lib() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let src = tmp.path().join("src");
        fs::create_dir_all(&src)?;
        fs::write(
            tmp.path().join("incan.toml"),
            r#"
[project]
name = "metadata_demo"
version = "0.1.0"
"#,
        )?;
        fs::write(
            src.join("lib.incn"),
            r#"
pub const LABEL = "demo"

pub def label(prefix: str, suffix: str = "/") -> str:
    return prefix

pub quick_label = partial label(prefix=LABEL)
"#,
        )?;

        let package = collect_api_metadata_package(tmp.path())?;
        assert_eq!(package.schema_version, CHECKED_API_METADATA_SCHEMA_VERSION);
        assert_eq!(
            package.package,
            Some(CheckedApiPackageIdentity {
                name: "metadata_demo".to_string(),
                version: Some("0.1.0".to_string()),
            })
        );
        assert_eq!(package.modules.len(), 1);
        assert_eq!(package.modules[0].module_path, vec!["lib".to_string()]);
        assert_eq!(package.modules[0].declarations.len(), 3);
        assert!(
            package.modules[0]
                .declarations
                .iter()
                .any(|decl| matches!(decl, ApiDeclaration::Partial(partial) if partial.name == "quick_label")),
            "expected tools metadata api to preserve public partial declarations"
        );
        let markdown = render_api_metadata_markdown(&package);
        assert!(
            markdown.contains("pub quick_label = partial label(prefix: str = ..., suffix: str = ...) -> str")
                && markdown.contains("- Presets: `prefix`"),
            "expected generated API Markdown to render partial signatures and provenance, got:\n{markdown}"
        );
        Ok(())
    }

    #[test]
    fn collect_codegraph_document_exports_modules_declarations_and_imports() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let src = tmp.path().join("src");
        fs::create_dir_all(&src)?;
        fs::write(
            tmp.path().join("incan.toml"),
            r#"
[project]
name = "codegraph_demo"
version = "0.1.0"
"#,
        )?;
        fs::write(
            src.join("helper.incn"),
            r#"
pub def helper() -> int:
    return 1
"#,
        )?;
        fs::write(
            src.join("lib.incn"),
            r#"
from helper import helper

pub model User:
    id: int

pub def load() -> int:
    return helper()

pub def label(kind: str) -> str:
    match kind:
        "create" => "Create"
        "update" => "Update"
        _ => "Other"
"#,
        )?;

        let document = collect_codegraph_document(tmp.path(), false)?;

        assert_eq!(document.schema_version, CODEGRAPH_SCHEMA_VERSION);
        assert!(
            document
                .nodes
                .iter()
                .any(|node| node.kind == CodegraphNodeKind::Declaration
                    && node.label == "User"
                    && node.facts.get("declaration_kind").is_some_and(|kind| kind == "model")
                    && node
                        .facts
                        .get("checked_api_anchor_id")
                        .is_some_and(|anchor| anchor == "src::lib::User")),
            "expected model declaration node to carry checked API metadata facts: {document:?}"
        );
        assert!(
            document
                .nodes
                .iter()
                .any(|node| node.kind == CodegraphNodeKind::ApiMember
                    && node.label == "id"
                    && node.facts.get("member_kind").is_some_and(|kind| kind == "field")
                    && node.facts.get("checked_api_type").is_some_and(|ty| ty == "int")),
            "expected model field API member node in codegraph export: {document:?}"
        );
        assert!(
            document
                .nodes
                .iter()
                .any(|node| node.kind == CodegraphNodeKind::Declaration
                    && node.label == "load"
                    && node
                        .facts
                        .get("checked_api_signature")
                        .is_some_and(|signature| signature == "pub def load() -> int")),
            "expected function declaration node to carry checked API signature: {document:?}"
        );
        assert!(
            document
                .nodes
                .iter()
                .any(|node| node.kind == CodegraphNodeKind::Import && node.label == "from helper import helper"),
            "expected import node in codegraph export: {document:?}"
        );
        assert!(
            document
                .edges
                .iter()
                .any(|edge| edge.kind == CodegraphEdgeKind::Imports),
            "expected import edge in codegraph export: {document:?}"
        );
        assert!(
            document
                .nodes
                .iter()
                .any(|node| node.kind == CodegraphNodeKind::CallSite
                    && node
                        .facts
                        .get("callee_key")
                        .is_some_and(|callee| callee == "call:ident:helper")),
            "expected function call site node in codegraph export: {document:?}"
        );
        assert!(
            document
                .nodes
                .iter()
                .any(|node| node.kind == CodegraphNodeKind::Reference
                    && node
                        .facts
                        .get("reference_key")
                        .is_some_and(|reference| reference == "ident:helper")),
            "expected identifier reference node in codegraph export: {document:?}"
        );
        assert!(
            document.nodes.iter().any(|node| {
                node.kind == CodegraphNodeKind::MatchDispatch
                    && node
                        .facts
                        .get("domain_key")
                        .is_some_and(|domain| domain == "ident:kind")
                    && node
                        .facts
                        .get("explicit_pattern_count")
                        .is_some_and(|count| count == "2")
                    && node.facts.get("arm_count").is_some_and(|count| count == "3")
                    && node
                        .facts
                        .get("has_default_arm")
                        .is_some_and(|has_default| has_default == "true")
                    && node.facts.get("pattern_labels").is_some_and(|patterns| {
                        patterns.contains("\\\"create\\\"") && patterns.contains("\\\"update\\\"")
                    })
            }),
            "expected match dispatch node in codegraph export: {document:?}"
        );
        Ok(())
    }

    #[test]
    fn collect_codegraph_document_globs_directory_sources() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let nested = tmp.path().join("nested");
        fs::create_dir_all(&nested)?;
        fs::write(
            tmp.path().join("alpha.incn"),
            r#"
pub def alpha() -> int:
    return 1
"#,
        )?;
        fs::write(
            nested.join("beta.incn"),
            r#"
pub model Beta:
    id: int
"#,
        )?;

        let document = collect_codegraph_document(tmp.path(), false)?;

        assert!(
            document
                .nodes
                .iter()
                .any(|node| node.kind == CodegraphNodeKind::File && node.file_path.as_deref() == Some("alpha.incn")),
            "expected directory export to include top-level .incn file: {document:?}"
        );
        assert!(
            document
                .nodes
                .iter()
                .any(|node| node.kind == CodegraphNodeKind::File
                    && node.file_path.as_deref() == Some("nested/beta.incn")),
            "expected directory export to include nested .incn file: {document:?}"
        );
        assert!(
            document.nodes.iter().any(|node| node.kind == CodegraphNodeKind::Module
                && node.module_path == vec!["nested".to_string(), "beta".to_string()]),
            "expected nested directory source to get path-derived module facts: {document:?}"
        );
        Ok(())
    }

    #[test]
    fn collect_codegraph_document_allow_errors_exports_unchecked_source_graph() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        fs::write(
            tmp.path().join("broken.incn"),
            r#"
pub def broken() -> int:
    return "wrong"
"#,
        )?;

        assert!(
            collect_codegraph_document(tmp.path(), false).is_err(),
            "expected checked codegraph export to fail on type errors"
        );

        let document = collect_codegraph_document(tmp.path(), true)?;

        assert!(
            document
                .nodes
                .iter()
                .any(|node| node.kind == CodegraphNodeKind::Declaration
                    && node.label == "broken"
                    && node
                        .facts
                        .get("declaration_kind")
                        .is_some_and(|kind| kind == "function")),
            "expected unchecked source graph to keep declaration facts: {document:?}"
        );
        assert!(
            !document
                .nodes
                .iter()
                .any(|node| node.facts.contains_key("checked_api_signature")),
            "expected unchecked export to omit checked API facts for failing modules: {document:?}"
        );
        Ok(())
    }

    #[test]
    fn cargo_config_hints_detect_vendor_source_replacement() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let cargo_dir = tmp.path().join(".cargo");
        fs::create_dir_all(&cargo_dir)?;
        let config = cargo_dir.join("config.toml");
        fs::write(
            config,
            r#"
[net]
offline = true

[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
"#,
        )?;

        let hints = CargoConfigHints::collect(Some(tmp.path()), None);
        assert!(hints.source_replacement_detected);
        assert!(hints.vendor_source_detected);
        assert!(hints.net_offline_detected);
        assert_eq!(hints.files.len(), 1);
        Ok(())
    }
}
