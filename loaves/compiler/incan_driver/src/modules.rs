//! Module collection: from an entrypoint to the ordered set of parsed modules a command compiles, with the import
//! graph that fixes their order.
//!
//! Collection is source-first: it reads the project's own files, consults the SDK catalog only to keep migrated
//! standard-library sources out of consumer graphs, and sorts dependencies before dependents so typechecking sees
//! imported types resolved.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::{env, fs};

use incan_lang::lang::stdlib;
use incan_lang::lang::surface::result_methods;

use crate::diagnostics::{CliDiagnosticFailure, render_module_warnings};
use crate::error::{CliError, CliResult};
use crate::project::{collect_incan_source_files, read_source_for_diagnostics, resolve_stdlib_module_source_path};
use crate::session::CompilationSession;
use incan_frontend::ast::{ImportKind, ImportPath, Program, Span};
use incan_frontend::module::{
    SourceModuleImportResolution, canonicalize_source_module_segments, logical_module_segments_from_file,
    logical_source_import_candidates, resolve_program_source_imports, self_import_diagnostic_message,
};
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::{ast_walk, diagnostics, typechecker};
use incan_provider::dependency_resolver::{DependencyError, InlineRustImport};
use incan_provider::inventory::sdk_provider_bootstrap_namespace_roots;
use incan_provider::{FeatureSelection, ProviderModuleResolution, ProviderPlan, SDK_PROVIDER_BUILD_ENV};
use oven_model::manifest::ProjectManifest;
/// Return whether a parsed module uses RFC 088 iterator surface methods that require stdlib adapter modules.
pub fn uses_iterator_adapter_surface(program: &Program) -> bool {
    ast_walk::any_expr_in_program(program, |expr| match expr {
        incan_frontend::ast::Expr::MethodCall(_, method, _, _) => matches!(
            method.as_str(),
            "iter"
                | "map"
                | "filter"
                | "enumerate"
                | "zip"
                | "take"
                | "skip"
                | "take_while"
                | "skip_while"
                | "chain"
                | "flat_map"
                | "batch"
                | "collect"
                | "count"
                | "reduce"
                | "fold"
                | "any"
                | "all"
                | "find"
                | "for_each"
                | "sum"
        ),
        _ => false,
    })
}

/// Return whether a parsed module uses RFC 070 Result combinators backed by std.result helpers.
pub fn uses_result_combinator_surface(program: &Program) -> bool {
    ast_walk::any_expr_in_program(program, |expr| match expr {
        incan_frontend::ast::Expr::MethodCall(_, method, _, _) => result_methods::from_str(method).is_some(),
        _ => false,
    })
}

/// Collect and parse the entry file and all its dependencies.
///
/// # Note on Prelude
///
/// The stdlib root prelude (`stdlib/prelude.incn`) exists, but it is not auto-imported into every compilation unit.
/// Unmigrated source-backed stdlib trait modules and builtin fallback traits are still discovered explicitly when the
/// parsed AST needs them. Migrated modules are resolved through the compiled built-in artifact instead.
pub fn collect_modules(entry_path: &str) -> CliResult<Vec<ParsedModule>> {
    collect_modules_detailed(entry_path).map_err(|failure| CliError::failure(failure.render_human()))
}

/// Return whether the SDK catalog claims a module that source collection must never materialize locally.
///
/// Disabled and unavailable providers still own their namespace claims. Their imports must reach provider-aware
/// diagnostics instead of silently loading a nearby stdlib checkout and producing cascaded errors from the wrong
/// source graph.
fn sdk_catalog_claims_module_for_collection(provider_plan: &ProviderPlan, module_path: &[String]) -> bool {
    !matches!(
        provider_plan.resolve_module(module_path),
        ProviderModuleResolution::Unknown
    )
}

/// Collect and parse the entry file and all its dependencies, preserving structured diagnostic context.
pub fn collect_modules_detailed(entry_path: &str) -> Result<Vec<ParsedModule>, CliDiagnosticFailure> {
    collect_modules_detailed_with_feature_selection(entry_path, &FeatureSelection::default())
}

/// Collect and parse the entry file and all dependencies for one explicit Incan package-feature projection.
pub fn collect_modules_detailed_with_feature_selection(
    entry_path: &str,
    feature_selection: &FeatureSelection,
) -> Result<Vec<ParsedModule>, CliDiagnosticFailure> {
    collect_modules_detailed_with_selections(entry_path, feature_selection, None)
}

/// Collect and parse the entry file and all dependencies for one package-feature and SDK-profile projection.
pub fn collect_modules_detailed_with_selections(
    entry_path: &str,
    feature_selection: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> Result<Vec<ParsedModule>, CliDiagnosticFailure> {
    let path = if Path::new(entry_path).is_absolute() {
        PathBuf::from(entry_path)
    } else {
        std::env::current_dir()
            .map_err(|error| {
                CliDiagnosticFailure::single(
                    entry_path,
                    "",
                    diagnostics::CompileError::new(
                        format!("failed to determine current directory: {error}"),
                        Span::default(),
                    ),
                    diagnostics::DiagnosticPhase::Tooling,
                )
            })?
            .join(entry_path)
    };
    let session = match sdk_profile_override {
        Some(profile) => CompilationSession::discover_with_selections(&path, feature_selection, Some(profile)),
        None => CompilationSession::discover_with_feature_selection(&path, feature_selection),
    }
    .map_err(|error| {
        CliDiagnosticFailure::single(
            path.to_string_lossy(),
            "",
            diagnostics::CompileError::new(error.message, Span::default()),
            diagnostics::DiagnosticPhase::Import,
        )
    })?;
    collect_modules_detailed_with_session(path, &session)
}

/// Collect one source graph through an already-resolved compilation session.
pub fn collect_modules_detailed_with_session(
    path: PathBuf,
    session: &CompilationSession,
) -> Result<Vec<ParsedModule>, CliDiagnosticFailure> {
    collect_modules_detailed_with_session_at_path(path, session, vec!["main".to_string()])
}

/// Collect one source graph while preserving the caller-proven logical identity of its entry module.
///
/// Directory-wide tooling may analyze every source file as a temporary graph root. Such a file still owns its
/// source-root-relative module identity; treating each temporary root as `main` would mint different canonical
/// identities depending on traversal order.
pub fn collect_modules_detailed_with_session_at_path(
    path: PathBuf,
    session: &CompilationSession,
    path_segments: Vec<String>,
) -> Result<Vec<ParsedModule>, CliDiagnosticFailure> {
    let module_name = path_segments.join("_");
    collect_modules_detailed_from_seeds(
        path.clone(),
        session,
        vec![(path.to_string_lossy().to_string(), module_name, path_segments)],
    )
}

/// Collect every authored source module for library publication, including modules not imported by `src/lib.incn`.
pub fn collect_library_modules_detailed_with_session(
    path: PathBuf,
    session: &CompilationSession,
) -> Result<Vec<ParsedModule>, CliDiagnosticFailure> {
    let seeds = library_source_seeds(&path, session).map_err(CliDiagnosticFailure::from)?;
    let entry_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let seeds = seeds
        .into_iter()
        .map(|(source_file, module_name, path_segments)| {
            (source_file.to_string_lossy().to_string(), module_name, path_segments)
        })
        .collect();
    let mut modules = collect_modules_detailed_from_seeds(path.clone(), session, seeds)?;
    if let Some(entry_index) = modules.iter().position(|module| {
        module
            .file_path
            .canonicalize()
            .unwrap_or_else(|_| module.file_path.clone())
            == entry_path
    }) {
        let entry = modules.remove(entry_index);
        modules.push(entry);
    }
    Ok(modules)
}

/// Build the single validated source set used by checked and unprojected library publication.
pub fn library_source_seeds(
    path: &Path,
    session: &CompilationSession,
) -> CliResult<Vec<(PathBuf, String, Vec<String>)>> {
    let mut source_files = Vec::new();
    collect_incan_source_files(&session.source_root, &mut source_files).map_err(|error| {
        CliError::failure(format!(
            "failed to discover library source modules under {}: {error}",
            session.source_root.display()
        ))
    })?;
    source_files.sort();

    let entry_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let sdk_namespace_roots = if env::var_os(SDK_PROVIDER_BUILD_ENV).is_some() {
        let project_root = session
            .manifest
            .as_ref()
            .map(ProjectManifest::project_root)
            .unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")));
        Some(sdk_provider_bootstrap_namespace_roots(project_root)?)
    } else {
        None
    };
    let mut logical_sources = BTreeMap::<Vec<String>, PathBuf>::new();
    let mut seeds = vec![(path.to_path_buf(), "main".to_string(), vec!["main".to_string()])];
    for source_file in source_files {
        let canonical = source_file.canonicalize().unwrap_or_else(|_| source_file.clone());
        if canonical == entry_path {
            continue;
        }
        let Some(path_segments) = logical_module_segments_from_file(&session.source_root, &source_file) else {
            continue;
        };
        if is_unselected_package_entrypoint(&session.source_root, &source_file, &entry_path) {
            continue;
        }
        if sdk_namespace_roots
            .as_ref()
            .is_some_and(|roots| path_segments.first().is_none_or(|root| !roots.contains(root)))
        {
            continue;
        }
        if let Some(existing) = logical_sources.insert(path_segments.clone(), canonical.clone()) {
            return Err(CliError::failure(format!(
                "{} and {} both resolve to library module `{}`; use either a module file or a directory entrypoint",
                existing.display(),
                canonical.display(),
                path_segments.join(".")
            )));
        }
        seeds.push((canonical, path_segments.join("_"), path_segments));
    }
    Ok(seeds)
}

/// Return whether a root `main.incn` or `lib.incn` is the package entrypoint not selected by this build.
fn is_unselected_package_entrypoint(source_root: &Path, source_file: &Path, selected_entrypoint: &Path) -> bool {
    let canonical_source_root = source_root.canonicalize().unwrap_or_else(|_| source_root.to_path_buf());
    let canonical_source_file = source_file.canonicalize().unwrap_or_else(|_| source_file.to_path_buf());
    if canonical_source_file == selected_entrypoint || canonical_source_file.parent() != Some(&canonical_source_root) {
        return false;
    }
    canonical_source_file
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| matches!(stem, "lib" | "main"))
}

