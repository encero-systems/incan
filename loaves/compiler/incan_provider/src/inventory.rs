//! Discovering the active SDK inventory and projecting the selected components into project requirements.
//!
//! A command either finds a published inventory or prepares one (`sdk_build`), then narrows it to the components
//! the manifest and the program's imports actually use.

#[cfg(test)]
use std::cell::Cell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{env, fs};

use incan_core::lang::stdlib;

use crate::error::{ProviderError, ProviderResult};
use crate::requirements::{ProjectRequirements, merge_requirement_dependency};
use crate::sdk_build::prepare_sdk_provider_inventory;
use crate::{
    BackendImplementationRequirement, ProviderPlan, ResolvedSdkComponents, SDK_INVENTORY_FILE, SDK_PROVIDER_BUILD_ENV,
    SDK_SOURCE_CATALOG_FILE, SdkArtifactProjection, SdkComponentSelection, SdkDependencyRebinding, SdkInventory,
    SdkResolutionError, SdkSourceCatalog,
};
use incan_frontend::ast::ImportKind;
use incan_frontend::library_manifest::{ProviderCargoDependency, ProviderCargoDependencySource};
use incan_frontend::parsed_module::ParsedModule;
use oven_model::manifest::{DependencySource, DependencySpec, ProjectManifest};
/// Explicit active SDK inventory override used by toolchain selection and SDK publication.
pub const SDK_INVENTORY_OVERRIDE_ENV: &str = "INCAN_SDK_INVENTORY";

