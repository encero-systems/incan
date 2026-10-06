//! Exact workspace feature variants for explicit Rust-unit bakes inside the compiler suite.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use oven_model::manifest::{DependencySource, DependencySpec};
use oven_rustc::native_contract::{OvenCompilerWorkspaceLibrary, OvenCompilerWorkspaceLibraryKey};

use super::{CliError, CliResult};

/// Publisher-selected source and third-party bindings, with authored production dependency tables.
struct WorkspacePackage {
    library: OvenCompilerWorkspaceLibrary,
    root: PathBuf,
    manifest: toml::Value,
    dependencies: BTreeMap<String, toml::Value>,
}

/// Read a source-owned manifest without running Cargo or trusting target-directory metadata.
fn read_manifest(path: &Path) -> CliResult<toml::Value> {
    let text = fs::read_to_string(path)
        .map_err(|error| CliError::failure(format!("cannot read {}: {error}", path.display())))?;
    toml::from_str(&text).map_err(|error| CliError::failure(format!("cannot parse {}: {error}", path.display())))
}

/// Merge workspace-inherited production edges while retaining member-specific feature requests.
fn production_dependencies(
    manifest: &toml::Value,
    workspace: &toml::Value,
) -> CliResult<BTreeMap<String, toml::Value>> {
    let mut dependencies = BTreeMap::new();
    let mut tables = Vec::new();
    if let Some(table) = manifest.get("dependencies").and_then(toml::Value::as_table) {
        tables.push(table);
    }
    if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
        tables.extend(
            targets
                .values()
                .filter_map(|target| target.get("dependencies").and_then(toml::Value::as_table)),
        );
    }
    for table in tables {
        for (name, value) in table {
            let mut effective = value.clone();
            if value.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
                effective = workspace
                    .get("workspace")
                    .and_then(|table| table.get("dependencies"))
                    .and_then(|table| table.get(name))
                    .cloned()
                    .ok_or_else(|| CliError::failure(format!("workspace dependency `{name}` has no declaration")))?;
                if let (Some(inherited), Some(member)) = (effective.as_table_mut(), value.as_table()) {
                    let mut features = inherited
                        .get("features")
                        .and_then(toml::Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    features.extend(
                        member
                            .get("features")
                            .and_then(toml::Value::as_array)
                            .into_iter()
                            .flatten()
                            .cloned(),
                    );
                    inherited.insert("features".to_string(), toml::Value::Array(features));
                    for field in ["optional", "default-features"] {
                        if let Some(value) = member.get(field) {
                            inherited.insert(field.to_string(), value.clone());
                        }
                    }
                }
            }
            dependencies.insert(name.clone(), effective);
        }
    }
    Ok(dependencies)
}

/// Resolved local features, optional edges and requests forwarded to workspace dependencies.
struct ExpandedFeatures {
    features: BTreeSet<String>,
    optional: BTreeSet<String>,
    forwarded: BTreeMap<String, BTreeSet<String>>,
}

/// Expand local feature groups and collect forwarded dependency feature requests to a fixed point.
fn expand_features(package: &WorkspacePackage, requested: &BTreeSet<String>) -> CliResult<ExpandedFeatures> {
    let table = package.manifest.get("features").and_then(toml::Value::as_table);
    let mut features = BTreeSet::new();
    let mut optional = BTreeSet::new();
    let mut forwarded = BTreeMap::<String, BTreeSet<String>>::new();
    let mut pending = requested.iter().cloned().collect::<Vec<_>>();
    while let Some(feature) = pending.pop() {
        if !features.insert(feature.clone()) {
            continue;
        }
        let Some(entries) = table
            .and_then(|table| table.get(&feature))
            .and_then(toml::Value::as_array)
        else {
            if package
                .dependencies
                .get(&feature)
                .and_then(|value| value.get("optional"))
                .and_then(toml::Value::as_bool)
                == Some(true)
            {
                optional.insert(feature);
                continue;
            }
            return Err(CliError::failure(format!(
                "workspace package `{}` has no feature `{feature}`",
                package.library.key.package_name
            )));
        };
        for entry in entries {
            let entry = entry
                .as_str()
                .ok_or_else(|| CliError::failure("workspace feature entry must be a string"))?;
            if let Some(dependency) = entry.strip_prefix("dep:") {
                optional.insert(dependency.to_string());
            } else if let Some((dependency, feature)) = entry.split_once('/') {
                let weak = dependency.ends_with('?');
                let dependency = dependency.trim_end_matches('?');
                if !weak {
                    optional.insert(dependency.to_string());
                }
                forwarded
                    .entry(dependency.to_string())
                    .or_default()
                    .insert(feature.to_string());
            } else {
                pending.push(entry.to_string());
            }
        }
    }
    Ok(ExpandedFeatures {
        features,
        optional,
        forwarded,
    })
}

