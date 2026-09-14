//! Projecting a compiled provider's metadata into its manifest: dependencies, facets, fact requirements and the
//! declaration facts registry inspection reads.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::{env, fs};

use crate::driver::build::CompiledProviderMetadataInputs;
use crate::driver::error::{CliError, CliResult};
use crate::driver::project::resolve_source_root;
use crate::frontend::ast::{Declaration, Expr, ImportKind, Literal, Spanned, Statement, Visibility};
use crate::frontend::library_manifest_index::{LibraryArtifactKind, LibraryManifestIndex, LibraryManifestIndexEntry};
use crate::frontend::module::{
    SourceModuleImportResolution, resolve_program_source_imports, resolve_source_module_import_from_source_file,
    self_import_diagnostic_message,
};
use crate::frontend::{ParsedModule, diagnostics, typechecker};
use crate::library_manifest::{
    CompiledProviderMetadata, LibraryManifest, ProviderCargoDependency, ProviderCargoDependencySource,
    ProviderDependencyKind, ProviderDependencyMetadata, ProviderFactKind, ProviderFactRequirement,
    ProviderImplementationFacet, ProviderModuleClaim, ProviderOperationMetadata, digest_provider_artifact,
    digest_provider_source_inputs,
};
use crate::manifest::{DependencySource, DependencySpec};
use crate::provider::{PackageFeatureGraph, PackageFeaturePlan, ProviderPlan, SDK_PROVIDER_BUILD_ENV};

/// Synchronize newly published public dependency metadata with the exact projected paths rendered into Cargo.toml.
pub(crate) fn synchronize_projected_provider_dependencies(
    library_manifest: &mut LibraryManifest,
    artifact_root: &Path,
    dependencies: &[DependencySpec],
) -> CliResult<()> {
    for descriptor in library_manifest
        .contract_metadata
        .provider
        .provider_dependencies
        .iter_mut()
        .filter(|dependency| dependency.kind == ProviderDependencyKind::PublicPackage)
    {
        let Some(dependency) = dependencies
            .iter()
            .find(|dependency| dependency.crate_name == descriptor.dependency_key)
        else {
            continue;
        };
        let cargo_package = dependency.package.as_deref().unwrap_or(dependency.crate_name.as_str());
        if cargo_package != descriptor.provider_name {
            return Err(CliError::failure(format!(
                "projected Cargo dependency `{}` names package `{cargo_package}`, but its checked provider edge names `{}`",
                descriptor.dependency_key, descriptor.provider_name
            )));
        }
        let DependencySource::Path { path } = &dependency.source else {
            continue;
        };
        descriptor.relative_artifact_path = relative_provider_artifact_path(artifact_root, path)?;
        descriptor.artifact_digest = digest_provider_artifact(path).map_err(|error| {
            CliError::failure(format!(
                "failed to hash projected provider dependency `{}` artifact {}: {error}",
                descriptor.dependency_key,
                path.display()
            ))
        })?;
    }
    Ok(())
}

/// Build transport-stable provider facts from the checked physical artifact projection.
pub(crate) fn compiled_provider_metadata(
    inputs: CompiledProviderMetadataInputs<'_>,
) -> CliResult<CompiledProviderMetadata> {
    let graph =
        PackageFeatureGraph::from_manifest(inputs.manifest).map_err(|error| CliError::failure(error.to_string()))?;
    let root_features = inputs
        .feature_plan
        .root_package()
        .map(|package| &package.features)
        .ok_or_else(|| CliError::failure("resolved package feature plan is missing its root package"))?;
    let library_entrypoint = inputs
        .modules
        .iter()
        .find(|module| module.file_path == inputs.active_library_entrypoint.file_path)
        .ok_or_else(|| CliError::failure("unprojected provider graph is missing its library entrypoint"))?;
    let source_root = resolve_source_root(inputs.manifest.project_root(), Some(inputs.manifest));
    let module_requirements =
        provider_module_reachability_requirements(inputs.modules, library_entrypoint, &source_root)?;
    let mut namespace_claims = inputs
        .modules
        .iter()
        .filter(|module| {
            module.file_path != inputs.active_library_entrypoint.file_path
                && !module.path_segments.is_empty()
                && !module_is_owned_by_dependency_provider(inputs.provider_plan, &module.path_segments)
        })
        .flat_map(|module| {
            module_requirements
                .get(&module.path_segments)
                .into_iter()
                .flatten()
                .map(|required_features| ProviderModuleClaim {
                    module_path: module.path_segments.clone(),
                    required_features: required_features.clone(),
                })
        })
        .collect::<Vec<_>>();
    namespace_claims.sort();
    namespace_claims.dedup();

    let public_features = graph.provider_metadata();
    let mut fact_requirements = Vec::new();
    for module in inputs
        .modules
        .iter()
        .filter(|module| !module_is_owned_by_dependency_provider(inputs.provider_plan, &module.path_segments))
    {
        let requirements = module_requirements.get(&module.path_segments).ok_or_else(|| {
            CliError::failure(format!(
                "unprojected provider module `{}` has no reachability predicate from the library entrypoint",
                module.path_segments.join(".")
            ))
        })?;
        fact_requirements.extend(provider_fact_requirements(module, requirements));
    }
    fact_requirements.extend(
        namespace_claims
            .iter()
            .filter(|claim| !claim.required_features.is_empty())
            .map(|claim| ProviderFactRequirement {
                kind: ProviderFactKind::Module,
                identity: claim.module_path.join("."),
                required_features: claim.required_features.clone(),
            }),
    );
    fact_requirements.extend(public_features.iter().flat_map(|(feature, metadata)| {
        metadata
            .required_sdk_components
            .iter()
            .map(move |component| ProviderFactRequirement {
                kind: ProviderFactKind::ComponentRequirement,
                identity: component.clone(),
                required_features: BTreeSet::from([feature.clone()]),
            })
    }));
    fact_requirements.sort();
    fact_requirements.dedup();

    let provider_dependencies = compiled_provider_dependencies(
        inputs.feature_plan,
        inputs.library_manifest_index,
        inputs.provider_plan,
        inputs.artifact_root,
    )?;
    let implementation_facets = provider_implementation_facets(&namespace_claims);
    let operation_descriptors = provider_operation_metadata_from_checked_type_info(inputs.checked_type_info_by_path)?;
    let semantic_source_inputs = inputs
        .modules
        .iter()
        .filter(|module| !module_is_owned_by_dependency_provider(inputs.provider_plan, &module.path_segments))
        .map(|module| {
            let label = if module.path_segments.is_empty() {
                "<root>".to_string()
            } else {
                module.path_segments.join(".")
            };
            (label, module.file_path.clone())
        })
        .collect::<Vec<_>>();
    let trusted_source_roots = crate::toolchain_layout::find_stdlib_source_dir()
        .into_iter()
        .collect::<Vec<_>>();
    let semantic_source_digest = digest_provider_source_inputs(
        inputs.manifest.project_root(),
        inputs.manifest.path(),
        &semantic_source_inputs,
        &trusted_source_roots,
    )
    .map_err(|error| CliError::failure(format!("failed to fingerprint authored provider inputs: {error}")))?;
    Ok(CompiledProviderMetadata {
        semantic_source_digest: Some(semantic_source_digest),
        namespace_claims,
        public_features,
        active_features: root_features.active_features.clone(),
        provider_dependencies,
        fact_requirements,
        required_sdk_components: root_features.required_sdk_components.clone(),
        implementation_facets,
        operation_descriptors,
        ..CompiledProviderMetadata::default()
    })
}

