//! In-process native SDK component publication using checked library metadata and retained sealed units.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use crate::build::library_project::{LibraryPreparation, prepare_native_sdk_component};
use crate::error::{CliError, CliResult};
use incan_provider::{SdkInventory, SdkSourceCatalog};
use oven_rustc::sdk_closure::SdkCompiledClosure;

/// Bind a caller's source receipt to the prepared native SDK and select its declared direct dependency aliases.
///
/// This explicit compiler-tooling boundary consumes the already published SDK; it never resolves dependencies or
/// invokes Cargo. The canonical native receipt catalog enters the returned receipt's build-unit identity, so an
/// SDK change cannot reuse a stale plan. Every requested local or adopted binding must match the selected SDK's
/// source, version, features and domain, and the returned plan retains its output leases through compilation.
pub fn select_prepared_native_sdk_plan(
    store: &oven_store::store::OvenStore,
    source_receipt: &oven_store::OvenReceipt,
    dependencies: &[oven_model::manifest::DependencySpec],
) -> CliResult<(oven_store::OvenReceipt, oven_rustc::plan::OvenDirectRustcPlanSelection)> {
    let inventory = incan_provider::inventory::discover_or_reuse_published_sdk_inventory()?
        .ok_or_else(|| CliError::failure("compiler tooling requires a prepared native SDK"))?;
    let catalog = std::fs::read(inventory.root.join(".sealed-native-receipts.json"))
        .map_err(|error| CliError::failure(error.to_string()))?;
    let receipt = bind_native_catalog(source_receipt, &catalog)?;
    let roots = oven_store::digest_dependency_specs(dependencies, incan_oven_facet::provider_hooks().as_ref())
        .map_err(|error| CliError::failure(error.to_string()))?;
    let receipt = oven_store::receipt_with_build_unit_input(&receipt, "sdk-native-roots", roots)
        .map_err(|error| CliError::failure(error.to_string()))?;
    let selection = super::native_sdk_plan::select_native_sdk_plan(store, &receipt, dependencies)?
        .ok_or_else(|| CliError::failure("prepared SDK has no native dependency plan"))?;
    Ok((receipt, selection.plan_selection))
}

/// Canonicalize the catalog before receipt binding so JSON ordering never selects a different native closure.
fn bind_native_catalog(source_receipt: &oven_store::OvenReceipt, bytes: &[u8]) -> CliResult<oven_store::OvenReceipt> {
    let catalog: std::collections::BTreeMap<String, String> =
        serde_json::from_slice(bytes).map_err(|error| CliError::failure(error.to_string()))?;
    let bytes = serde_json::to_vec(&catalog).map_err(|error| CliError::failure(error.to_string()))?;
    oven_store::receipt_with_build_unit_input(source_receipt, "sdk-native-closure", oven_store::digest_bytes(&bytes))
        .map_err(|error| CliError::failure(error.to_string()))
}

/// Explicit context for one checked SDK component; the publisher owns the closure through durable publication.
pub(crate) struct NativeSdkPublicationContext<'a> {
    /// Already checked providers, with remaining components recorded as unavailable.
    pub inventory: &'a SdkInventory,
    /// Reserved roots granted by the source catalog to this component only.
    pub namespace_roots: BTreeSet<String>,
    /// Frozen native selection retained for inspection and facet validation.
    pub closure: &'a SdkCompiledClosure,
    /// Exact crate names already supplied by native units, excluded from public Incan package edges.
    pub native_facets: BTreeSet<String>,
    /// Component-owned dependency requirements disambiguate versions in the full compiler closure.
    pub dependencies: std::collections::HashMap<String, oven_model::manifest::DependencySpec>,
}

