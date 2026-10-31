//! The compilation session: one per unit, owning the unit's description, the compiled Loaves it imports from, and
//! the analysis every stage after parsing consumes (DD-0007).
//!
//! A session is opened once from the unit description Oven supplies (DD-0008) and reused by every stage, so the
//! Loaves, the vocabulary they bring and the active features are decided once per unit.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use incan_lang::lang::standard_packages::{StandardPackageNamespacePolicy, standard_package_namespace_policy};
use incan_lang::lang::stdlib;

use crate::diagnostics::CliDiagnosticFailure;
use crate::error::{CliError, CliResult};
use crate::typecheck::{TypecheckedModules, typecheck_modules};
use crate::unit::UnitDescription;
use incan_frontend::ast::{Program, Span};
use incan_frontend::contract_metadata::{
    CanonicalModelBundle, materialize_contract_models, read_project_model_bundles,
};
use incan_frontend::hir::build_semantic_module_snapshot_v0;
use incan_frontend::loaf_index::{LIBRARY_MANIFEST_EXTENSION, LoafIndex};
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::testing_markers::{TestingMarkerSemantics, testing_marker_semantics_from_manifest};
use incan_frontend::typechecker::{TypeCheckInfo, TypeChecker};
use incan_frontend::{diagnostics, lexer, parser, vocab_desugar_pass};

/// Checked products produced by one [`CompilationSession`] analysis pass: each module's checked facts, which Body IR
/// lowers from, and its portable semantic snapshot, which tooling reads.
#[derive(Debug, Clone)]
pub struct CompilationAnalysis {
    type_info_by_path: BTreeMap<PathBuf, Arc<TypeCheckInfo>>,
    type_info_by_module_path: BTreeMap<Vec<String>, Arc<TypeCheckInfo>>,
    semantic_snapshots_by_path: BTreeMap<PathBuf, incan_semantics_core::SemanticModuleSnapshot>,
}

impl CompilationAnalysis {
    /// Return the checked facts of one collected source file.
    pub fn type_info_for_path(&self, path: &Path) -> Option<&TypeCheckInfo> {
        self.type_info_by_path.get(path).map(Arc::as_ref)
    }

    /// Return the checked facts of one compiler module identity.
    ///
    /// Identity is distinct from a source file path because the test runner may create multiple compiler modules rooted
    /// at the same file.
    pub fn type_info_for_module_path(&self, path: &[String]) -> Option<&TypeCheckInfo> {
        self.type_info_by_module_path.get(path).map(Arc::as_ref)
    }

    /// Return portable HIR and semantic fact snapshots keyed by source path.
    pub fn semantic_snapshots(&self) -> &BTreeMap<PathBuf, incan_semantics_core::SemanticModuleSnapshot> {
        &self.semantic_snapshots_by_path
    }
}

/// Rust facts: the checker's view of the Rust items a unit imports, keyed by path.
pub type RustFacts = BTreeMap<String, incan_lang::interop::RustItemMetadata>;

/// Where a session reads the Rust facts no ABI cache holds: the compiler's own rustc, over the unit's crates. The
/// caches answer first; a source is asked only for the paths they leave uncovered, so a crate never has two answers.
pub trait RustFactsSource: std::fmt::Debug + Send + Sync {
    /// Facts for `paths`.
    fn facts(&self, paths: &BTreeSet<String>) -> Result<RustFacts, String>;
}

/// Where one session's Rust facts come from: the ABI caches of the crates the unit links, then the source.
#[derive(Debug, Clone, Default)]
pub struct RustFactsSetup {
    /// The ABI caches the session reads first, in order.
    pub caches: Vec<PathBuf>,
    /// Where the facts of paths no cache holds come from, when the caller can read them.
    pub source: Option<Arc<dyn RustFactsSource>>,
}