/// Project declaration-side provider-operation facts into the selected library artifact.
///
/// The typechecker is the only source of these pairs: it resolved each decorated operation's capability in the
/// declaring module. Sorting and rejecting duplicate canonical identities makes manifest output deterministic and
/// prevents a package with two copies of one declaration from acquiring order-dependent provider meaning.
fn provider_operation_metadata_from_checked_type_info(
    checked_type_info_by_path: &BTreeMap<PathBuf, typechecker::TypeCheckInfo>,
) -> CliResult<Vec<ProviderOperationMetadata>> {
    let mut operation_descriptors = checked_type_info_by_path
        .values()
        .flat_map(|type_info| type_info.declarations.provider_operations.values())
        .map(|operation| ProviderOperationMetadata {
            operation: operation.operation.clone(),
            required_capability: operation.required_capability.clone(),
            runtime_requirements: operation.runtime_requirements.clone(),
        })
        .collect::<Vec<_>>();
    operation_descriptors.sort_by(|left, right| left.operation.cmp(&right.operation));
    if let Some(duplicate) = operation_descriptors
        .windows(2)
        .find(|entries| entries[0].operation == entries[1].operation)
    {
        return Err(CliError::failure(format!(
            "provider operation metadata contains duplicate declaration `{}`",
            duplicate[0].operation.declaration_name
        )));
    }
    Ok(operation_descriptors)
}