impl NativeSdkPublicationContext<'_> {
    /// Inspect the exact frozen SDK graph while the enclosing transaction retains its source and output leases.
    #[cfg(feature = "rust_inspect")]
    pub fn inspection_workspace(&self) -> CliResult<crate::lock::PreparedRustInspectWorkspace> {
        crate::rust_inspect_workspace::mark_oven_direct_rust_inspection(&self.inventory.root)?;
        std::fs::write(
            self.inventory.root.join(rust_inspect::OVEN_LOAF_ONLY_INSPECTION_MARKER),
            b"1\n",
        )
        .map_err(|error| CliError::failure(error.to_string()))?;
        Ok(crate::lock::PreparedRustInspectWorkspace::from_retained_sdk_graph(
            self.inventory.root.clone(),
        ))
    }

    /// Require the component's declared facet to be present in the retained native selection.
    pub fn validate_component_facets(&self, manifest: &oven_model::manifest::ProjectManifest) -> CliResult<()> {
        let declaration =
            std::fs::read_to_string(manifest.path()).map_err(|error| CliError::failure(error.to_string()))?;
        let declaration: toml::Value =
            toml::from_str(&declaration).map_err(|error| CliError::failure(error.to_string()))?;
        if let Some(name) = declaration
            .get("rust")
            .and_then(|rust| rust.get("name"))
            .and_then(toml::Value::as_str)
            && !self.closure.units().iter().any(|unit| {
                unit.output()
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|file| file == format!("lib{name}.rlib") || file.starts_with(&format!("lib{name}-")))
            })
        {
            return Err(CliError::failure(format!(
                "SDK component native facet `{name}` is unavailable"
            )));
        }
        Ok(())
    }
}

/// Discover an installed SDK or publish source components with the driver's checked in-process publisher.
pub fn prepare_or_discover_sdk_inventory() -> CliResult<Option<Arc<SdkInventory>>> {
    if let Some(inventory) = incan_provider::inventory::discover_or_reuse_published_sdk_inventory()? {
        return Ok(Some(inventory));
    }
    if std::env::var_os(incan_provider::SDK_PROVIDER_BUILD_ENV).is_some() {
        return Ok(None);
    }
    let Some(stdlib) = oven_model::toolchain_layout::find_stdlib_root()
        .filter(|root| root.join(incan_provider::SDK_SOURCE_CATALOG_FILE).is_file())
    else {
        return Ok(None);
    };
    let store = incan_provider::sdk_store::default_sdk_provider_store(
        &stdlib,
        std::env::var_os("INCAN_HOME"),
        std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")),
    );
    let inputs = incan_provider::sdk_native::SdkNativeInputs::discover(&store)?;
    let catalog = SdkSourceCatalog::read_from_path(&stdlib.join(incan_provider::SDK_SOURCE_CATALOG_FILE))
        .map_err(|error| CliError::failure(error.to_string()))?;
    incan_provider::sdk_build::prepare_sdk_provider_inventory_with_native_publisher(
        None,
        None,
        &inputs,
        |project, output, inventory, closure| {
            publish_component(&catalog, project, output, inventory, closure)
                .map_err(|error| incan_provider::error::ProviderError::failure(error.to_string()))
        },
    )
    .map(Some)
    .map_err(Into::into)
}

/// Publish checked contracts and portable executable surfaces without generating compatibility Cargo metadata.
fn publish_component(
    catalog: &SdkSourceCatalog,
    project: &Path,
    output: &Path,
    inventory: &SdkInventory,
    closure: &SdkCompiledClosure,
) -> CliResult<()> {
    let component = catalog
        .components
        .values()
        .find(|component| component.project_root == project)
        .ok_or_else(|| CliError::failure("native publisher received an unknown SDK component"))?;
    let context = NativeSdkPublicationContext {
        dependencies: oven_model::manifest::ProjectManifest::discover(project)
            .map_err(|error| CliError::failure(error.to_string()))?
            .ok_or_else(|| CliError::failure("SDK component has no manifest"))?
            .rust_dependencies()
            .clone(),
        inventory,
        namespace_roots: component.namespace_roots.clone(),
        closure,
        native_facets: closure
            .units()
            .iter()
            .filter_map(|unit| unit.output().file_stem().and_then(|name| name.to_str()))
            .map(|name| name.trim_start_matches("lib").to_string())
            .collect(),
    };
    let LibraryPreparation::Native { manifest, executable } = prepare_native_sdk_component(project, output, &context)?
    else {
        return Err(CliError::failure("native publisher received an ordinary project plan"));
    };
    let manifest = *manifest;
    let manifest_path = output.join(format!("{}.incnlib", manifest.name));
    let executable_path =
        incan_frontend::library_manifest::published_layout::executable_surface_path(&manifest_path, &manifest)
            .ok_or_else(|| CliError::failure("native provider has no executable surface descriptor"))?;
    let parent = executable_path
        .parent()
        .ok_or_else(|| CliError::failure("native executable surface has no parent"))?;
    std::fs::create_dir_all(parent).map_err(|error| CliError::failure(error.to_string()))?;
    std::fs::write(&executable_path, executable).map_err(|error| {
        CliError::failure(format!(
            "native executable surface {}: {error}",
            executable_path.display()
        ))
    })?;
    manifest
        .write_to_path(&manifest_path)
        .map_err(|error| CliError::failure(error.to_string()))?;
    let native = incan_frontend::library_manifest::NativeProviderArtifact {
        schema_version: 1,
        name: manifest.name,
        version: manifest.version,
        receipts: incan_provider::sdk_native::sdk_native_receipts(closure)?,
        output: compile_native_sdk_facade(&manifest_path, output, &context)?,
    };
    std::fs::write(
        output.join("native-provider.json"),
        serde_json::to_vec_pretty(&native).map_err(|error| CliError::failure(error.to_string()))?,
    )
    .map_err(|error| CliError::failure(error.to_string()))?;
    Ok(())
}

