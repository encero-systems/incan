//! The Loaf a library unit publishes (DD-0007): its library manifest, its executable representation and its native
//! closure, written beside the unit's rlib in the profile's artifact directory.
//!
//! The library unit's own checking pass supplies every fact. [`PublicationFacts`] reads each module's checker while
//! [`crate::session::CompilationSession::analyze_unit`] visits it; nothing here checks again.

mod exports;

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::{CliError, CliResult};
use exports::{LibraryReexportResolver, module_key};
use incan_frontend::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadata, CheckedApiMetadataPackage, CheckedApiPackageIdentity,
    collect_checked_api_alias_metadata, collect_checked_api_metadata, materialize_api_alias_projections,
    materialize_checked_api_public_namespaces, validate_checked_api_docstrings,
};
use incan_frontend::contract_metadata::{CanonicalModelBundle, ContractMetadataPackage};
use incan_frontend::diagnostics;
use incan_frontend::library_exports::{CheckedNamedExport, checked_exports_by_name, collect_checked_public_exports};
use incan_frontend::library_manifest::public_identities::public_executable_identities;
use incan_frontend::library_manifest::{ExecutableRepresentationExport, LibraryManifest};
use incan_frontend::loaf_index::LoafNativeClosure;
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::registry_metadata::{
    CHECKED_REGISTRY_METADATA_SCHEMA_VERSION, CheckedRegistryMetadataModule, CheckedRegistryMetadataPackage,
    CheckedRegistryPackageIdentity, collect_checked_registry_metadata, materialize_registry_reexport_projections,
};
use incan_frontend::typechecker::TypeChecker;
use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::executable_representation::{EXECUTABLE_REPRESENTATION_VERSION, build_surface};

/// What a library unit's checking pass collects for its publication, one module at a time.
pub struct PublicationFacts {
    /// The package identity every declaration of the unit carries.
    package: String,
    /// Checked public exports by module key, then by source name.
    exports_by_module: HashMap<String, HashMap<String, Vec<CheckedNamedExport>>>,
    /// Checked public exports by source module, in checking order.
    exports_by_source_module: Vec<(Vec<String>, Vec<CheckedNamedExport>)>,
    /// Checked API metadata, one per module.
    api_modules: Vec<CheckedApiMetadata>,
    /// The `pub from … import` aliases each module declares, under its registry module path.
    alias_modules: Vec<CheckedApiMetadata>,
    /// Checked registries and entries, one per module.
    registry_modules: Vec<CheckedRegistryMetadataModule>,
}

impl PublicationFacts {
    /// Facts for the library unit of `package`.
    pub fn new(package: &str) -> Self {
        Self {
            package: package.to_string(),
            exports_by_module: HashMap::new(),
            exports_by_source_module: Vec::new(),
            api_modules: Vec::new(),
            alias_modules: Vec::new(),
            registry_modules: Vec::new(),
        }
    }

    /// Collect one checked module's public exports, API metadata and registries. A registry declared in the unit's
    /// entry module is published under the module path `lib`.
    pub fn visit(&mut self, module: &ParsedModule, checker: &TypeChecker, entry: bool) {
        let exports = collect_checked_public_exports(&module.ast, checker);
        let registry_path = if entry {
            vec!["lib".to_string()]
        } else {
            module.path_segments.clone()
        };
        self.api_modules.push(collect_checked_api_metadata(
            &module.ast,
            checker,
            module.path_segments.clone(),
        ));
        self.alias_modules
            .push(collect_checked_api_alias_metadata(&module.ast, registry_path.clone()));
        self.registry_modules.push(collect_checked_registry_metadata(
            checker.type_info(),
            registry_path,
            &self.package,
        ));
        self.exports_by_source_module
            .push((module.path_segments.clone(), exports.clone()));
        self.exports_by_module
            .insert(module_key(&module.path_segments), checked_exports_by_name(exports));
    }

