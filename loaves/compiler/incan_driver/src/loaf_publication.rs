//! The Loaf a library unit publishes (DD-0007): its library manifest, its executable representation and its native
//! closure, written beside the unit's rlib in the profile's artifact directory.
//!
//! The library unit's own checking pass supplies every fact. [`PublicationFacts`] reads each module's checker while
//! [`crate::session::CompilationSession::analyze_unit`] visits it; nothing here checks again.

mod exports;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::{CliError, CliResult};
use crate::unit::UnitDescription;
use exports::{LibraryReexportResolver, module_key};
use incan_frontend::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadata, CheckedApiMetadataPackage, CheckedApiPackageIdentity,
    collect_checked_api_alias_metadata, collect_checked_api_metadata, materialize_api_alias_projections,
    materialize_checked_api_public_namespaces, validate_checked_api_docstrings,
};
use incan_frontend::ast::{Declaration, ImportKind, Visibility};
use incan_frontend::contract_metadata::{CanonicalModelBundle, ContractMetadataPackage};
use incan_frontend::diagnostics;
use incan_frontend::library_exports::{CheckedNamedExport, checked_exports_by_name, collect_checked_public_exports};
use incan_frontend::library_manifest::public_identities::public_executable_identities;
use incan_frontend::library_manifest::{
    ExecutableRepresentationExport, LibraryManifest, LibraryRustAbi, NativeUnionExport, NativeUnionOwnerExport, TypeRef,
};
use incan_frontend::loaf_index::LoafNativeClosure;
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::registry_metadata::{
    CHECKED_REGISTRY_METADATA_SCHEMA_VERSION, CheckedRegistryMetadataModule, CheckedRegistryMetadataPackage,
    CheckedRegistryPackageIdentity, collect_checked_registry_metadata, materialize_registry_reexport_projections,
};
use incan_frontend::typechecker::TypeChecker;
use incan_lang::interop::RustItemMetadata;
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::body_ir::NativeUnionDeclaration;
use incan_semantics_core::executable_representation::{
    EXECUTABLE_REPRESENTATION_VERSION, SurfaceReader, build_surface,
};

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
    /// Inspected native facts used by declarations and deferred bodies, independent of source import visibility.
    rust_items: Vec<RustItemMetadata>,
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
            rust_items: Vec::new(),
        }
    }

    /// Collect one checked module's public exports, API metadata and registries. A registry declared in the unit's
    /// entry module is published under the module path `lib`.
    pub fn visit(&mut self, module: &ParsedModule, checker: &TypeChecker, entry: bool) {
        self.collect_native_rust_items(module, checker);
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

    /// Ship imported and demanded native facts with aliases, signatures, and deferred bodies. A private Rust import
    /// can back a public rusttype or generic function; retaining its ABI grants no source-visible import binding.
    fn collect_native_rust_items(&mut self, module: &ParsedModule, checker: &TypeChecker) {
        let mut paths = checker.observed_rust_metadata_queries();
        let mut public_paths = BTreeSet::new();
        for declaration in &module.ast.declarations {
            let Declaration::Import(import) = &declaration.node else {
                continue;
            };
            let ImportKind::RustFrom {
                crate_name,
                path,
                items,
                ..
            } = &import.kind
            else {
                continue;
            };
            for item in items {
                let canonical = std::iter::once(crate_name.as_str())
                    .chain(path.iter().map(String::as_str))
                    .chain(std::iter::once(item.name.as_str()))
                    .collect::<Vec<_>>()
                    .join("::");
                if import.visibility == Visibility::Public {
                    public_paths.insert(canonical.clone());
                }
                paths.insert(canonical);
            }
        }
        for path in paths {
            // The installed toolchain already supplies these complete ABI caches; copying them into every Loaf
            // inflates manifests without adding a dependency-owned fact.
            if !public_paths.contains(&path)
                && (!path.contains("::") || matches!(path.split("::").next(), Some("std" | "core" | "alloc")))
            {
                continue;
            }
            if let Some(metadata) = checker.rust_item_metadata_for_path(&path) {
                self.rust_items.push(metadata.as_ref().clone());
            }
        }
    }

    /// Build the Loaf's library manifest: the entry module's public names, its checked API and identity graph, its
    /// public registries and its publishable contract models. `modules` are the unit's checked modules, for
    /// diagnostics.
    pub fn library_manifest(
        mut self,
        modules: &[ParsedModule],
        entry: &ParsedModule,
        unit: &UnitDescription,
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
        let mut manifest = LibraryManifest::from_checked_exports(&self.package, &unit.version, &selected);
        manifest.rust_abi = LibraryRustAbi::from_items(self.rust_items);
        retain_feature_requirements(&mut manifest, modules, unit);
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
                version: Some(unit.version.clone()),
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
                version: Some(unit.version.clone()),
            }),
            modules: self.registry_modules,
        });
        Ok(manifest)
    }
}