/// Emit a checked native provider's Rust facade without a generated Cargo package or another dependency resolution.
#[allow(clippy::too_many_arguments)]
pub(crate) fn generate_native_sdk_sources(
    output: &Path,
    name: &str,
    modules: &[incan_frontend::ParsedModule],
    checked: &std::collections::BTreeMap<std::path::PathBuf, incan_frontend::typechecker::TypeCheckInfo>,
    stdlib_cache: incan_frontend::typechecker::stdlib_loader::StdlibAstCache,
    provider_plan: &Arc<incan_provider::ProviderPlan>,
    manifest: &mut incan_frontend::library_manifest::LibraryManifest,
    exports: &[incan_frontend::library_exports::CheckedNamedExport],
    declared: &std::collections::HashSet<String>,
    inspection: Option<&Path>,
) -> CliResult<()> {
    use crate::backend::{IrCodegen, ProjectGenerator};
    let root = modules
        .last()
        .ok_or_else(|| CliError::failure("native provider has no root module"))?;
    let dependencies = &modules[..modules.len() - 1];
    let mut codegen = IrCodegen::new();
    codegen.set_standard_library_source(true);
    codegen.set_preserve_dependency_public_items(true);
    codegen.set_registry_package_identity(Some(name.to_string()));
    codegen.set_canonical_emission_package_identity(Some(name.to_string()));
    codegen.set_root_source_module_name(
        root.file_path
            .file_stem()
            .and_then(|name| name.to_str())
            .map(str::to_string),
    );
    codegen.set_stdlib_cache(stdlib_cache);
    codegen.set_declared_crate_names(declared.clone());
    codegen.set_provider_plan(Arc::clone(provider_plan));
    let root_info = checked
        .get(&root.file_path)
        .cloned()
        .ok_or_else(|| CliError::failure("missing checked native root"))?;
    let mut dependency_info = std::collections::HashMap::new();
    let compiled = incan_provider::compiled_sdk::CompiledSdkModules::from_provider_plan(provider_plan);
    let mut paths = Vec::new();
    for module in dependencies {
        let info = checked
            .get(&module.file_path)
            .cloned()
            .ok_or_else(|| CliError::failure("missing checked native dependency"))?;
        dependency_info.insert(module.path_segments.clone(), info);
        if compiled.contains_emission_path(&module.path_segments) {
            codegen.add_dependency_symbol_module_with_path_segments(
                &module.name,
                &module.ast,
                module.path_segments.clone(),
            );
        } else {
            codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
            paths.push(module.path_segments.clone());
        }
    }
    codegen.set_prechecked_type_info(root_info, dependency_info);
    codegen.set_public_ordinal_type_identities(crate::build::library_exports::public_ordinal_type_identities(
        root, name, exports,
    ));
    #[cfg(feature = "rust_inspect")]
    if let Some(inspection) = inspection {
        codegen.set_rust_inspect_manifest_dir(inspection.to_path_buf());
    }
    #[cfg(not(feature = "rust_inspect"))]
    let _ = inspection;
    codegen.set_publication_api(manifest.contract_metadata.api.clone());
    codegen.set_publication_identities(name.to_string(), manifest.contract_metadata.identity_graph.clone());
    let mut generator = ProjectGenerator::new(output, name, false);
    generator.set_native_sdk_publication();
    generator.set_provider_plan(provider_plan);
    if let Some(api) = manifest.contract_metadata.api.as_ref() {
        generator.set_public_namespace_facades(api);
    }
    let ((source, generated), metadata) = codegen
        .try_generate_multi_file_nested_with_metadata(&root.ast, &paths, &root.path_segments)
        .map_err(|error| CliError::failure(format!("native SDK source emission failed: {error}")))?;
    generator
        .generate_nested(&source, &generated)
        .map_err(|error| CliError::failure(error.to_string()))?;
    metadata
        .apply_to_library_manifest(manifest)
        .map_err(|error| CliError::failure(error.to_string()))?;
    Ok(())
}

