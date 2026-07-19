//! Compiler-backed codegraph inspection.
//!
//! `incan inspect codegraph` emits the first durable RFC 106 graph slice under the broader RFC 102 semantic inspection
//! umbrella. The export is intentionally source- and syntax-fact oriented in 0.4: it gives tools stable files,
//! modules, declarations, imports, exports, containment, and diagnostics without introducing a storage/indexing engine
//! into the compiler.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::ValueEnum;
use incan_codegraph::{
    CODEGRAPH_SCHEMA_VERSION, CodegraphCallRecord, CodegraphComponentSelectionReason, CodegraphContainmentRecord,
    CodegraphDeclarationRecord, CodegraphDependencyFeatureProjection, CodegraphDiagnosticRecord,
    CodegraphDiagnosticRelatedSpan, CodegraphExportRecord, CodegraphFeatureActivationReason,
    CodegraphFeatureReasonProjection, CodegraphFileRecord, CodegraphHeaderRecord, CodegraphImportRecord,
    CodegraphLanguage, CodegraphMode, CodegraphModuleRecord, CodegraphPackage, CodegraphPackageFeatureProjection,
    CodegraphProvenance, CodegraphProviderParticipation, CodegraphProviderProjection, CodegraphProviderProvenance,
    CodegraphRecord, CodegraphReferenceRecord, CodegraphSdkComponentProjection, CodegraphSdkProjection,
    CodegraphSemanticContext, CodegraphSourceSpan, to_jsonl,
};
use incan_semantics_core::{CompilerNodeId, SemanticModuleSnapshot, SemanticSourceTarget};

use crate::cli::prelude::ParsedModule;
use crate::cli::{CliError, CliResult, ExitCode};
use crate::frontend::ast::{
    AssertKind, CallArg, ComprehensionClause, Condition, Declaration, Decorator, DecoratorArg, DecoratorArgValue,
    DictEntry, Expr, FStringPart, FunctionDecl, ImportDecl, ImportItem, ImportKind, ImportPath, ListEntry, MatchBody,
    RaceForBody, Span, Spanned, Statement, SurfaceExprPayload, SurfaceStmtPayload, TypeParam, Visibility,
};
use crate::frontend::diagnostics::{self, StableDiagnostic};
use crate::frontend::module::canonicalize_source_module_segments;
use crate::provider::{
    BackendImplementationRequirement, ComponentSelectionReason, FeatureActivationReason, FeatureSelection,
    ProviderParticipation, ProviderPlan, ProviderProvenance,
};
use crate::version::INCAN_VERSION;

use super::common::{
    CliDiagnosticFailure, CompilationSession, collect_modules_detailed_with_selections,
    collect_modules_detailed_with_session, discover_effective_project_manifest, read_source, resolve_project_root,
};

/// Output format for `incan inspect codegraph`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodegraphInspectionFormat {
    /// Newline-delimited JSON records.
    Jsonl,
}