/// Key one source file by identity rather than by the spelling used to reach it.
///
/// Module collection dedupes visited files, records dependency edges, and correlates the topological sort by a
/// string key. That key has to name the file, not the path a caller happened to spell: the resolver canonicalizes
/// every local import it locates (`resolve_module_path_from_base`), so a module reached through an import always
/// arrives under its canonical path, while a seed arrives under whatever the caller wrote. When the two disagree, as
/// they do for an entry spelled through a symlink (macOS keeps its temporary directory behind one) whose import cycle
/// reaches back into it, the entry is collected twice and the two parsed modules carry one module identity, which the
/// replacement execution graph refuses as a duplicate rather than dispatching by assembly order (#1557).
///
/// A path that cannot be canonicalized keeps its spelling, which is the only identity it has; the file is read next
/// and reports the real failure. The key is only ever used for de-duplication and ordering: a module's
/// [`ParsedModule::file_path`] keeps the spelling it was reached by, so callers that located the entry themselves can
/// still find it by the path they passed in.
///
/// Canonicalizing on every platform also covers the spellings Windows offers for one file (`\\?\` extended-length
/// prefix, either separator, case-insensitive components), which is the #1357 shape the Windows line applies only
/// there. Every caller of [`topologically_sort_modules`] must key its dependency edges through this same function;
/// the test runner's collector in `testing::module_graph` is the other one.
pub(crate) fn source_identity_key(path: &str) -> String {
    fs::canonicalize(path)
        .map(|canonical| canonical.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string())
}

/// Collect one source graph from explicit seed modules through an already-resolved compilation session.
///
/// Seeds are `(file path, module name, module segments)` triples. Each file is parsed once, its imports are resolved
/// into further seeds, and the resulting edges drive a topological sort so a module is always parsed before the
/// modules that depend on it.
///
/// Visited files and dependency edges are keyed through [`source_identity_key`] rather than by the path string the
/// file happened to be reached by. Both sides of that keying must agree with the key
/// [`topologically_sort_modules`] builds its module map from: if they diverge, edges silently fail to match a module
/// and the sort quietly loses ordering constraints rather than reporting a problem.
fn collect_modules_detailed_from_seeds(
    path: PathBuf,
    session: &CompilationSession,
    mut to_process: Vec<(String, String, Vec<String>)>,
) -> Result<Vec<ParsedModule>, CliDiagnosticFailure> {
    let base_dir = path.parent().unwrap_or(Path::new("."));
    let mut modules = Vec::new();
    let mut processed = HashSet::new();
    let mut dependency_edges: HashMap<String, HashSet<String>> = HashMap::new();
    let mut incan_source_stdlib_module_paths: HashMap<String, PathBuf> = HashMap::new();
    let compiling_sdk_provider = env::var_os(SDK_PROVIDER_BUILD_ENV).is_some();
    let stdlib_module_segments = |module_path: &[String]| {
        if compiling_sdk_provider {
            module_path.iter().skip(1).cloned().collect()
        } else {
            let mut segments = vec![stdlib::INCAN_STD_NAMESPACE.to_string()];
            segments.extend(module_path.iter().skip(1).cloned());
            segments
        }
    };
    while let Some((file_path, module_name, path_segments)) = to_process.pop() {
        let file_key = source_identity_key(&file_path);
        if processed.contains(&file_key) {
            continue;
        }
        processed.insert(file_key.clone());
        dependency_edges.entry(file_key.clone()).or_default();

        let source = read_source_for_diagnostics(&file_path)?;
        let file_path_obj = Path::new(&file_path);
        let is_incan_source_stdlib_module = path_segments
            .first()
            .is_some_and(|segment| segment == stdlib::INCAN_STD_NAMESPACE);
        let ast = match session.parse_source(file_path_obj, &source, !is_incan_source_stdlib_module) {
            Ok(a) => {
                // Surface any non-fatal parser warnings (e.g. RFC 005 dot-notation nudges) immediately,
                // so they reach the user regardless of which build/run/debug command was invoked. They are
                // collected for machine-readable reports later, off the retained AST, by the typecheck pass.
                render_module_warnings(&file_path, &source, &a.warnings);
                a
            }
            Err(errs) => {
                return Err(CliDiagnosticFailure::from_errors(
                    file_path,
                    source,
                    errs,
                    diagnostics::DiagnosticPhase::Parse,
                ));
            }
        };

        let current_base = file_path_obj.parent().unwrap_or(base_dir);
        if uses_iterator_adapter_surface(&ast) {
            let module_path = vec![
                stdlib::STDLIB_ROOT.to_string(),
                "derives".to_string(),
                "collection".to_string(),
            ];
            if !sdk_catalog_claims_module_for_collection(&session.provider_plan, &module_path) {
                let source_path = resolve_stdlib_module_source_path(&module_path)?;
                let module_segments = stdlib_module_segments(&module_path);
                let module_name = module_segments.join("_");
                let dep_path_str = source_path.to_string_lossy().to_string();
                let dep_key = source_identity_key(&dep_path_str);
                if !processed.contains(&dep_key) {
                    to_process.push((dep_path_str, module_name, module_segments));
                }
                dependency_edges.entry(file_key.clone()).or_default().insert(dep_key);
            }
        }
        if uses_result_combinator_surface(&ast) {
            let module_path = vec![stdlib::STDLIB_ROOT.to_string(), "result".to_string()];
            if !sdk_catalog_claims_module_for_collection(&session.provider_plan, &module_path) {
                let source_path = resolve_stdlib_module_source_path(&module_path)?;
                let module_segments = stdlib_module_segments(&module_path);
                let module_name = module_segments.join("_");
                let dep_path_str = source_path.to_string_lossy().to_string();
                let dep_key = source_identity_key(&dep_path_str);
                if !processed.contains(&dep_key) {
                    to_process.push((dep_path_str, module_name, module_segments));
                }
                dependency_edges.entry(file_key.clone()).or_default().insert(dep_key);
            }
        }
        for resolved in resolve_program_source_imports(&ast, current_base, Some(&session.source_root)) {
            let span = resolved.span;
            match resolved.resolution {
                SourceModuleImportResolution::Stdlib { module_path } => {
                    if stdlib::stdlib_stub_path(&module_path).is_none() {
                        continue;
                    }
                    if sdk_catalog_claims_module_for_collection(&session.provider_plan, &module_path) {
                        continue;
                    }
                    let stdlib_key = module_path.join(".");
                    let source_path = if let Some(cached_path) = incan_source_stdlib_module_paths.get(&stdlib_key) {
                        cached_path.clone()
                    } else {
                        let resolved = resolve_stdlib_module_source_path(&module_path)?;
                        incan_source_stdlib_module_paths.insert(stdlib_key, resolved.clone());
                        resolved
                    };

                    let module_segments = stdlib_module_segments(&module_path);
                    let module_name = module_segments.join("_");
                    let dep_path_str = source_path.to_string_lossy().to_string();
                    let dep_key = source_identity_key(&dep_path_str);
                    if !processed.contains(&dep_key) {
                        to_process.push((dep_path_str, module_name, module_segments));
                    }
                    dependency_edges.entry(file_key.clone()).or_default().insert(dep_key);
                }
                SourceModuleImportResolution::Local(module_ref) => {
                    let dep_path_str = module_ref.file_path.to_string_lossy().to_string();
                    let dep_key = source_identity_key(&dep_path_str);
                    let module_segments = canonicalize_source_module_segments(&module_ref.path_segments);
                    let module_name = module_segments.join("_");
                    if !processed.contains(&dep_key) {
                        to_process.push((dep_path_str, module_name, module_segments));
                    }
                    dependency_edges.entry(file_key.clone()).or_default().insert(dep_key);
                }
                SourceModuleImportResolution::SelfImport {
                    module_ref,
                    import_path,
                    can_use_root_import,
                } => {
                    return Err(CliDiagnosticFailure::single(
                        file_path,
                        source,
                        diagnostics::CompileError::new(
                            self_import_diagnostic_message(&module_ref, &import_path, can_use_root_import),
                            span,
                        ),
                        diagnostics::DiagnosticPhase::Typecheck,
                    ));
                }
                SourceModuleImportResolution::External => {}
            }
        }

        modules.push(ParsedModule {
            name: module_name,
            path_segments,
            file_path: PathBuf::from(&file_path),
            source,
            ast,
        });
    }

    Ok(topologically_sort_modules(modules, &dependency_edges)?)
}