/// Freeze the active Incan dependency edges into artifact-owned, relocation-safe provider metadata.
fn compiled_provider_dependencies(
    feature_plan: &PackageFeaturePlan,
    library_manifest_index: &LibraryManifestIndex,
    provider_plan: &ProviderPlan,
    artifact_root: &Path,
) -> CliResult<Vec<ProviderDependencyMetadata>> {
    let mut dependencies = Vec::new();
    for edge in feature_plan
        .edges()
        .filter(|edge| edge.from.as_path() == feature_plan.root())
    {
        let entry = library_manifest_index.get(&edge.dependency_key).ok_or_else(|| {
            CliError::failure(format!(
                "active provider dependency `pub::{}` is missing from the checked library manifest index",
                edge.dependency_key
            ))
        })?;
        let (manifest, metadata) = match entry {
            LibraryManifestIndexEntry::Loaded { manifest, metadata } => (manifest, metadata),
            LibraryManifestIndexEntry::Failed(failure) => {
                return Err(CliError::failure(format!(
                    "active provider dependency `pub::{}` could not be loaded from {}: {}",
                    edge.dependency_key,
                    failure.path.display(),
                    failure.message
                )));
            }
        };
        if metadata.kind != LibraryArtifactKind::Materialized {
            return Err(CliError::failure(format!(
                "active provider dependency `pub::{}` has parser-only metadata; build its compiled artifact before publishing this provider",
                edge.dependency_key
            )));
        }
        let artifact_digest = digest_provider_artifact(&metadata.crate_root).map_err(|error| {
            CliError::failure(format!(
                "failed to hash provider dependency `pub::{}` artifact {}: {error}",
                edge.dependency_key,
                metadata.crate_root.display()
            ))
        })?;
        dependencies.push(ProviderDependencyMetadata {
            kind: crate::library_manifest::ProviderDependencyKind::PublicPackage,
            dependency_key: edge.dependency_key.clone(),
            provider_name: manifest.name.clone(),
            provider_version: manifest.version.clone(),
            artifact_digest,
            relative_artifact_path: relative_provider_artifact_path(artifact_root, &metadata.crate_root)?,
            requested_features: edge.requested_features.clone(),
            default_features: edge.default_features,
            optional: edge.optional,
        });
    }
    for provider in provider_plan.sdk_link_roots() {
        let Some(metadata) = provider.artifact.as_ref() else {
            continue;
        };
        let artifact_digest = digest_provider_artifact(&metadata.crate_root).map_err(|error| {
            CliError::failure(format!(
                "failed to hash private SDK provider dependency `{}` artifact {}: {error}",
                provider.identity.name,
                metadata.crate_root.display()
            ))
        })?;
        dependencies.push(ProviderDependencyMetadata {
            kind: crate::library_manifest::ProviderDependencyKind::PrivateImplementation,
            dependency_key: metadata.dependency_key.clone(),
            provider_name: provider.identity.name.clone(),
            provider_version: provider.identity.version.clone(),
            artifact_digest,
            relative_artifact_path: relative_provider_artifact_path(artifact_root, &metadata.crate_root)?,
            requested_features: provider.identity.feature_projection.clone(),
            default_features: false,
            optional: false,
        });
    }
    dependencies.sort();
    dependencies.dedup();
    Ok(dependencies)
}

/// Compute one normalized portable path between two existing provider artifact roots.
fn relative_provider_artifact_path(from: &Path, to: &Path) -> CliResult<String> {
    let from = fs::canonicalize(from).map_err(|error| {
        CliError::failure(format!(
            "failed to canonicalize provider artifact root {}: {error}",
            from.display()
        ))
    })?;
    let to = fs::canonicalize(to).map_err(|error| {
        CliError::failure(format!(
            "failed to canonicalize dependency artifact root {}: {error}",
            to.display()
        ))
    })?;
    let from_components = from.components().collect::<Vec<_>>();
    let to_components = to.components().collect::<Vec<_>>();
    let common = from_components
        .iter()
        .zip(&to_components)
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        return Err(CliError::failure(format!(
            "provider artifact roots {} and {} have no relocatable filesystem ancestor",
            from.display(),
            to.display()
        )));
    }
    let mut relative = PathBuf::new();
    for _ in common..from_components.len() {
        relative.push("..");
    }
    for component in &to_components[common..] {
        relative.push(component.as_os_str());
    }
    let rendered = relative.to_string_lossy().replace('\\', "/");
    if rendered.is_empty() {
        return Err(CliError::failure("a provider artifact cannot depend on itself"));
    }
    Ok(rendered)
}

/// Parse the complete local provider graph without dropping inactive feature-conditioned declarations.
///
/// The checked API and generated Rust remain specialized to the selected feature projection. This parallel metadata
/// view preserves the complete positive condition inventory so consumers and inspection can explain inactive facts
/// without reparsing provider source.
pub(crate) fn collect_unprojected_provider_modules(
    library_entrypoint: &Path,
    session: &crate::driver::session::CompilationSession,
) -> CliResult<Vec<ParsedModule>> {
    let mut pending = crate::driver::modules::library_source_seeds(library_entrypoint, session)?;
    let mut processed = HashSet::new();
    let mut modules = Vec::new();

    while let Some((file_path, module_name, path_segments)) = pending.pop() {
        let canonical_path = file_path.canonicalize().unwrap_or_else(|_| file_path.clone());
        if !processed.insert(canonical_path) {
            continue;
        }
        let source = fs::read_to_string(&file_path)
            .map_err(|error| CliError::failure(format!("failed to read {}: {error}", file_path.display())))?;
        let ast = session
            .parse_source_unprojected(&file_path, &source, false)
            .map_err(|errors| {
                let rendered = errors
                    .iter()
                    .map(|error| diagnostics::format_error(file_path.to_string_lossy().as_ref(), &source, error))
                    .collect::<String>();
                CliError::failure(rendered.trim_end())
            })?;
        session.validate_parsed_program_features(&ast).map_err(|errors| {
            let rendered = errors
                .iter()
                .map(|error| diagnostics::format_error(file_path.to_string_lossy().as_ref(), &source, error))
                .collect::<String>();
            CliError::failure(rendered.trim_end())
        })?;
        let base_dir = file_path.parent().unwrap_or(session.source_root.as_path());
        for resolved in resolve_program_source_imports(&ast, base_dir, Some(&session.source_root)) {
            match resolved.resolution {
                SourceModuleImportResolution::Local(module) => {
                    pending.push((module.file_path, module.module_name, module.path_segments));
                }
                SourceModuleImportResolution::SelfImport {
                    module_ref,
                    import_path,
                    can_use_root_import,
                } => {
                    let error = diagnostics::CompileError::new(
                        self_import_diagnostic_message(&module_ref, &import_path, can_use_root_import),
                        resolved.span,
                    );
                    return Err(CliError::failure(
                        diagnostics::format_error(file_path.to_string_lossy().as_ref(), &source, &error).trim_end(),
                    ));
                }
                SourceModuleImportResolution::Stdlib { .. } | SourceModuleImportResolution::External => {}
            }
        }
        modules.push(ParsedModule {
            name: module_name,
            path_segments,
            file_path,
            source,
            ast,
        });
    }

    Ok(modules)
}