/// Emit compiler-backed codegraph facts for one Incan file or directory.
pub fn inspect_codegraph(
    path: &Path,
    format: CodegraphInspectionFormat,
    allow_errors: bool,
    feature_selection: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> CliResult<ExitCode> {
    let normalized = normalize_input_path(path)?;
    let records = collect_codegraph_records(&normalized, allow_errors, feature_selection, sdk_profile_override)?;
    match format {
        CodegraphInspectionFormat::Jsonl => {
            let jsonl = to_jsonl(&records)
                .map_err(|error| CliError::failure(format!("failed to serialize codegraph JSONL: {error}")))?;
            print!("{jsonl}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Collect checked or tolerant graph records for one normalized input path.
fn collect_codegraph_records(
    path: &Path,
    allow_errors: bool,
    feature_selection: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> CliResult<Vec<CodegraphRecord>> {
    let package = package_identity(path)?;
    let mut builder = CodegraphBuilder::new(path, package, allow_errors);
    builder.semantic_contexts =
        collect_codegraph_semantic_contexts(path, feature_selection, sdk_profile_override, allow_errors)?;

    if path.is_dir() {
        let files = discover_incan_files(path)?;
        let (modules, diagnostics, semantic_snapshots_by_path) =
            directory_modules_diagnostics_and_info(&files, feature_selection, sdk_profile_override)?;
        if !diagnostics.is_empty() && !allow_errors {
            return Err(CliError::failure(render_diagnostics(&diagnostics)));
        }
        if diagnostics.is_empty() {
            builder.set_semantic_snapshots(semantic_snapshots_by_path);
            builder.seed_source_target_ids(&modules);
            for module in &modules {
                builder.collect_parsed_module(module, Vec::new());
            }
        } else {
            let mut sessions = BTreeMap::new();
            for file in &files {
                let project_root = resolve_project_root(file);
                if !sessions.contains_key(&project_root) {
                    sessions.insert(
                        project_root.clone(),
                        CompilationSession::discover_with_selections(file, feature_selection, sdk_profile_override)?,
                    );
                }
                let Some(session) = sessions.get(&project_root) else {
                    return Err(CliError::failure(format!(
                        "failed to prepare codegraph compilation session for {}",
                        project_root.display()
                    )));
                };
                builder.collect_tolerant_file_with_session(file, session)?;
            }
        }
        builder.collect_diagnostics(diagnostics);
        if !allow_errors && builder.has_diagnostics() {
            return Err(CliError::failure(render_diagnostics(builder.diagnostics())));
        }
    } else {
        let session = CompilationSession::discover_with_selections(path, feature_selection, sdk_profile_override)?;
        match collect_modules_detailed_with_session(path.to_path_buf(), &session) {
            Ok(modules) => {
                let (diagnostics, semantic_snapshots_by_path) = typecheck_diagnostics_and_info(&session, &modules)?;
                if !diagnostics.is_empty() && !allow_errors {
                    return Err(CliError::failure(render_diagnostics(&diagnostics)));
                }
                builder.set_semantic_snapshots(semantic_snapshots_by_path);
                builder.seed_source_target_ids(&modules);
                for module in &modules {
                    builder.collect_parsed_module(module, diagnostics_for_file(&diagnostics, &module.file_path));
                }
                builder.collect_diagnostics(diagnostics);
            }
            Err(failure) if allow_errors => {
                builder.collect_tolerant_failure(path, failure, feature_selection, sdk_profile_override)?;
            }
            Err(failure) => return Err(CliError::failure(failure.render_human())),
        }
    }

    Ok(builder.finish())
}

type DirectoryTypecheckArtifacts = (
    Vec<ParsedModule>,
    Vec<StableDiagnostic>,
    BTreeMap<PathBuf, SemanticModuleSnapshot>,
);

/// Collect and typecheck every discovered directory source root, keeping semantic artifacts only when the whole
/// directory graph is clean.
fn directory_modules_diagnostics_and_info(
    files: &[PathBuf],
    feature_selection: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> CliResult<DirectoryTypecheckArtifacts> {
    let file_set = files.iter().cloned().collect::<BTreeSet<_>>();
    let mut modules_by_path = BTreeMap::new();
    let mut diagnostics = Vec::new();
    let mut sessions = BTreeMap::new();
    let mut semantic_snapshots_by_path = BTreeMap::new();

    for file in files {
        let project_root = resolve_project_root(file);
        if !sessions.contains_key(&project_root) {
            sessions.insert(
                project_root.clone(),
                CompilationSession::discover_with_selections(file, feature_selection, sdk_profile_override)?,
            );
        }
        let Some(session) = sessions.get(&project_root) else {
            return Err(CliError::failure(format!(
                "failed to prepare codegraph compilation session for {}",
                project_root.display()
            )));
        };
        match collect_modules_detailed_with_session(file.clone(), session) {
            Ok(modules) => {
                for module in &modules {
                    if file_set.contains(&module.file_path) {
                        modules_by_path
                            .entry(module.file_path.clone())
                            .or_insert_with(|| module.clone());
                    }
                }
                match session.analyze_modules(
                    &modules,
                    #[cfg(feature = "rust_inspect")]
                    None,
                ) {
                    Ok(analysis) => {
                        for (path, snapshot) in analysis.semantic_snapshots() {
                            semantic_snapshots_by_path
                                .entry(path.clone())
                                .or_insert_with(|| snapshot.clone());
                        }
                    }
                    Err(failure) => {
                        dedup_diagnostics(&mut diagnostics, stable_diagnostics(failure));
                    }
                }
            }
            Err(failure) => {
                dedup_diagnostics(&mut diagnostics, stable_diagnostics(failure));
            }
        }
    }

    if diagnostics.is_empty() {
        Ok((
            modules_by_path.into_values().collect(),
            diagnostics,
            semantic_snapshots_by_path,
        ))
    } else {
        Ok((Vec::new(), diagnostics, BTreeMap::new()))
    }
}

/// Run typechecking and keep reusable semantic artifacts when the checked graph succeeds.
fn typecheck_diagnostics_and_info(
    session: &CompilationSession,
    modules: &[ParsedModule],
) -> CliResult<(Vec<StableDiagnostic>, BTreeMap<PathBuf, SemanticModuleSnapshot>)> {
    match session.analyze_modules(
        modules,
        #[cfg(feature = "rust_inspect")]
        None,
    ) {
        Ok(analysis) => Ok((Vec::new(), analysis.semantic_snapshots().clone())),
        Err(failure) => Ok((stable_diagnostics(failure), BTreeMap::new())),
    }
}

/// Convert shared CLI diagnostic failures into the public diagnostic projection used by both `incan check` and
/// codegraph records.
fn stable_diagnostics(failure: CliDiagnosticFailure) -> Vec<StableDiagnostic> {
    failure
        .diagnostics
        .iter()
        .map(|diagnostic| {
            diagnostics::stable_diagnostic(
                &diagnostic.file_path,
                &diagnostic.source,
                &diagnostic.error,
                diagnostic.phase,
            )
        })
        .collect()
}

/// Append diagnostics while suppressing duplicate records produced by overlapping directory entrypoint checks.
fn dedup_diagnostics(target: &mut Vec<StableDiagnostic>, diagnostics: Vec<StableDiagnostic>) {
    for diagnostic in diagnostics {
        if !target.contains(&diagnostic) {
            target.push(diagnostic);
        }
    }
}

/// Return the diagnostics whose primary span belongs to one parsed module file.
fn diagnostics_for_file(diagnostics: &[StableDiagnostic], file_path: &Path) -> Vec<StableDiagnostic> {
    let file_path = file_path.to_string_lossy();
    diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.primary_span.file == file_path)
        .cloned()
        .collect()
}

/// Render a compact strict-mode failure summary without duplicating the full human diagnostic renderer in
/// JSONL-specific code.
fn render_diagnostics(diagnostics: &[StableDiagnostic]) -> String {
    let mut output = String::from("codegraph export failed because the checked graph has diagnostics");
    for diagnostic in diagnostics {
        output.push_str("\n- ");
        output.push_str(diagnostic.code);
        output.push_str(": ");
        output.push_str(&diagnostic.message);
    }
    output
}

/// Discover `.incn` files below a directory in deterministic path order.
fn discover_incan_files(root: &Path) -> CliResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_incan_files(root, &mut files)?;
    files.sort();
    Ok(files)
}

/// Recursively collect Incan source files while skipping build, VCS, and agent state directories that are not source
/// roots.
fn collect_incan_files(dir: &Path, files: &mut Vec<PathBuf>) -> CliResult<()> {
    for entry in fs::read_dir(dir)
        .map_err(|error| CliError::failure(format!("failed to read directory {}: {error}", dir.display())))?
    {
        let entry = entry.map_err(|error| CliError::failure(format!("failed to read directory entry: {error}")))?;
        let path = entry.path();
        if path.is_dir() {
            if should_skip_directory(&path) {
                continue;
            }
            collect_incan_files(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "incn") {
            files.push(path);
        }
    }
    Ok(())
}

/// Return whether a directory should be ignored by broad directory codegraph inspection.
fn should_skip_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, ".git" | ".agents" | ".venv" | "node_modules" | "target"))
}

/// Read package identity from the nearest manifest so exported graph headers can be joined with build reports and
/// metadata exports.
fn package_identity(path: &Path) -> CliResult<Option<CodegraphPackage>> {
    let project_root = resolve_project_root(path);
    let manifest = discover_effective_project_manifest(&project_root)
        .map_err(|error| CliError::failure(format!("failed to load project manifest: {error}")))?;
    Ok(manifest.map(|manifest| CodegraphPackage {
        name: manifest.project.as_ref().and_then(|project| project.name.clone()),
        version: manifest.project.as_ref().and_then(|project| project.version.clone()),
        root_path: Some(path_string(manifest.project_root())),
    }))
}

/// Build the typed provider/component/feature header projection from the same sessions used by codegraph checking.
fn collect_codegraph_semantic_contexts(
    path: &Path,
    feature_selection: &FeatureSelection,
    sdk_profile_override: Option<&str>,
    allow_errors: bool,
) -> CliResult<Vec<CodegraphSemanticContext>> {
    let files = if path.is_dir() {
        discover_incan_files(path)?
    } else {
        vec![path.to_path_buf()]
    };
    let mut projects: BTreeMap<PathBuf, (PathBuf, BTreeMap<PathBuf, ParsedModule>)> = BTreeMap::new();
    for file in files {
        let project_root = resolve_project_root(&file);
        let (_, modules_by_path) = projects
            .entry(project_root.clone())
            .or_insert_with(|| (file.clone(), BTreeMap::new()));
        if let Ok(modules) =
            collect_modules_detailed_with_selections(&file.to_string_lossy(), feature_selection, sdk_profile_override)
        {
            for module in modules {
                if resolve_project_root(&module.file_path) == project_root {
                    modules_by_path.insert(module.file_path.clone(), module);
                }
            }
        }
    }

    let mut contexts = Vec::new();
    for (project_root, (representative, modules_by_path)) in projects {
        let session = match CompilationSession::discover_with_selections(
            &representative,
            feature_selection,
            sdk_profile_override,
        ) {
            Ok(session) => session,
            Err(_) if allow_errors => continue,
            Err(error) => return Err(error),
        };
        let modules = modules_by_path.into_values().collect::<Vec<_>>();
        let provider_plan = match session.provider_plan_for_modules(&modules) {
            Ok(plan) => plan,
            Err(_) if allow_errors => Arc::clone(&session.provider_plan),
            Err(error) => return Err(error),
        };
        contexts.push(codegraph_semantic_context(&project_root, &session, &provider_plan));
    }
    Ok(contexts)
}

/// Convert compiler-owned semantic plans to the storage-agnostic codegraph wire contract.
fn codegraph_semantic_context(
    project_root: &Path,
    session: &CompilationSession,
    provider_plan: &ProviderPlan,
) -> CodegraphSemanticContext {
    let sdk = session
        .sdk_inventory
        .as_ref()
        .zip(session.sdk_components.as_ref())
        .map(|(inventory, components)| CodegraphSdkProjection {
            identity: inventory.identity(),
            profile: components.profile.clone(),
            components: inventory
                .components
                .values()
                .map(|component| CodegraphSdkComponentProjection {
                    id: component.id.clone(),
                    version: component.version.clone(),
                    available: component.available,
                    enabled: components.enabled.contains(&component.id),
                    mandatory: component.mandatory,
                    dependencies: component.dependencies.iter().cloned().collect(),
                    reason: components
                        .reasons
                        .get(&component.id)
                        .map(codegraph_component_selection_reason),
                })
                .collect(),
        });
    let packages = session
        .package_feature_plan
        .iter()
        .flat_map(|plan| plan.packages())
        .map(|package| CodegraphPackageFeatureProjection {
            package: package.package_name.clone(),
            project_root: path_string(&package.project_root),
            active_features: package.features.active_features.iter().cloned().collect(),
            active_optional_dependencies: package.features.active_optional_dependencies.iter().cloned().collect(),
            dependency_features: package
                .features
                .dependency_features
                .iter()
                .map(|(dependency, features)| CodegraphDependencyFeatureProjection {
                    dependency: dependency.clone(),
                    features: features.iter().cloned().collect(),
                })
                .collect(),
            required_sdk_components: package.features.required_sdk_components.iter().cloned().collect(),
            reasons: package
                .features
                .reasons
                .iter()
                .map(|(feature, reasons)| CodegraphFeatureReasonProjection {
                    feature: feature.clone(),
                    reasons: reasons.iter().map(codegraph_feature_activation_reason).collect(),
                })
                .collect(),
        })
        .collect();
    let providers = provider_plan
        .records()
        .map(|provider| CodegraphProviderProjection {
            identity: provider.identity.stable_key(),
            available: provider.available,
            enabled: provider.enabled,
            participation: codegraph_provider_participation(provider_plan.participation(provider)),
            provenance: codegraph_provider_provenance(&provider.provenance),
            namespace_claims: provider.namespace_claims.iter().cloned().collect(),
            used_modules: provider_plan.used_modules(provider).into_iter().collect(),
            active_features: provider.identity.feature_projection.iter().cloned().collect(),
            implementation_facets: provider_plan
                .selected_implementation_facets(provider)
                .into_iter()
                .map(|facet| facet.id.clone())
                .collect(),
            backend_requirements: provider_plan
                .selected_backend_requirements(provider)
                .iter()
                .map(codegraph_backend_requirement)
                .collect(),
            manifest_path: provider
                .artifact
                .as_ref()
                .map(|artifact| path_string(&artifact.manifest_path)),
        })
        .collect();
    CodegraphSemanticContext {
        project_root: path_string(project_root),
        sdk,
        packages,
        providers,
    }
}

/// Project one compiler component-selection reason into the versioned codegraph schema.
fn codegraph_component_selection_reason(reason: &ComponentSelectionReason) -> CodegraphComponentSelectionReason {
    match reason {
        ComponentSelectionReason::Mandatory => CodegraphComponentSelectionReason::Mandatory,
        ComponentSelectionReason::Profile { profile } => CodegraphComponentSelectionReason::Profile(profile.clone()),
        ComponentSelectionReason::Explicit => CodegraphComponentSelectionReason::Explicit,
        ComponentSelectionReason::Dependency { required_by } => {
            CodegraphComponentSelectionReason::Dependency(required_by.clone())
        }
    }
}

/// Project one package-feature activation reason into the versioned codegraph schema.
fn codegraph_feature_activation_reason(reason: &FeatureActivationReason) -> CodegraphFeatureActivationReason {
    match reason {
        FeatureActivationReason::Default => CodegraphFeatureActivationReason::Default,
        FeatureActivationReason::Requested => CodegraphFeatureActivationReason::Requested,
        FeatureActivationReason::AllFeatures => CodegraphFeatureActivationReason::AllFeatures,
        FeatureActivationReason::IncludedBy(feature) => CodegraphFeatureActivationReason::IncludedBy(feature.clone()),
        FeatureActivationReason::DependencyRequest { package, dependency } => {
            CodegraphFeatureActivationReason::DependencyRequest {
                package: package.clone(),
                dependency: dependency.clone(),
            }
        }
    }
}

/// Project provider availability, enablement, and use into the versioned codegraph participation enum.
fn codegraph_provider_participation(participation: ProviderParticipation) -> CodegraphProviderParticipation {
    match participation {
        ProviderParticipation::Unavailable => CodegraphProviderParticipation::Unavailable,
        ProviderParticipation::Disabled => CodegraphProviderParticipation::Disabled,
        ProviderParticipation::Enabled => CodegraphProviderParticipation::Enabled,
        ProviderParticipation::Used => CodegraphProviderParticipation::Used,
    }
}

/// Project provider authority and origin into portable codegraph provenance.
fn codegraph_provider_provenance(provenance: &ProviderProvenance) -> CodegraphProviderProvenance {
    match provenance {
        ProviderProvenance::ProjectDependency {
            dependency_key,
            manifest_path,
        } => CodegraphProviderProvenance::ProjectDependency {
            dependency_key: dependency_key.clone(),
            manifest_path: path_string(manifest_path),
        },
        ProviderProvenance::Sdk {
            sdk_identity,
            component_id,
            inventory_path,
        } => CodegraphProviderProvenance::Sdk {
            sdk_identity: sdk_identity.clone(),
            component_id: component_id.clone(),
            inventory_path: inventory_path.as_ref().map(|path| path_string(path)),
        },
        ProviderProvenance::Compiler => CodegraphProviderProvenance::Compiler,
    }
}

/// Render one private provider implementation requirement in the stable codegraph vocabulary.
fn codegraph_backend_requirement(requirement: &BackendImplementationRequirement) -> String {
    match requirement {
        BackendImplementationRequirement::CargoFeature { crate_name, feature } => {
            format!("cargo-feature:{crate_name}/{feature}")
        }
        BackendImplementationRequirement::CargoDependency { dependency } => {
            format!("cargo-dependency:{}", dependency.crate_name)
        }
    }
}

/// Normalize a user-provided CLI path relative to the current working directory.
fn normalize_input_path(path: &Path) -> CliResult<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(env::current_dir()
            .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))?
            .join(path))
    }
}