/// Return modules in stable topological order (dependencies first).
///
/// Discovery traversal uses a stack, which is not guaranteed to produce dependency-safe ordering for siblings.
/// This explicit sort guarantees each module appears only after its direct and transitive dependencies for acyclic
/// portions of the graph. For cyclic components (for example stdlib prelude re-export loops), we keep deterministic
/// fallback ordering rather than hard-failing in collection.
///
/// `dependency_edges` is keyed by [`source_identity_key`] on both sides, and the module map built here uses the same
/// key, so an edge recorded under a file's canonical path still finds the module that was reached by another
/// spelling.
pub fn topologically_sort_modules(
    modules: Vec<ParsedModule>,
    dependency_edges: &HashMap<String, HashSet<String>>,
) -> CliResult<Vec<ParsedModule>> {
    if modules.is_empty() {
        return Ok(modules);
    }

    let mut module_by_path: HashMap<String, ParsedModule> = HashMap::new();
    let mut order_index: HashMap<String, usize> = HashMap::new();
    for (idx, module) in modules.into_iter().enumerate() {
        let key = source_identity_key(&module.file_path.to_string_lossy());
        order_index.insert(key.clone(), idx);
        module_by_path.insert(key, module);
    }

    let mut indegree: HashMap<String, usize> = module_by_path.keys().cloned().map(|key| (key, 0usize)).collect();
    let mut reverse_adj: HashMap<String, Vec<String>> = HashMap::new();

    for (module_path, deps) in dependency_edges {
        if !module_by_path.contains_key(module_path) {
            continue;
        }
        for dep in deps {
            if !module_by_path.contains_key(dep) {
                continue;
            }
            if let Some(value) = indegree.get_mut(module_path) {
                *value += 1;
            }
            reverse_adj.entry(dep.clone()).or_default().push(module_path.clone());
        }
    }

    let mut ready: BTreeSet<(usize, String)> = indegree
        .iter()
        .filter_map(|(path, &degree)| {
            (degree == 0).then_some((order_index.get(path).copied().unwrap_or(usize::MAX), path.clone()))
        })
        .collect();

    let mut sorted = Vec::new();
    while let Some((_, next)) = ready.pop_first() {
        let Some(module) = module_by_path.remove(&next) else {
            continue;
        };
        sorted.push(module);

        if let Some(dependents) = reverse_adj.get(&next) {
            for dependent in dependents {
                if let Some(value) = indegree.get_mut(dependent)
                    && *value > 0
                {
                    *value -= 1;
                    if *value == 0 {
                        ready.insert((
                            order_index.get(dependent).copied().unwrap_or(usize::MAX),
                            dependent.clone(),
                        ));
                    }
                }
            }
        }
    }

    if !module_by_path.is_empty() {
        // Kahn's algorithm leaves cycle members (and dependents blocked by them) unresolved.
        // Preserve deterministic behavior by appending unresolved modules in reverse discovery order, which matches the
        // previous `modules.reverse()` shape that existing stdlib integration tests rely on.
        let mut unresolved: Vec<(usize, ParsedModule)> = module_by_path
            .into_iter()
            .map(|(path, module)| (order_index.get(&path).copied().unwrap_or(usize::MAX), module))
            .collect();
        unresolved.sort_by_key(|(idx, _)| std::cmp::Reverse(*idx));
        sorted.extend(unresolved.into_iter().map(|(_, module)| module));
    }

    Ok(sorted)
}

/// Format a Rust import base path like `rust::serde_json` or `rust::chrono::naive::date`.
pub fn format_rust_import_base_path(crate_name: &str, path: &[String]) -> String {
    if path.is_empty() {
        format!("rust::{}", crate_name)
    } else {
        format!("rust::{}::{}", crate_name, path.join("::"))
    }
}

/// Format a Rust from-import path like `from rust::serde_json import from_str, to_string`.
pub fn format_rust_from_import_path(crate_name: &str, path: &[String], imported: &[String]) -> String {
    format!(
        "from {} import {}",
        format_rust_import_base_path(crate_name, path),
        imported.join(", ")
    )
}

/// Build an inline Rust import record for dependency resolution.
pub fn build_inline_rust_import(
    crate_name: &str,
    import_path: String,
    version: &Option<String>,
    features: &[String],
    span: Span,
    file_path: &Path,
    is_test_context: bool,
) -> InlineRustImport {
    InlineRustImport {
        crate_name: crate_name.to_string(),
        import_path,
        version: version.clone(),
        features: features.to_vec(),
        span,
        file_path: file_path.to_path_buf(),
        is_test_context,
    }
}

/// Extract inline Rust crate imports from a parsed module.
pub fn collect_inline_rust_imports(module: &ParsedModule, is_test_context: bool) -> Vec<InlineRustImport> {
    let mut imports = Vec::new();

    for decl in &module.ast.declarations {
        let incan_frontend::ast::Declaration::Import(import) = &decl.node else {
            continue;
        };

        match &import.kind {
            ImportKind::RustCrate {
                crate_name,
                path,
                version,
                features,
                ..
            } => {
                let import_path = format_rust_import_base_path(crate_name, path);
                imports.push(build_inline_rust_import(
                    crate_name,
                    import_path,
                    version,
                    features,
                    decl.span,
                    &module.file_path,
                    is_test_context,
                ));
            }
            ImportKind::RustFrom {
                crate_name,
                path,
                items,
                version,
                features,
                ..
            } => {
                let imported = items.iter().map(|item| item.name.clone()).collect::<Vec<_>>();
                let import_path = format_rust_from_import_path(crate_name, path, &imported);
                imports.push(build_inline_rust_import(
                    crate_name,
                    import_path,
                    version,
                    features,
                    decl.span,
                    &module.file_path,
                    is_test_context,
                ));
            }
            _ => {}
        }
    }

    imports
}

/// Extract all Rust dependency uses from a parsed module.
pub fn collect_rust_dependency_uses(module: &ParsedModule, is_test_context: bool) -> Vec<InlineRustImport> {
    let mut imports = collect_inline_rust_imports(module, is_test_context);
    let Some(rust_module_path) = &module.ast.rust_module_path else {
        return imports;
    };
    let Some(crate_name) = rust_module_path.node.split("::").next().filter(|name| !name.is_empty()) else {
        return imports;
    };
    if crate_name == stdlib::STDLIB_ROOT || stdlib::is_path_extra_crate_dep(crate_name) {
        return imports;
    }

    imports.push(build_inline_rust_import(
        crate_name,
        format!("rust.module(\"{}\")", rust_module_path.node),
        &None,
        &[],
        rust_module_path.span,
        &module.file_path,
        is_test_context,
    ));
    imports
}

/// Build a map of file paths to source contents for error reporting.
pub fn build_source_map(modules: &[ParsedModule]) -> HashMap<PathBuf, String> {
    let mut sources = HashMap::new();
    for module in modules {
        sources.insert(module.file_path.clone(), module.source.clone());
    }
    sources
}

