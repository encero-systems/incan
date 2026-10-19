//! The compilation session: one per command, owning the project, the compiled Loaves it imports from, and the
//! analysis every stage after parsing consumes (DD-0007).
//!
//! A session is opened once from the project and reused by every stage, so the Loaves, the vocabulary they bring and
//! the active features are decided once per command.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use incan_lang::lang::standard_packages::{
    STANDARD_PACKAGE_NAMESPACE_POLICIES, StandardPackageNamespacePolicy, standard_package_namespace_policy,
};
use incan_lang::lang::stdlib;

#[cfg(feature = "cli")]
use crate::diagnostics::CliDiagnosticFailure;
use crate::error::{CliError, CliResult};
use crate::features::{FeatureSelection, PackageFeatureGraph};
use crate::project::{discover_effective_project_manifest, resolve_project_root, resolve_source_root};
use crate::typecheck::typecheck_modules;
use incan_frontend::ast::{Program, Span};
use incan_frontend::contract_metadata::{
    CanonicalModelBundle, materialize_contract_models, read_project_model_bundles,
};
use incan_frontend::hir::build_semantic_module_snapshot_v0;
use incan_frontend::loaf_index::{LoafEntry, LoafIndex, artifact_dir};
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::testing_markers::{TestingMarkerSemantics, testing_marker_semantics_from_manifest};
use incan_frontend::typechecker::TypeCheckInfo;
use incan_frontend::{diagnostics, lexer, parser, vocab_desugar_pass};
use oven_model::manifest::{LOAF_MANIFEST_FILENAME, ProjectManifest};
use oven_model::toolchain_layout::CompilerOwnedSourceLayout;

/// The build profile a command reads Loaf artifacts for unless it names another.
pub const DEFAULT_PROFILE: &str = "debug";

/// Checked products produced by one [`CompilationSession`] analysis pass: each module's checked facts, which Body IR
/// lowers from, and its portable semantic snapshot, which tooling reads.
#[derive(Debug, Clone)]
pub struct CompilationAnalysis {
    type_info_by_path: BTreeMap<PathBuf, TypeCheckInfo>,
    type_info_by_module_path: BTreeMap<Vec<String>, TypeCheckInfo>,
    semantic_snapshots_by_path: BTreeMap<PathBuf, incan_semantics_core::SemanticModuleSnapshot>,
}

/// One module's checked facts and portable semantic snapshot from one [`CompilationAnalysis`].
pub struct CompilationModuleAnalysis<'analysis> {
    type_info: &'analysis TypeCheckInfo,
    semantic_snapshot: &'analysis incan_semantics_core::SemanticModuleSnapshot,
}

impl CompilationModuleAnalysis<'_> {
    /// The module's checked facts.
    pub fn type_info(&self) -> &TypeCheckInfo {
        self.type_info
    }

    /// The module's portable semantic snapshot.
    pub fn semantic_snapshot(&self) -> &incan_semantics_core::SemanticModuleSnapshot {
        self.semantic_snapshot
    }
}

impl CompilationAnalysis {
    /// Return the paired checked products for one collected source file.
    pub fn module_analysis_for_path(&self, path: &Path) -> Option<CompilationModuleAnalysis<'_>> {
        Some(CompilationModuleAnalysis {
            type_info: self.type_info_by_path.get(path)?,
            semantic_snapshot: self.semantic_snapshots_by_path.get(path)?,
        })
    }

    /// Return the checked facts of one collected source file.
    pub fn type_info_for_path(&self, path: &Path) -> Option<&TypeCheckInfo> {
        self.type_info_by_path.get(path)
    }

    /// Return the checked facts of one compiler module identity.
    ///
    /// Identity is distinct from a source file path because the test runner may create multiple compiler modules rooted
    /// at the same file.
    pub fn type_info_for_module_path(&self, path: &[String]) -> Option<&TypeCheckInfo> {
        self.type_info_by_module_path.get(path)
    }

    /// Return portable HIR and semantic fact snapshots keyed by source path.
    pub fn semantic_snapshots(&self) -> &BTreeMap<PathBuf, incan_semantics_core::SemanticModuleSnapshot> {
        &self.semantic_snapshots_by_path
    }
}