struct CodegraphBuilder {
    records: Vec<CodegraphRecord>,
    diagnostics: Vec<StableDiagnostic>,
    file_ids: BTreeMap<String, String>,
    module_ids: BTreeSet<String>,
    mode: CodegraphMode,
    root_path: String,
    root_path_buf: PathBuf,
    package: Option<CodegraphPackage>,
    semantic_contexts: Vec<CodegraphSemanticContext>,
    next_body_fact_index: usize,
    semantic_snapshots_by_path: BTreeMap<PathBuf, SemanticModuleSnapshot>,
    source_target_ids: BTreeMap<(Vec<String>, String, String), String>,
}

/// Compact source declaration facts used before serializing a public declaration record.
struct DeclarationSummary {
    kind: String,
    name: String,
    visibility: Visibility,
    type_params: Vec<String>,
    signature: Option<String>,
}

impl CodegraphBuilder {
    /// Create a record builder for one strict or tolerant export.
    fn new(root_path: &Path, package: Option<CodegraphPackage>, allow_errors: bool) -> Self {
        Self {
            records: Vec::new(),
            diagnostics: Vec::new(),
            file_ids: BTreeMap::new(),
            module_ids: BTreeSet::new(),
            mode: if allow_errors {
                CodegraphMode::AllowErrors
            } else {
                CodegraphMode::Strict
            },
            root_path: path_string(root_path),
            root_path_buf: root_path.to_path_buf(),
            package,
            semantic_contexts: Vec::new(),
            next_body_fact_index: 0,
            semantic_snapshots_by_path: BTreeMap::new(),
            source_target_ids: BTreeMap::new(),
        }
    }

    /// Attach session-owned semantic facts for checked body target population.
    fn set_semantic_snapshots(&mut self, semantic_snapshots_by_path: BTreeMap<PathBuf, SemanticModuleSnapshot>) {
        self.semantic_snapshots_by_path = semantic_snapshots_by_path;
    }

    /// Precompute declaration ids for source targets before body facts are emitted.
    ///
    /// Body facts consume typechecker-proven target artifacts. This map only connects those artifacts to declaration
    /// records that this export will emit, including public source reexports whose declaration identity lives in
    /// another module.
    fn seed_source_target_ids(&mut self, modules: &[ParsedModule]) {
        for module in modules {
            for (index, declaration) in module.ast.declarations.iter().enumerate() {
                let Some(summary) = declaration_summary(&declaration.node) else {
                    continue;
                };
                let declaration_id = declaration_id(module, declaration, index);
                for module_path in source_module_target_paths(module) {
                    self.source_target_ids.insert(
                        (module_path, summary.name.clone(), summary.kind.clone()),
                        declaration_id.clone(),
                    );
                }
            }
        }

        for _ in 0..modules.len() {
            let before = self.source_target_ids.len();
            for module in modules {
                self.seed_import_source_target_ids(module);
            }
            if self.source_target_ids.len() == before {
                break;
            }
        }
    }

    /// Connect one module's source imports/reexports to already-known source declaration ids.
    fn seed_import_source_target_ids(&mut self, module: &ParsedModule) {
        for declaration in &module.ast.declarations {
            let Declaration::Import(import) = &declaration.node else {
                continue;
            };
            let ImportKind::From {
                module: imported,
                items,
            } = &import.kind
            else {
                continue;
            };
            if imported.parent_levels != 0 {
                continue;
            }
            let target_module_path = canonicalize_source_module_segments(&imported.segments);
            for item in items {
                let local_name = item.alias.as_ref().unwrap_or(&item.name);
                let Some((target_kind, target_id)) =
                    self.source_target_id_for_module_name(&target_module_path, &item.name)
                else {
                    continue;
                };
                for module_path in source_module_target_paths(module) {
                    self.source_target_ids.insert(
                        (module_path, local_name.clone(), target_kind.clone()),
                        target_id.clone(),
                    );
                }
            }
        }
    }

    /// Return the first known source target id for one module/name pair.
    fn source_target_id_for_module_name(&self, module_path: &[String], name: &str) -> Option<(String, String)> {
        self.source_target_ids
            .iter()
            .find(|((candidate_module_path, candidate_name, _), _)| {
                candidate_module_path == module_path && candidate_name == name
            })
            .map(|((_, _, kind), id)| (kind.clone(), id.clone()))
    }

    /// Recover as much source structure as possible after the ordinary entrypoint collection path failed.
    fn collect_tolerant_failure(
        &mut self,
        path: &Path,
        failure: CliDiagnosticFailure,
        feature_selection: &FeatureSelection,
        sdk_profile_override: Option<&str>,
    ) -> CliResult<()> {
        let before = self.diagnostics.len();
        if path.is_file() {
            self.collect_tolerant_file(path, feature_selection, sdk_profile_override)?;
        }
        if self.diagnostics.len() == before {
            self.collect_diagnostics(stable_diagnostics(failure));
        }
        Ok(())
    }

    /// Parse one file with project-aware vocabulary context and record either syntax facts or parse diagnostics.
    fn collect_tolerant_file(
        &mut self,
        path: &Path,
        feature_selection: &FeatureSelection,
        sdk_profile_override: Option<&str>,
    ) -> CliResult<()> {
        let session = CompilationSession::discover_with_selections(path, feature_selection, sdk_profile_override)?;
        self.collect_tolerant_file_with_session(path, &session)
    }

    /// Parse one file with a caller-provided project-aware session, avoiding repeated manifest/vocab discovery.
    fn collect_tolerant_file_with_session(&mut self, path: &Path, session: &CompilationSession) -> CliResult<()> {
        let source = read_source(&path.to_string_lossy())?;
        match session.parse_source_for_collection(path, &source) {
            Ok(ast) => {
                let file_id = self.ensure_file_record(path, &source, false);
                let module = ParsedModule {
                    name: module_name_for_file(path),
                    path_segments: self.fallback_module_segments(path),
                    file_path: path.to_path_buf(),
                    source,
                    ast,
                };
                self.collect_module_records(&module, &file_id, Vec::new());
            }
            Err(errors) => {
                self.ensure_file_record(path, &source, true);
                let diagnostics = errors
                    .iter()
                    .map(|error| {
                        diagnostics::stable_diagnostic(
                            &path.to_string_lossy(),
                            &source,
                            error,
                            diagnostics::DiagnosticPhase::Parse,
                        )
                    })
                    .collect::<Vec<_>>();
                self.collect_diagnostics(diagnostics);
            }
        }
        Ok(())
    }