/// Format a dependency resolution error with source-file context.
pub fn format_dependency_error(error: &DependencyError, sources: &HashMap<PathBuf, String>) -> String {
    let file_path = error.file_path.to_string_lossy();
    if let Some(source) = sources.get(&error.file_path) {
        return diagnostics::format_error(&file_path, source, &error.error);
    }
    if let Ok(source) = fs::read_to_string(&error.file_path) {
        return diagnostics::format_error(&file_path, &source, &error.error);
    }

    format!("error: {}\n  --> {}\n", error.error.message, error.file_path.display())
}

/// Build a lookup map from canonical module key (`a_b_c`) to module index in `collect_modules` output.
pub fn module_key_index(modules: &[ParsedModule]) -> HashMap<String, usize> {
    let mut module_idx_by_key: HashMap<String, usize> = HashMap::new();
    for (idx, module) in modules.iter().enumerate() {
        let key = canonicalize_source_module_segments(&module.path_segments).join("_");
        module_idx_by_key.insert(key, idx);
    }
    module_idx_by_key
}

/// Resolve imported source-module dependencies for one collected module using a precomputed module key index.
///
/// Public signatures in a directly imported module may reference types from that module's own imports, so the
/// typechecker needs the transitive source-module dependency closure rather than just the immediate import list. This
/// helper preserves stable module ordering by returning dependencies in collected-module index order. Bare sibling
/// paths and absolute `crate.*` paths are both local source-module edges and must contribute to the same closure.
///
/// Use this variant inside per-module loops to avoid rebuilding the module key map on every iteration.
pub fn imported_module_deps_for_with_index<'m>(
    modules: &'m [ParsedModule],
    module_index: usize,
    module_idx_by_key: &HashMap<String, usize>,
) -> Vec<(&'m str, &'m Program)> {
    imported_module_deps_for_with_index_and_plan(modules, module_index, module_idx_by_key, None)
}

/// Resolve imported source dependencies with the SDK producer's bootstrap namespace bridge enabled.
pub fn imported_module_deps_for_with_provider_plan<'m>(
    modules: &'m [ParsedModule],
    module_index: usize,
    module_idx_by_key: &HashMap<String, usize>,
    provider_plan: &ProviderPlan,
) -> Vec<(&'m str, &'m Program)> {
    imported_module_deps_for_with_index_and_plan(modules, module_index, module_idx_by_key, Some(provider_plan))
}

/// Shared source-dependency closure with an optional provider-build namespace bridge.
fn imported_module_deps_for_with_index_and_plan<'m>(
    modules: &'m [ParsedModule],
    module_index: usize,
    module_idx_by_key: &HashMap<String, usize>,
    provider_plan: Option<&ProviderPlan>,
) -> Vec<(&'m str, &'m Program)> {
    // ---- Context: bounds and setup ----
    if module_index >= modules.len() {
        return Vec::new();
    }

    // ---- Context: walk the transitive local source-module import closure ----
    /// Collect immediate local source dependencies for one module from both bare and absolute `crate.*` imports.
    fn direct_local_dep_indexes(
        modules: &[ParsedModule],
        module_index: usize,
        module_idx_by_key: &HashMap<String, usize>,
        provider_plan: Option<&ProviderPlan>,
    ) -> BTreeSet<usize> {
        /// Resolve one import path to the exact collected source module, including a safe nested-entry fallback.
        fn resolve_local_dep_index(
            current_module_path: &[String],
            path: &ImportPath,
            module_idx_by_key: &HashMap<String, usize>,
            provider_plan: Option<&ProviderPlan>,
        ) -> Option<usize> {
            let exact = logical_source_import_candidates(current_module_path, path)
                .into_iter()
                .find_map(|candidate| {
                    let key = canonicalize_source_module_segments(&candidate).join("_");
                    module_idx_by_key.get(&key).copied()
                });
            if exact.is_some() {
                return exact;
            }

            // An SDK producer writes its public spelling (`std.registry`) while compiling the physical provider
            // source module (`registry`). Only its explicit bootstrap grant authorizes this source-graph edge.
            if path.parent_levels == 0
                && !path.is_absolute
                && provider_plan.is_some_and(|plan| plan.bootstrap_owns_sdk_module(&path.segments))
                && path.segments.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT)
            {
                let physical_key = canonicalize_source_module_segments(&path.segments[1..]).join("_");
                if let Some(index) = module_idx_by_key.get(&physical_key).copied() {
                    return Some(index);
                }
            }
            if path.is_absolute || path.parent_levels > 0 {
                return None;
            }

            // CLI entrypoints retain the synthetic logical name `main` even when their file lives in a nested source
            // directory. The on-disk resolver has already admitted the sibling into this module set; recover that
            // canonical identity only when the bare import suffix identifies exactly one collected module.
            let suffix = canonicalize_source_module_segments(&path.segments).join("_");
            if suffix.is_empty() {
                return None;
            }
            let suffix = format!("_{suffix}");
            let mut matches = module_idx_by_key
                .iter()
                .filter_map(|(key, index)| key.ends_with(&suffix).then_some(*index))
                .collect::<Vec<_>>();
            matches.sort_unstable();
            matches.dedup();
            match matches.as_slice() {
                [index] => Some(*index),
                _ => None,
            }
        }

        let mut dep_indexes: BTreeSet<usize> = BTreeSet::new();
        for decl in &modules[module_index].ast.declarations {
            let incan_frontend::ast::Declaration::Import(import) = &decl.node else {
                continue;
            };
            match &import.kind {
                ImportKind::From { module, .. } => {
                    if let Some(dep_idx) = resolve_local_dep_index(
                        &modules[module_index].path_segments,
                        module,
                        module_idx_by_key,
                        provider_plan,
                    ) && dep_idx != module_index
                    {
                        dep_indexes.insert(dep_idx);
                    }
                }
                ImportKind::Module(path) => {
                    let dep_idx = resolve_local_dep_index(
                        &modules[module_index].path_segments,
                        path,
                        module_idx_by_key,
                        provider_plan,
                    )
                    .or_else(|| {
                        let mut parent_path = path.clone();
                        parent_path.segments.pop();
                        resolve_local_dep_index(
                            &modules[module_index].path_segments,
                            &parent_path,
                            module_idx_by_key,
                            provider_plan,
                        )
                    });
                    if let Some(dep_idx) = dep_idx
                        && dep_idx != module_index
                    {
                        dep_indexes.insert(dep_idx);
                    }
                }
                _ => {}
            }
        }
        dep_indexes
    }

    let mut dep_indexes: BTreeSet<usize> = BTreeSet::new();
    let mut pending: Vec<usize> = direct_local_dep_indexes(modules, module_index, module_idx_by_key, provider_plan)
        .into_iter()
        .collect();
    while let Some(dep_idx) = pending.pop() {
        if dep_idx == module_index || !dep_indexes.insert(dep_idx) {
            continue;
        }
        pending.extend(direct_local_dep_indexes(
            modules,
            dep_idx,
            module_idx_by_key,
            provider_plan,
        ));
    }

    // ---- Context: materialize dependency pairs for typechecker.check_with_imports ----
    dep_indexes
        .into_iter()
        .map(|idx| (modules[idx].name.as_str(), &modules[idx].ast))
        .collect()
}