/// Freeze the source SDK publisher's current Rust-backend mappings into provider-owned artifact facets.
///
/// Consumers read these mappings from `.incnlib`; they never rediscover Cargo features or dependencies from a
/// compiler-side stdlib module inventory. This bootstrap adapter can disappear once provider source can author the
/// equivalent backend mappings directly.
fn provider_implementation_facets(namespace_claims: &[ProviderModuleClaim]) -> Vec<ProviderImplementationFacet> {
    if env::var_os(SDK_PROVIDER_BUILD_ENV).is_none() {
        return Vec::new();
    }
    let roots = namespace_claims
        .iter()
        .filter_map(|claim| claim.module_path.first().cloned())
        .collect::<BTreeSet<_>>();
    roots
        .into_iter()
        .filter_map(|root| {
            let namespace = incan_core::lang::stdlib::find_namespace(&root)?;
            let required_modules = namespace_claims
                .iter()
                .filter(|claim| claim.module_path.first() == Some(&root))
                .map(|claim| claim.module_path.clone())
                .collect();
            let cargo_features = namespace
                .feature
                .map(|feature| {
                    BTreeMap::from([(
                        crate::backend::project::INCAN_STDLIB_CRATE_NAME.to_string(),
                        BTreeSet::from([feature.to_string()]),
                    )])
                })
                .unwrap_or_default();
            let cargo_dependencies = namespace
                .extra_crate_deps
                .iter()
                .map(|dependency| ProviderCargoDependency {
                    crate_name: dependency.crate_name.to_string(),
                    package: incan_core::lang::stdlib::extra_crate_package_alias(dependency.crate_name)
                        .map(str::to_string),
                    version: match dependency.source {
                        incan_core::lang::stdlib::StdlibExtraCrateSource::Version(version) => Some(version.to_string()),
                        incan_core::lang::stdlib::StdlibExtraCrateSource::Path(_) => None,
                    },
                    features: dependency
                        .features
                        .iter()
                        .map(|feature| (*feature).to_string())
                        .collect(),
                    default_features: true,
                    source: match dependency.source {
                        incan_core::lang::stdlib::StdlibExtraCrateSource::Version(_) => {
                            ProviderCargoDependencySource::Registry
                        }
                        incan_core::lang::stdlib::StdlibExtraCrateSource::Path(relative_path) => {
                            ProviderCargoDependencySource::Toolchain {
                                relative_path: relative_path.to_string(),
                            }
                        }
                    },
                })
                .collect();
            Some(ProviderImplementationFacet {
                id: format!("rust_{root}"),
                required_modules,
                required_features: BTreeSet::new(),
                cargo_features,
                cargo_dependencies,
            })
        })
        .collect()
}

/// Return whether an already-linked SDK provider owns this emitted `__incan_std.*` module.
fn module_is_owned_by_dependency_provider(provider_plan: &ProviderPlan, emission_path: &[String]) -> bool {
    let prefix = [incan_core::lang::stdlib::INCAN_STD_NAMESPACE.to_string()];
    let relative = if let Some(relative) = emission_path.strip_prefix(prefix.as_slice()) {
        relative
    } else if env::var_os(SDK_PROVIDER_BUILD_ENV).is_some() {
        emission_path
    } else {
        return false;
    };
    let mut canonical = vec![incan_core::lang::stdlib::STDLIB_ROOT.to_string()];
    canonical.extend(relative.iter().cloned());
    provider_plan.active_sdk_provider_for_module(&canonical).is_some()
}

/// Derive positive feature predicates for entrypoint-reachable modules and disconnected automatic namespace roots.
///
/// Multiple incomparable predicates represent alternative additive paths. A broader predicate subsumes narrower paths,
/// so an unconditional import collapses every conditional route to the same module. Conditions accumulate across nested
/// imports, while modules outside the entrypoint graph remain unconditional roots of the published source hierarchy.
fn provider_module_reachability_requirements(
    modules: &[ParsedModule],
    entrypoint: &ParsedModule,
    source_root: &Path,
) -> CliResult<BTreeMap<Vec<String>, Vec<BTreeSet<String>>>> {
    let modules_by_path = modules
        .iter()
        .map(|module| (canonical_provider_source_path(&module.file_path), module))
        .collect::<BTreeMap<_, _>>();
    let entrypoint_path = canonical_provider_source_path(&entrypoint.file_path);
    if !modules_by_path.contains_key(&entrypoint_path) {
        return Err(CliError::failure(
            "unprojected provider graph does not contain its library entrypoint",
        ));
    }

    let mut requirements = BTreeMap::new();
    insert_provider_feature_predicate(&mut requirements, entrypoint.path_segments.clone(), BTreeSet::new());
    let mut pending = vec![(entrypoint_path, BTreeSet::new())];
    propagate_provider_feature_predicates(&modules_by_path, source_root, &mut requirements, &mut pending)?;

    let disconnected_modules = modules
        .iter()
        .filter(|module| !requirements.contains_key(&module.path_segments))
        .collect::<Vec<_>>();
    for module in disconnected_modules {
        insert_provider_feature_predicate(&mut requirements, module.path_segments.clone(), BTreeSet::new());
        pending.push((canonical_provider_source_path(&module.file_path), BTreeSet::new()));
    }
    propagate_provider_feature_predicates(&modules_by_path, source_root, &mut requirements, &mut pending)?;

    Ok(requirements)
}