    /// Add graph records for one module that was already parsed by the canonical collection path.
    fn collect_parsed_module(&mut self, module: &ParsedModule, diagnostics: Vec<StableDiagnostic>) {
        let degraded = !diagnostics.is_empty();
        let file_id = self.ensure_file_record(&module.file_path, &module.source, degraded);
        self.collect_module_records(module, &file_id, diagnostics);
    }

    /// Add file, module, and module-containment facts before descending into declarations.
    fn collect_module_records(&mut self, module: &ParsedModule, file_id: &str, diagnostics: Vec<StableDiagnostic>) {
        let degraded = !diagnostics.is_empty();
        let module_id = module_id(module);
        if self.module_ids.insert(module_id.clone()) {
            let module_span = source_span(&module.file_path, &module.source, Span::new(0, module.source.len()));
            self.records.push(CodegraphRecord::Module(CodegraphModuleRecord {
                id: module_id.clone(),
                language: CodegraphLanguage::Incan,
                file_id: file_id.to_string(),
                module_path: module.path_segments.clone(),
                name: module.name.clone(),
                span: Some(module_span),
                provenance: CodegraphProvenance::Syntax,
                degraded,
            }));
            self.records
                .push(CodegraphRecord::Containment(CodegraphContainmentRecord {
                    id: format!("contains:{file_id}:{module_id}"),
                    language: CodegraphLanguage::Incan,
                    parent_id: file_id.to_string(),
                    child_id: module_id.clone(),
                    kind: "file_contains_module".to_string(),
                    span: None,
                    provenance: CodegraphProvenance::Source,
                    degraded,
                }));
        }
        self.collect_program_records(module, &module_id, degraded);
    }

    /// Add declaration, import, export, and containment records for a parsed module body.
    fn collect_program_records(&mut self, module: &ParsedModule, module_id: &str, degraded: bool) {
        self.index_module_declaration_targets(module);
        for (index, declaration) in module.ast.declarations.iter().enumerate() {
            match &declaration.node {
                Declaration::Import(import) => {
                    let import_id = import_id(module, index);
                    self.records.push(CodegraphRecord::Import(import_record(
                        module,
                        module_id,
                        &import_id,
                        import,
                        declaration.span,
                        degraded,
                    )));
                    self.records.push(CodegraphRecord::Containment(containment_record(
                        module_id,
                        &import_id,
                        "module_contains_import",
                        &module.file_path,
                        &module.source,
                        declaration.span,
                        degraded,
                    )));
                    if import.visibility == Visibility::Public {
                        for name in import_export_names(import) {
                            self.records.push(CodegraphRecord::Export(export_record(
                                module,
                                module_id,
                                &import_id,
                                &name,
                                "import",
                                declaration.span,
                                degraded,
                            )));
                        }
                    }
                }
                Declaration::Docstring(_) => {}
                _ => {
                    let declaration_id = declaration_id(module, declaration, index);
                    let Some(summary) = declaration_summary(&declaration.node) else {
                        continue;
                    };
                    self.records
                        .push(CodegraphRecord::Declaration(CodegraphDeclarationRecord {
                            id: declaration_id.clone(),
                            language: CodegraphLanguage::Incan,
                            module_id: module_id.to_string(),
                            kind: summary.kind.clone(),
                            name: summary.name.clone(),
                            visibility: visibility_spelling(summary.visibility).to_string(),
                            type_params: summary.type_params,
                            signature: summary.signature,
                            span: Some(source_span(&module.file_path, &module.source, declaration.span)),
                            provenance: CodegraphProvenance::Syntax,
                            degraded,
                        }));
                    self.records.push(CodegraphRecord::Containment(containment_record(
                        module_id,
                        &declaration_id,
                        "module_contains_declaration",
                        &module.file_path,
                        &module.source,
                        declaration.span,
                        degraded,
                    )));
                    if summary.visibility == Visibility::Public {
                        self.records.push(CodegraphRecord::Export(export_record(
                            module,
                            module_id,
                            &declaration_id,
                            &summary.name,
                            "declaration",
                            declaration.span,
                            degraded,
                        )));
                    }
                    self.collect_declaration_body_records(module, module_id, &declaration_id, declaration, degraded);
                }
            }
        }
    }

    /// Index declaration ids before walking bodies so forward calls can resolve to later declarations in the module.
    fn index_module_declaration_targets(&mut self, module: &ParsedModule) {
        for (index, declaration) in module.ast.declarations.iter().enumerate() {
            let Some(summary) = declaration_summary(&declaration.node) else {
                continue;
            };
            let declaration_id = declaration_id(module, declaration, index);
            for module_path in source_module_target_paths(module) {
                self.source_target_ids.insert(
                    (module_path, summary.name.clone(), summary.kind.clone()),
                    declaration_id.clone(),
                );
            }
        }
    }

    /// Add body-level reference and call facts under a declaration owner.
    fn collect_declaration_body_records(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: &str,
        declaration: &Spanned<Declaration>,
        degraded: bool,
    ) {
        match &declaration.node {
            Declaration::Const(decl) => self.collect_expr(module, module_id, Some(owner_id), &decl.value, degraded),
            Declaration::Static(decl) => self.collect_expr(module, module_id, Some(owner_id), &decl.value, degraded),
            Declaration::Model(decl) => {
                self.collect_decorators(module, module_id, Some(owner_id), &decl.decorators, degraded);
                for field in &decl.fields {
                    if let Some(default) = &field.node.default {
                        self.collect_expr(module, module_id, Some(owner_id), default, degraded);
                    }
                }
                for method in &decl.methods {
                    self.collect_method_body_records(module, module_id, Some(owner_id), &method.node, degraded);
                }
                for property in &decl.properties {
                    if let Some(body) = &property.node.body {
                        self.collect_statements(module, module_id, Some(owner_id), body, degraded);
                    }
                }
            }
            Declaration::Class(decl) => {
                self.collect_decorators(module, module_id, Some(owner_id), &decl.decorators, degraded);
                for field in &decl.fields {
                    if let Some(default) = &field.node.default {
                        self.collect_expr(module, module_id, Some(owner_id), default, degraded);
                    }
                }
                for method in &decl.methods {
                    self.collect_method_body_records(module, module_id, Some(owner_id), &method.node, degraded);
                }
                for property in &decl.properties {
                    if let Some(body) = &property.node.body {
                        self.collect_statements(module, module_id, Some(owner_id), body, degraded);
                    }
                }
            }
            Declaration::Trait(decl) => {
                self.collect_decorators(module, module_id, Some(owner_id), &decl.decorators, degraded);
                for method in &decl.methods {
                    self.collect_method_body_records(module, module_id, Some(owner_id), &method.node, degraded);
                }
                for property in &decl.properties {
                    if let Some(body) = &property.node.body {
                        self.collect_statements(module, module_id, Some(owner_id), body, degraded);
                    }
                }
            }
            Declaration::Newtype(decl) => {
                self.collect_decorators(module, module_id, Some(owner_id), &decl.decorators, degraded);
                for rebinding in &decl.rebindings {
                    self.collect_expr(module, module_id, Some(owner_id), &rebinding.node.target, degraded);
                }
                for edge in &decl.interop_edges {
                    self.collect_expr(module, module_id, Some(owner_id), &edge.node.adapter, degraded);
                }
                for method in &decl.methods {
                    self.collect_method_body_records(module, module_id, Some(owner_id), &method.node, degraded);
                }
            }
            Declaration::Enum(decl) => {
                self.collect_decorators(module, module_id, Some(owner_id), &decl.decorators, degraded);
                for method in &decl.methods {
                    self.collect_method_body_records(module, module_id, Some(owner_id), &method.node, degraded);
                }
            }
            Declaration::Function(decl) => {
                self.collect_decorators(module, module_id, Some(owner_id), &decl.decorators, degraded);
                self.collect_param_defaults(module, module_id, Some(owner_id), &decl.params, degraded);
                self.collect_statements(module, module_id, Some(owner_id), &decl.body, degraded);
            }
            Declaration::TestModule(decl) => {
                for nested in &decl.body {
                    self.collect_declaration_body_records(module, module_id, owner_id, nested, degraded);
                }
            }
            Declaration::Import(_)
            | Declaration::Alias(_)
            | Declaration::Partial(_)
            | Declaration::TypeAlias(_)
            | Declaration::Docstring(_) => {}
        }
    }

    /// Collect method decorators, parameter defaults, and body facts under the enclosing declaration owner.
    fn collect_method_body_records(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        method: &crate::frontend::ast::MethodDecl,
        degraded: bool,
    ) {
        self.collect_decorators(module, module_id, owner_id, &method.decorators, degraded);
        self.collect_param_defaults(module, module_id, owner_id, &method.params, degraded);
        if let Some(body) = &method.body {
            self.collect_statements(module, module_id, owner_id, body, degraded);
        }
    }