/// Register every collected module's real path segments with a checker before typechecking.
///
/// The dependency cache is keyed by the flattened, underscore-joined module name, which also names the emitted Rust
/// module and is therefore not injective: `pkg.helpers` and a module literally named `pkg_helpers` flatten to one
/// string. Supplying the true segments lets a canonical identity name the module that answered rather than the
/// spelling that asked. Mirrors the pair `IrCodegen::add_module_with_path_segments` already carries for emission.
pub fn register_module_path_segments(checker: &mut typechecker::TypeChecker, modules: &[ParsedModule]) {
    for module in modules {
        checker.register_dependency_module_path_segments(&module.name, module.path_segments.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use incan_frontend::typechecker::IdentKind;
    use incan_frontend::{lexer, parser};
    use incan_provider::test_support::parsed_module_for_test;

    #[test]
    fn collect_rust_dependency_uses_includes_rust_module_root() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test("rust.module(\"datafusion::prelude\")\n\ndef main() -> None:\n  pass\n")?;

        let imports = collect_rust_dependency_uses(&module, false);

        assert!(
            imports.iter().any(|import| import.crate_name == "datafusion"
                && import.import_path == "rust.module(\"datafusion::prelude\")"),
            "rust.module roots should participate in dependency resolution: {imports:?}"
        );
        Ok(())
    }

    #[test]
    fn collect_rust_dependency_uses_skips_stdlib_path_extra_crate_roots() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test("rust.module(\"incan_web_macros\")\n\ndef main() -> None:\n  pass\n")?;

        let imports = collect_rust_dependency_uses(&module, false);

        assert!(
            imports.iter().all(|import| import.crate_name != "incan_web_macros"),
            "stdlib-managed path crates should come from project requirements, not rust.module dependency uses: {imports:?}"
        );
        Ok(())
    }

    #[test]
    fn collect_modules_canonicalizes_directory_entrypoints() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )?;

        let src_dir = project_root.join("src");
        std::fs::create_dir_all(src_dir.join("dataset"))?;
        std::fs::write(
            src_dir.join("lib.incn"),
            "from dataset.mod import DataSet\nfrom dataset.ops import filter_ds\n",
        )?;
        std::fs::write(
            src_dir.join("dataset").join("mod.incn"),
            "pub trait DataSet[T]:\n    pass\n",
        )?;
        std::fs::write(
            src_dir.join("dataset").join("ops.incn"),
            "from dataset.mod import DataSet\npub def filter_ds[T](ds: DataSet[T]) -> DataSet[T]:\n    return ds\n",
        )?;

        let entry = src_dir.join("lib.incn");
        let entry_str = entry
            .to_str()
            .ok_or("entry path should be valid utf-8 for collect_modules test")?;
        let modules = collect_modules(entry_str)?;

        let dataset_mod = modules
            .iter()
            .find(|module| module.file_path.ends_with(Path::new("dataset").join("mod.incn")))
            .ok_or("expected dataset/mod.incn to be collected")?;
        assert_eq!(dataset_mod.path_segments, vec!["dataset".to_string()]);
        assert_ne!(
            dataset_mod.path_segments,
            vec!["dataset".to_string(), "mod".to_string()]
        );

        let dataset_ops = modules
            .iter()
            .find(|module| module.file_path.ends_with(Path::new("dataset").join("ops.incn")))
            .ok_or("expected dataset/ops.incn to be collected")?;
        assert_eq!(
            dataset_ops.path_segments,
            vec!["dataset".to_string(), "ops".to_string()]
        );

        Ok(())
    }

    #[test]
    fn collect_modules_keeps_migrated_stdlib_sources_out_of_consumer_graphs() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let entry = tmp.path().join("main.incn");
        std::fs::write(
            &entry,
            "from std.environ import get_str\n\ndef main() -> None:\n    get_str(\"HOME\")\n",
        )?;

        let modules = collect_modules(&entry.to_string_lossy())?;
        assert_eq!(
            modules.len(),
            1,
            "migrated stdlib imports must be supplied by the artifact, not source modules"
        );
        assert_eq!(modules[0].path_segments, ["main"]);
        Ok(())
    }

    #[test]
    fn collect_modules_supports_init_directory_entrypoints() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )?;

        let src_dir = project_root.join("src");
        std::fs::create_dir_all(src_dir.join("dataset"))?;
        std::fs::write(src_dir.join("lib.incn"), "from dataset import DataSet\n")?;
        std::fs::write(
            src_dir.join("dataset").join("__init__.incn"),
            "pub trait DataSet[T]:\n    pass\n",
        )?;

        let entry = src_dir.join("lib.incn");
        let entry_str = entry
            .to_str()
            .ok_or("entry path should be valid utf-8 for collect_modules test")?;
        let modules = collect_modules(entry_str)?;

        let dataset_init = modules
            .iter()
            .find(|module| module.file_path.ends_with(Path::new("dataset").join("__init__.incn")))
            .ok_or("expected dataset/__init__.incn to be collected")?;
        assert_eq!(dataset_init.path_segments, vec!["dataset".to_string()]);

        Ok(())
    }

    #[test]
    fn collect_modules_skips_unknown_stdlib_source_resolution() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let src_dir = tmp.path().join("src");
        std::fs::create_dir_all(&src_dir)?;
        let entry = src_dir.join("main.incn");
        std::fs::write(&entry, "from std.unknown_module import thing\n")?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        assert_eq!(modules.len(), 1, "unknown std.* imports should not queue source stubs");
        Ok(())
    }

    #[test]
    fn collect_modules_resolves_source_root_for_examples_entrypoints() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("loaf.toml"),
            r#"[project]
name = "demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        let examples_dir = project_root.join("examples");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&examples_dir)?;

        std::fs::write(
            src_dir.join("dataset.incn"),
            r#"pub trait DataSet[T]:
    pass
"#,
        )?;
        let entry = examples_dir.join("trait_hierarchy.incn");
        std::fs::write(
            &entry,
            r#"from dataset import DataSet

def main() -> None:
    pass
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        assert_eq!(modules.len(), 2, "example entrypoint should pull source-root imports");
        assert!(
            modules.iter().any(|m| m.file_path.ends_with("src/dataset.incn")),
            "expected dataset module to resolve from source root"
        );
        Ok(())
    }

    /// A module cycle that re-enters the entry through a differently spelled path collects the entry once.
    ///
    /// The resolver canonicalizes every local import it locates, so a back edge into the entry hands collection the
    /// entry's canonical path. When the caller spelled the entry through a symlink (macOS keeps its temporary
    /// directory behind one), keying the visited set by spelling collected the entry twice, and the two parsed
    /// modules then carried one module identity, which the replacement execution graph refuses as a duplicate
    /// (#1557).
    #[cfg(unix)]
    #[test]
    fn collect_modules_collects_a_symlink_spelled_entry_once_through_its_own_back_edge_issue1557()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let real_root = tmp.path().canonicalize()?.join("real");
        std::fs::create_dir_all(real_root.join("src"))?;
        std::fs::write(real_root.join("loaf.toml"), "[project]\nname = \"frame_cycle\"\n")?;
        std::fs::write(
            real_root.join("src/main.incn"),
            "from helper import bounce\n\npub def step(value: int) -> int:\n    if value == 0:\n        return 42\n    return bounce(value - 1)\n\ndef main() -> int:\n    return step(4)\n",
        )?;
        std::fs::write(
            real_root.join("src/helper.incn"),
            "from main import step\n\npub def bounce(value: int) -> int:\n    return step(value)\n",
        )?;
        let linked_root = tmp.path().canonicalize()?.join("linked");
        std::os::unix::fs::symlink(&real_root, &linked_root)?;
        let entry = linked_root.join("src/main.incn");

        let session = CompilationSession::discover_for_collection_with_feature_selection(&entry, &Default::default())?;
        let modules =
            collect_modules_detailed_with_session(entry.clone(), &session).map_err(|failure| failure.render_human())?;

        let entry_modules: Vec<&Path> = modules
            .iter()
            .filter(|module| module.path_segments == ["main"])
            .map(|module| module.file_path.as_path())
            .collect();
        assert_eq!(
            entry_modules,
            vec![entry.as_path()],
            "the entry must be collected exactly once, under the spelling it was reached by: {entry_modules:?}"
        );
        let collected: Vec<&Path> = modules.iter().map(|module| module.file_path.as_path()).collect();
        assert_eq!(collected.len(), 2, "one entry and one helper: {collected:?}");
        Ok(())
    }

    #[test]
    fn collect_modules_orders_dependencies_before_dependents() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("loaf.toml"),
            r#"[project]
name = "dep_order_demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;

        std::fs::write(
            src_dir.join("substrait_model.incn"),
            r#"pub model SubstraitPlan:
    pub rels: list[str]
"#,
        )?;
        std::fs::write(
            src_dir.join("substrait_builder.incn"),
            r#"from substrait_model import SubstraitPlan

pub def plan_from_named_table(name: str) -> SubstraitPlan:
    _ = name
    return SubstraitPlan(rels=[])
"#,
        )?;
        let entry = src_dir.join("lib.incn");
        std::fs::write(
            &entry,
            r#"from substrait_builder import plan_from_named_table
from substrait_model import SubstraitPlan

pub def probe() -> SubstraitPlan:
    return plan_from_named_table(str("orders"))
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        let mut model_idx = None;
        let mut builder_idx = None;
        let mut entry_idx = None;
        for (idx, module) in modules.iter().enumerate() {
            if module.file_path.ends_with("src/substrait_model.incn") {
                model_idx = Some(idx);
            } else if module.file_path.ends_with("src/substrait_builder.incn") {
                builder_idx = Some(idx);
            } else if module.file_path.ends_with("src/lib.incn") {
                entry_idx = Some(idx);
            }
        }

        let Some(model_idx) = model_idx else {
            panic!("expected substrait_model module");
        };
        let Some(builder_idx) = builder_idx else {
            panic!("expected substrait_builder module");
        };
        let Some(entry_idx) = entry_idx else {
            panic!("expected entry module");
        };

        assert!(
            model_idx < builder_idx,
            "dependency module must be ordered before dependent module"
        );
        assert!(
            builder_idx < entry_idx,
            "entry module must be ordered after imported modules"
        );
        Ok(())
    }

    #[test]
    fn collect_modules_order_keeps_imported_types_resolved_during_typecheck() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("loaf.toml"),
            r#"[project]