/// Shared source-analysis context for the pinned driver, the compiler's commands and the language server.
///
/// The session owns the unit-level inputs that affect parsing and checking, so no stage re-derives the Loaves,
/// vocabulary or checked contract metadata.
#[derive(Debug, Clone)]
pub struct CompilationSession {
    /// Where Rust facts no ABI cache holds come from, when the compiler can read them.
    rust_facts_source: Option<Arc<dyn RustFactsSource>>,
    /// Native vocabulary workers and pure responses shared across cloned sessions.
    pub(crate) vocab_runtime: Arc<Mutex<vocab_desugar_pass::NativeDesugarerRuntime>>,
    /// Node-local expansion failures retained while the rest of each module undergoes ordinary checking.
    desugar_diagnostics: Arc<Mutex<BTreeMap<PathBuf, Vec<diagnostics::CompileError>>>>,
    /// The unit as Oven describes it.
    unit: UnitDescription,
    /// The unit's source root, copied from its description.
    pub source_root: PathBuf,
    /// The compiled Loaves the program imports from: its dependencies and the standard packages.
    loaves: Arc<LoafIndex>,
    /// The standard package the program is, when it is one; its `std.*` roots are its own modules.
    own_standard_package: Option<&'static StandardPackageNamespacePolicy>,
    /// Active root-package features used to project compilation-unit `when feature(...)` declarations.
    pub active_features: BTreeSet<String>,
    /// Declared root-package features used for source diagnostics before projection.
    pub declared_features: BTreeSet<String>,
    /// Vocabulary keywords the imported Loaves register, which the parser activates.
    pub library_imported_vocab: parser::ImportedLibraryVocab,
    /// DSL surfaces the imported Loaves register, which the parser activates.
    pub library_imported_dsl_surfaces: parser::ImportedLibraryDslSurfaces,
    /// The project's contract model bundles, read when the session opens.
    pub contract_model_bundles: Vec<CanonicalModelBundle>,
}

impl CompilationSession {
    /// Open the session for one described unit, reading every Loaf it reaches from its artifact directory.
    ///
    /// Every Loaf is compiled before its consumer is checked (DD-0005), so a Loaf without an artifact refuses here.
    pub fn new(unit: &UnitDescription) -> CliResult<Self> {
        Self::open(unit, false)
    }

    /// Open a native program session with admitted inventories and first-use manifest demand. Source analysis,
    /// declaration closure and lowering run anew; no previous source footprint supplies negative lookup facts.
    pub fn for_native_program(unit: &UnitDescription) -> CliResult<Self> {
        Self::open(unit, false)
    }

    /// Open a parsing-only session for formatting without preparing native dependencies or checked model bundles.
    ///
    /// Available Loaf manifests use the ordinary admitted inventories and canonical demands, including every global
    /// vocabulary contributor. Missing manifests supply no vocabulary; syntax that needs them must refuse at parsing
    /// rather than inventing a source-based replacement.
    /// Checking and compilation must use [`Self::new`], which requires every reached artifact.
    pub fn for_formatting(unit: &UnitDescription) -> CliResult<Self> {
        Self::open(unit, true)
    }

    /// Initialize the shared parser context from Loaf artifacts; only formatting may omit absent manifests.
    fn open(unit: &UnitDescription, formatting: bool) -> CliResult<Self> {
        let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_driver::open_session", input = %unit.source.display()).entered();
        let mut deferred = Vec::new();
        for loaf in &unit.loaves {
            let key = if loaf.standard { &loaf.name } else { &loaf.key };
            let manifest = loaf.dir.join(format!("{}.{LIBRARY_MANIFEST_EXTENSION}", loaf.name));
            if formatting
                && !manifest
                    .try_exists()
                    .map_err(|error| CliError::failure(format!("{}: {error}", manifest.display())))?
            {
                continue;
            }
            deferred.push((key.clone(), loaf.name.clone(), loaf.dir.clone(), loaf.standard));
        }
        let loaves = LoafIndex::deferred(deferred).map_err(|error| CliError::failure(error.to_string()))?;
        let loaves = Arc::new(loaves);
        // The implicit prelude also owns the linked string-conversion and entry-initialization contracts.
        let _ = loaves.owner_of(&[stdlib::STDLIB_ROOT.to_owned(), "prelude".to_owned()]);
        let library_imported_vocab = loaves.imported_vocab();
        let library_imported_dsl_surfaces = loaves.imported_dsl_surfaces();
        loaves
            .validate_demands()
            .map_err(|error| CliError::failure(error.to_string()))?;
        let contract_model_bundles = if formatting {
            Vec::new()
        } else {
            read_project_model_bundles(&unit.project_root, &unit.contract_models)
                .map_err(|error| CliError::failure(error.to_string()))?
        };
        Ok(Self {
            rust_facts_source: None,
            vocab_runtime: Arc::default(),
            desugar_diagnostics: Arc::default(),
            source_root: unit.source_root.clone(),
            library_imported_vocab,
            library_imported_dsl_surfaces,
            loaves,
            own_standard_package: standard_package_namespace_policy(&unit.package),
            active_features: unit.active_features.clone(),
            declared_features: unit.declared_features.clone(),
            contract_model_bundles,
            unit: unit.clone(),
        })
    }