    /// Collect expression-valued decorator arguments.
    fn collect_decorators(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        decorators: &[Spanned<Decorator>],
        degraded: bool,
    ) {
        for decorator in decorators {
            for arg in &decorator.node.args {
                match arg {
                    DecoratorArg::Positional(value) => {
                        self.collect_expr(module, module_id, owner_id, value, degraded);
                    }
                    DecoratorArg::Named(_, DecoratorArgValue::Expr(value)) => {
                        self.collect_expr(module, module_id, owner_id, value, degraded);
                    }
                    DecoratorArg::Named(_, DecoratorArgValue::Type(_)) => {}
                }
            }
        }
    }

    /// Collect default expressions attached to function or method parameters.
    fn collect_param_defaults(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        params: &[Spanned<crate::frontend::ast::Param>],
        degraded: bool,
    ) {
        for param in params {
            if let Some(default) = &param.node.default {
                self.collect_expr(module, module_id, owner_id, default, degraded);
            }
        }
    }

    /// Collect expression facts from a statement list in source order.
    fn collect_statements(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        statements: &[Spanned<Statement>],
        degraded: bool,
    ) {
        for statement in statements {
            self.collect_statement(module, module_id, owner_id, statement, degraded);
        }
    }

    /// Collect expression facts from one statement and its descendants.
    fn collect_statement(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        statement: &Spanned<Statement>,
        degraded: bool,
    ) {
        match &statement.node {
            Statement::Assignment(stmt) => self.collect_expr(module, module_id, owner_id, &stmt.value, degraded),
            Statement::FieldAssignment(stmt) => {
                self.collect_expr(module, module_id, owner_id, &stmt.object, degraded);
                self.collect_expr(module, module_id, owner_id, &stmt.value, degraded);
            }
            Statement::IndexAssignment(stmt) => {
                self.collect_expr(module, module_id, owner_id, &stmt.object, degraded);
                self.collect_expr(module, module_id, owner_id, &stmt.index, degraded);
                self.collect_expr(module, module_id, owner_id, &stmt.value, degraded);
            }
            Statement::Return(Some(expr)) | Statement::Expr(expr) | Statement::Break(Some(expr)) => {
                self.collect_expr(module, module_id, owner_id, expr, degraded);
            }
            Statement::If(stmt) => {
                self.collect_condition(module, module_id, owner_id, &stmt.condition, degraded);
                self.collect_statements(module, module_id, owner_id, &stmt.then_body, degraded);
                for (condition, body) in &stmt.elif_branches {
                    self.collect_expr(module, module_id, owner_id, condition, degraded);
                    self.collect_statements(module, module_id, owner_id, body, degraded);
                }
                if let Some(body) = &stmt.else_body {
                    self.collect_statements(module, module_id, owner_id, body, degraded);
                }
            }
            Statement::Loop(stmt) => self.collect_statements(module, module_id, owner_id, &stmt.body, degraded),
            Statement::While(stmt) => {
                self.collect_condition(module, module_id, owner_id, &stmt.condition, degraded);
                self.collect_statements(module, module_id, owner_id, &stmt.body, degraded);
            }
            Statement::For(stmt) => {
                self.collect_expr(module, module_id, owner_id, &stmt.iter, degraded);
                self.collect_statements(module, module_id, owner_id, &stmt.body, degraded);
            }
            Statement::VocabExpressionItem(item) => {
                self.collect_expr(module, module_id, owner_id, &item.expr, degraded);
                for modifier in &item.modifiers {
                    self.collect_expr(module, module_id, owner_id, &modifier.value, degraded);
                }
            }
            Statement::Assert(stmt) => {
                match &stmt.kind {
                    AssertKind::Condition(condition) => {
                        self.collect_expr(module, module_id, owner_id, condition, degraded);
                    }
                    AssertKind::IsPattern { value, .. } => {
                        self.collect_expr(module, module_id, owner_id, value, degraded);
                    }
                    AssertKind::Raises { call, .. } => {
                        self.collect_expr(module, module_id, owner_id, call, degraded);
                    }
                }
                if let Some(message) = &stmt.message {
                    self.collect_expr(module, module_id, owner_id, message, degraded);
                }
            }
            Statement::CompoundAssignment(stmt) => {
                self.collect_expr(module, module_id, owner_id, &stmt.value, degraded)
            }
            Statement::TupleUnpack(stmt) => self.collect_expr(module, module_id, owner_id, &stmt.value, degraded),
            Statement::TupleAssign(stmt) => {
                for target in &stmt.targets {
                    self.collect_expr(module, module_id, owner_id, target, degraded);
                }
                self.collect_expr(module, module_id, owner_id, &stmt.value, degraded);
            }
            Statement::ChainedAssignment(stmt) => self.collect_expr(module, module_id, owner_id, &stmt.value, degraded),
            Statement::Surface(stmt) => match &stmt.payload {
                SurfaceStmtPayload::KeywordArgs(args) => {
                    for arg in args {
                        self.collect_expr(module, module_id, owner_id, arg, degraded);
                    }
                }
            },
            Statement::VocabBlock(block) => {
                self.collect_decorators(module, module_id, owner_id, &block.decorators, degraded);
                for arg in &block.header_args {
                    self.collect_expr(module, module_id, owner_id, arg, degraded);
                }
                self.collect_statements(module, module_id, owner_id, &block.body, degraded);
            }
            Statement::Return(None) | Statement::Pass | Statement::Break(None) | Statement::Continue => {}
        }
    }

    /// Collect expression facts from a condition.
    fn collect_condition(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        condition: &Condition,
        degraded: bool,
    ) {
        match condition {
            Condition::Expr(expr) | Condition::Let { value: expr, .. } => {
                self.collect_expr(module, module_id, owner_id, expr, degraded);
            }
        }
    }

    /// Collect expression facts from one call argument.
    fn collect_call_arg(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        arg: &CallArg,
        degraded: bool,
    ) {
        match arg {
            CallArg::Positional(expr)
            | CallArg::Named(_, expr)
            | CallArg::PositionalUnpack(expr)
            | CallArg::KeywordUnpack(expr) => self.collect_expr(module, module_id, owner_id, expr, degraded),
        }
    }