/// Return a dependency's authored features and its default group, if that group exists.
fn dependency_features(value: &toml::Value, package: &WorkspacePackage) -> BTreeSet<String> {
    let mut features = value
        .get("features")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    if value.get("default-features").and_then(toml::Value::as_bool) != Some(false)
        && package
            .manifest
            .get("features")
            .and_then(|table| table.get("default"))
            .is_some()
    {
        features.insert("default".to_string());
    }
    features
}

/// Find the exact active production edge for one publisher-authorized workspace dependency.
fn workspace_edge<'a>(package: &'a WorkspacePackage, child: &WorkspacePackage) -> Option<(&'a str, &'a toml::Value)> {
    package.dependencies.iter().find_map(|(name, value)| {
        let declared = value.get("package").and_then(toml::Value::as_str).unwrap_or(name);
        (declared == child.library.key.package_name || name.replace('-', "_") == child.library.key.crate_name)
            .then_some((name.as_str(), value))
    })
}

/// Derive feature-unified workspace variants solely from authored roots and the publisher's admitted source DAG.
///
/// Test-only features and dev-dependency edges are discarded. No source absent from the admitted DAG can enter this
/// plan; missing path roots and unsupported feature requests fail by name instead of asking Cargo to repair them.
pub(crate) fn compiler_suite_rust_unit_variants(
    compiler_root: &Path,
    libraries: &[OvenCompilerWorkspaceLibrary],
    dependencies: &[DependencySpec],
) -> CliResult<Vec<OvenCompilerWorkspaceLibrary>> {
    let workspace = read_manifest(&compiler_root.join("Cargo.toml"))?;
    let mut packages = BTreeMap::<String, WorkspacePackage>::new();
    for library in libraries {
        if packages.contains_key(&library.key.package_name) {
            continue;
        }
        let source = compiler_root.join(&library.key.source_relative_path);
        let root = source
            .ancestors()
            .skip(1)
            .find(|path| path.join("Cargo.toml").is_file())
            .ok_or_else(|| {
                CliError::failure(format!("workspace source {} has no package manifest", source.display()))
            })?;
        let manifest = read_manifest(&root.join("Cargo.toml"))?;
        packages.insert(
            library.key.package_name.clone(),
            WorkspacePackage {
                library: library.clone(),
                root: fs::canonicalize(root).map_err(|error| CliError::failure(error.to_string()))?,
                dependencies: production_dependencies(&manifest, &workspace)?,
                manifest,
            },
        );
    }
    let mut requested = BTreeMap::<String, BTreeSet<String>>::new();
    for dependency in dependencies {
        let DependencySource::Path { path } = &dependency.source else {
            continue;
        };
        let root = fs::canonicalize(path)
            .map_err(|error| CliError::failure(format!("cannot resolve `{}`: {error}", dependency.crate_name)))?;
        let (name, package) = packages
            .iter()
            .find(|(_, package)| package.root == root)
            .ok_or_else(|| {
                CliError::failure(format!(
                    "Rust-unit dependency `{}` is absent from the admitted workspace DAG",
                    dependency.crate_name
                ))
            })?;
        let mut features = dependency.features.iter().cloned().collect::<BTreeSet<_>>();
        if dependency.default_features
            && package
                .manifest
                .get("features")
                .and_then(|table| table.get("default"))
                .is_some()
        {
            features.insert("default".to_string());
        }
        requested.entry(name.clone()).or_default().extend(features);
    }
    loop {
        let previous = requested.clone();
        for (name, features) in &previous {
            let package = packages
                .get(name)
                .ok_or_else(|| CliError::failure(format!("workspace package `{name}` disappeared")))?;
            let ExpandedFeatures {
                features,
                optional,
                forwarded,
            } = expand_features(package, features)?;
            requested.entry(name.clone()).or_default().extend(features);
            for child_key in &package.library.dependencies {
                let child = packages.get(&child_key.package_name).ok_or_else(|| {
                    CliError::failure(format!("undeclared workspace dependency `{}`", child_key.package_name))
                })?;
                let Some((edge_name, edge)) = workspace_edge(package, child) else {
                    continue;
                };
                if edge.get("optional").and_then(toml::Value::as_bool) == Some(true) && !optional.contains(edge_name) {
                    continue;
                }
                let mut features = dependency_features(edge, child);
                features.extend(forwarded.get(edge_name).into_iter().flatten().cloned());
                requested
                    .entry(child_key.package_name.clone())
                    .or_default()
                    .extend(features);
            }
        }
        if requested == previous {
            break;
        }
    }
    let keys = requested
        .iter()
        .map(|(name, features)| {
            let mut key = packages[name].library.key.clone();
            key.features = features.iter().cloned().collect();
            (name.clone(), key)
        })
        .collect::<BTreeMap<String, OvenCompilerWorkspaceLibraryKey>>();
    let mut variants = Vec::new();
    for (name, features) in &requested {
        let package = &packages[name];
        let ExpandedFeatures { optional, .. } = expand_features(package, features)?;
        let mut library = package.library.clone();
        library.key = keys[name].clone();
        library.dependencies = library
            .dependencies
            .iter()
            .filter_map(|key| {
                let child = packages.get(&key.package_name)?;
                let (edge_name, edge) = workspace_edge(package, child)?;
                if edge.get("optional").and_then(toml::Value::as_bool) == Some(true) && !optional.contains(edge_name) {
                    return None;
                }
                keys.get(&key.package_name).cloned()
            })
            .collect();
        variants.push(library);
    }
    Ok(variants)
}