    /// Build the Loaf's library manifest: the entry module's public names, its checked API and identity graph, its
    /// public registries and its publishable contract models. `modules` are the unit's checked modules, for
    /// diagnostics.
    pub fn library_manifest(
        mut self,
        modules: &[ParsedModule],
        entry: &ParsedModule,
        version: &str,
        model_bundles: &[CanonicalModelBundle],
    ) -> CliResult<LibraryManifest> {
        materialize_api_alias_projections(&mut self.api_modules);
        let docstring_errors = validate_checked_api_docstrings(&self.api_modules)
            .into_iter()
            .map(|diagnostic| {
                match modules
                    .iter()
                    .find(|module| module.path_segments == diagnostic.module_path)
                {
                    Some(module) => diagnostics::format_error(
                        module.file_path.to_string_lossy().as_ref(),
                        &module.source,
                        &diagnostic.error,
                    ),
                    None => format!("{}\n", diagnostic.error.message),
                }
            })
            .collect::<String>();
        if !docstring_errors.is_empty() {
            return Err(CliError::failure(docstring_errors.trim_end()));
        }

        // ---- Public names and their identities ----
        let selected = LibraryReexportResolver::new(&self.exports_by_module)
            .resolve(entry)
            .map_err(|errors| {
                let rendered = errors
                    .iter()
                    .map(|error| {
                        diagnostics::format_error(entry.file_path.to_string_lossy().as_ref(), &entry.source, error)
                    })
                    .collect::<String>();
                CliError::failure(rendered.trim_end())
            })?;
        let mut manifest = LibraryManifest::from_checked_exports(&self.package, version, &selected);
        manifest.contract_metadata.models = ContractMetadataPackage::new(
            model_bundles
                .iter()
                .filter(|bundle| bundle.publishable)
                .cloned()
                .collect(),
        );

        // ---- Checked API and module identities ----
        let mut api = CheckedApiMetadataPackage {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            package: Some(CheckedApiPackageIdentity {
                name: self.package.clone(),
                version: Some(version.to_string()),
            }),
            modules: self.api_modules,
            public_namespaces: Vec::new(),
        };
        materialize_checked_api_public_namespaces(&mut api)
            .map_err(|error| CliError::failure(format!("failed to publish checked module namespaces: {error}")))?;
        manifest
            .contract_metadata
            .identity_graph
            .extend_checked_api_exports(&self.package, &api, &self.exports_by_source_module)
            .map_err(|error| CliError::failure(format!("failed to publish checked module identities: {error}")))?;
        manifest.contract_metadata.api = Some(api);

        // ---- Public registries ----
        materialize_registry_reexport_projections(&mut self.registry_modules, &self.alias_modules);
        for module in &mut self.registry_modules {
            module.registries.retain(|registry| registry.public);
            module.entries.retain(|entry| entry.registry_public);
        }
        self.registry_modules
            .retain(|module| !module.registries.is_empty() || !module.entries.is_empty());
        manifest.contract_metadata.registry = Some(CheckedRegistryMetadataPackage {
            schema_version: CHECKED_REGISTRY_METADATA_SCHEMA_VERSION,
            package: Some(CheckedRegistryPackageIdentity {
                name: self.package.clone(),
                version: Some(version.to_string()),
            }),
            modules: self.registry_modules,
        });
        Ok(manifest)
    }
}

/// Write a Loaf's files beside its rlib in `dir`: `<name>.incnbody`, the executable representation of `bodies` (the
/// unit's own Body IR modules) projected onto the manifest's public identities; `<name>.native.json`, the closure
/// the unit linked; and `<name>.incnlib`, the manifest, last, so its presence means the Loaf is complete.
pub fn write_loaf(
    dir: &Path,
    mut manifest: LibraryManifest,
    bodies: &[BodyIrModule],
    closure: &LoafNativeClosure,
) -> CliResult<()> {
    let name = manifest.name.clone();
    let write = |file: String, bytes: &[u8]| {
        let path = dir.join(file);
        fs::write(&path, bytes).map_err(|error| CliError::failure(format!("cannot write {}: {error}", path.display())))
    };

    // ---- Executable representation ----
    // A body with a gap in its Body IR is unrepresentable for every consumer: what a package publishes is a fact
    // about the package.
    let public = public_executable_identities(&manifest);
    let unrepresentable = bodies
        .iter()
        .flat_map(|module| module.bodies.iter())
        .filter(|body| body.first_representation_gap().is_some())
        .filter_map(|body| body.canonical.clone())
        .collect::<BTreeSet<_>>();
    let surface = build_surface(bodies, &name, &manifest.version, &public, &unrepresentable).map_err(|error| {
        CliError::failure(format!(
            "failed to produce the executable representation of {name}: {error}"
        ))
    })?;
    manifest.contract_metadata.executable_representation = Some(ExecutableRepresentationExport {
        representation_version: EXECUTABLE_REPRESENTATION_VERSION,
        content_digest: hex::encode(Sha256::digest(&surface)),
    });
    write(format!("{name}.incnbody"), &surface)?;

    // ---- Native closure and manifest ----
    let closure = serde_json::to_vec_pretty(closure)
        .map_err(|error| CliError::failure(format!("cannot encode the native closure of {name}: {error}")))?;
    write(format!("{name}.native.json"), &closure)?;
    let manifest = manifest
        .to_json_string()
        .map_err(|error| CliError::failure(format!("invalid library manifest for {name}: {error}")))?;
    write(format!("{name}.incnlib"), manifest.as_bytes())
}