    /// Collect source-level reference and call facts from one expression.
    fn collect_expr(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        expr: &Spanned<Expr>,
        degraded: bool,
    ) {
        match &expr.node {
            Expr::Ident(name) => {
                self.push_reference(module, module_id, owner_id, name, "identifier", expr.span, degraded)
            }
            Expr::SelfExpr => self.push_reference(module, module_id, owner_id, "self", "self", expr.span, degraded),
            Expr::Literal(_) => {}
            Expr::Binary(left, _, right) | Expr::Index(left, right) => {
                self.collect_expr(module, module_id, owner_id, left, degraded);
                self.collect_expr(module, module_id, owner_id, right, degraded);
            }
            Expr::Unary(_, value) | Expr::Try(value) | Expr::Paren(value) => {
                self.collect_expr(module, module_id, owner_id, value, degraded);
            }
            Expr::Call(callee, type_args, args) => {
                self.push_call(
                    module,
                    module_id,
                    owner_id,
                    &expr_label(&callee.node),
                    "function",
                    args.len(),
                    type_args.len(),
                    expr.span,
                    degraded,
                );
                self.collect_expr(module, module_id, owner_id, callee, degraded);
                for arg in args {
                    self.collect_call_arg(module, module_id, owner_id, arg, degraded);
                }
            }
            Expr::MethodCall(receiver, method, type_args, args) => {
                self.push_call(
                    module,
                    module_id,
                    owner_id,
                    method,
                    "method",
                    args.len(),
                    type_args.len(),
                    expr.span,
                    degraded,
                );
                self.collect_expr(module, module_id, owner_id, receiver, degraded);
                for arg in args {
                    self.collect_call_arg(module, module_id, owner_id, arg, degraded);
                }
            }
            Expr::Partial(partial) => {
                self.collect_expr(module, module_id, owner_id, &partial.target, degraded);
                for arg in &partial.args {
                    self.collect_expr(module, module_id, owner_id, &arg.value, degraded);
                }
            }
            Expr::Slice(base, slice) => {
                self.collect_expr(module, module_id, owner_id, base, degraded);
                for value in [&slice.start, &slice.end, &slice.step].into_iter().flatten() {
                    self.collect_expr(module, module_id, owner_id, value, degraded);
                }
            }
            Expr::Field(base, field) => {
                self.collect_expr(module, module_id, owner_id, base, degraded);
                self.push_reference(module, module_id, owner_id, field, "field", expr.span, degraded);
            }
            Expr::Constructor(name, args) => {
                self.push_call(
                    module,
                    module_id,
                    owner_id,
                    name,
                    "constructor",
                    args.len(),
                    0,
                    expr.span,
                    degraded,
                );
                for arg in args {
                    self.collect_call_arg(module, module_id, owner_id, arg, degraded);
                }
            }
            Expr::Match(scrutinee, arms) => {
                self.collect_expr(module, module_id, owner_id, scrutinee, degraded);
                for arm in arms {
                    if let Some(guard) = &arm.node.guard {
                        self.collect_expr(module, module_id, owner_id, guard, degraded);
                    }
                    match &arm.node.body {
                        MatchBody::Expr(value) => self.collect_expr(module, module_id, owner_id, value, degraded),
                        MatchBody::Block(body) => self.collect_statements(module, module_id, owner_id, body, degraded),
                    }
                }
            }
            Expr::If(if_expr) => {
                self.collect_expr(module, module_id, owner_id, &if_expr.condition, degraded);
                self.collect_statements(module, module_id, owner_id, &if_expr.then_body, degraded);
                if let Some(body) = &if_expr.else_body {
                    self.collect_statements(module, module_id, owner_id, body, degraded);
                }
            }
            Expr::Loop(loop_expr) => self.collect_statements(module, module_id, owner_id, &loop_expr.body, degraded),
            Expr::ListComp(comp) => {
                self.collect_expr(module, module_id, owner_id, &comp.expr, degraded);
                self.collect_comprehension_clauses(module, module_id, owner_id, &comp.clauses, degraded);
            }
            Expr::DictComp(comp) => {
                self.collect_expr(module, module_id, owner_id, &comp.key, degraded);
                self.collect_expr(module, module_id, owner_id, &comp.value, degraded);
                self.collect_comprehension_clauses(module, module_id, owner_id, &comp.clauses, degraded);
            }
            Expr::Generator(generator) => {
                self.collect_expr(module, module_id, owner_id, &generator.expr, degraded);
                self.collect_comprehension_clauses(module, module_id, owner_id, &generator.clauses, degraded);
            }
            Expr::Closure(params, body) => {
                self.collect_param_defaults(module, module_id, owner_id, params, degraded);
                self.collect_expr(module, module_id, owner_id, body, degraded);
            }
            Expr::Tuple(items) | Expr::Set(items) => {
                for item in items {
                    self.collect_expr(module, module_id, owner_id, item, degraded);
                }
            }
            Expr::List(entries) => {
                for entry in entries {
                    match entry {
                        ListEntry::Element(value) | ListEntry::Spread(value) => {
                            self.collect_expr(module, module_id, owner_id, value, degraded);
                        }
                    }
                }
            }
            Expr::Dict(entries) => {
                for entry in entries {
                    match entry {
                        DictEntry::Pair(key, value) => {
                            self.collect_expr(module, module_id, owner_id, key, degraded);
                            self.collect_expr(module, module_id, owner_id, value, degraded);
                        }
                        DictEntry::Spread(value) => self.collect_expr(module, module_id, owner_id, value, degraded),
                    }
                }
            }
            Expr::FString(parts) => {
                for part in parts {
                    if let FStringPart::Expr { expr, .. } = part {
                        self.collect_expr(module, module_id, owner_id, expr, degraded);
                    }
                }
            }
            Expr::Yield(Some(value)) => self.collect_expr(module, module_id, owner_id, value, degraded),
            Expr::Range { start, end, .. } => {
                self.collect_expr(module, module_id, owner_id, start, degraded);
                self.collect_expr(module, module_id, owner_id, end, degraded);
            }
            Expr::Surface(surface) => match &surface.payload {
                SurfaceExprPayload::PrefixUnary(value) => {
                    self.collect_expr(module, module_id, owner_id, value, degraded);
                }
                SurfaceExprPayload::RaceFor(race) => {
                    for arm in &race.arms {
                        self.collect_expr(module, module_id, owner_id, &arm.awaitable, degraded);
                        match &arm.body {
                            RaceForBody::Expr(value) => {
                                self.collect_expr(module, module_id, owner_id, value, degraded);
                            }
                            RaceForBody::Block(body) => {
                                self.collect_statements(module, module_id, owner_id, body, degraded);
                            }
                        }
                    }
                }
                SurfaceExprPayload::LeadingDotPath { segments, .. } => {
                    self.push_reference(
                        module,
                        module_id,
                        owner_id,
                        &segments.join("."),
                        "surface_path",
                        expr.span,
                        degraded,
                    );
                }
                SurfaceExprPayload::ScopedGlyph { left, right, .. } => {
                    self.collect_expr(module, module_id, owner_id, left, degraded);
                    self.collect_expr(module, module_id, owner_id, right, degraded);
                }
                SurfaceExprPayload::ScopedSymbolCall { symbol, args, .. } => {
                    self.push_call(
                        module,
                        module_id,
                        owner_id,
                        symbol,
                        "surface_symbol",
                        args.len(),
                        0,
                        expr.span,
                        degraded,
                    );
                    for arg in args {
                        self.collect_call_arg(module, module_id, owner_id, arg, degraded);
                    }
                }
            },
            Expr::VocabBlock(block) => {
                self.collect_decorators(module, module_id, owner_id, &block.decorators, degraded);
                for arg in &block.header_args {
                    self.collect_expr(module, module_id, owner_id, arg, degraded);
                }
                self.collect_statements(module, module_id, owner_id, &block.body, degraded);
            }
            Expr::Yield(None) => {}
        }
    }

    /// Collect comprehension clause expressions.
    fn collect_comprehension_clauses(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        clauses: &[ComprehensionClause],
        degraded: bool,
    ) {
        for clause in clauses {
            match clause {
                ComprehensionClause::For { iter, .. } | ComprehensionClause::If(iter) => {
                    self.collect_expr(module, module_id, owner_id, iter, degraded);
                }
            }
        }
    }

    /// Push one source-level reference record and its owner containment edge.
    #[allow(clippy::too_many_arguments)]
    fn push_reference(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        name: &str,
        kind: &str,
        span: Span,
        degraded: bool,
    ) {
        let id = self.next_body_fact_id("reference", module, span, name);
        let target_id = self.source_target_id(module, span);
        let provenance = provenance_for_target(target_id.as_ref());
        self.records.push(CodegraphRecord::Reference(CodegraphReferenceRecord {
            id: id.clone(),
            language: CodegraphLanguage::Incan,
            module_id: module_id.to_string(),
            owner_id: owner_id.map(str::to_string),
            name: name.to_string(),
            kind: kind.to_string(),
            target_id,
            span: Some(source_span(&module.file_path, &module.source, span)),
            provenance,
            degraded,
        }));
        if let Some(owner_id) = owner_id {
            self.records.push(CodegraphRecord::Containment(containment_record(
                owner_id,
                &id,
                "declaration_contains_reference",
                &module.file_path,
                &module.source,
                span,
                degraded,
            )));
        }
    }

    /// Push one source-level call record and its owner containment edge.
    #[allow(clippy::too_many_arguments)]
    fn push_call(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        callee: &str,
        kind: &str,
        argument_count: usize,
        type_argument_count: usize,
        span: Span,
        degraded: bool,
    ) {
        let id = self.next_body_fact_id("call", module, span, callee);
        let target_id = self.source_target_id(module, span);
        let provenance = provenance_for_target(target_id.as_ref());
        self.records.push(CodegraphRecord::Call(CodegraphCallRecord {
            id: id.clone(),
            language: CodegraphLanguage::Incan,
            module_id: module_id.to_string(),
            owner_id: owner_id.map(str::to_string),
            callee: callee.to_string(),
            kind: kind.to_string(),
            argument_count,
            type_argument_count,
            target_id,
            span: Some(source_span(&module.file_path, &module.source, span)),
            provenance,
            degraded,
        }));
        if let Some(owner_id) = owner_id {
            self.records.push(CodegraphRecord::Containment(containment_record(
                owner_id,
                &id,
                "declaration_contains_call",
                &module.file_path,
                &module.source,
                span,
                degraded,
            )));
        }
    }

    /// Return the next deterministic body-fact id for one export.
    fn next_body_fact_id(&mut self, kind: &str, module: &ParsedModule, span: Span, label: &str) -> String {
        let index = self.next_body_fact_index;
        self.next_body_fact_index += 1;
        format!(
            "{kind}:{}:{}:{index}:{}",
            module.file_path.to_string_lossy(),
            span.start,
            sanitize_record_label(label)
        )
    }

    /// Insert one file record if it has not already been seen, returning the stable file id.
    fn ensure_file_record(&mut self, path: &Path, source: &str, degraded: bool) -> String {
        let path = path_string(path);
        if let Some(id) = self.file_ids.get(&path) {
            return id.clone();
        }
        let id = format!("file:{path}");
        self.records.push(CodegraphRecord::File(CodegraphFileRecord {
            id: id.clone(),
            language: CodegraphLanguage::Incan,
            path: path.clone(),
            size_bytes: source.len(),
            provenance: CodegraphProvenance::Source,
            degraded,
        }));
        self.file_ids.insert(path, id.clone());
        id
    }

    /// Buffer diagnostics until `finish` appends them after syntax facts.
    fn collect_diagnostics(&mut self, diagnostics: Vec<StableDiagnostic>) {
        dedup_diagnostics(&mut self.diagnostics, diagnostics);
    }

    /// Return whether the export has diagnostics and should be considered degraded.
    fn has_diagnostics(&self) -> bool {
        !self.diagnostics.is_empty()
    }

    /// Return buffered diagnostics for strict-mode failure rendering.
    fn diagnostics(&self) -> &[StableDiagnostic] {
        &self.diagnostics
    }

