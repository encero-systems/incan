//! Checked metadata packages: the API and registry metadata a project publishes, collected once from a checked
//! module graph.
//!
//! `tools metadata` renders these; the replacement-compatibility inventory and the feature-inventory reference
//! read them. Collection lives here so a reader below the CLI never reaches into a command to get one.

use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{CliError, CliResult};
use crate::features::FeatureSelection;
use crate::modules::collect_modules_detailed_with_session;
use crate::project::resolve_project_root;
use crate::session::{CompilationSession, DEFAULT_PROFILE};
use crate::typecheck::typecheck_modules;
use incan_frontend::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, CheckedApiPackageIdentity,
    collect_checked_api_alias_metadata, collect_checked_api_metadata, materialize_api_alias_projections,
    materialize_checked_api_public_namespaces, validate_checked_api_docstrings,
};
use incan_frontend::diagnostics;
use incan_frontend::loaf_index::LoafIndex;
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::registry_metadata::{
    CHECKED_REGISTRY_METADATA_SCHEMA_VERSION, CheckedRegistryMetadataPackage, CheckedRegistryPackageIdentity,
    collect_checked_registry_metadata, materialize_registry_reexport_projections,
};
use oven_model::manifest::ProjectManifest;

/// Type-check a metadata entry path and collect checked API metadata for all local modules.
pub fn collect_api_metadata_package(path: &Path) -> CliResult<CheckedApiMetadataPackage> {
    let entry_path = resolve_metadata_entry_path(path)?;
    let session = CompilationSession::discover(&entry_path, &FeatureSelection::default(), DEFAULT_PROFILE)?;
    let modules = collect_modules_detailed_with_session(entry_path.clone(), &session)
        .map_err(|failure| CliError::failure(failure.render_human()))?;
    let mut metadata_modules = Vec::new();
    typecheck_modules(
        &modules,
        session.manifest.as_ref(),
        session.loaves(),
        session.own_standard_package(),
        None,
        None,
        &mut |module, checker| {
            metadata_modules.push(collect_checked_api_metadata(
                &module.ast,
                checker,
                metadata_module_path(module, &entry_path),
            ));
        },
        #[cfg(feature = "rust_inspect")]
        None,
    )
    .map_err(|failure| CliError::failure(failure.render_human()))?;

    materialize_api_alias_projections(&mut metadata_modules);
    let mut all_errors = String::new();
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

    let mut package = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: session.manifest.as_ref().and_then(checked_api_package_identity),
        modules: metadata_modules,
        public_namespaces: Vec::new(),
    };
    materialize_checked_api_public_namespaces(&mut package)
        .map_err(|error| CliError::failure(format!("failed to inspect checked module namespaces: {error}")))?;
    Ok(package)
}

/// Analyze a registry inspection entry path once and collect compiler-owned metadata for all local modules.
pub fn collect_registry_metadata_package(path: &Path) -> CliResult<CheckedRegistryMetadataPackage> {
    collect_registry_metadata_with_dependencies(path).map(|(package, _)| package)
}

/// Collect local registry facts and the same dependency manifests used by their checked session.
///
/// Metadata renderers must consume this snapshot instead of reopening mutable dependency paths after validation.
/// The returned index is metadata only; it does not replace execution leases or authorize native compilation.
pub fn collect_registry_metadata_with_dependencies(
    path: &Path,
) -> CliResult<(CheckedRegistryMetadataPackage, Arc<LoafIndex>)> {
    let entry_path = resolve_metadata_entry_path(path)?;
    let session = CompilationSession::discover(&entry_path, &FeatureSelection::default(), DEFAULT_PROFILE)?;
    let modules = collect_modules_detailed_with_session(entry_path.clone(), &session)
        .map_err(|failure| CliError::failure(failure.render_human()))?;
    let analysis = session
        .analyze_modules(
            &modules,
            #[cfg(feature = "rust_inspect")]
            None,
        )
        .map_err(|failure| CliError::failure(failure.render_human()))?;
    let project_root = resolve_project_root(&entry_path);
    // Imported source modules are canonicalized by module resolution. Keep the producer-boundary comparison in the
    // same path space so macOS's `/var` -> `/private/var` alias cannot silently drop a local module from checked
    // registry inspection or the artifact it publishes.
    let canonical_project_root = project_root.canonicalize().unwrap_or_else(|_| project_root.clone());
    let manifest = session.manifest.clone();
    let runtime_package_identity = manifest
        .as_ref()
        .and_then(|manifest| manifest.project.as_ref())
        .and_then(|project| project.name.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| {
            entry_path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "<unpackaged>".to_string());
    let mut metadata_modules = Vec::new();
    let mut api_metadata_modules = Vec::new();

    for module in &modules {
        // Only the project's own sources are this package's registry metadata, except an explicitly selected entry
        // outside it (the capability-inventory generator selects a standard package's source).
        let selected_source_entry = module.file_path == entry_path;
        if !selected_source_entry && !module.file_path.starts_with(&canonical_project_root) {
            continue;
        }
        let type_info = analysis
            .type_info_for_module_path(&module.path_segments)
            .ok_or_else(|| CliError::failure(format!("missing session analysis for {}", module.file_path.display())))?;
        let module_path = metadata_module_path(module, &entry_path);
        metadata_modules.push(collect_checked_registry_metadata(
            type_info,
            module_path.clone(),
            &runtime_package_identity,
        ));
        api_metadata_modules.push(collect_checked_api_alias_metadata(&module.ast, module_path));
    }
    materialize_api_alias_projections(&mut api_metadata_modules);
    materialize_registry_reexport_projections(&mut metadata_modules, &api_metadata_modules);
    metadata_modules.sort_by(|left, right| left.module_path.cmp(&right.module_path));
    Ok((
        CheckedRegistryMetadataPackage {
            schema_version: CHECKED_REGISTRY_METADATA_SCHEMA_VERSION,
            package: manifest.as_ref().and_then(checked_registry_package_identity),
            modules: metadata_modules,
        },
        Arc::clone(session.loaves()),
    ))
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

/// Convert a manifest project identity into the optional checked-registry package identity.
fn checked_registry_package_identity(manifest: &ProjectManifest) -> Option<CheckedRegistryPackageIdentity> {
    let project = manifest.project.as_ref()?;
    let name = project.name.as_ref()?.trim();
    if name.is_empty() {
        return None;
    }
    Some(CheckedRegistryPackageIdentity {
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
pub fn resolve_metadata_entry_path(path: &Path) -> CliResult<PathBuf> {
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