/// Retain the selected feature projection and source-gated export requirements for consumer diagnostics. Inactive
/// names supply remedies only; they remain absent from the checked public exports and executable publication.
fn retain_feature_requirements(manifest: &mut LibraryManifest, modules: &[ParsedModule], unit: &UnitDescription) {
    let provider = &mut manifest.contract_metadata.provider;
    provider.active_features = unit.active_features.clone();
    provider.public_features = unit
        .declared_features
        .iter()
        .map(|name| (name.clone(), Default::default()))
        .collect();
    provider.fact_requirements = modules
        .iter()
        .flat_map(|module| module.gated_exports.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
}

/// Write a Loaf's files beside its rlib in `dir`: `<name>.incnbody`, the executable representation of `bodies` (the
/// unit's own Body IR modules) projected onto the manifest's public identities; `<name>.incngraph`, the already-encoded
/// graph from that same checking pass; `<name>.native.json`, the unit's native closure; an indexed `<name>.rustabi`
/// when dependency-owned native facts are needed; and `<name>.incnlib` last,
/// so its presence means the artifact set is complete.
pub fn write_loaf(
    dir: &Path,
    mut manifest: LibraryManifest,
    bodies: &[BodyIrModule],
    closure: &LoafNativeClosure,
    graph: &[u8],
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
    if tracing::enabled!(target: "incan::invocation", tracing::Level::DEBUG) {
        let reader = SurfaceReader::open(&surface).map_err(|error| CliError::failure(error.to_string()))?;
        tracing::debug!(target: "incan::invocation", package = %name, coverage = ?reader.index().declarations, "published governed coverage");
    }
    manifest.contract_metadata.executable_representation = Some(ExecutableRepresentationExport {
        representation_version: EXECUTABLE_REPRESENTATION_VERSION,
        content_digest: hex::encode(Sha256::digest(&surface)),
    });
    write(format!("{name}.incnbody"), &surface)?;
    write(format!("{name}.incngraph"), graph)?;
    write_rust_abi_cache(dir, &mut manifest)?;

    // ---- Native closure and manifest ----
    let closure = serde_json::to_vec_pretty(closure)
        .map_err(|error| CliError::failure(format!("cannot encode the native closure of {name}: {error}")))?;
    write(format!("{name}.native.json"), &closure)?;
    let manifest = manifest
        .to_json_string()
        .map_err(|error| CliError::failure(format!("invalid library manifest for {name}: {error}")))?;
    write(format!("{name}.incnlib"), manifest.as_bytes())
}

/// Keep explicit Rust reexports embedded for their source binding contract, and publish other checked native facts
/// through the existing indexed ABI codec. The manifest digest authenticates the asset without eager item decoding.
fn write_rust_abi_cache(dir: &Path, manifest: &mut LibraryManifest) -> CliResult<()> {
    let public = manifest
        .contract_metadata
        .identity_graph
        .exports
        .iter()
        .filter_map(|export| export.target_path())
        .filter(|path| path.first().is_some_and(|root| root == "rust"))
        .map(|path| path[1..].join("::"))
        .collect::<BTreeSet<_>>();
    let Some(abi) = &mut manifest.rust_abi else {
        return Ok(());
    };
    let (embedded, indexed): (Vec<_>, Vec<_>) = std::mem::take(&mut abi.items)
        .into_iter()
        .partition(|item| public.contains(&item.canonical_path));
    abi.items = embedded;
    if indexed.is_empty() {
        return Ok(());
    }
    let indexed = indexed
        .into_iter()
        .map(|item| (item.canonical_path.clone(), item))
        .collect::<BTreeMap<_, _>>();
    let bytes = incan_lang::interop::encode_rust_abi_cache(&indexed).map_err(CliError::failure)?;
    let path = dir.join(format!("{}.rustabi", manifest.name));
    fs::write(&path, &bytes).map_err(|error| CliError::failure(format!("cannot write {}: {error}", path.display())))?;
    abi.cache_digest = Some(format!("sha256:{:x}", Sha256::digest(&bytes)));
    Ok(())
}

/// Record the lowering's actual native union definitions in the manifest that authenticates their rlib. The semantic
/// carrier and ordered members retain canonical nominal keys; no consumer spelling participates in publication.
pub fn publish_native_unions(manifest: &mut LibraryManifest, unions: Vec<NativeUnionDeclaration>) -> CliResult<()> {
    manifest.contract_metadata.native_unions = unions
        .into_iter()
        .map(|declaration| {
            Ok(NativeUnionExport {
                owner: NativeUnionOwnerExport::ContainingArtifact,
                rust_name: declaration.rust_item,
                closure_items: declaration.closure_items,
                closure_members: declaration.closure_members,
                members: declaration
                    .members
                    .iter()
                    .map(native_union_member)
                    .collect::<CliResult<_>>()?,
                semantic_type: Some(declaration.ty),
                local_nominals: Default::default(),
                checked_projection: None,
            })
        })
        .collect::<CliResult<_>>()?;
    Ok(())
}

/// Encode an admitted native payload without shortening its canonical nominal leaves. Union publication must refuse
/// an unrepresentable payload rather than make a new layout from an approximate type.
fn native_union_member(ty: &IncanType) -> CliResult<TypeRef> {
    Ok(match ty {
        IncanType::Primitive(primitive) => TypeRef::Named {
            name: primitive.to_string(),
            origin: None,
        },
        IncanType::Named(name) => TypeRef::Named {
            name: name.clone(),
            origin: None,
        },
        IncanType::Generic { base, args } => TypeRef::Applied {
            name: base.clone(),
            args: args.iter().map(native_union_member).collect::<CliResult<_>>()?,
            origin: None,
        },
        IncanType::Tuple(elements) => TypeRef::Tuple {
            elements: elements.iter().map(native_union_member).collect::<CliResult<_>>()?,
        },
        IncanType::Function { params, return_type } => TypeRef::Function {
            params: params
                .iter()
                .map(|parameter| native_union_member(&parameter.ty))
                .collect::<CliResult<_>>()?,
            return_type: Box::new(native_union_member(return_type)?),
        },
        _ => return Err(CliError::failure(format!("cannot publish native union payload {ty}"))),
    })
}