    /// Infer module path segments for independently parsed files using the inspected root as the stable base.
    fn fallback_module_segments(&self, path: &Path) -> Vec<String> {
        let base = if self.root_path_buf.is_dir() {
            self.root_path_buf.as_path()
        } else {
            self.root_path_buf.parent().unwrap_or_else(|| Path::new("."))
        };
        module_segments_for_file(path, base)
    }

    /// Assemble the final header, syntax records, and diagnostic records in stable JSONL order.
    fn finish(mut self) -> Vec<CodegraphRecord> {
        let degraded = self.records.iter().any(record_degraded) || !self.diagnostics.is_empty();
        let mut records = vec![CodegraphRecord::Header(CodegraphHeaderRecord {
            schema_version: CODEGRAPH_SCHEMA_VERSION,
            compiler_version: INCAN_VERSION.to_string(),
            mode: self.mode,
            root_path: self.root_path,
            languages: vec![CodegraphLanguage::Incan],
            package: self.package,
            semantic_contexts: self.semantic_contexts,
            degraded,
        })];
        records.append(&mut self.records);
        for (index, diagnostic) in self.diagnostics.iter().enumerate() {
            records.push(CodegraphRecord::Diagnostic(diagnostic_record(index, diagnostic)));
        }
        records
    }

    /// Return a declaration record id for a compiler-proven source target at `span`.
    fn source_target_id(&self, module: &ParsedModule, span: Span) -> Option<String> {
        let module_identity = if module.path_segments.is_empty() {
            "<module>".to_string()
        } else {
            module.path_segments.join("::")
        };
        let subject = CompilerNodeId::expression_span(&module_identity, span.start, span.end);
        let target = self
            .semantic_snapshots_by_path
            .get(&module.file_path)?
            .facts
            .source_targets_for(&subject)
            .next()?;
        self.declaration_id_for_source_target(target)
    }

    /// Resolve one source target artifact to an emitted declaration id.
    fn declaration_id_for_source_target(&self, target: &SemanticSourceTarget) -> Option<String> {
        self.source_target_ids
            .get(&(
                target.module_path.clone(),
                target.name.clone(),
                target.kind.as_str().to_string(),
            ))
            .cloned()
    }
}

/// Read the degraded flag from any codegraph record variant.
fn record_degraded(record: &CodegraphRecord) -> bool {
    match record {
        CodegraphRecord::Header(record) => record.degraded,
        CodegraphRecord::File(record) => record.degraded,
        CodegraphRecord::Module(record) => record.degraded,
        CodegraphRecord::Declaration(record) => record.degraded,
        CodegraphRecord::Import(record) => record.degraded,
        CodegraphRecord::Export(record) => record.degraded,
        CodegraphRecord::Reference(record) => record.degraded,
        CodegraphRecord::Call(record) => record.degraded,
        CodegraphRecord::Containment(record) => record.degraded,
        CodegraphRecord::Diagnostic(record) => record.degraded,
    }
}

/// Return module path spellings that can refer to one parsed source module in checked target artifacts.
fn source_module_target_paths(module: &ParsedModule) -> Vec<Vec<String>> {
    let mut paths = BTreeSet::new();
    paths.insert(module.path_segments.clone());
    if let Some(stem) = module.file_path.file_stem().and_then(|stem| stem.to_str()) {
        paths.insert(vec![stem.to_string()]);
    }
    paths.into_iter().collect()
}

/// Return provenance for body facts that may carry compiler-checked source targets.
fn provenance_for_target(target_id: Option<&String>) -> CodegraphProvenance {
    if target_id.is_some() {
        CodegraphProvenance::Checked
    } else {
        CodegraphProvenance::Syntax
    }
}

/// Return a compact source-facing callee label for a call expression.
fn expr_label(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name) => name.clone(),
        Expr::SelfExpr => "self".to_string(),
        Expr::Field(base, field) => format!("{}.{}", expr_label(&base.node), field),
        Expr::Paren(inner) => expr_label(&inner.node),
        Expr::Surface(surface) => match &surface.payload {
            SurfaceExprPayload::ScopedSymbolCall { symbol, .. } => symbol.clone(),
            SurfaceExprPayload::LeadingDotPath { segments, .. } => segments.join("."),
            _ => "<expr>".to_string(),
        },
        _ => "<expr>".to_string(),
    }
}

/// Sanitize free-form labels so record ids stay readable and single-line.
fn sanitize_record_label(label: &str) -> String {
    let sanitized = label
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "_".to_string()
    } else {
        sanitized
    }
}

/// Build one import record from source AST import syntax.
fn import_record(
    module: &ParsedModule,
    module_id: &str,
    import_id: &str,
    import: &ImportDecl,
    span: Span,
    degraded: bool,
) -> CodegraphImportRecord {
    let (kind, path, items) = import_shape(import);
    CodegraphImportRecord {
        id: import_id.to_string(),
        language: CodegraphLanguage::Incan,
        module_id: module_id.to_string(),
        kind,
        path,
        items,
        alias: import.alias.clone(),
        visibility: visibility_spelling(import.visibility).to_string(),
        span: Some(source_span(&module.file_path, &module.source, span)),
        provenance: CodegraphProvenance::Syntax,
        degraded,
    }
}

/// Build one containment edge between two source-backed records.
fn containment_record(
    parent_id: &str,
    child_id: &str,
    kind: &str,
    file_path: &Path,
    source: &str,
    span: Span,
    degraded: bool,
) -> CodegraphContainmentRecord {
    CodegraphContainmentRecord {
        id: format!("contains:{parent_id}:{child_id}"),
        language: CodegraphLanguage::Incan,
        parent_id: parent_id.to_string(),
        child_id: child_id.to_string(),
        kind: kind.to_string(),
        span: Some(source_span(file_path, source, span)),
        provenance: CodegraphProvenance::Syntax,
        degraded,
    }
}

/// Build one public export fact from either a declaration or public import source record.
fn export_record(
    module: &ParsedModule,
    module_id: &str,
    source_id: &str,
    name: &str,
    kind: &str,
    span: Span,
    degraded: bool,
) -> CodegraphExportRecord {
    CodegraphExportRecord {
        id: format!("export:{module_id}:{name}:{kind}"),
        language: CodegraphLanguage::Incan,
        module_id: module_id.to_string(),
        name: name.to_string(),
        kind: kind.to_string(),
        source_id: source_id.to_string(),
        span: Some(source_span(&module.file_path, &module.source, span)),
        provenance: CodegraphProvenance::Syntax,
        degraded,
    }
}

/// Convert a stable diagnostic into the codegraph diagnostic record shape.
fn diagnostic_record(index: usize, diagnostic: &StableDiagnostic) -> CodegraphDiagnosticRecord {
    CodegraphDiagnosticRecord {
        id: format!(
            "diagnostic:{}:{}:{}",
            diagnostic.primary_span.file, diagnostic.primary_span.start.offset, index
        ),
        language: CodegraphLanguage::Incan,
        code: diagnostic.code.to_string(),
        severity: diagnostic.severity.to_string(),
        phase: diagnostic.phase.as_str().to_string(),
        origin: diagnostic.origin.as_str().to_string(),
        message: diagnostic.message.clone(),
        primary_span: CodegraphSourceSpan {
            file: diagnostic.primary_span.file.clone(),
            start: diagnostic.primary_span.start.offset,
            end: diagnostic.primary_span.end.offset,
            start_line: diagnostic.primary_span.start.line,
            start_column: diagnostic.primary_span.start.column,
            end_line: diagnostic.primary_span.end.line,
            end_column: diagnostic.primary_span.end.column,
        },
        notes: diagnostic.notes.clone(),
        hints: diagnostic.hints.clone(),
        expected: diagnostic.expected.clone(),
        actual: diagnostic.actual.clone(),
        related_spans: diagnostic
            .related_spans
            .iter()
            .map(|related| CodegraphDiagnosticRelatedSpan {
                span: CodegraphSourceSpan {
                    file: related.span.file.clone(),
                    start: related.span.start.offset,
                    end: related.span.end.offset,
                    start_line: related.span.start.line,
                    start_column: related.span.start.column,
                    end_line: related.span.end.line,
                    end_column: related.span.end.column,
                },
                label: related.label.clone(),
            })
            .collect(),
        explain: diagnostic.explain.clone(),
        provenance: CodegraphProvenance::Diagnostic,
        degraded: true,
    }
}