/// Propagate inherited feature predicates through one bounded set of local provider imports.
fn propagate_provider_feature_predicates(
    modules_by_path: &BTreeMap<PathBuf, &ParsedModule>,
    source_root: &Path,
    requirements: &mut BTreeMap<Vec<String>, Vec<BTreeSet<String>>>,
    pending: &mut Vec<(PathBuf, BTreeSet<String>)>,
) -> CliResult<()> {
    while let Some((module_path, inherited_features)) = pending.pop() {
        let Some(module) = modules_by_path.get(&module_path) else {
            return Err(CliError::failure(format!(
                "unprojected provider graph lost module {}",
                module_path.display()
            )));
        };
        let base_dir = module.file_path.parent().unwrap_or(source_root);
        for declaration in &module.ast.declarations {
            let Declaration::Import(import) = &declaration.node else {
                continue;
            };
            let target = match resolve_source_module_import_from_source_file(
                base_dir,
                Some(source_root),
                Some(&module.file_path),
                import,
            ) {
                SourceModuleImportResolution::Local(target) => target,
                SourceModuleImportResolution::SelfImport {
                    module_ref,
                    import_path,
                    can_use_root_import,
                } => {
                    return Err(CliError::failure(self_import_diagnostic_message(
                        &module_ref,
                        &import_path,
                        can_use_root_import,
                    )));
                }
                SourceModuleImportResolution::Stdlib { .. } | SourceModuleImportResolution::External => continue,
            };
            let target_path = canonical_provider_source_path(&target.file_path);
            let Some(target_module) = modules_by_path.get(&target_path) else {
                return Err(CliError::failure(format!(
                    "unprojected provider graph is missing imported module `{}` at {}",
                    target.path_segments.join("."),
                    target.file_path.display()
                )));
            };
            let mut required_features = inherited_features.clone();
            required_features.extend(declaration.required_features.iter().cloned());
            if insert_provider_feature_predicate(
                requirements,
                target_module.path_segments.clone(),
                required_features.clone(),
            ) {
                pending.push((target_path, required_features));
            }
        }
    }

    Ok(())
}

/// Canonicalize source identity when possible while retaining useful fixture paths when it is not.
fn canonical_provider_source_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Insert one predicate into a deterministic minimal antichain for a provider module.
fn insert_provider_feature_predicate(
    requirements: &mut BTreeMap<Vec<String>, Vec<BTreeSet<String>>>,
    module_path: Vec<String>,
    candidate: BTreeSet<String>,
) -> bool {
    let predicates = requirements.entry(module_path).or_default();
    if predicates.iter().any(|existing| existing.is_subset(&candidate)) {
        return false;
    }
    predicates.retain(|existing| !candidate.is_subset(existing));
    predicates.push(candidate);
    predicates.sort();
    true
}

/// Preserve positive feature predicates on checked declarations for inspection and artifact projection.
fn provider_fact_requirements(
    module: &ParsedModule,
    module_requirements: &[BTreeSet<String>],
) -> Vec<ProviderFactRequirement> {
    let module_name = module.path_segments.join(".");
    let mut requirements = Vec::new();
    for declaration in &module.ast.declarations {
        let mut combined_requirements = Vec::new();
        for module_requirement in module_requirements {
            let mut combined = module_requirement.clone();
            combined.extend(declaration.required_features.iter().cloned());
            if !combined.is_empty()
                && !combined_requirements
                    .iter()
                    .any(|existing: &BTreeSet<String>| existing.is_subset(&combined))
            {
                combined_requirements.retain(|existing| !combined.is_subset(existing));
                combined_requirements.push(combined);
            }
        }
        combined_requirements.sort();

        for required_features in combined_requirements {
            match &declaration.node {
                Declaration::Import(import) => {
                    requirements.push(ProviderFactRequirement {
                        kind: ProviderFactKind::ProviderDependency,
                        identity: format!("{module_name}::{}", provider_import_identity(&import.kind)),
                        required_features: required_features.clone(),
                    });
                    if import.visibility == Visibility::Public {
                        let reexported_items = match &import.kind {
                            ImportKind::From { items, .. } | ImportKind::PubFrom { items, .. } => items.as_slice(),
                            _ => &[],
                        };
                        requirements.extend(reexported_items.iter().map(|item| ProviderFactRequirement {
                            kind: ProviderFactKind::Export,
                            identity: format!("{module_name}::{}", item.alias.as_deref().unwrap_or(item.name.as_str())),
                            required_features: required_features.clone(),
                        }));
                    }
                }
                Declaration::Docstring(_) => requirements.push(ProviderFactRequirement {
                    kind: ProviderFactKind::Documentation,
                    identity: format!("{module_name}::module-docstring"),
                    required_features,
                }),
                Declaration::TestModule(test_module) => {
                    requirements.push(ProviderFactRequirement {
                        kind: ProviderFactKind::Export,
                        identity: format!("{module_name}::{}", test_module.name),
                        required_features: required_features.clone(),
                    });
                    requirements.extend(provider_nested_test_fact_requirements(
                        &module_name,
                        &test_module.body,
                        &required_features,
                    ));
                }
                declaration => {
                    let Some(name) = provider_declaration_name(declaration) else {
                        continue;
                    };
                    let identity = format!("{module_name}::{name}");
                    requirements.push(ProviderFactRequirement {
                        kind: if provider_declaration_is_public(declaration) {
                            ProviderFactKind::Export
                        } else {
                            ProviderFactKind::ImplementationFacet
                        },
                        identity: identity.clone(),
                        required_features: required_features.clone(),
                    });
                    if provider_declaration_has_docstring(declaration) {
                        requirements.push(ProviderFactRequirement {
                            kind: ProviderFactKind::Documentation,
                            identity: identity.clone(),
                            required_features: required_features.clone(),
                        });
                    }
                    if provider_declaration_is_registry_entry(declaration) {
                        requirements.push(ProviderFactRequirement {
                            kind: ProviderFactKind::RegistryEntry,
                            identity,
                            required_features,
                        });
                    }
                }
            }
        }
    }
    requirements
}