    /// The unit as Oven describes it.
    pub fn unit(&self) -> &UnitDescription {
        &self.unit
    }

    /// The compiled Loaves the program imports from.
    pub fn loaves(&self) -> &Arc<LoafIndex> {
        &self.loaves
    }

    /// Read the Rust facts of the unit's crates from their ABI caches and ask the setup's source for any path they
    /// leave uncovered.
    pub fn with_rust_facts(mut self, setup: &RustFactsSetup) -> CliResult<Self> {
        for cache in &setup.caches {
            let decoded =
                LoafIndex::read_rust_abi_cache(cache).map_err(|error| CliError::failure(error.to_string()))?;
            Arc::make_mut(&mut self.loaves).add_rust_abi_cache(decoded);
        }
        Arc::make_mut(&mut self.loaves).declare_rust_crates(self.unit.rust_crates.iter().cloned());
        self.rust_facts_source = setup.source.clone();
        Ok(self)
    }

    /// Before checking: the facts of the Rust paths `modules` import that neither a Loaf nor an ABI cache holds, from
    /// the session's source. A path the source describes nothing at is absent from its crate, and its import is
    /// refused. Without a source, an uncovered path keeps no facts and its uses refuse where checked. Manifest
    /// inventory determines coverage before the caches and source are consulted.
    pub fn resolve_rust_facts(&mut self, modules: &[ParsedModule]) -> CliResult<()> {
        let Some(source) = self.rust_facts_source.clone() else {
            return Ok(());
        };
        let uncovered = imported_rust_paths(modules)
            .into_iter()
            .filter(|path| self.loaves.rust_abi_item(path).is_none())
            .collect::<BTreeSet<_>>();
        self.loaves
            .validate_demands()
            .map_err(|error| CliError::failure(error.to_string()))?;
        if uncovered.is_empty() {
            return Ok(());
        }
        let facts = source.facts(&uncovered).map_err(CliError::failure)?;
        Arc::make_mut(&mut self.loaves).add_rust_facts(&uncovered, facts);
        Ok(())
    }