name = "dep_check_demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;

        std::fs::write(
            src_dir.join("substrait_model.incn"),
            r#"@derive(Clone)
pub model SubstraitRelNode:
    pub rel_id: str

@derive(Clone)
pub model SubstraitPlan:
    pub plan_id: str
    pub root_rel_id: str
    pub rels: list[SubstraitRelNode]
    pub profile_tags: list[str]

pub def empty_substrait_plan() -> SubstraitPlan:
    return SubstraitPlan(plan_id=str("p"), root_rel_id=str(""), rels=[], profile_tags=[])
"#,
        )?;
        std::fs::write(
            src_dir.join("substrait_builder.incn"),
            r#"from substrait_model import SubstraitPlan, SubstraitRelNode, empty_substrait_plan

pub def build_one() -> SubstraitPlan:
    plan = empty_substrait_plan()
    mut rels = plan.rels
    rel = SubstraitRelNode(rel_id=str("r1"))
    rels.append(rel)
    return SubstraitPlan(plan_id=plan.plan_id, root_rel_id=rel.rel_id, rels=rels, profile_tags=plan.profile_tags)
"#,
        )?;
        let entry = src_dir.join("lib.incn");
        std::fs::write(
            &entry,
            r#"from substrait_builder import build_one
from substrait_model import SubstraitPlan

pub def probe() -> SubstraitPlan:
    return build_one()
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        let module_idx_by_key = module_key_index(&modules);
        for (idx, module) in modules.iter().enumerate() {
            let deps = imported_module_deps_for_with_index(&modules, idx, &module_idx_by_key);
            let mut checker = typechecker::TypeChecker::new();
            if let Err(errs) = checker.check_with_imports(&module.ast, &deps) {
                return Err(format!(
                    "typecheck failed for module {}: {:?}",
                    module.file_path.display(),
                    errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>()
                )
                .into());
            }
        }
        Ok(())
    }

    #[test]
    fn provider_bootstrap_std_import_adds_the_physical_source_dependency() -> Result<(), Box<dyn std::error::Error>> {
        let make_module = |name: &str, source: &str| -> Result<ParsedModule, Box<dyn std::error::Error>> {
            let tokens = lexer::lex(source).map_err(|errors| format!("{name} lex failed: {errors:?}"))?;
            let ast = parser::parse(&tokens).map_err(|errors| format!("{name} parse failed: {errors:?}"))?;
            Ok(ParsedModule {
                name: name.to_string(),
                path_segments: vec![name.to_string()],
                file_path: PathBuf::from(format!("{name}.incn")),
                source: source.to_string(),
                ast,
            })
        };
        let modules = vec![
            make_module("registry", "pub model Registry:\n    label: str\n")?,
            make_module(
                "features",
                "from std.registry import Registry\n\npub def label(value: Registry) -> str:\n    return value.label\n",
            )?,
        ];
        let module_idx_by_key = module_key_index(&modules);
        let features_index = modules
            .iter()
            .position(|module| module.path_segments == ["features".to_string()])
            .ok_or("expected features module")?;
        assert!(
            imported_module_deps_for_with_index(&modules, features_index, &module_idx_by_key).is_empty(),
            "an ordinary consumer must not reinterpret std.registry as a local source import"
        );

        let provider_plan = ProviderPlan::default().with_bootstrap_sdk_namespace_roots(["registry".to_string()]);
        let dependencies =
            imported_module_deps_for_with_provider_plan(&modules, features_index, &module_idx_by_key, &provider_plan);
        assert_eq!(
            dependencies.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            vec!["registry"],
            "the bootstrap grant must add the exact physical provider source edge"
        );
        Ok(())
    }

    #[test]
    fn imported_module_deps_preserve_bare_sibling_class_privacy_issue886() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let src_dir = tmp.path().join("src");
        let pkg_dir = src_dir.join("pkg");
        std::fs::create_dir_all(&pkg_dir)?;
        std::fs::write(
            tmp.path().join("loaf.toml"),
            "[project]\nname = \"sibling_private_class\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            pkg_dir.join("vaults.incn"),
            "pub class Vault:\n    secret: str = \"sealed\"\n    pub label: str\n",
        )?;
        let consumer_path = pkg_dir.join("consumer.incn");
        std::fs::write(
            &consumer_path,
            r#"from vaults import Vault

def leak() -> str:
    value = Vault(label="visible")
    return value.secret
"#,
        )?;

        let modules = collect_modules(consumer_path.to_string_lossy().as_ref())?;
        let consumer_index = modules
            .iter()
            .position(|module| module.file_path == consumer_path)
            .ok_or("expected nested consumer module")?;
        let module_idx_by_key = module_key_index(&modules);
        let dependencies = imported_module_deps_for_with_index(&modules, consumer_index, &module_idx_by_key);
        assert!(
            dependencies.iter().any(|(name, _)| *name == "pkg_vaults"),
            "bare sibling imports must retain the canonical nested dependency; modules={:?}, dependencies={:?}",
            modules
                .iter()
                .map(|module| (module.name.clone(), module.path_segments.clone()))
                .collect::<Vec<_>>(),
            dependencies
                .iter()
                .map(|(name, _)| (*name).to_string())
                .collect::<Vec<_>>()
        );

        let mut checker = typechecker::TypeChecker::new();
        checker.set_current_module_path(Some(modules[consumer_index].path_segments.clone()));
        let errors = match checker.check_with_imports(&modules[consumer_index].ast, &dependencies) {
            Ok(()) => return Err("private sibling field access must fail typechecking".into()),
            Err(errors) => errors,
        };
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("Field 'secret' on 'Vault' is private")),
            "expected private-field diagnostic, got: {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        );
        Ok(())
    }

    /// Verifies that absolute from-imports and module imports both contribute local dependency metadata before
    /// typechecking.
    #[test]
    fn imported_module_deps_preserve_absolute_crate_public_type_metadata_issue882()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("loaf.toml"),
            r#"[project]
name = "absolute_crate_public_types"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;

        std::fs::write(
            src_dir.join("types.incn"),
            r#"pub enum Access:
    Allowed
    Denied


pub model Decision:
    pub admitted: bool
    pub reason: str
"#,
        )?;
        std::fs::write(
            src_dir.join("consumer.incn"),
            r#"from crate.types import Access, Decision


pub def allowed() -> Access:
    return Access.Allowed


pub def explain(decision: Decision) -> str:
    if decision.admitted:
        return decision.reason
    return "denied"
"#,
        )?;
        std::fs::write(src_dir.join("module_consumer.incn"), "import crate.types\n")?;
        let entry = src_dir.join("lib.incn");
        std::fs::write(
            &entry,
            r#"pub from crate.consumer import allowed, explain
pub from crate.types import Access, Decision
import crate.module_consumer
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        let module_idx_by_key = module_key_index(&modules);
        let consumer_idx = modules
            .iter()
            .position(|module| module.file_path.ends_with("src/consumer.incn"))
            .ok_or("expected src/consumer.incn module")?;
        let consumer_deps = imported_module_deps_for_with_index(&modules, consumer_idx, &module_idx_by_key);
        assert!(
            consumer_deps.iter().any(|(name, _)| *name == "types"),
            "expected absolute from-import dependency `consumer -> types`, got: {:?}",
            consumer_deps
                .iter()
                .map(|(name, _)| (*name).to_string())
                .collect::<Vec<_>>()
        );

        let module_consumer_idx = modules
            .iter()
            .position(|module| module.file_path.ends_with("src/module_consumer.incn"))
            .ok_or("expected src/module_consumer.incn module")?;
        let module_consumer_deps =
            imported_module_deps_for_with_index(&modules, module_consumer_idx, &module_idx_by_key);
        assert!(
            module_consumer_deps.iter().any(|(name, _)| *name == "types"),
            "expected absolute module-import dependency `module_consumer -> types`, got: {:?}",
            module_consumer_deps
                .iter()
                .map(|(name, _)| (*name).to_string())
                .collect::<Vec<_>>()
        );

        for (idx, module) in modules.iter().enumerate() {
            let deps = imported_module_deps_for_with_index(&modules, idx, &module_idx_by_key);
            let mut checker = typechecker::TypeChecker::new();
            if let Err(errs) = checker.check_with_imports(&module.ast, &deps) {
                return Err(format!(
                    "typecheck failed for module {}: {:?}",
                    module.file_path.display(),
                    errs.iter().map(|error| error.message.clone()).collect::<Vec<_>>()
                )
                .into());
            }
        }
        Ok(())
    }

    #[test]
    fn imported_module_deps_for_includes_forward_edge_in_cycle() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("loaf.toml"),
            r#"[project]