/// Compile the native-driver Loaf's authored path dependencies as profile-specific suite workspace variants.
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_rust_unit_capability(
    compiler_root: &Path,
    libraries: &[OvenCompilerWorkspaceLibrary],
    closure: &oven_cargo_compat::OvenCompilerTestSuiteArtifactClosure,
    intent: &super::OvenBuildIntent,
    receipt: &super::OvenReceipt,
    artifact_root: &Path,
    rustc: &Path,
    output_directory: &Path,
    foundation_references: &[super::OvenCompilerTestSuiteFoundationReference],
    foundations: Option<&BTreeMap<String, super::CompilerSuiteFoundationExecution>>,
    cache: &mut BTreeMap<String, super::OvenCallerOwnedRustcLibrary>,
) -> CliResult<String> {
    use oven_model::compiler_suite_env::{OvenCompilerSuiteRustUnitCapability, OvenCompilerSuiteRustUnitLibrary};
    let dependencies = rust_unit_manifest_dependencies(compiler_root)?;
    let variants = compiler_suite_rust_unit_variants(compiler_root, libraries, &dependencies)?;
    let mut debug_intent = intent.clone();
    debug_intent.profile = "debug".to_string();
    let outputs = super::bake_planned_compiler_suite_workspace_libraries(
        &variants,
        closure,
        &debug_intent,
        receipt,
        artifact_root,
        rustc,
        compiler_root,
        output_directory,
        foundation_references,
        foundations,
        cache,
    )?;
    let mut selected = Vec::new();
    for dependency in &dependencies {
        let DependencySource::Path { path } = &dependency.source else {
            continue;
        };
        let package_root = fs::canonicalize(path).map_err(|error| CliError::failure(error.to_string()))?;
        let variant = variants
            .iter()
            .find(|library| library.key.crate_name == dependency.crate_name.replace('-', "_"))
            .ok_or_else(|| CliError::failure(format!("no exact workspace variant for `{}`", dependency.crate_name)))?;
        let output = outputs
            .get(&variant.key)
            .ok_or_else(|| CliError::failure("workspace variant was not compiled"))?;
        selected.push(OvenCompilerSuiteRustUnitLibrary {
            crate_name: dependency.crate_name.clone(),
            package_root,
            requested_features: dependency.features.clone(),
            default_features: dependency.default_features,
            features: variant.key.features.clone(),
            output: output.output.clone(),
            digest: output.digest.clone(),
        });
    }
    // The plan/lowering callers also declare this default-enabled kernel dependency independently of the driver.
    let mut paths = outputs
        .values()
        .filter_map(|output| output.output.parent().map(Path::to_path_buf))
        .collect::<Vec<_>>();
    let artifacts = closure.manifest_for_workspace_library(
        variants
            .first()
            .ok_or_else(|| CliError::failure("Rust-unit workspace closure is empty"))?,
        debug_intent.clone(),
    );
    let base = match foundations {
        Some(foundations) => {
            super::compiler_suite_composed_artifact_plan(&artifacts, foundation_references, foundations, &debug_intent)?
        }
        None => artifacts
            .materialize_trusted_store(artifact_root, &debug_intent)
            .map_err(super::oven_error)?,
    };
    paths.extend(base.dependency_search_paths);
    paths.sort();
    paths.dedup();
    OvenCompilerSuiteRustUnitCapability {
        schema_version: 3,
        target: debug_intent.target,
        profile: debug_intent.profile,
        rustc: fs::canonicalize(rustc).map_err(|error| CliError::failure(error.to_string()))?,
        libraries: selected,
        workspace_instances: outputs
            .values()
            .map(
                |output| oven_model::compiler_suite_env::OvenCompilerSuiteRustUnitArtifact {
                    crate_name: output.crate_name.clone(),
                    output: output.output.clone(),
                    digest: output.digest.clone(),
                },
            )
            .collect(),
        workspace_dependency_graph: variants
            .iter()
            .map(|library| {
                (
                    library.key.crate_name.clone(),
                    library
                        .dependencies
                        .iter()
                        .map(|dependency| dependency.crate_name.clone())
                        .collect(),
                )
            })
            .collect(),
        registry_instances: compiler_suite_registry_instances(foundations, foundation_references, &variants)?,
        dependency_search_paths: paths,
    }
    .encode()
    .map_err(CliError::failure)
}

