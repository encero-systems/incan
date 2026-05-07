//! Local toolchain inspection commands.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::ValueEnum;
use serde_json::json;

use crate::cli::prelude::ParsedModule;
use crate::cli::{CliError, CliResult, ExitCode};
use crate::frontend::api_metadata::{
    ApiDeclaration, ApiFunction, ApiPartial, CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage,
    CheckedApiPackageIdentity, collect_checked_api_metadata, validate_checked_api_docstrings,
};
use crate::frontend::contract_metadata::{
    CanonicalModelBundle, read_model_bundles_from_json, read_project_model_bundles,
};
use crate::frontend::diagnostics;
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::typechecker;
use crate::library_manifest::{LibraryManifest, ParamExport, ParamKindExport, TypeRef};
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
            "metadata API extraction requires an Incan source file, or a project directory with `src/lib.incn` or `src/main.incn`: {}",
            absolute.display()
        )));
    }

    Err(CliError::failure(format!(
        "metadata API extraction path does not exist: {}",
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