/// Preserve nested inline-test predicates together with their enclosing test-module predicate.
fn provider_nested_test_fact_requirements(
    module_name: &str,
    declarations: &[Spanned<Declaration>],
    parent_features: &BTreeSet<String>,
) -> Vec<ProviderFactRequirement> {
    declarations
        .iter()
        .filter_map(|declaration| {
            let name = provider_declaration_name(&declaration.node)?;
            let mut required_features = parent_features.clone();
            required_features.extend(declaration.required_features.iter().cloned());
            Some(ProviderFactRequirement {
                kind: ProviderFactKind::ImplementationFacet,
                identity: format!("{module_name}::tests::{name}"),
                required_features,
            })
        })
        .collect()
}

/// Render a stable provider-local import identity without depending on source offsets.
fn provider_import_identity(import: &ImportKind) -> String {
    match import {
        ImportKind::Module(path) => format!("import:{}", path.segments.join(".")),
        ImportKind::From { module, .. } => format!("from:{}", module.segments.join(".")),
        ImportKind::PubLibrary { library, path } => {
            format!("import:pub::{library}{}", format_pub_module_suffix(path))
        }
        ImportKind::PubFrom { library, path, .. } => {
            format!("from:pub::{library}{}", format_pub_module_suffix(path))
        }
        ImportKind::Python(module) => format!("import:python:{module}"),
        ImportKind::RustCrate { crate_name, path, .. } => {
            format!("import:rust::{crate_name}::{}", path.join("::"))
        }
        ImportKind::RustFrom { crate_name, path, .. } => {
            format!("from:rust::{crate_name}::{}", path.join("::"))
        }
    }
}

/// Render a nested public-package module path for stable provider import identity.
fn format_pub_module_suffix(path: &[String]) -> String {
    path.iter().map(|segment| format!(".{segment}")).collect()
}

/// Return one declaration's stable local name.
fn provider_declaration_name(declaration: &Declaration) -> Option<&str> {
    match declaration {
        Declaration::Const(item) => Some(&item.name),
        Declaration::Static(item) => Some(&item.name),
        Declaration::Model(item) => Some(&item.name),
        Declaration::Class(item) => Some(&item.name),
        Declaration::Trait(item) => Some(&item.name),
        Declaration::Alias(item) => Some(&item.name),
        Declaration::Partial(item) => Some(&item.name),
        Declaration::TypeAlias(item) => Some(&item.name),
        Declaration::Newtype(item) => Some(&item.name),
        Declaration::Enum(item) => Some(&item.name),
        Declaration::Function(item) => Some(&item.name),
        Declaration::TestModule(item) => Some(&item.name),
        Declaration::Capability(item) => Some(&item.name),
        Declaration::Import(_) | Declaration::VocabBlock(_) | Declaration::Docstring(_) => None,
    }
}

/// Return whether one declaration contributes to the package's public checked surface.
fn provider_declaration_is_public(declaration: &Declaration) -> bool {
    let visibility = match declaration {
        Declaration::Capability(item) => item.visibility,
        Declaration::Const(item) => item.visibility,
        Declaration::Static(item) => item.visibility,
        Declaration::Model(item) => item.visibility,
        Declaration::Class(item) => item.visibility,
        Declaration::Trait(item) => item.visibility,
        Declaration::Alias(item) => item.visibility,
        Declaration::Partial(item) => item.visibility,
        Declaration::TypeAlias(item) => item.visibility,
        Declaration::Newtype(item) => item.visibility,
        Declaration::Enum(item) => item.visibility,
        Declaration::Function(item) => item.visibility,
        Declaration::Import(item) => item.visibility,
        Declaration::TestModule(_) | Declaration::VocabBlock(_) | Declaration::Docstring(_) => Visibility::Private,
    };
    matches!(visibility, Visibility::Public)
}