name = "cycle_dep_resolver_demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            src_dir.join("a.incn"),
            r#"from b import pong

pub def ping() -> int:
    return pong()
"#,
        )?;
        std::fs::write(
            src_dir.join("b.incn"),
            r#"from a import ping

pub def pong() -> int:
    return 1
"#,
        )?;
        let entry = src_dir.join("main.incn");
        std::fs::write(
            &entry,
            r#"from a import ping

pub def main() -> int:
    return ping()
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        let Some(b_index) = modules
            .iter()
            .position(|module| module.file_path.ends_with("src/b.incn"))
        else {
            panic!("expected src/b.incn module");
        };
        let module_idx_by_key = module_key_index(&modules);
        let deps = imported_module_deps_for_with_index(&modules, b_index, &module_idx_by_key);
        assert!(
            deps.iter().any(|(name, _)| *name == "a"),
            "expected cyclic forward dependency `b -> a` to be resolved, got: {:?}",
            deps.iter().map(|(name, _)| (*name).to_string()).collect::<Vec<_>>()
        );
        Ok(())
    }

    #[test]
    fn imported_module_deps_for_includes_transitive_signature_dependencies() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("loaf.toml"),
            r#"[project]
name = "transitive_signature_dep_demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            src_dir.join("dataset.incn"),
            r#"pub class LazyFrame[T]:
    def clone(self) -> Self:
        return self