/// Compile the generated facade against exact native units and already published dependency providers.
///
/// The enclosing source/receipt identity owns this staging tree. The admitted descriptor seals the output digest,
/// and the complete publication is renamed atomically only after source-current identity validation.
fn compile_native_sdk_facade(
    manifest_path: &Path,
    output: &Path,
    context: &NativeSdkPublicationContext<'_>,
) -> CliResult<incan_frontend::library_manifest::NativeProviderOutput> {
    use crate::backend::ProjectGenerator;
    let manifest = incan_frontend::library_manifest::LibraryManifest::read_from_path(manifest_path)
        .map_err(|error| CliError::failure(error.to_string()))?;
    let crate_name = ProjectGenerator::rust_target_name(&manifest.name);
    let relative_path = format!("native/lib{crate_name}.rlib");
    let artifact = output.join(&relative_path);
    std::fs::create_dir_all(output.join("native")).map_err(|error| CliError::failure(error.to_string()))?;
    let rustc = oven_rustc::rustc::resolve_active_rustc().map_err(|error| CliError::failure(error.to_string()))?;
    let mut command = std::process::Command::new(rustc);
    oven_rustc::rustc::clear_inherited_cargo_environment(&mut command);
    command
        .arg(output.join("src/lib.rs"))
        .args(["--edition=2024", "--crate-type=rlib", "--crate-name"])
        .arg(&crate_name)
        .args(["-C", "debuginfo=0", "--remap-path-prefix"])
        .arg(format!(
            "{}=incan-sdk://{}",
            context.inventory.root.display(),
            manifest.name
        ))
        .arg("-o")
        .arg(&artifact);
    let generated_sources = native_facade_source_text(&output.join("src"))?;
    let mut externs = std::collections::BTreeMap::new();
    let mut paths = BTreeSet::new();
    for unit in context.closure.units() {
        let path = unit.output();
        let name = path
            .file_stem()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("lib"))
            .ok_or_else(|| CliError::failure("native facade input has no crate name"))?;
        let matches_component = context
            .dependencies
            .get(name)
            .map(|dependency| native_facade_binding_matches(dependency, unit.binding()))
            .transpose()?
            .unwrap_or(true);
        if matches_component
            && generated_sources.contains(&format!("{name}::"))
            && (unit.binding().domain == "target" || path.extension().is_some_and(|extension| extension == "dylib"))
            && externs.insert(name.to_string(), path.to_path_buf()).is_some()
        {
            return Err(CliError::failure(format!("native facade has ambiguous input `{name}`")));
        }
        if let Some(parent) = path.parent() {
            paths.insert(parent.to_path_buf());
        }
    }
    for provider in context
        .inventory
        .components
        .values()
        .flat_map(|component| &component.providers)
    {
        let (Some(root), Some(path)) = (&provider.crate_root, &provider.manifest_path) else {
            continue;
        };
        let dependency = incan_frontend::library_manifest::LibraryManifest::read_from_path(path)
            .map_err(|error| CliError::failure(error.to_string()))?;
        let native = incan_frontend::library_manifest::read_native_provider_artifact(root, &dependency)
            .map_err(|error| CliError::failure(error.to_string()))?
            .ok_or_else(|| CliError::failure(format!("SDK dependency {} has no native descriptor", provider.name)))?;
        let path = root.join(native.output.relative_path);
        if let Some(parent) = path.parent() {
            paths.insert(parent.to_path_buf());
        }
        if externs.insert(native.output.crate_name.clone(), path).is_some() {
            return Err(CliError::failure(format!(
                "SDK dependency {} conflicts with a native facet",
                provider.name
            )));
        }
    }
    for path in paths {
        command.arg("-L").arg(format!("dependency={}", path.display()));
    }
    for (name, path) in externs {
        command.arg("--extern").arg(format!("{name}={}", path.display()));
    }
    let result = command.output().map_err(|error| CliError::failure(error.to_string()))?;
    if !result.status.success() {
        return Err(CliError::failure(format!(
            "native SDK facade {} failed:\n{}",
            manifest.name,
            String::from_utf8_lossy(&result.stderr)
        )));
    }
    let digest =
        oven_store::digest_bytes(&std::fs::read(&artifact).map_err(|error| CliError::failure(error.to_string()))?);
    Ok(incan_frontend::library_manifest::NativeProviderOutput {
        crate_name,
        relative_path,
        digest,
    })
}