/// Return whether the declaration owns checked source documentation.
fn provider_declaration_has_docstring(declaration: &Declaration) -> bool {
    match declaration {
        Declaration::Function(item) => item.body.first().is_some_and(|statement| {
            matches!(
                &statement.node,
                Statement::Expr(expression)
                    if matches!(&expression.node, Expr::Literal(Literal::String(_)))
            )
        }),
        Declaration::Model(item) => item.docstring.is_some(),
        Declaration::Class(item) => item.docstring.is_some(),
        Declaration::Trait(item) => item.docstring.is_some(),
        Declaration::Newtype(item) => item.docstring.is_some(),
        Declaration::Enum(item) => item.docstring.is_some(),
        _ => false,
    }
}

/// Return whether the declaration is a checked `std.registry` entry described by `@describe`.
fn provider_declaration_is_registry_entry(declaration: &Declaration) -> bool {
    let decorators = match declaration {
        Declaration::Model(item) => &item.decorators,
        Declaration::Class(item) => &item.decorators,
        Declaration::Trait(item) => &item.decorators,
        Declaration::Newtype(item) => &item.decorators,
        Declaration::Enum(item) => &item.decorators,
        Declaration::Function(item) => &item.decorators,
        _ => return false,
    };
    decorators.iter().any(|decorator| decorator.node.name == "describe")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;

    use crate::frontend::{ParsedModule, lexer, parser};
    use crate::library_manifest::{
        LibraryManifest, ProviderDependencyKind, ProviderDependencyMetadata, ProviderFactKind, digest_provider_artifact,
    };
    use crate::manifest::{DependencySource, DependencySpec};
    use crate::provider::FeatureSelection;

    #[test]
    fn provider_operation_metadata_is_projected_from_checked_declaration_facts()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::collections::BTreeMap;
        use std::path::PathBuf;

        use crate::frontend::typechecker::{ProviderOperationDeclarationInfo, TypeCheckInfo};
        use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind};

        let operation = CanonicalSymbolId::module_declaration(
            vec!["provider".to_string(), "billing".to_string()],
            "charge",
            SemanticSourceTargetKind::Function,
            HirSourceSpan::new(20, 26),
        );
        let required_capability = CanonicalSymbolId::module_declaration(
            vec!["provider".to_string(), "billing".to_string()],
            "charge_card",
            SemanticSourceTargetKind::Capability,
            HirSourceSpan::new(1, 12),
        );
        let mut type_info = TypeCheckInfo::default();
        type_info.declarations.provider_operations.insert(
            operation.clone(),
            ProviderOperationDeclarationInfo {
                operation: operation.clone(),
                required_capability: required_capability.clone(),
                runtime_requirements: Vec::new(),
            },
        );

        let descriptors = provider_operation_metadata_from_checked_type_info(&BTreeMap::from([(
            PathBuf::from("src/billing.incn"),
            type_info,
        )]))?;
        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].operation, operation);
        assert_eq!(descriptors[0].required_capability, required_capability);
        assert!(descriptors[0].runtime_requirements.is_empty());
        Ok(())
    }

    #[test]
    fn emitted_library_metadata_tracks_projected_dependency_issue911() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let artifact_root = workspace.path().join("published");
        let projected_root = workspace.path().join("projected");
        fs::create_dir_all(&artifact_root)?;
        fs::create_dir_all(projected_root.join("src"))?;
        fs::write(
            projected_root.join("Cargo.toml"),
            "[package]\nname = \"projected_provider\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(projected_root.join("src/lib.rs"), "pub fn marker() {}\n")?;
        let mut manifest = LibraryManifest::new("published", "0.1.0");
        manifest
            .contract_metadata
            .provider
            .provider_dependencies
            .push(ProviderDependencyMetadata {
                kind: ProviderDependencyKind::PublicPackage,
                dependency_key: "provider_alias".to_string(),
                provider_name: "projected_provider".to_string(),
                provider_version: "0.1.0".to_string(),
                artifact_digest: "sha256:stale".to_string(),
                relative_artifact_path: "../stale".to_string(),
                requested_features: BTreeSet::new(),
                default_features: false,
                optional: false,
            });
        let dependencies = vec![DependencySpec {
            crate_name: "provider_alias".to_string(),
            version: None,
            features: Vec::new(),
            default_features: false,
            source: DependencySource::Path {
                path: projected_root.clone(),
            },
            optional: false,
            package: Some("projected_provider".to_string()),
        }];

        synchronize_projected_provider_dependencies(&mut manifest, &artifact_root, &dependencies)?;

        let descriptor = &manifest.contract_metadata.provider.provider_dependencies[0];
        assert_eq!(descriptor.artifact_digest, digest_provider_artifact(&projected_root)?);
        assert_eq!(
            fs::canonicalize(artifact_root.join(&descriptor.relative_artifact_path))?,
            fs::canonicalize(projected_root)?
        );
        Ok(())
    }

    #[test]
    fn feature_conditions_are_preserved_for_provider_exports_docs_registries_and_reexports()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"when feature("catalog"):
    @describe(summary="Catalog entry")
    pub def catalog_entry() -> str:
        """Return the selected catalog entry."""
        return "catalog"

when feature("widgets"):
    pub from widgets import Widget