#[cfg(test)]
thread_local! {
    static COMPILATION_SESSION_ANALYSIS_INVOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Discover the active component-aware SDK relative to the selected toolchain or an explicit override. Discover the
/// installed SDK inventory without publishing source-checkout providers.
///
/// Oven consumers use this narrow read-only path. A normal command must treat an absent inventory as an explicit
/// preparation requirement, never as authority to invoke the legacy Cargo publisher.
pub fn discover_active_sdk_inventory() -> ProviderResult<Option<Arc<SdkInventory>>> {
    let explicit = env::var_os(SDK_INVENTORY_OVERRIDE_ENV)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    let inventory_path = if let Some(path) = explicit.as_ref() {
        if !path.is_file() {
            return Err(ProviderError::failure(format!(
                "{SDK_INVENTORY_OVERRIDE_ENV} points to missing SDK inventory {}",
                path.display()
            )));
        }
        Some(path.clone())
    } else {
        oven_model::toolchain_layout::current_executable_search_bases()
            .into_iter()
            .flat_map(|base| {
                [
                    base.join(SDK_INVENTORY_FILE),
                    base.join("share").join("incan").join(SDK_INVENTORY_FILE),
                    base.join("share").join("incan").join("sdk").join(SDK_INVENTORY_FILE),
                ]
            })
            .find(|path| path.is_file())
    };
    let Some(path) = inventory_path else {
        return Ok(None);
    };
    let inventory = SdkInventory::read_from_path(&path).map_err(|error| ProviderError::failure(error.to_string()))?;
    inventory
        .validate_compiler_compatibility(
            incan_core::version::INCAN_VERSION,
            incan_core::version::SDK_PROVIDER_CODEGEN_REVISION,
        )
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    Ok(Some(Arc::new(inventory)))
}

/// Discover an installed SDK inventory or publish the source checkout's component providers on demand.
pub fn prepare_or_discover_sdk_inventory() -> ProviderResult<Option<Arc<SdkInventory>>> {
    if let Some(inventory) = discover_active_sdk_inventory()? {
        return Ok(Some(inventory));
    }
    if env::var_os(SDK_PROVIDER_BUILD_ENV).is_some() {
        return Ok(None);
    }
    let has_source_catalog = oven_model::toolchain_layout::find_stdlib_root()
        .is_some_and(|root| root.join(SDK_SOURCE_CATALOG_FILE).is_file());
    if has_source_catalog {
        prepare_sdk_provider_inventory().map(Some)
    } else {
        Ok(None)
    }
}

/// Reject explicit component-aware selection when the active toolchain exposes only the legacy monolithic SDK.
pub fn validate_component_inventory_selection(
    manifest: Option<&ProjectManifest>,
    sdk_profile_override: Option<&str>,
    inventory: Option<&SdkInventory>,
) -> ProviderResult<()> {
    let explicit_selection = manifest.and_then(ProjectManifest::sdk).is_some() || sdk_profile_override.is_some();
    if inventory.is_some() || !explicit_selection {
        return Ok(());
    }
    let message = concat!(
        "the active Incan SDK has no component inventory, so explicit `[sdk]` selection is unavailable; ",
        "use a component-aware v0.5 SDK or remove the explicit SDK selection",
    );
    let message = manifest
        .and_then(|manifest| sdk_manifest_value_location(manifest, &[]))
        .map_or_else(|| message.to_string(), |location| format!("{location}: {message}"));
    Err(ProviderError::failure(message))
}

/// Resolve one SDK component selection and retain the manifest or command provenance of configuration failures.
pub fn resolve_sdk_component_selection(
    inventory: &SdkInventory,
    selection: &SdkComponentSelection,
    manifest: Option<&ProjectManifest>,
    sdk_profile_override: Option<&str>,
    require_available: bool,
) -> ProviderResult<ResolvedSdkComponents> {
    let result = if require_available {
        inventory.resolve(selection)
    } else {
        inventory.resolve_catalog(selection)
    };
    result.map_err(|error| {
        ProviderError::failure(format_sdk_selection_error(
            &error,
            selection,
            manifest,
            sdk_profile_override,
        ))
    })
}

/// Attach exact `[sdk]` source provenance to one component-resolution failure when it came from the manifest.
fn format_sdk_selection_error(
    error: &SdkResolutionError,
    selection: &SdkComponentSelection,
    manifest: Option<&ProjectManifest>,
    sdk_profile_override: Option<&str>,
) -> String {
    if matches!(error, SdkResolutionError::UnknownProfile { profile, .. } if sdk_profile_override == Some(profile)) {
        return format!("{error} (selected by the current command's `--sdk-profile` override)");
    }
    let candidates = match error {
        SdkResolutionError::UnknownProfile { profile, .. } => vec![profile.clone()],
        SdkResolutionError::UnknownComponent { component, .. }
        | SdkResolutionError::MandatoryComponentExcluded { component, .. }
        | SdkResolutionError::SelectedComponentExcluded { component }
        | SdkResolutionError::ExcludedRequiredComponent { component, .. }
        | SdkResolutionError::EnabledComponentUnavailable { component, .. } => {
            vec![component.clone(), selection.profile.clone()]
        }
    };
    manifest
        .and_then(|manifest| sdk_manifest_value_location(manifest, &candidates))
        .map_or_else(|| error.to_string(), |location| format!("{location}: {error}"))
}

/// Locate an authored SDK selection value, falling back to the `[sdk]` table header for derived failures.
fn sdk_manifest_value_location(manifest: &ProjectManifest, candidates: &[String]) -> Option<String> {
    let content = fs::read_to_string(manifest.path()).ok()?;
    let mut in_sdk = false;
    let mut sdk_header = None;
    let mut sdk_lines = Vec::new();
    for (line_index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_sdk = trimmed == "[sdk]";
            if in_sdk {
                sdk_header = Some(format!("{}:{}:1", manifest.path().display(), line_index + 1));
            }
            continue;
        }
        if !in_sdk {
            continue;
        }
        sdk_lines.push((line_index, line));
    }
    for candidate in candidates {
        for (line_index, line) in &sdk_lines {
            let quoted = format!("\"{candidate}\"");
            if let Some(column) = line.find(&quoted) {
                return Some(format!(
                    "{}:{}:{}",
                    manifest.path().display(),
                    line_index + 1,
                    column + 1
                ));
            }
        }
    }
    sdk_header
}

/// Add linked generated crates selected by active compiled providers to the current backend requirements.
pub fn extend_requirements_with_provider_plan(
    requirements: &mut ProjectRequirements,
    provider_plan: &ProviderPlan,
) -> ProviderResult<()> {
    let sdk_providers = provider_plan
        .sdk_link_roots()
        .into_iter()
        .map(|provider| provider.identity.stable_key())
        .collect::<BTreeSet<_>>();
    extend_requirements_with_selected_sdk_providers(requirements, provider_plan, &sdk_providers)
}