/// Shared source-analysis context for CLI commands and the LSP.
///
/// The session owns the project-level inputs that affect parsing and checking, so entrypoints never rediscover
/// manifests, Loaves, vocabulary or checked contract metadata.
#[derive(Debug, Clone)]
pub struct CompilationSession {
    /// Vocabulary module map shared across cloned sessions; fresh source and guest state remain local.
    pub(crate) vocab_runtime: Arc<Mutex<Option<vocab_desugar_pass::WasmDesugarerRuntime>>>,
    pub manifest: Option<ProjectManifest>,
    pub source_root: PathBuf,
    /// The compiled Loaves the program imports from: its dependencies and the standard packages.
    loaves: Arc<LoafIndex>,
    /// The standard package the program is, when it is one; its `std.*` roots are its own modules.
    own_standard_package: Option<&'static StandardPackageNamespacePolicy>,
    /// The build profile whose Loaf artifacts the session reads.
    profile: String,
    /// Active root-package features used to project compilation-unit `when feature(...)` declarations.
    pub active_features: BTreeSet<String>,
    /// Declared root-package features used for source diagnostics before projection.
    pub declared_features: BTreeSet<String>,
    pub library_imported_vocab: parser::ImportedLibraryVocab,
    pub library_imported_dsl_surfaces: parser::ImportedLibraryDslSurfaces,
    pub contract_model_bundles: Vec<CanonicalModelBundle>,
}

impl CompilationSession {
    /// Open the session for the program whose entry is `entry_path`, reading the Loaf artifacts of `profile`.
    ///
    /// Every dependency is compiled before its consumer is checked (DD-0005), so a dependency without an artifact
    /// refuses here rather than being checked from source.
    pub fn discover(entry_path: &Path, features: &FeatureSelection, profile: &str) -> CliResult<Self> {
        let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_driver::discover_session", input = %entry_path.display()).entered();
        let inferred_project_root = resolve_project_root(entry_path);
        let manifest = discover_effective_project_manifest(&inferred_project_root)?;
        let project_root = manifest
            .as_ref()
            .map(|manifest| manifest.project_root().to_path_buf())
            .unwrap_or(inferred_project_root);
        let source_root = resolve_source_root(&project_root, manifest.as_ref());
        let graph = manifest
            .as_ref()
            .map(PackageFeatureGraph::from_manifest)
            .transpose()
            .map_err(|error| CliError::failure(error.to_string()))?;
        let resolved = graph
            .as_ref()
            .map(|graph| graph.resolve(features))
            .transpose()
            .map_err(|error| CliError::failure(error.to_string()))?
            .unwrap_or_default();
        let declared_features = graph
            .map(|graph| graph.declared_features().map(str::to_string).collect())
            .unwrap_or_default();
        let own_standard_package = manifest
            .as_ref()
            .and_then(|manifest| manifest.project.as_ref()?.name.as_deref())
            .and_then(standard_package_namespace_policy);
        let loaves = Arc::new(index_loaves(
            manifest.as_ref(),
            &resolved.active_optional_dependencies,
            own_standard_package,
            profile,
        )?);
        let contract_model_bundles = manifest
            .as_ref()
            .map(|manifest| read_project_model_bundles(&project_root, &manifest.contract_model_bundle_paths()))
            .transpose()
            .map_err(|error| CliError::failure(error.to_string()))?
            .unwrap_or_default();
        Ok(Self {
            vocab_runtime: Arc::default(),
            manifest,
            source_root,
            library_imported_vocab: loaves.imported_vocab(),
            library_imported_dsl_surfaces: loaves.imported_dsl_surfaces(),
            loaves,
            own_standard_package,
            profile: profile.to_string(),
            active_features: resolved.active_features,
            declared_features,
            contract_model_bundles,
        })
    }

    /// The compiled Loaves the program imports from.
    pub fn loaves(&self) -> &Arc<LoafIndex> {
        &self.loaves
    }