/// Unify selectors authored by the native driver and its source-owned lowering dependency.
///
/// The lowering Loaf is independently prepared as a generated library. Its kernel path dependency must therefore
/// have an exact selector in the same capability; treating that edge as an ordinary caller path creates a second unit.
fn rust_unit_manifest_dependencies(compiler_root: &Path) -> CliResult<Vec<DependencySpec>> {
    let mut dependencies = Vec::new();
    for manifest in [
        "loaves/toolchain/incan-rustc-driver/loaf.toml",
        "loaves/compiler/incan_mir_lowering/loaf.toml",
    ] {
        let manifest = oven_model::manifest::ProjectManifest::load(&compiler_root.join(manifest))
            .map_err(|error| CliError::failure(error.to_string()))?;
        for dependency in manifest.rust_dependencies().values() {
            if !dependencies.contains(dependency) {
                dependencies.push(dependency.clone());
            }
        }
    }
    Ok(dependencies)
}

/// Transport only foundation artifacts present in the scheduler's verified metadata closure.
///
/// Each foundation partition repeats the complete publisher artifact index. Deduplicate those records and resolve
/// their files through the exact admitted partition and digest; filenames never choose a compilation domain.
fn compiler_suite_registry_instances(
    foundations: Option<&BTreeMap<String, super::CompilerSuiteFoundationExecution>>,
    references: &[super::OvenCompilerTestSuiteFoundationReference],
    libraries: &[OvenCompilerWorkspaceLibrary],
) -> CliResult<Vec<oven_model::compiler_suite_env::OvenCompilerSuiteRustUnitRegistryArtifact>> {
    use oven_model::compiler_suite_env::{
        OvenCompilerSuiteRustUnitArtifact, OvenCompilerSuiteRustUnitRegistryArtifact,
    };
    let foundations = foundations
        .ok_or_else(|| CliError::failure("Rust-unit registry dependencies have no admitted foundation unit records"))?;
    let selected = references
        .iter()
        .map(|reference| {
            foundations.get(&reference.identity).ok_or_else(|| {
                CliError::failure(format!(
                    "Rust-unit foundation `{}` is not held by the scheduler",
                    reference.label
                ))
            })
        })
        .collect::<CliResult<Vec<_>>>()?;
    let records = selected
        .iter()
        .filter_map(|foundation| foundation.payload.family.as_ref())
        .flat_map(|family| &family.artifact_index)
        .collect::<BTreeSet<_>>();
    let mut instances = Vec::new();
    for record in records {
        for file in &record.files {
            let file = Path::new(file);
            if file.extension().and_then(|extension| extension.to_str()) != Some("rlib") {
                continue;
            }
            let consumers = libraries
                .iter()
                .filter(|library| {
                    library
                        .externs
                        .iter()
                        .any(|artifact| Path::new(&artifact.relative_path) == file)
                })
                .map(|library| library.key.crate_name.clone())
                .collect::<Vec<_>>();
            if consumers.is_empty() {
                continue;
            }
            let (foundation, declaration) = selected
                .iter()
                .find_map(|foundation| {
                    foundation
                        .payload
                        .artifact_closure
                        .supporting_artifacts
                        .iter()
                        .find(|artifact| Path::new(&artifact.relative_path) == file)
                        .map(|artifact| (*foundation, artifact))
                })
                .ok_or_else(|| {
                    CliError::failure(format!(
                        "Rust-unit registry dependency `{}` has no selected artifact for {}",
                        record.package,
                        file.display()
                    ))
                })?;
            let output = foundation.stored.artifact_root.join(file);
            let digest = super::digest_bytes(&fs::read(&output).map_err(|error| CliError::failure(error.to_string()))?);
            if digest != declaration.digest {
                return Err(CliError::failure(format!(
                    "Rust-unit registry dependency `{}` changed after foundation admission",
                    record.package
                )));
            }
            instances.push(OvenCompilerSuiteRustUnitRegistryArtifact {
                package: record.package.clone(),
                version: record.version.clone(),
                host: record.platform.is_none(),
                consumers,
                artifact: OvenCompilerSuiteRustUnitArtifact {
                    crate_name: record.target_name.replace('-', "_"),
                    output,
                    digest,
                },
            });
        }
    }
    Ok(instances)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Preserve the independently authored lowering selector even when the driver disables the same defaults.
    #[test]
    fn rust_unit_selectors_include_the_lowering_manifest() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        for (root, declaration) in [
            (
                "loaves/toolchain/incan-rustc-driver",
                "incan_semantics_core = { path = '../../kernel/incan_semantics_core', default-features = false }",
            ),
            (
                "loaves/compiler/incan_mir_lowering",
                "incan_semantics_core = { path = '../../kernel/incan_semantics_core' }",
            ),
        ] {
            let root = workspace.path().join(root);
            fs::create_dir_all(&root)?;
            fs::write(
                root.join("loaf.toml"),
                format!("[project]\nname = 'fixture'\nversion = '1.0.0'\n[rust-dependencies]\n{declaration}\n"),
            )?;
        }
        let dependencies = rust_unit_manifest_dependencies(workspace.path())?;
        assert_eq!(dependencies.len(), 2);
        assert!(dependencies.iter().any(|dependency| dependency.default_features));
        assert!(dependencies.iter().any(|dependency| !dependency.default_features));
        assert!(
            dependencies
                .iter()
                .all(|dependency| dependency.crate_name == "incan_semantics_core")
        );
        Ok(())
    }

    /// Construct a publisher template with deliberately broader test features than the authored production request.
    fn library(name: &str, dependencies: Vec<OvenCompilerWorkspaceLibraryKey>) -> OvenCompilerWorkspaceLibrary {
        OvenCompilerWorkspaceLibrary {
            key: OvenCompilerWorkspaceLibraryKey {
                package_name: name.to_string(),
                crate_name: name.to_string(),
                target_kind: "lib".to_string(),
                source_relative_path: format!("{name}/src/lib.rs"),
                features: vec!["test_support".to_string()],
            },
            source_evidence_key: format!("source:{name}"),
            edition: "2024".to_string(),
            compile_environment: BTreeMap::new(),
            externs: Vec::new(),
            dependencies,
        }
    }

    /// Forward production features, unify an independently declared child, and omit test-only optional edges.
    #[test]
    fn rust_unit_variants_unify_authored_features_without_test_features() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        fs::write(
            workspace.path().join("Cargo.toml"),
            "[workspace.dependencies]\nchild = { path = 'child', default-features = false }\n",
        )?;
        for (name, manifest) in [
            (
                "parent",
                "[package]\nname = 'parent'\n[features]\ndefault = ['test_support']\ncli = ['child/std_async']\ntest_support = ['dep:fixtures']\n[dependencies]\nchild = { workspace = true }\nfixtures = { path = '../fixtures', optional = true }\n",
            ),
            (
                "child",
                "[package]\nname = 'child'\n[features]\ndefault = ['test_support']\nstd_async = []\ntest_support = []\n",
            ),
            ("fixtures", "[package]\nname = 'fixtures'\n"),
        ] {
            fs::create_dir_all(workspace.path().join(name).join("src"))?;
            fs::write(workspace.path().join(name).join("Cargo.toml"), manifest)?;
            fs::write(workspace.path().join(name).join("src/lib.rs"), "")?;
        }
        let child = library("child", Vec::new());
        let fixtures = library("fixtures", Vec::new());
        let parent = library("parent", vec![child.key.clone(), fixtures.key.clone()]);
        let root = DependencySpec {
            crate_name: "parent".to_string(),
            version: None,
            features: vec!["cli".to_string()],
            default_features: false,
            source: DependencySource::Path {
                path: workspace.path().join("parent"),
            },
            optional: false,
            package: None,
        };
        let mut direct_child = root.clone();
        direct_child.crate_name = "child".to_string();
        direct_child.features.clear();
        direct_child.source = DependencySource::Path {
            path: workspace.path().join("child"),
        };
        let templates = vec![parent, child, fixtures];
        let variants = compiler_suite_rust_unit_variants(workspace.path(), &templates, &[root.clone(), direct_child])?;
        assert_eq!(variants.len(), 2);
        let parent = variants
            .iter()
            .find(|library| library.key.crate_name == "parent")
            .ok_or("missing parent")?;
        let child = variants
            .iter()
            .find(|library| library.key.crate_name == "child")
            .ok_or("missing child")?;
        assert_eq!(parent.key.features, ["cli"]);
        assert_eq!(child.key.features, ["std_async"]);
        assert_eq!(parent.dependencies.as_slice(), std::slice::from_ref(&child.key));
        let mut default_root = root;
        default_root.default_features = true;
        let default_variants = compiler_suite_rust_unit_variants(workspace.path(), &templates, &[default_root])?;
        assert_eq!(default_variants.len(), 3);
        assert!(
            default_variants
                .iter()
                .find(|library| library.key.crate_name == "parent")
                .ok_or("missing default parent")?
                .key
                .features
                .contains(&"default".to_string())
        );
        Ok(())
    }
}