    /// The standard package the program is, when it is one.
    pub fn own_standard_package(&self) -> Option<&'static StandardPackageNamespacePolicy> {
        self.own_standard_package
    }

    /// Analyze one collected module graph exactly once: no command re-runs checking to derive its own facts.
    pub fn analyze_modules(&self, modules: &[ParsedModule]) -> Result<CompilationAnalysis, CliDiagnosticFailure> {
        self.analyze_unit(modules, None, &mut |_, _| {})
    }

    /// Analyze one unit's module graph exactly once. A library unit passes its `package`, the package identity of every
    /// declaration it checks, and `visit` sees each module's checker so its publication reads the same pass.
    pub fn analyze_unit(
        &self,
        modules: &[ParsedModule],
        package: Option<&str>,
        visit: &mut dyn FnMut(&ParsedModule, &TypeChecker),
    ) -> Result<CompilationAnalysis, CliDiagnosticFailure> {
        let type_infos = self.typecheck_unit(modules, package, visit)?.type_infos;
        let mut type_info_by_path = BTreeMap::new();
        let mut type_info_by_module_path = BTreeMap::new();
        let mut semantic_snapshots_by_path = BTreeMap::new();
        for (module, type_info) in modules.iter().zip(type_infos) {
            let snapshot = build_semantic_module_snapshot_v0(&module.ast, &module.path_segments, &type_info);
            let type_info = Arc::new(type_info);
            type_info_by_path.insert(module.file_path.clone(), Arc::clone(&type_info));
            type_info_by_module_path.insert(module.path_segments.clone(), type_info);
            semantic_snapshots_by_path.insert(module.file_path.clone(), snapshot);
        }
        Ok(CompilationAnalysis {
            type_info_by_path,
            type_info_by_module_path,
            semantic_snapshots_by_path,
        })
    }

    /// Check the unit once and combine retained expansion errors with ordinary diagnostics and warnings.
    /// Both check reports and native compilation use this boundary, so delaying a node-local expansion failure never
    /// makes it disappear from one command or prevents independent statements from being checked.
    pub(crate) fn typecheck_unit(
        &self,
        modules: &[ParsedModule],
        package: Option<&str>,
        visit: &mut dyn FnMut(&ParsedModule, &TypeChecker),
    ) -> Result<TypecheckedModules, CliDiagnosticFailure> {
        let checked = typecheck_modules(
            modules,
            &self.unit,
            &self.loaves,
            self.own_standard_package,
            package,
            None,
            visit,
        );
        let mut expansion_errors = self.expansion_failures(modules);
        match checked {
            Ok(checked) if expansion_errors.diagnostics.is_empty() => Ok(checked),
            Ok(checked) => {
                expansion_errors.warnings = checked.warnings;
                Err(expansion_errors)
            }
            Err(mut failure) => {
                failure.diagnostics.append(&mut expansion_errors.diagnostics);
                Err(failure)
            }
        }
    }

    /// Project retained expansion failures with each module's original source after typechecking has continued.
    fn expansion_failures(&self, modules: &[ParsedModule]) -> CliDiagnosticFailure {
        let mut failure = CliDiagnosticFailure {
            diagnostics: Vec::new(),
            warnings: Vec::new(),
        };
        let retained = match self.desugar_diagnostics.lock() {
            Ok(retained) => retained,
            Err(_) => {
                return CliDiagnosticFailure::single(
                    self.unit.source.to_string_lossy(),
                    "",
                    diagnostics::CompileError::new("vocabulary diagnostic owner is poisoned".into(), Span::default()),
                    diagnostics::DiagnosticPhase::Parse,
                );
            }
        };
        for module in modules {
            if let Some(errors) = retained.get(&module.file_path) {
                let mut projected = CliDiagnosticFailure::from_errors(
                    module.file_path.to_string_lossy(),
                    &module.source,
                    errors.clone(),
                    diagnostics::DiagnosticPhase::Parse,
                );
                failure.diagnostics.append(&mut projected.diagnostics);
            }
        }
        failure
    }

    /// The test runner's marker contract, read from the standard package owning `std.testing`; `None` when no such
    /// package is indexed.
    pub fn testing_marker_semantics(&self) -> CliResult<Option<TestingMarkerSemantics>> {
        let module = [stdlib::STDLIB_ROOT.to_string(), "testing".to_string()];
        let Some(entry) = self.loaves.owner_of(&module) else {
            return Ok(None);
        };
        testing_marker_semantics_from_manifest(&entry.artifact.manifest)
            .map_err(|error| CliError::failure(error.to_string()))
    }

    /// Require `std.testing` marker semantics.
    pub fn require_testing_marker_semantics(&self) -> CliResult<TestingMarkerSemantics> {
        self.testing_marker_semantics()?.ok_or_else(|| {
            CliError::failure("std.testing marker syntax requires the compiled standard package owning std.testing")
        })
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
        self.parse_source_in_context(Some(file_path_display.as_ref()), source)
    }

    /// Parse syntax for formatting with the unit's vocabulary, without enforcing source-file placement rules.
    ///
    /// Formatting a standalone module must preserve public reexports even before that module is placed under `src/`.
    /// Compilation and collection retain their actual module path and its context-sensitive diagnostics.
    pub fn parse_source_for_formatting(&self, source: &str) -> Result<Program, Vec<diagnostics::CompileError>> {
        self.parse_source_in_context(None, source)
    }

    /// Share vocabulary activation and tolerant fragment lexing across contextual parsing and syntax-only formatting.
    fn parse_source_in_context(
        &self,
        module_path: Option<&str>,
        source: &str,
    ) -> Result<Program, Vec<diagnostics::CompileError>> {
        match lexer::lex(source) {
            Ok(tokens) => parser::parse_with_source(
                &tokens,
                module_path,
                Some(&self.library_imported_vocab),
                Some(&self.library_imported_dsl_surfaces),
                source,
            ),
            Err(_) => {
                let (tokens, lex_errors) = lexer::lex_tolerant(source);
                parser::parse_with_source_and_lex_errors(
                    &tokens,
                    module_path,
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
        if self.unit.vocabulary {
            crate::vocabulary::validate_source(&ast, &self.unit, file_path)?;
        }
        let file_path_display = file_path.to_string_lossy();
        {
            let mut runtime = self.vocab_runtime.lock().map_err(|_| {
                vec![diagnostics::CompileError::new(
                    "vocabulary runtime owner is poisoned".to_string(),
                    Span::default(),
                )]
            })?;
            runtime.configure(
                self.unit.vocab_admissions.clone(),
                self.unit.project_root.clone(),
                self.unit.vocab_policy.clone(),
            );
            let expansion = vocab_desugar_pass::desugar_program_vocab_blocks_with_runtime(
                &mut ast,
                Some(file_path_display.as_ref()),
                &self.loaves,
                &mut runtime,
            );
            let mut retained = self.desugar_diagnostics.lock().map_err(|_| {
                vec![diagnostics::CompileError::new(
                    "vocabulary diagnostic owner is poisoned".into(),
                    Span::default(),
                )]
            })?;
            retained.insert(file_path.to_path_buf(), expansion.err().unwrap_or_default());
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

/// The Rust items a `rust.module` facade names: potential derive exports named by its traits, and the items its
/// `@rust.extern` functions delegate to. Missing exports supply no derive permission; checking still requires an
/// inspected derive macro item. An extern function's item is inspected to learn whether it is a callable function or
/// an attribute macro that the declaration only names.
fn facade_rust_paths(
    rust_module: &str,
    declarations: &[incan_frontend::ast::Spanned<incan_frontend::ast::Declaration>],
    paths: &mut BTreeSet<String>,
) {
    use incan_frontend::ast::Declaration;
    for declaration in declarations {
        match &declaration.node {
            Declaration::Trait(trait_) => {
                paths.insert(format!("{rust_module}::{}", trait_.name));
            }
            Declaration::Function(function)
                if function
                    .decorators
                    .iter()
                    .any(|decorator| decorator.node.path.segments == ["rust", "extern"]) =>
            {
                paths.insert(format!("{rust_module}::{}", function.name));
            }
            _ => {}
        }
    }
}

/// The Rust paths a unit imports, including those its `rust.module` facades name.
fn imported_rust_paths(modules: &[ParsedModule]) -> BTreeSet<String> {
    use incan_frontend::ast::{Declaration, DecoratorArg, Expr, ImportKind, Literal};
    let mut paths = BTreeSet::new();
    for module in modules {
        if let Some(rust_module) = &module.ast.rust_module_path {
            facade_rust_paths(&rust_module.node, &module.ast.declarations, &mut paths);
        }
        for declaration in &module.ast.declarations {
            let decorators = match &declaration.node {
                Declaration::Model(value) => value.decorators.as_slice(),
                Declaration::Class(value) => value.decorators.as_slice(),
                Declaration::Newtype(value) => value.decorators.as_slice(),
                Declaration::Enum(value) => value.decorators.as_slice(),
                Declaration::Trait(value) => value.decorators.as_slice(),
                _ => &[],
            };
            for decorator in decorators {
                if decorator.node.path.segments != ["rust", "derive"] {
                    continue;
                }
                for argument in &decorator.node.args {
                    if let DecoratorArg::Positional(expression) = argument
                        && let Expr::Literal(Literal::String(path)) = &expression.node
                        && path.contains("::")
                    {
                        paths.insert(path.clone());
                    }
                }
            }
            let Declaration::Import(import) = &declaration.node else {
                continue;
            };
            match &import.kind {
                ImportKind::RustCrate { crate_name, path, .. } => {
                    let segments = std::iter::once(crate_name.clone()).chain(path.iter().cloned());
                    paths.insert(segments.collect::<Vec<_>>().join("::"));
                }
                ImportKind::RustFrom {
                    crate_name,
                    path,
                    items,
                    ..
                } => {
                    for item in items {
                        let segments = std::iter::once(crate_name.clone())
                            .chain(path.iter().cloned())
                            .chain(std::iter::once(item.name.clone()));
                        paths.insert(segments.collect::<Vec<_>>().join("::"));
                    }
                }
                _ => {}
            }
        }
    }
    paths
}