/// Add ordinary providers and the selected direct SDK provider set to backend requirements.
fn extend_requirements_with_selected_sdk_providers(
    requirements: &mut ProjectRequirements,
    provider_plan: &ProviderPlan,
    sdk_providers: &BTreeSet<String>,
) -> ProviderResult<()> {
    // Projection helpers do not retain the ProviderPlan. Preserve the complete active SDK path catalog separately
    // from the minimal set of providers linked directly into this generated crate: a copied compiled artifact can
    // still carry a non-descriptor Cargo edge to an active provider supplied transitively or unused by this consumer.
    for provider in provider_plan.active_sdk_records() {
        if let Some(artifact) = provider.artifact.as_ref() {
            merge_requirement_dependency(
                &mut requirements.sdk_path_dependencies,
                artifact.to_dependency_spec(),
                format!("active SDK provider `{}`", provider.identity.name),
            )?;
        }
        // Semantic identity hashes the complete compiled artifact, including dependencies of facets this consumer
        // does not select. Keep their exact toolchain coordinates in the catalog so a narrow consumer and the Loaf
        // publisher normalize the same provider bytes identically. Actual links remain selected in the loop below.
        for requirement in provider
            .implementation_facets
            .iter()
            .flat_map(|facet| &facet.backend_requirements)
        {
            let BackendImplementationRequirement::CargoDependency { dependency } = requirement else {
                continue;
            };
            if matches!(dependency.source, ProviderCargoDependencySource::Toolchain { .. }) {
                merge_requirement_dependency(
                    &mut requirements.sdk_path_dependencies,
                    provider_cargo_dependency_spec(dependency),
                    format!("active SDK provider `{}` toolchain dependency", provider.identity.name),
                )?;
            }
        }
    }
    for provider in provider_plan.active_records() {
        if matches!(provider.authority, crate::NamespaceAuthority::SdkReserved)
            && !sdk_providers.contains(&provider.identity.stable_key())
        {
            continue;
        }
        let Some(artifact) = provider.artifact.as_ref() else {
            continue;
        };
        let mut provider_dependency = artifact.to_dependency_spec();
        if matches!(provider.authority, crate::NamespaceAuthority::SdkReserved) {
            // Checked private SDK edges freeze an exact feature projection and never inherit the provider crate's
            // conventional Cargo defaults. Emit that contract explicitly so future artifacts need no legacy repair.
            provider_dependency.default_features = false;
        }
        merge_requirement_dependency(
            &mut requirements.dependencies,
            provider_dependency,
            format!("compiled provider `{}`", provider.identity.name),
        )?;
        for requirement in provider_plan.selected_backend_requirements(provider) {
            if let BackendImplementationRequirement::CargoDependency { dependency } = requirement {
                let dependency_spec = provider_cargo_dependency_spec(&dependency);
                if matches!(dependency.source, ProviderCargoDependencySource::Toolchain { .. }) {
                    merge_requirement_dependency(
                        &mut requirements.sdk_path_dependencies,
                        dependency_spec.clone(),
                        format!("compiled provider `{}` toolchain dependency", provider.identity.name),
                    )?;
                }
                merge_requirement_dependency(
                    &mut requirements.dependencies,
                    dependency_spec,
                    format!("compiled provider `{}` implementation facet", provider.identity.name),
                )?;
            }
        }
        for requirement in provider_plan.selected_backend_requirements(provider) {
            let BackendImplementationRequirement::CargoFeature { crate_name, feature } = requirement else {
                continue;
            };
            let Some(dependency) = requirements
                .dependencies
                .iter_mut()
                .find(|dependency| dependency.crate_name == crate_name)
            else {
                return Err(ProviderError::failure(format!(
                    "provider `{}` implementation facet selects Cargo feature `{crate_name}/{feature}` without declaring that dependency",
                    provider.identity.name
                )));
            };
            dependency.features.push(feature);
            dependency.features.sort();
            dependency.features.dedup();
        }
    }
    requirements
        .sdk_dependency_rebindings
        .extend_from_slice(provider_plan.sdk_dependency_rebindings());
    normalize_sdk_dependency_rebindings(&mut requirements.sdk_dependency_rebindings);
    requirements
        .sdk_artifact_projections
        .extend_from_slice(provider_plan.sdk_artifact_projections());
    normalize_sdk_artifact_projections(&mut requirements.sdk_artifact_projections);
    requirements.stdlib_facets.sort();
    requirements.stdlib_facets.dedup();
    Ok(())
}