"#,
        )?;
        std::fs::write(
            src_dir.join("session.incn"),
            r#"from dataset import LazyFrame

pub class Session:
    def read_csv[T](self) -> Result[LazyFrame[T], str]:
        return Err(str("not implemented"))
"#,
        )?;
        let entry = src_dir.join("main.incn");
        std::fs::write(
            &entry,
            r#"from session import Session

def main() -> Result[None, str]:
    session = Session()
    lines = session.read_csv[int]()?
    lines.clone()
    return Ok(None)
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        let Some(main_index) = modules
            .iter()
            .position(|module| module.file_path.ends_with("src/main.incn"))
        else {
            return Err("expected src/main.incn module".into());
        };
        let module_idx_by_key = module_key_index(&modules);
        let deps = imported_module_deps_for_with_index(&modules, main_index, &module_idx_by_key);
        assert!(
            deps.iter().any(|(name, _)| *name == "dataset"),
            "expected transitive dependency `dataset` to be included for imported signature resolution, got: {:?}",
            deps.iter().map(|(name, _)| (*name).to_string()).collect::<Vec<_>>()
        );

        let mut checker = typechecker::TypeChecker::new();
        if let Err(errs) = checker.check_with_imports(&modules[main_index].ast, &deps) {
            return Err(format!(
                "typecheck failed: {:?}",
                errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>()
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn session_analysis_keeps_crate_root_facade_class_reexports_as_types() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let source_root = project_root.join("src");
        let session_root = source_root.join("session");
        std::fs::create_dir_all(&session_root)?;
        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"crate_root_facade\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(session_root.join("types.incn"), "pub class Session:\n    pub id: int\n")?;
        std::fs::write(
            session_root.join("mod.incn"),
            "pub from crate.session.types import Session\n",
        )?;
        let main_path = source_root.join("main.incn");
        let main_source = "from session import Session\n\ndef main() -> None:\n    session = Session(id=1)\n";
        std::fs::write(&main_path, main_source)?;

        let session = CompilationSession::discover_with_feature_selection(&main_path, &FeatureSelection::default())?;
        let modules = collect_modules_detailed_with_session(main_path.clone(), &session)
            .map_err(|failure| failure.render_human())?;
        let module_idx_by_key = module_key_index(&modules);
        let facade_index = modules
            .iter()
            .position(|module| module.file_path.ends_with("src/session/mod.incn"))
            .ok_or("expected the session facade module")?;
        let facade_dependencies = imported_module_deps_for_with_index(&modules, facade_index, &module_idx_by_key);
        assert!(
            facade_dependencies.iter().any(|(name, _)| *name == "session_types"),
            "crate-root imports must contribute their source dependency to the session analysis closure; got: {:?}",
            facade_dependencies
                .iter()
                .map(|(name, _)| (*name).to_string())
                .collect::<Vec<_>>()
        );

        let analysis = session
            .analyze_modules(
                &modules,
                #[cfg(feature = "rust_inspect")]
                None,
            )
            .map_err(|failure| failure.render_human())?;
        let callee_start = main_source.find("Session(id=1)").ok_or("expected constructor call")?;
        assert_eq!(
            analysis
                .type_info_for_path(&main_path)
                .ok_or("expected main session analysis")?
                .ident_kind(Span::new(callee_start, callee_start + "Session".len())),
            Some(IdentKind::TypeName),
            "a public facade re-export of a crate-root class must stay a class constructor in shared session facts"
        );
        Ok(())
    }

    #[test]
    fn dependency_closure_includes_crate_root_module_imports() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let source_root = project_root.join("src");
        let types_root = source_root.join("types");
        std::fs::create_dir_all(&types_root)?;
        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"crate_root_module_import\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(types_root.join("user.incn"), "pub class User:\n    pub id: int\n")?;
        let consumer_path = source_root.join("consumer.incn");
        std::fs::write(
            &consumer_path,
            "import crate.types.user\n\npub def consume() -> None:\n    pass\n",
        )?;
        let main_path = source_root.join("main.incn");
        std::fs::write(
            &main_path,
            "from consumer import consume\n\ndef main() -> None:\n    consume()\n",
        )?;

        let session = CompilationSession::discover_with_feature_selection(&main_path, &FeatureSelection::default())?;
        let modules = collect_modules_detailed_with_session(main_path.clone(), &session)
            .map_err(|failure| failure.render_human())?;
        let module_idx_by_key = module_key_index(&modules);
        let consumer_index = modules
            .iter()
            .position(|module| module.file_path.ends_with("src/consumer.incn"))
            .ok_or("expected the consumer module")?;
        let dependencies = imported_module_deps_for_with_index(&modules, consumer_index, &module_idx_by_key);
        assert!(
            dependencies.iter().any(|(name, _)| *name == "types_user"),
            "crate-root module imports must contribute their source dependency to the closure; got: {:?}",
            dependencies
                .iter()
                .map(|(name, _)| (*name).to_string())
                .collect::<Vec<_>>()
        );
        Ok(())
    }

    #[test]
    fn collect_modules_supports_example_entry_with_cyclic_src_interfaces() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("loaf.toml"),
            r#"[project]
name = "example_cycle_demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        let examples_dir = project_root.join("examples");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&examples_dir)?;
        std::fs::write(
            src_dir.join("functions.incn"),
            r#"from dataset import DataFrame, DataSet

pub def display[T](data: DataSet[T]) -> None:
    pass

pub def sink[T](data: DataFrame[T]) -> None:
    pass
"#,
        )?;
        std::fs::write(
            src_dir.join("session.incn"),
            r#"from dataset import DataFrame, LazyFrame

pub model SessionError:
    pub message: str

pub class Session:
    @staticmethod
    def default() -> Session:
        return Session()

    def read_csv[T](self, _logical_name: str, _uri: str) -> Result[LazyFrame[T], SessionError]:
        return Err(SessionError(message=str("not implemented")))

    def activate(self) -> None:
        pass

pub def collect_with_active_session[T](data: LazyFrame[T]) -> Result[DataFrame[T], SessionError]:
    return Err(SessionError(message=str("not implemented")))
"#,
        )?;
        std::fs::write(
            src_dir.join("dataset.incn"),
            r#"from session import SessionError, collect_with_active_session

pub trait DataSet[T]:
    pass

pub class DataFrame[T] with DataSet:
    def clone(self) -> Self:
        return self

pub class LazyFrame[T] with DataSet:
    def clone(self) -> Self:
        return self

    def collect(self) -> Result[DataFrame[T], SessionError]:
        return collect_with_active_session[T](self.clone())
"#,
        )?;
        let entry = examples_dir.join("main.incn");
        std::fs::write(
            &entry,
            r#"from functions import display
from session import Session, SessionError

def main() -> Result[None, SessionError]:
    mut session = Session.default()
    lines = session.read_csv[int](str("orders"), str("input.csv"))?
    transformed = lines.clone()
    session.activate()
    df = transformed.clone().collect()?
    display(df)
    return Ok(None)
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        let module_idx_by_key = module_key_index(&modules);
        for (idx, module) in modules.iter().enumerate() {
            let deps = imported_module_deps_for_with_index(&modules, idx, &module_idx_by_key);
            let mut checker = typechecker::TypeChecker::new();
            if let Err(errs) = checker.check_with_imports(&module.ast, &deps) {
                return Err(format!(
                    "typecheck failed for module {}: {:?}",
                    module.file_path.display(),
                    errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>()
                )
                .into());
            }
        }
        Ok(())
    }

    #[test]
    fn collect_modules_supports_directory_module_cycles_from_example_entry() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("loaf.toml"),
            r#"[project]
name = "example_directory_cycle_demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        let dataset_dir = src_dir.join("dataset");
        let examples_dir = project_root.join("examples");
        std::fs::create_dir_all(&dataset_dir)?;
        std::fs::create_dir_all(&examples_dir)?;
        std::fs::write(
            src_dir.join("session.incn"),
            r#"from dataset import DataFrame, LazyFrame

pub model SessionError:
    pub message: str

pub class Session:
    @staticmethod
    def default() -> Session:
        return Session()

    def read_csv[T with Clone](self, _logical_name: str, _uri: str) -> Result[LazyFrame[T], SessionError]:
        return Err(SessionError(message=str("not implemented")))

pub def collect_with_active_session[T with Clone](data: LazyFrame[T]) -> Result[DataFrame[T], SessionError]:
    return Err(SessionError(message=str("not implemented")))
"#,
        )?;
        std::fs::write(
            dataset_dir.join("mod.incn"),
            r#"from session import SessionError, collect_with_active_session

pub trait DataSet[T with Clone]:
    pass

pub class DataFrame[T with Clone] with DataSet:
    def clone(self) -> Self:
        return self

pub class LazyFrame[T with Clone] with DataSet:
    def clone(self) -> Self:
        return self

    def collect(self) -> Result[DataFrame[T], SessionError]:
        return collect_with_active_session[T](self.clone())
"#,
        )?;
        let entry = examples_dir.join("main.incn");
        std::fs::write(
            &entry,
            r#"from session import Session, SessionError

@derive(Clone)
pub model OrderLine:
    pub sku: str

def main() -> Result[None, SessionError]:
    session = Session.default()
    lines = session.read_csv[OrderLine](str("orders"), str("input.csv"))?
    df = lines.clone().collect()?
    df.clone()
    return Ok(None)
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        let module_idx_by_key = module_key_index(&modules);
        for (idx, module) in modules.iter().enumerate() {
            let deps = imported_module_deps_for_with_index(&modules, idx, &module_idx_by_key);
            let mut checker = typechecker::TypeChecker::new();
            if let Err(errs) = checker.check_with_imports(&module.ast, &deps) {
                return Err(format!(
                    "typecheck failed for module {}: {:?}",
                    module.file_path.display(),
                    errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>()
                )
                .into());
            }
        }
        Ok(())
    }

    #[test]
    fn collect_modules_cycle_falls_back_to_deterministic_order() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("loaf.toml"),
            r#"[project]
name = "cycle_demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;

        std::fs::write(
            src_dir.join("a.incn"),
            r#"from b import pong

pub def ping() -> int:
    return pong()
"#,
        )?;
        std::fs::write(
            src_dir.join("b.incn"),
            r#"from a import ping

pub def pong() -> int:
    return 1
"#,
        )?;
        let entry = src_dir.join("main.incn");
        std::fs::write(
            &entry,
            r#"from a import ping

pub def main() -> int:
    return ping()
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        assert_eq!(modules.len(), 3, "expected all modules to be collected even with cycle");
        assert!(modules[0].file_path.ends_with("src/b.incn"));
        assert!(modules[1].file_path.ends_with("src/a.incn"));
        assert!(modules[2].file_path.ends_with("src/main.incn"));
        Ok(())
    }

    #[test]
    fn library_source_seeds_deduplicate_imports_across_noncanonical_source_roots_issue948()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let source_root = tmp.path().join("stdlib");
        let project_root = source_root.join("components/core");
        let entrypoint = project_root.join("src/lib.incn");
        let operations = source_root.join("traits/ops.incn");
        std::fs::create_dir_all(entrypoint.parent().ok_or("entrypoint must have a parent")?)?;
        std::fs::create_dir_all(operations.parent().ok_or("operations must have a parent")?)?;
        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"core\"\n\n[build]\nsource-root = \"../..\"\n",
        )?;
        std::fs::write(&entrypoint, "import traits.ops\n")?;
        std::fs::write(
            &operations,
            "pub def add(left: int, right: int) -> int:\n  return left + right\n",
        )?;

        let session = CompilationSession::discover_with_feature_selection(&entrypoint, &FeatureSelection::default())?;
        let modules = collect_library_modules_detailed_with_session(entrypoint, &session)
            .map_err(|failure| failure.render_human())?;
        let operations_modules = modules
            .iter()
            .filter(|module| module.path_segments == ["traits".to_string(), "ops".to_string()])
            .collect::<Vec<_>>();

        assert_eq!(
            operations_modules.len(),
            1,
            "all-source discovery and an authored import must share one canonical source identity"
        );
        assert_eq!(operations_modules[0].file_path, operations.canonicalize()?);
        Ok(())
    }

    #[test]
    fn library_source_seeds_exclude_the_unselected_root_entrypoint_issue948() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let source_root = tmp.path().join("src");
        std::fs::create_dir_all(&source_root)?;
        let lib = source_root.join("lib.incn");
        let main = source_root.join("main.incn");
        std::fs::write(&lib, "pub def exported() -> int:\n  return 1\n")?;
        std::fs::write(&main, "def main() -> None:\n  pass\n")?;

        assert!(!is_unselected_package_entrypoint(
            &source_root,
            &lib,
            &lib.canonicalize()?
        ));
        assert!(is_unselected_package_entrypoint(
            &source_root,
            &main,
            &lib.canonicalize()?
        ));
        Ok(())
    }

    #[test]
    fn compilation_session_analysis_bundles_lowering_inputs_with_semantic_facts()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let source_root = project_root.join("src");
        std::fs::create_dir_all(&source_root)?;
        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"analysis_consumer\"\n",
        )?;
        let main_path = source_root.join("main.incn");
        std::fs::write(
            &main_path,
            "def helper() -> int:\n  return 1\n\ndef main() -> int:\n  return helper()\n",
        )?;

        let session = CompilationSession::discover_for_collection_with_feature_selection(
            &main_path,
            &FeatureSelection::default(),
        )?;
        let modules = collect_modules_detailed_with_session(main_path.clone(), &session)
            .map_err(|failure| failure.render_human())?;
        let analysis = session
            .analyze_modules(
                &modules,
                #[cfg(feature = "rust_inspect")]
                None,
            )
            .map_err(|failure| failure.render_human())?;
        let entry_analysis = analysis
            .module_analysis_for_path(&main_path)
            .ok_or("expected one bundled session analysis for the entry module")?;
        let snapshot = entry_analysis.semantic_snapshot();
        let type_info = entry_analysis.type_info();
        let retained_type_info = analysis
            .type_info_for_path(&main_path)
            .ok_or("expected the session to retain lowering input for the entry module")?;

        assert!(std::ptr::eq(type_info, retained_type_info));
        let entry_module = modules
            .iter()
            .find(|module| module.file_path == main_path)
            .ok_or("expected the session to collect the entry module")?;
        let lowered =
            incan_frontend::body_ir::build_body_ir_module_v0(&entry_module.ast, &entry_module.path_segments, type_info);
        assert!(lowered.render_snapshot().contains("body main"));
        assert!(snapshot.render_snapshot().contains("decl:main::helper type=() -> int"));
        assert!(
            snapshot
                .render_snapshot()
                .contains("symbol_target=function:main::helper")
        );
        Ok(())
    }
}