/// Summarize a top-level source declaration for the baseline codegraph record set.
fn declaration_summary(declaration: &Declaration) -> Option<DeclarationSummary> {
    match declaration {
        Declaration::Const(decl) => Some(DeclarationSummary {
            kind: "const".to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: Vec::new(),
            signature: decl.ty.as_ref().map(|ty| format!("const {}: {}", decl.name, ty.node)),
        }),
        Declaration::Static(decl) => Some(DeclarationSummary {
            kind: "static".to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: Vec::new(),
            signature: Some(format!("static {}: {}", decl.name, decl.ty.node)),
        }),
        Declaration::Model(decl) => Some(DeclarationSummary {
            kind: "model".to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: type_param_names(&decl.type_params),
            signature: Some(format_type_decl_signature("model", &decl.name, &decl.type_params)),
        }),
        Declaration::Class(decl) => Some(DeclarationSummary {
            kind: "class".to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: type_param_names(&decl.type_params),
            signature: Some(format_type_decl_signature("class", &decl.name, &decl.type_params)),
        }),
        Declaration::Trait(decl) => Some(DeclarationSummary {
            kind: "trait".to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: type_param_names(&decl.type_params),
            signature: Some(format_type_decl_signature("trait", &decl.name, &decl.type_params)),
        }),
        Declaration::Alias(decl) => Some(DeclarationSummary {
            kind: "alias".to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: Vec::new(),
            signature: Some(format!("{} = alias {}", decl.name, import_path_display(&decl.target))),
        }),
        Declaration::Partial(decl) => Some(DeclarationSummary {
            kind: "partial".to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: Vec::new(),
            signature: Some(format!("{} = partial {}", decl.name, import_path_display(&decl.target))),
        }),
        Declaration::TypeAlias(decl) => Some(DeclarationSummary {
            kind: "type_alias".to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: type_param_names(&decl.type_params),
            signature: Some(format!(
                "type {}{} = {}",
                decl.name,
                format_type_params(&decl.type_params),
                decl.target.node
            )),
        }),
        Declaration::Newtype(decl) => Some(DeclarationSummary {
            kind: if decl.is_rusttype { "rusttype" } else { "newtype" }.to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: type_param_names(&decl.type_params),
            signature: Some(format_type_decl_signature(
                if decl.is_rusttype { "rusttype" } else { "newtype" },
                &decl.name,
                &decl.type_params,
            )),
        }),
        Declaration::Enum(decl) => Some(DeclarationSummary {
            kind: "enum".to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: type_param_names(&decl.type_params),
            signature: Some(format_type_decl_signature("enum", &decl.name, &decl.type_params)),
        }),
        Declaration::Function(decl) => Some(DeclarationSummary {
            kind: "function".to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: type_param_names(&decl.type_params),
            signature: Some(function_signature(decl)),
        }),
        Declaration::TestModule(decl) => Some(DeclarationSummary {
            kind: "test_module".to_string(),
            name: decl.name.clone(),
            visibility: Visibility::Private,
            type_params: Vec::new(),
            signature: Some(format!("module {}", decl.name)),
        }),
        Declaration::Import(_) | Declaration::Docstring(_) => None,
    }
}

/// Format a source-level function signature from parsed parameter and return annotations.
fn function_signature(decl: &FunctionDecl) -> String {
    let params = decl
        .params
        .iter()
        .map(|param| format!("{}: {}", param.node.name, param.node.ty.node))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "def {}{}({}) -> {}",
        decl.name,
        format_type_params(&decl.type_params),
        params,
        decl.return_type.node
    )
}

/// Format a declaration signature prefix for type-bearing declarations.
fn format_type_decl_signature(kind: &str, name: &str, type_params: &[TypeParam]) -> String {
    format!("{kind} {name}{}", format_type_params(type_params))
}

/// Extract generic parameter names without serializing their full bounds yet.
fn type_param_names(type_params: &[TypeParam]) -> Vec<String> {
    type_params.iter().map(|param| param.name.clone()).collect()
}

/// Format generic parameters in source syntax for signature summaries.
fn format_type_params(type_params: &[TypeParam]) -> String {
    if type_params.is_empty() {
        String::new()
    } else {
        format!(
            "[{}]",
            type_params
                .iter()
                .map(|param| param.name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

/// Return the import kind, path, and item list for one parsed import declaration.
fn import_shape(import: &ImportDecl) -> (String, String, Vec<String>) {
    match &import.kind {
        ImportKind::Module(path) => ("module".to_string(), import_path_display(path), Vec::new()),
        ImportKind::From { module, items } => (
            "from".to_string(),
            import_path_display(module),
            items.iter().map(import_item_display).collect(),
        ),
        ImportKind::PubLibrary { library } => ("pub_library".to_string(), format!("pub::{library}"), Vec::new()),
        ImportKind::PubFrom { library, items } => (
            "pub_from".to_string(),
            format!("pub::{library}"),
            items.iter().map(import_item_display).collect(),
        ),
        ImportKind::Python(module) => ("python".to_string(), module.clone(), Vec::new()),
        ImportKind::RustCrate {
            crate_name,
            path,
            version: _,
            features: _,
        } => (
            "rust_crate".to_string(),
            rust_path_display(crate_name, path),
            Vec::new(),
        ),
        ImportKind::RustFrom {
            crate_name,
            path,
            version: _,
            features: _,
            items,
        } => (
            "rust_from".to_string(),
            rust_path_display(crate_name, path),
            items.iter().map(import_item_display).collect(),
        ),
    }
}

/// Return the public names produced by a public import declaration.
fn import_export_names(import: &ImportDecl) -> Vec<String> {
    match &import.kind {
        ImportKind::Module(path) => vec![import.alias.clone().unwrap_or_else(|| {
            path.segments
                .last()
                .cloned()
                .unwrap_or_else(|| import_path_display(path))
        })],
        ImportKind::From { items, .. } | ImportKind::PubFrom { items, .. } | ImportKind::RustFrom { items, .. } => {
            items
                .iter()
                .map(|item| item.alias.clone().unwrap_or_else(|| item.name.clone()))
                .collect()
        }
        ImportKind::PubLibrary { library } => vec![import.alias.clone().unwrap_or_else(|| library.clone())],
        ImportKind::Python(module) => vec![import.alias.clone().unwrap_or_else(|| module.clone())],
        ImportKind::RustCrate { crate_name, path, .. } => vec![
            import
                .alias
                .clone()
                .unwrap_or_else(|| path.last().cloned().unwrap_or_else(|| crate_name.clone())),
        ],
    }
}

/// Format one imported item, preserving local alias spelling when present.
fn import_item_display(item: &ImportItem) -> String {
    if let Some(alias) = &item.alias {
        format!("{} as {alias}", item.name)
    } else {
        item.name.clone()
    }
}

/// Format a parsed Incan import path without resolving it to a filesystem path.
fn import_path_display(path: &ImportPath) -> String {
    let mut parts = Vec::new();
    if path.is_absolute {
        parts.push("crate".to_string());
    }
    for _ in 0..path.parent_levels {
        parts.push("..".to_string());
    }
    parts.extend(path.segments.clone());
    parts.join("::")
}

/// Format a Rust import path with the `rust::` namespace marker used by Incan source.
fn rust_path_display(crate_name: &str, path: &[String]) -> String {
    if path.is_empty() {
        format!("rust::{crate_name}")
    } else {
        format!("rust::{crate_name}::{}", path.join("::"))
    }
}

/// Return the stable JSON spelling for source visibility.
fn visibility_spelling(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Private => "private",
        Visibility::Public => "public",
    }
}

/// Build a module id that keeps same-named modules in different files distinct.
fn module_id(module: &ParsedModule) -> String {
    format!(
        "module:{}:{}",
        module.file_path.to_string_lossy(),
        module.path_segments.join("::")
    )
}

/// Build a declaration id from file, span, and symbol name so declaration order changes do not alone rename ids.
fn declaration_id(module: &ParsedModule, declaration: &Spanned<Declaration>, index: usize) -> String {
    let name = declaration_summary(&declaration.node)
        .map(|summary| summary.name)
        .unwrap_or_else(|| format!("decl-{index}"));
    format!(
        "decl:{}:{}:{name}",
        module.file_path.to_string_lossy(),
        declaration.span.start
    )
}

/// Build an import id from file and declaration index.
fn import_id(module: &ParsedModule, index: usize) -> String {
    format!("import:{}:{index}", module.file_path.to_string_lossy())
}

/// Infer a fallback module name for independently parsed directory files.
fn module_name_for_file(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("module")
        .to_string()
}

/// Infer fallback module path segments for independently parsed directory files.
fn module_segments_for_file(path: &Path, base: &Path) -> Vec<String> {
    let relative = path.strip_prefix(base).unwrap_or(path);
    let mut segments = relative
        .components()
        .filter_map(|component| component.as_os_str().to_str().map(str::to_string))
        .collect::<Vec<_>>();
    if let Some(last) = segments.last_mut()
        && let Some(stem) = last.strip_suffix(".incn")
    {
        *last = stem.to_string();
    }
    if segments.is_empty() {
        vec![module_name_for_file(path)]
    } else {
        segments
    }
}

/// Convert an AST byte span into the public codegraph source span shape.
fn source_span(path: &Path, source: &str, span: Span) -> CodegraphSourceSpan {
    let start = span.start.min(source.len());
    let end = span.end.min(source.len()).max(start);
    let (start_line, start_column) = line_column_for_offset(source, start);
    let (end_line, end_column) = line_column_for_offset(source, end);
    CodegraphSourceSpan {
        file: path_string(path),
        start,
        end,
        start_line,
        start_column,
        end_line,
        end_column,
    }
}

/// Convert a byte offset into 1-based line and column coordinates.
fn line_column_for_offset(source: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(source.len());
    let mut line = 1usize;
    let mut column = 1usize;
    for (idx, ch) in source.char_indices() {
        if idx >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

/// Format a filesystem path using the process-native display spelling.
fn path_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}