/// Match a facade's direct registry requirement without selecting another version from the compiler closure.
fn native_facade_binding_matches(
    dependency: &oven_model::manifest::DependencySpec,
    binding: &oven_rustc::sdk_closure::SdkLockedUnit,
) -> CliResult<bool> {
    if !matches!(dependency.source, oven_model::manifest::DependencySource::Registry) {
        return Ok(true);
    }
    let package = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
    if binding.loaf != format!("crates-io/{package}") {
        return Ok(false);
    }
    let version = semver::Version::parse(&binding.version).map_err(|error| CliError::failure(error.to_string()))?;
    let requirement = dependency
        .version
        .as_deref()
        .map(semver::VersionReq::parse)
        .transpose()
        .map_err(|error| CliError::failure(error.to_string()))?;
    Ok(requirement.is_none_or(|requirement| requirement.matches(&version))
        && dependency
            .features
            .iter()
            .all(|feature| binding.features.contains(feature)))
}

/// Read only generated Rust sources to select the facade's explicit external crate names.
fn native_facade_source_text(root: &Path) -> CliResult<String> {
    let mut text = String::new();
    for entry in std::fs::read_dir(root).map_err(|error| CliError::failure(error.to_string()))? {
        let entry = entry.map_err(|error| CliError::failure(error.to_string()))?;
        let path = entry.path();
        if path.is_dir() {
            text.push_str(&native_facade_source_text(&path)?);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            text.push_str(&std::fs::read_to_string(path).map_err(|error| CliError::failure(error.to_string()))?);
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    /// A reordered SDK catalog reuses its plan identity, while changed native units invalidate that identity.
    #[test]
    fn native_catalog_binding_is_canonical_and_source_sensitive() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("root.rs");
        std::fs::write(&source, "fn main() {}")?;
        let source_receipt = oven_store::receipt_generated_project(
            &oven_store::OvenGeneratedProjectRequest::new(
                root.path(),
                "fixture",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc fixture",
                "debug",
                Vec::new(),
            )
            .with_generated_source("test-root", source),
        )?;
        let first = super::bind_native_catalog(&source_receipt, br#"{"b":"unit-b","a":"unit-a"}"#)?;
        let reordered = super::bind_native_catalog(&source_receipt, br#"{"a":"unit-a","b":"unit-b"}"#)?;
        let changed = super::bind_native_catalog(&source_receipt, br#"{"a":"unit-a-changed","b":"unit-b"}"#)?;
        assert_eq!(first.identity, reordered.identity);
        assert_ne!(first.build_unit_identity, changed.build_unit_identity);
        first.verify_identity()?;
        changed.verify_identity()?;
        Ok(())
    }

    /// A compiler companion's older rustix must not compete with the SDK component's declared major version.
    #[test]
    fn native_facade_selects_component_dependency_version() -> Result<(), Box<dyn std::error::Error>> {
        let mut dependency = oven_model::manifest::DependencySpec {
            crate_name: "rustix".into(),
            version: Some("1.1".into()),
            features: vec!["fs".into()],
            default_features: true,
            source: oven_model::manifest::DependencySource::Registry,
            optional: false,
            package: None,
        };
        let mut binding = oven_rustc::sdk_closure::SdkLockedUnit {
            loaf: "crates-io/rustix".into(),
            version: "0.38.44".into(),
            archive_digest: "test".into(),
            domain: "target".into(),
            features: vec!["fs".into()],
            target_predicates: Vec::new(),
            edges: None,
        };
        assert!(!super::native_facade_binding_matches(&dependency, &binding)?);
        binding.version = "1.1.5".into();
        assert!(super::native_facade_binding_matches(&dependency, &binding)?);
        binding.features.clear();
        assert!(!super::native_facade_binding_matches(&dependency, &binding)?);
        dependency.source = oven_model::manifest::DependencySource::Path {
            path: "/sdk/core".into(),
        };
        binding.loaf = "incan_std_core".into();
        assert!(super::native_facade_binding_matches(&dependency, &binding)?);
        Ok(())
    }
}