/// Sort and de-duplicate physical SDK projections independently of module-group traversal order.
pub fn normalize_sdk_dependency_rebindings(rebindings: &mut Vec<SdkDependencyRebinding>) {
    rebindings.sort_by(|left, right| {
        (
            &left.containing_artifact.crate_root,
            &left.provider_name,
            &left.dependency_key,
            &left.source_crate_root,
            &left.active_crate_root,
        )
            .cmp(&(
                &right.containing_artifact.crate_root,
                &right.provider_name,
                &right.dependency_key,
                &right.source_crate_root,
                &right.active_crate_root,
            ))
    });
    rebindings.dedup();
}

/// Sort and de-duplicate projected artifacts by their immutable compiled crate root.
pub fn normalize_sdk_artifact_projections(projections: &mut Vec<SdkArtifactProjection>) {
    projections.sort_by(|left, right| left.artifact.crate_root.cmp(&right.artifact.crate_root));
    projections.dedup_by(|left, right| left.artifact.crate_root == right.artifact.crate_root);
}

/// Translate relocatable provider-owned Cargo metadata at the final Rust-backend boundary.
fn provider_cargo_dependency_spec(dependency: &ProviderCargoDependency) -> DependencySpec {
    let source = match &dependency.source {
        ProviderCargoDependencySource::Registry => DependencySource::Registry,
        ProviderCargoDependencySource::Toolchain { relative_path } => DependencySource::Path {
            path: oven_model::toolchain_layout::resolve_toolchain_relative_path(Path::new(relative_path)),
        },
    };
    DependencySpec {
        crate_name: dependency.crate_name.clone(),
        version: dependency.version.clone(),
        features: dependency.features.iter().cloned().collect(),
        default_features: dependency.default_features,
        source,
        optional: false,
        package: dependency.package.clone(),
    }
    .normalized()
}

/// Collect canonical provider module use from resolved source modules and authored import edges.
pub fn provider_used_module_paths(modules: &[ParsedModule]) -> BTreeSet<Vec<String>> {
    let mut used = BTreeSet::new();
    if !modules.is_empty() && env::var_os(SDK_PROVIDER_BUILD_ENV).is_none() {
        // Every ordinary compilation consumes the implicit language prelude. Recording that compiler requirement
        // keeps the mandatory core provider linked even when generated support such as iterator adapters is the only
        // emitted path into `std.derives.*`.
        used.insert(vec![stdlib::STDLIB_ROOT.to_string(), "prelude".to_string()]);
    }
    for module in modules {
        if module.path_segments.first().map(String::as_str) == Some(stdlib::INCAN_STD_NAMESPACE) {
            let mut canonical = vec![stdlib::STDLIB_ROOT.to_string()];
            canonical.extend(module.path_segments.iter().skip(1).cloned());
            used.insert(canonical);
        }
        for declaration in &module.ast.declarations {
            let incan_frontend::ast::Declaration::Import(import) = &declaration.node else {
                continue;
            };
            // Root imports name their provider module in each imported item, not in the `std` path itself.
            if let ImportKind::From { module: path, items } = &import.kind
                && path.parent_levels == 0
                && !path.is_absolute
                && path.segments.as_slice() == [stdlib::STDLIB_ROOT]
            {
                used.extend(
                    items
                        .iter()
                        .map(|item| vec![stdlib::STDLIB_ROOT.to_string(), item.name.clone()]),
                );
                continue;
            }
            let path = match &import.kind {
                ImportKind::Module(path) | ImportKind::From { module: path, .. }
                    if path.parent_levels == 0
                        && !path.is_absolute
                        && path.segments.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT) =>
                {
                    Some(path.segments.clone())
                }
                _ => None,
            };
            used.extend(path);
        }
    }
    used
}