    /// The standard package the program is, when it is one.
    pub fn own_standard_package(&self) -> Option<&'static StandardPackageNamespacePolicy> {
        self.own_standard_package
    }

    /// The build profile whose Loaf artifacts the session reads.
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// Prepare the CLI check path's Rust metadata context from this session's manifest.
    /// Both diagnostics and native Body IR consumers must use this boundary before analysis so Rust signatures and
    /// derives are checked under the same defaults. The returned lease must remain alive through checking; programs
    /// without metadata queries need no workspace.
    #[cfg(feature = "rust_inspect")]
    pub fn prepare_check_rust_inspection(
        &self,
        entry_path: &Path,
        modules: &[ParsedModule],
    ) -> CliResult<Option<crate::lock::PreparedRustInspectTypecheckWorkspace>> {
        let project_root = resolve_project_root(entry_path);
        let project_name = self
            .manifest
            .as_ref()
            .and_then(|manifest| manifest.project.as_ref().and_then(|project| project.name.clone()))
            .or_else(|| {
                entry_path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(ToString::to_string)
            })
            .unwrap_or_else(|| "incan_check".to_string());
        let cargo_features = oven_model::lock::CargoFeatureSelection::default().normalized();
        crate::lock::rust_inspect::prepare_rust_inspect_typecheck_workspace(crate::lock::RustInspectTypecheckRequest {
            project_root: &project_root,
            project_name: project_name.as_str(),
            manifest: self.manifest.as_ref(),
            modules,
            cargo_features: &cargo_features,
            rust_edition: self
                .manifest
                .as_ref()
                .and_then(|manifest| manifest.rust_edition().map(str::to_string)),
        })
    }

    /// Analyze one collected module graph exactly once: no command re-runs checking to derive its own facts.
    pub fn analyze_modules(
        &self,
        modules: &[ParsedModule],
        #[cfg(feature = "rust_inspect")] rust_inspect_manifest_dir: Option<&Path>,
    ) -> Result<CompilationAnalysis, CliDiagnosticFailure> {
        let type_infos = typecheck_modules(
            modules,
            self.manifest.as_ref(),
            &self.loaves,
            self.own_standard_package,
            None,
            #[cfg(feature = "rust_inspect")]
            rust_inspect_manifest_dir,
        )?
        .type_infos;
        let mut type_info_by_path = BTreeMap::new();
        let mut type_info_by_module_path = BTreeMap::new();
        let mut semantic_snapshots_by_path = BTreeMap::new();
        for (module, type_info) in modules.iter().zip(type_infos) {
            let snapshot = build_semantic_module_snapshot_v0(&module.ast, &module.path_segments, &type_info);
            type_info_by_path.insert(module.file_path.clone(), type_info.clone());
            type_info_by_module_path.insert(module.path_segments.clone(), type_info);
            semantic_snapshots_by_path.insert(module.file_path.clone(), snapshot);
        }
        Ok(CompilationAnalysis {
            type_info_by_path,
            type_info_by_module_path,
            semantic_snapshots_by_path,
        })
    }

    /// The test runner's marker contract, read from the standard package owning `std.testing`; `None` when no such
    /// package is indexed.
    pub fn testing_marker_semantics(&self) -> CliResult<Option<TestingMarkerSemantics>> {
        let module = [stdlib::STDLIB_ROOT.to_string(), "testing".to_string()];
        let Some(entry) = self.loaves.owner_of(&module) else {
            return Ok(None);
        };
        testing_marker_semantics_from_manifest(&entry.artifact.manifest).map_err(|error| CliError::failure(error.to_string()))
    }

    /// Require `std.testing` marker semantics.
    pub fn require_testing_marker_semantics(&self) -> CliResult<TestingMarkerSemantics> {
        self.testing_marker_semantics()?.ok_or_else(|| {
            CliError::failure("std.testing marker syntax requires the compiled standard package owning std.testing")
        })
    }

    /// Return the Rust crate names declared by the project manifest, or an empty set outside a project.
    pub fn declared_crate_names(&self) -> HashSet<String> {
        self.manifest
            .as_ref()
            .map(ProjectManifest::declared_rust_crate_names)
            .unwrap_or_default()
    }

    /// Lex and parse one source file using the project-aware vocabulary surfaces, without running desugarers or
    /// compile-time materialization passes.
    ///
    /// Always parses with the original `source` text available (RFC 081, `#1023`), so a descriptor-gated embedded
    /// fragment can claim eligible positions. The strict lexer runs first; only for source with bytes that are not
    /// ordinary token starts does this retry with the tolerant lexer and hand its lex errors to the parser, which keeps
    /// those inside a claimed fragment and reports the rest.
    pub fn parse_source_for_collection(
        &self,
        file_path: &Path,
        source: &str,
    ) -> Result<Program, Vec<diagnostics::CompileError>> {
        let file_path_display = file_path.to_string_lossy();
        match lexer::lex(source) {
            Ok(tokens) => parser::parse_with_source(
                &tokens,
                Some(file_path_display.as_ref()),
                Some(&self.library_imported_vocab),
                Some(&self.library_imported_dsl_surfaces),
                source,
            ),
            Err(_) => {
                let (tokens, lex_errors) = lexer::lex_tolerant(source);
                parser::parse_with_source_and_lex_errors(
                    &tokens,
                    Some(file_path_display.as_ref()),
                    Some(&self.library_imported_vocab),
                    Some(&self.library_imported_dsl_surfaces),
                    source,
                    lex_errors,
                )
            }
        }
    }

    /// Lex, parse, vocab-desugar, and optionally materialize checked contract models for one source file.
    pub fn parse_source(
        &self,
        file_path: &Path,
        source: &str,
        materialize_models: bool,
    ) -> Result<Program, Vec<diagnostics::CompileError>> {
        let parsed = self.parse_source_unprojected(file_path, source, materialize_models)?;
        self.project_parsed_program(parsed)
    }

    /// Parse and desugar one source file while retaining inactive compile-time feature declarations for tooling.
    pub fn parse_source_unprojected(
        &self,
        file_path: &Path,
        source: &str,
        materialize_models: bool,
    ) -> Result<Program, Vec<diagnostics::CompileError>> {
        let _trace = tracing::trace_span!(target: "incan::invocation", "incan_driver::parse_desugar_source", input = %file_path.display()).entered();
        let mut ast = self.parse_source_for_collection(file_path, source)?;
        let file_path_display = file_path.to_string_lossy();
        {
            let mut runtime = self.vocab_runtime.lock().map_err(|_| {
                vec![diagnostics::CompileError::new(
                    "vocabulary runtime owner is poisoned".to_string(),
                    Span::default(),
                )]
            })?;
            vocab_desugar_pass::desugar_program_vocab_blocks_with_runtime(
                &mut ast,
                Some(file_path_display.as_ref()),
                &self.loaves,
                &mut runtime,
            )?;
        }
        if materialize_models && let Err(error) = materialize_contract_models(&mut ast, &self.contract_model_bundles) {
            return Err(vec![diagnostics::CompileError::new(
                format!("Invalid checked contract metadata: {error}"),
                Span::default(),
            )]);
        }
        Ok(ast)
    }

    /// Validate and project one already-parsed source program through this session's active package features.
    pub fn project_parsed_program(&self, parsed: Program) -> Result<Program, Vec<diagnostics::CompileError>> {
        self.validate_parsed_program_features(&parsed)?;
        Ok(parsed.projected_for_features(&self.active_features))
    }

    /// Validate compile-time feature names while retaining the complete unprojected source program.
    pub fn validate_parsed_program_features(&self, parsed: &Program) -> Result<(), Vec<diagnostics::CompileError>> {
        let mut feature_errors = Vec::new();
        for declaration in &parsed.declarations {
            self.validate_declaration_feature_requirements(declaration, &mut feature_errors);
        }
        if !feature_errors.is_empty() {
            return Err(feature_errors);
        }
        Ok(())
    }

    /// Validate one declaration and any inline-test declarations nested inside it against the package feature graph.
    fn validate_declaration_feature_requirements(
        &self,
        declaration: &incan_frontend::ast::Spanned<incan_frontend::ast::Declaration>,
        errors: &mut Vec<diagnostics::CompileError>,
    ) {
        for feature in &declaration.required_features {
            if !self.declared_features.contains(feature) {
                errors.push(diagnostics::CompileError::new(
                    format!("Unknown package feature `{feature}` in compile-time condition"),
                    declaration.span,
                ));
            }
        }
        if let incan_frontend::ast::Declaration::TestModule(module) = &declaration.node {
            for nested in &module.body {
                self.validate_declaration_feature_requirements(nested, errors);
            }
        }
    }
}