"#;
        let tokens = lexer::lex(source).map_err(|errors| format!("lex errors: {errors:?}"))?;
        let ast = parser::parse_with_module_path(&tokens, Some("project/src/lib.incn"))
            .map_err(|errors| format!("parse errors: {errors:?}"))?;
        let module = ParsedModule {
            name: "main".to_string(),
            path_segments: vec!["main".to_string()],
            file_path: PathBuf::from("project/src/lib.incn"),
            source: source.to_string(),
            ast,
        };

        let requirements = provider_fact_requirements(&module, &[BTreeSet::new()]);
        let catalog_features = BTreeSet::from(["catalog".to_string()]);
        for kind in [
            ProviderFactKind::Export,
            ProviderFactKind::Documentation,
            ProviderFactKind::RegistryEntry,
        ] {
            assert!(requirements.iter().any(|requirement| {
                requirement.kind == kind
                    && requirement.identity == "main::catalog_entry"
                    && requirement.required_features == catalog_features
            }));
        }
        assert!(requirements.iter().any(|requirement| {
            requirement.kind == ProviderFactKind::ProviderDependency
                && requirement.identity == "main::from:widgets"
                && requirement.required_features == BTreeSet::from(["widgets".to_string()])
        }));
        assert!(requirements.iter().any(|requirement| {
            requirement.kind == ProviderFactKind::Export
                && requirement.identity == "main::Widget"
                && requirement.required_features == BTreeSet::from(["widgets".to_string()])
        }));
        Ok(())
    }

    #[test]
    fn provider_module_conditions_preserve_nested_and_alternative_feature_paths()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let source_root = project.path().join("src");
        fs::create_dir_all(&source_root)?;
        let entry_path = source_root.join("lib.incn");
        let nested_path = source_root.join("nested.incn");
        let leaf_path = source_root.join("leaf.incn");
        let entry_source = r#"when feature("outer"):
    from nested import Nested

when feature("alternate"):
    from leaf import Leaf
"#;
        let nested_source = r#"when feature("inner"):
    from leaf import Leaf

pub model Nested:
    pub value: int
"#;
        let leaf_source = "pub model Leaf:\n    pub value: int\n";
        fs::write(&entry_path, entry_source)?;
        fs::write(&nested_path, nested_source)?;
        fs::write(&leaf_path, leaf_source)?;

        let parse_module = |name: &str,
                            path_segments: Vec<String>,
                            file_path: PathBuf,
                            source: &str|
         -> Result<ParsedModule, Box<dyn std::error::Error>> {
            let tokens = lexer::lex(source).map_err(|errors| format!("lex errors: {errors:?}"))?;
            let ast = parser::parse_with_module_path(&tokens, file_path.to_str())
                .map_err(|errors| format!("parse errors: {errors:?}"))?;
            Ok(ParsedModule {
                name: name.to_string(),
                path_segments,
                file_path,
                source: source.to_string(),
                ast,
            })
        };
        let entry = parse_module("main", vec!["main".to_string()], entry_path, entry_source)?;
        let nested = parse_module("nested", vec!["nested".to_string()], nested_path, nested_source)?;
        let leaf = parse_module("leaf", vec!["leaf".to_string()], leaf_path, leaf_source)?;
        let modules = vec![entry.clone(), nested, leaf.clone()];

        let requirements = provider_module_reachability_requirements(&modules, &entry, &source_root)?;
        let nested_key = vec!["nested".to_string()];
        let leaf_key = vec!["leaf".to_string()];
        assert_eq!(
            requirements.get(&nested_key),
            Some(&vec![BTreeSet::from(["outer".to_string()])])
        );
        assert_eq!(
            requirements.get(&leaf_key),
            Some(&vec![
                BTreeSet::from(["alternate".to_string()]),
                BTreeSet::from(["inner".to_string(), "outer".to_string()]),
            ])
        );

        let leaf_facts = provider_fact_requirements(
            &leaf,
            requirements
                .get(&leaf_key)
                .ok_or("leaf reachability should be present")?,
        );
        assert!(leaf_facts.iter().any(|fact| {
            fact.kind == ProviderFactKind::Export
                && fact.identity == "leaf::Leaf"
                && fact.required_features == BTreeSet::from(["alternate".to_string()])
        }));
        assert!(leaf_facts.iter().any(|fact| {
            fact.kind == ProviderFactKind::Export
                && fact.identity == "leaf::Leaf"
                && fact.required_features == BTreeSet::from(["inner".to_string(), "outer".to_string()])
        }));
        Ok(())
    }

    #[test]
    fn unprojected_provider_collection_rejects_unknown_features_in_inactive_modules()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let source_root = project.path().join("src");
        fs::create_dir_all(&source_root)?;
        fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname = \"provider_features\"\n\n[project.features]\ndefault = []\nouter = []\n",
        )?;
        let entry_path = source_root.join("lib.incn");
        fs::write(&entry_path, "when feature(\"outer\"):\n    from nested import Nested\n")?;
        fs::write(
            source_root.join("nested.incn"),
            "when feature(\"missing\"):\n    pub model Nested:\n        pub value: int\n",
        )?;

        let session = crate::driver::session::CompilationSession::discover_with_feature_selection(
            &entry_path,
            &FeatureSelection::default(),
        )?;
        let error = collect_unprojected_provider_modules(&entry_path, &session)
            .err()
            .ok_or("unknown feature in inactive provider module should fail collection")?;

        assert!(error.message.contains("Unknown package feature `missing`"));
        Ok(())
    }
}