/// Resolve the reserved namespace roots granted to the SDK component currently being compiled from source.
pub fn sdk_provider_bootstrap_namespace_roots(project_root: &Path) -> ProviderResult<BTreeSet<String>> {
    let Some(component_marker) = env::var_os(SDK_PROVIDER_BUILD_ENV).filter(|value| !value.is_empty()) else {
        return Ok(BTreeSet::new());
    };
    let stdlib_root = oven_model::toolchain_layout::find_stdlib_root().ok_or_else(|| {
        ProviderError::failure("cannot locate the SDK source catalog while compiling an SDK provider")
    })?;
    let catalog = SdkSourceCatalog::read_from_path(&stdlib_root.join(SDK_SOURCE_CATALOG_FILE))
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    let component_marker = component_marker.to_string_lossy();
    let canonical_project_root = fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let component = catalog.components.get(component_marker.as_ref()).or_else(|| {
        catalog.components.values().find(|component| {
            fs::canonicalize(&component.project_root).unwrap_or_else(|_| component.project_root.clone())
                == canonical_project_root
        })
    });
    let component = component.ok_or_else(|| {
        ProviderError::failure(format!(
            "SDK provider bootstrap marker `{component_marker}` does not match a component in {}",
            stdlib_root.join(SDK_SOURCE_CATALOG_FILE).display()
        ))
    })?;
    Ok(component.namespace_roots.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::parsed_module_for_test;
    use incan_frontend::library_manifest::LibraryManifest;
    use incan_frontend::library_manifest_index::LibraryArtifactMetadata;

    #[test]
    fn explicit_sdk_selection_rejects_legacy_inventoryless_toolchains() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let manifest_path = project.path().join("loaf.toml");
        fs::write(
            &manifest_path,
            "[project]\nname = \"demo\"\n\n[sdk]\nprofile = \"minimal\"\n",
        )?;
        let manifest = ProjectManifest::load(&manifest_path)?;
        let error = validate_component_inventory_selection(Some(&manifest), None, None)
            .err()
            .ok_or("expected explicit SDK selection to require an inventory")?;

        assert!(error.message.contains("no component inventory"));
        assert!(
            error.message.contains("loaf.toml:4:1"),
            "expected the explicit SDK table location, got: {}",
            error.message
        );
        assert!(validate_component_inventory_selection(None, None, None).is_ok());
        Ok(())
    }

    #[test]
    fn sdk_selection_errors_retain_manifest_or_command_provenance() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let manifest_path = project.path().join("loaf.toml");
        fs::write(
            &manifest_path,
            "[project]\nname = \"demo\"\n\n[sdk]\nprofile = \"minimal\"\ncomponents = [\"stdlib-web\"]\n",
        )?;
        let manifest = ProjectManifest::load(&manifest_path)?;
        let selection = SdkComponentSelection::from_manifest(Some(&manifest));
        let component_error = SdkResolutionError::UnknownComponent {
            component: "stdlib-web".to_string(),
            sdk_identity: "incan@0.5.0".to_string(),
        };

        let rendered = format_sdk_selection_error(&component_error, &selection, Some(&manifest), None);
        assert!(
            rendered.contains("loaf.toml:6:15"),
            "expected exact SDK component location, got: {rendered}"
        );

        let profile_error = SdkResolutionError::UnknownProfile {
            profile: "tiny".to_string(),
            sdk_identity: "incan@0.5.0".to_string(),
        };
        let rendered = format_sdk_selection_error(&profile_error, &selection, Some(&manifest), Some("tiny"));
        assert!(
            rendered.contains("current command's `--sdk-profile` override"),
            "expected transient profile provenance, got: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn root_std_imports_select_the_same_provider_modules_as_qualified_imports() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = parsed_module_for_test("from std import math as arithmetic, serde\n")?;
        let qualified = parsed_module_for_test("import std.math\nimport std.serde\n")?;
        let root_paths = provider_used_module_paths(&[root]);
        assert_eq!(root_paths, provider_used_module_paths(&[qualified]));
        assert!(root_paths.contains(&vec!["std".to_string(), "math".to_string()]));
        assert!(root_paths.contains(&vec!["std".to_string(), "serde".to_string()]));
        assert!(!root_paths.contains(&vec!["std".to_string()]));
        Ok(())
    }

    #[test]
    fn root_std_provider_discovery_excludes_external_and_relative_imports() -> Result<(), Box<dyn std::error::Error>> {
        let external = parsed_module_for_test(
            "from rust::std import cmp\nfrom pub::std import math\nfrom ..std import serde\nfrom crate.std import json\n",
        )?;
        let empty = parsed_module_for_test("def main() -> None:\n    pass\n")?;
        assert_eq!(
            provider_used_module_paths(&[external]),
            provider_used_module_paths(&[empty])
        );
        Ok(())
    }

    /// A provider ships its complete artifact even when a caller selects only one of its implementation facets.
    #[test]
    fn sdk_provider_identity_is_independent_of_selected_facets() -> Result<(), Box<dyn std::error::Error>> {
        use incan_frontend::library_manifest::{ProviderImplementationFacet, digest_provider_artifact};
        use incan_frontend::provider::ImplementationFacet;

        let workspace = tempfile::tempdir()?;
        // Resolve a valid relocatable toolchain coordinate in an isolated child, without changing this test
        // process's environment while other provider tests run concurrently.
        const CHILD: &str = "INCAN_TEST_PROVIDER_IDENTITY_CHILD";
        if env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(env::current_exe()?)
                .args([
                    "--exact",
                    "inventory::tests::sdk_provider_identity_is_independent_of_selected_facets",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env("INCAN_TOOLCHAIN_CRATES_DIR", workspace.path())
                .output()?;
            assert!(
                output.status.success(),
                "provider identity child failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return Ok(());
        }
        let runtime = PathBuf::from(env::var_os("INCAN_TOOLCHAIN_CRATES_DIR").ok_or("child needs toolchain root")?)
            .join("facet_runtime");
        fs::create_dir_all(runtime.join("src"))?;
        fs::write(
            runtime.join("Cargo.toml"),
            "[package]\nname = \"facet_runtime\"\nversion = \"1.0.0\"\n",
        )?;
        fs::write(runtime.join("src/lib.rs"), "pub fn marker() {}\n")?;
        let dependency = ProviderCargoDependency {
            crate_name: "facet_runtime".to_string(),
            package: None,
            version: None,
            features: BTreeSet::new(),
            default_features: true,
            source: ProviderCargoDependencySource::Toolchain {
                relative_path: "crates/facet_runtime".to_string(),
            },
        };
        let provider_root = workspace.path().join("provider");
        fs::create_dir_all(provider_root.join("src"))?;
        fs::write(provider_root.join("src/lib.rs"), "pub fn provider() {}\n")?;
        fs::write(
            provider_root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"facet_provider\"\nversion = \"1.0.0\"\n[dependencies.facet_runtime]\npath = {:?}\n",
                runtime
            ),
        )?;
        let mut manifest = LibraryManifest::new("facet_provider", "1.0.0");
        manifest
            .contract_metadata
            .provider
            .implementation_facets
            .push(ProviderImplementationFacet {
                id: "rich".to_string(),
                required_modules: BTreeSet::from([vec!["rich".to_string()]]),
                required_features: BTreeSet::new(),
                cargo_features: Default::default(),
                cargo_dependencies: vec![dependency.clone()],
            });
        let manifest_path = provider_root.join("facet_provider.incnlib");
        manifest.write_to_path(&manifest_path)?;
        let record = crate::ProviderRecord {
            identity: crate::ProviderIdentity {
                name: "facet_provider".to_string(),
                version: "1.0.0".to_string(),
                digest: digest_provider_artifact(&provider_root)?,
                feature_projection: BTreeSet::new(),
            },
            provenance: crate::ProviderProvenance::Sdk {
                sdk_identity: "incan@1.0.0".to_string(),
                component_id: "facet-provider".to_string(),
                inventory_path: None,
            },
            authority: crate::NamespaceAuthority::SdkReserved,
            namespace_claims: BTreeSet::from([
                vec!["std".to_string(), "plain".to_string()],
                vec!["std".to_string(), "rich".to_string()],
            ]),
            available: true,
            enabled: true,
            manifest: Some(Arc::new(manifest)),
            artifact: Some(LibraryArtifactMetadata::from_manifest_path(
                "facet_provider",
                "facet_provider",
                manifest_path,
                provider_root,
            )),
            implementation_facets: vec![ImplementationFacet {
                id: "rich".to_string(),
                required_modules: BTreeSet::from([vec!["rich".to_string()]]),
                required_features: BTreeSet::new(),
                backend_requirements: vec![BackendImplementationRequirement::CargoDependency { dependency }],
            }],
        };
        let plan = |module: &str| -> Result<ProviderPlan, Box<dyn std::error::Error>> {
            Ok(ProviderPlan::new(
                Default::default(),
                vec![record.clone()],
                [vec!["std".to_string(), module.to_string()]],
            )?)
        };
        let narrow = plan("plain")?;
        let broad = plan("rich")?;
        let mut narrow_requirements = ProjectRequirements::default();
        let mut broad_requirements = ProjectRequirements::default();
        extend_requirements_with_provider_plan(&mut narrow_requirements, &narrow)?;
        extend_requirements_with_provider_plan(&mut broad_requirements, &broad)?;
        let narrow_identity =
            crate::lock_semantics::provider_semantic_identities(&narrow, &narrow_requirements.sdk_path_dependencies)?;
        let broad_identity =
            crate::lock_semantics::provider_semantic_identities(&broad, &broad_requirements.sdk_path_dependencies)?;
        assert_eq!(
            narrow_identity, broad_identity,
            "consumer facet selection must not change provider identity"
        );
        assert!(
            !narrow_requirements
                .dependencies
                .iter()
                .any(|spec| spec.crate_name == "facet_runtime")
        );
        assert!(
            broad_requirements
                .dependencies
                .iter()
                .any(|spec| spec.crate_name == "facet_runtime")
        );
        fs::write(runtime.join("src/lib.rs"), "pub fn changed_marker() {}\n")?;
        let changed =
            crate::lock_semantics::provider_semantic_identities(&narrow, &narrow_requirements.sdk_path_dependencies)?;
        assert_ne!(
            narrow_identity, changed,
            "runtime content must remain identity authority"
        );
        Ok(())
    }

    #[test]
    fn helper_requirements_keep_unused_active_sdk_path_targets_issue911() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let artifact = workspace.path().join("unused-sdk-provider");
        let record = crate::ProviderRecord {
            identity: crate::ProviderIdentity {
                name: "incan_issue911_unused_sdk".to_string(),
                version: "0.5.0".to_string(),
                digest: "sha256:issue911-unused".to_string(),
                feature_projection: BTreeSet::new(),
            },
            provenance: crate::ProviderProvenance::Sdk {
                sdk_identity: "incan@0.5.0".to_string(),
                component_id: "issue911-unused".to_string(),
                inventory_path: None,
            },
            authority: crate::NamespaceAuthority::SdkReserved,
            namespace_claims: BTreeSet::from([vec!["std".to_string(), "issue911_unused".to_string()]]),
            available: true,
            enabled: true,
            manifest: Some(Arc::new(LibraryManifest::new("incan_issue911_unused_sdk", "0.5.0"))),
            artifact: Some(LibraryArtifactMetadata::from_crate_root(
                "incan_issue911_unused_sdk",
                "incan_issue911_unused_sdk",
                &artifact,
            )),
            implementation_facets: Vec::new(),
        };
        let plan = ProviderPlan::new(
            incan_frontend::library_manifest_index::LibraryManifestIndex::default(),
            vec![record.clone()],
            std::iter::empty(),
        )?;
        assert!(
            plan.sdk_link_roots().is_empty(),
            "unused SDK provider must not become a direct link root"
        );

        let mut requirements = ProjectRequirements::default();
        extend_requirements_with_provider_plan(&mut requirements, &plan)?;

        assert!(
            requirements.dependencies.is_empty(),
            "unused provider must not be linked directly"
        );
        assert_eq!(requirements.sdk_path_dependencies.len(), 1);
        assert!(matches!(
            &requirements.sdk_path_dependencies[0].source,
            DependencySource::Path { path } if path == &artifact
        ));

        let used_plan = ProviderPlan::new(
            incan_frontend::library_manifest_index::LibraryManifestIndex::default(),
            vec![record],
            provider_used_module_paths(&[parsed_module_for_test("from std import issue911_unused as selected\n")?]),
        )?;
        let mut used_requirements = ProjectRequirements::default();
        extend_requirements_with_provider_plan(&mut used_requirements, &used_plan)?;
        assert_eq!(used_requirements.dependencies.len(), 1);
        assert!(
            !used_requirements.dependencies[0].default_features,
            "new direct SDK edges must render explicit default-features = false"
        );
        Ok(())
    }
}