/// Index every Loaf the program reaches: its active dependencies and theirs, read from each dependency's own root,
/// and the standard packages the toolchain has built, other than the program's own.
fn index_loaves(
    manifest: Option<&ProjectManifest>,
    active_optional_dependencies: &BTreeSet<String>,
    own_standard_package: Option<&'static StandardPackageNamespacePolicy>,
    profile: &str,
) -> CliResult<LoafIndex> {
    let mut entries = BTreeMap::<String, LoafEntry>::new();
    if let Some(manifest) = manifest {
        // A direct dependency is keyed as the consumer spells it; a transitive one, which no consumer spells, by its
        // package name.
        let mut pending = active_dependency_roots(manifest, active_optional_dependencies)
            .into_iter()
            .map(|(key, root)| (Some(key), root))
            .collect::<Vec<_>>();
        let mut visited = BTreeSet::new();
        while let Some((key, root)) = pending.pop() {
            if !visited.insert(root.clone()) {
                continue;
            }
            let dependency = ProjectManifest::load(&root.join(LOAF_MANIFEST_FILENAME))
                .map_err(|error| CliError::failure(format!("dependency at {}: {error}", root.display())))?;
            let name = dependency
                .project
                .as_ref()
                .and_then(|project| project.name.clone())
                .ok_or_else(|| CliError::failure(format!("dependency at {} names no package", root.display())))?;
            let key = key.unwrap_or_else(|| name.clone());
            let entry = LoafIndex::dependency(&key, &name, &root, profile).map_err(|error| {
                CliError::failure(format!(
                    "dependency `{key}` has no compiled Loaf for profile `{profile}`: {error}"
                ))
            })?;
            pending.extend(
                active_dependency_roots(&dependency, &BTreeSet::new())
                    .into_iter()
                    .map(|(_, root)| (None, root)),
            );
            entries.insert(key, entry);
        }
    }
    if let Some(std_root) = standard_root()? {
        for policy in STANDARD_PACKAGE_NAMESPACE_POLICIES {
            if own_standard_package.is_some_and(|own| own.package_name == policy.package_name) {
                continue;
            }
            let dir = artifact_dir(&std_root.join(policy.source_directory), profile);
            if !dir.is_dir() {
                continue;
            }
            let entry = LoafIndex::standard(policy.package_name, &std_root, profile)
                .map_err(|error| CliError::failure(error.to_string()))?;
            entries.insert(entry.key.clone(), entry);
        }
    }
    LoafIndex::new(entries.into_values()).map_err(|error| CliError::failure(error.to_string()))
}

/// The active path dependencies a manifest declares: every non-optional one and the optional ones its features
/// activate, each with its root.
fn active_dependency_roots(
    manifest: &ProjectManifest,
    active_optional_dependencies: &BTreeSet<String>,
) -> Vec<(String, PathBuf)> {
    manifest
        .library_dependencies()
        .iter()
        .filter(|(key, spec)| !spec.optional || active_optional_dependencies.contains(*key))
        .map(|(key, spec)| (key.clone(), spec.path.clone()))
        .collect()
}

/// The toolchain's standard-library root: `ROOT/stdlib` installed, `ROOT/loaves/stdlib` in a checkout.
fn standard_root() -> CliResult<Option<PathBuf>> {
    let Some(layout) = CompilerOwnedSourceLayout::discover().map_err(|error| CliError::failure(error.to_string()))?
    else {
        return Ok(None);
    };
    let root = layout
        .verified_source_root()
        .map_err(|error| CliError::failure(error.to_string()))?;
    Ok(Some(root.to_path_buf()))
}
