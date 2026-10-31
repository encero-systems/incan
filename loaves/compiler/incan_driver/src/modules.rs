//! Module collection: from an entrypoint to the ordered set of parsed modules a command compiles, with the import
//! graph that fixes their order.
//!
//! Collection reads the program's own source files only; every other module it imports belongs to a compiled Loaf.
//! Dependencies are sorted before dependents so typechecking sees imported types resolved.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use incan_lang::lang::standard_packages::StandardPackageNamespacePolicy;
use incan_lang::lang::stdlib;

use crate::diagnostics::{CliDiagnosticFailure, render_module_warnings};
use crate::error::{CliError, CliResult};
use crate::session::CompilationSession;
use crate::sources::{collect_incan_source_files, read_source_for_diagnostics};
use incan_frontend::ast::{ImportKind, ImportPath, Program};
use incan_frontend::module::{
    SourceModuleImportResolution, SourceModuleRef, canonicalize_source_module_segments,
    logical_module_segments_from_file, logical_source_import_candidates, resolve_program_source_imports,
    resolve_source_module_path, self_import_diagnostic_message,
};
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::{diagnostics, typechecker};

/// Collect one source graph through an already-resolved compilation session, preserving the entry's logical module
/// identity. A test or alternate entry must not claim `main` and collide with an imported sibling `main.incn`.
pub fn collect_modules_detailed_with_session(
    path: PathBuf,
    session: &CompilationSession,
) -> Result<Vec<ParsedModule>, CliDiagnosticFailure> {
    let _invocation =
        tracing::trace_span!(target: "incan::invocation", "incan_driver::collect_modules", input = %path.display())
            .entered();
    let path_segments =
        logical_module_segments_from_file(&session.source_root, &path).unwrap_or_else(|| vec!["main".to_string()]);
    collect_modules_detailed_with_session_at_path(path, session, path_segments)
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
    collect_document_modules(path, session, path_segments, &HashMap::new())
}

/// Collect an editor document's source graph: the document under its source-root-relative identity, with every
/// module that has an unsaved buffer in `open_sources` read from that buffer instead of its file.
pub fn collect_document_modules(
    path: PathBuf,
    session: &CompilationSession,
    path_segments: Vec<String>,
    open_sources: &HashMap<PathBuf, String>,
) -> Result<Vec<ParsedModule>, CliDiagnosticFailure> {
    let module_name = path_segments.join("_");
    collect_modules_detailed_from_seeds(
        path.clone(),
        session,
        vec![(path.to_string_lossy().to_string(), module_name, path_segments)],
        open_sources,
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
    let mut modules = collect_modules_detailed_from_seeds(path.clone(), session, seeds, &HashMap::new())?;
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
///
/// A standard package publishes only the modules under the `std.*` roots its policy assigns it.
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
    let own_roots = session
        .own_standard_package()
        .map(|policy| policy.namespace_roots.as_slice());
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
        if own_roots.is_some_and(|roots| path_segments.first().is_none_or(|root| !roots.contains(&root.as_str()))) {
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
/// every local import it locates, so a module reached through an import always arrives under its canonical path,
/// while a seed arrives under whatever the caller wrote. When the two disagree, as they do for an entry spelled
/// through a symlink whose import cycle reaches back into it, the entry would be collected twice under one module
/// identity (#1557).
///
/// A path that cannot be canonicalized keeps its spelling, which is the only identity it has; the file is read next
/// and reports the real failure. A module's [`ParsedModule::file_path`] keeps the spelling it was reached by, so
/// callers that located the entry themselves can still find it by the path they passed in. Every caller of
/// [`topologically_sort_modules`] must key its dependency edges through this same function; the test runner's
/// collector in `testing::module_graph` is the other one.
pub(crate) fn source_identity_key(path: &str) -> String {
    fs::canonicalize(path)
        .map(|canonical| canonical.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string())
}

/// Collect one source graph from explicit seed modules through an already-resolved compilation session.
///
/// Seeds are `(file path, module name, module segments)` triples. Each file is parsed once, its imports are resolved
/// into further seeds, and the resulting edges drive a topological sort so a module is always parsed before the
/// modules that depend on it. A `std.*` import is another Loaf's module, except one of a standard package's own
/// roots, which names its physical source module. A file with an entry in `open_sources` is read from it.
fn collect_modules_detailed_from_seeds(
    path: PathBuf,
    session: &CompilationSession,
    mut to_process: Vec<(String, String, Vec<String>)>,
    open_sources: &HashMap<PathBuf, String>,
) -> Result<Vec<ParsedModule>, CliDiagnosticFailure> {
    let base_dir = path.parent().unwrap_or(Path::new("."));
    let mut modules = Vec::new();
    let mut processed = HashSet::new();
    let mut dependency_edges: HashMap<String, HashSet<String>> = HashMap::new();
    while let Some((file_path, module_name, path_segments)) = to_process.pop() {
        let file_key = source_identity_key(&file_path);
        if processed.contains(&file_key) {
            continue;
        }
        processed.insert(file_key.clone());
        dependency_edges.entry(file_key.clone()).or_default();

        let source = match open_sources.get(Path::new(&file_path)) {
            Some(source) => source.clone(),
            None => read_source_for_diagnostics(&file_path)?,
        };
        let file_path_obj = Path::new(&file_path);
        let (ast, gated_exports) =
            match session
                .parse_source_unprojected(file_path_obj, &source, true)
                .and_then(|unprojected| {
                    let gated = incan_frontend::library_exports::gated_public_exports(&unprojected, &path_segments);
                    session.project_parsed_program(unprojected).map(|ast| (ast, gated))
                }) {
                Ok((a, gated)) => {
                    // Surface non-fatal parser warnings immediately, so they reach the user whichever command ran. They
                    // are collected for machine-readable reports later, off the retained AST, by the typecheck pass.
                    render_module_warnings(&file_path, &source, &a.warnings);
                    (a, gated)
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
        for resolved in resolve_program_source_imports(&ast, current_base, Some(&session.source_root)) {
            let span = resolved.span;
            let module_ref = match resolved.resolution {
                SourceModuleImportResolution::Stdlib { module_path } => {
                    let Some(module_ref) = own_standard_module(session, &module_path) else {
                        continue;
                    };
                    module_ref
                }
                SourceModuleImportResolution::Local(module_ref) => module_ref,
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
                SourceModuleImportResolution::External => continue,
            };
            let dep_path_str = module_ref.file_path.to_string_lossy().to_string();
            let dep_key = source_identity_key(&dep_path_str);
            let module_segments = canonicalize_source_module_segments(&module_ref.path_segments);
            let module_name = module_segments.join("_");
            if !processed.contains(&dep_key) {
                to_process.push((dep_path_str, module_name, module_segments));
            }
            dependency_edges.entry(file_key.clone()).or_default().insert(dep_key);
        }

        modules.push(ParsedModule {
            name: module_name,
            path_segments,
            file_path: PathBuf::from(&file_path),
            source,
            ast,
            gated_exports,
        });
    }

    Ok(topologically_sort_modules(modules, &dependency_edges)?)
}

/// The physical source module a standard package's own `std.<root>.…` import names, when the session is that
/// package and the root is one of its own.
fn own_standard_module(session: &CompilationSession, module_path: &[String]) -> Option<SourceModuleRef> {
    let policy = session.own_standard_package()?;
    let root = module_path.get(1)?;
    if !policy.namespace_roots.contains(&root.as_str()) {
        return None;
    }
    resolve_source_module_path(&session.source_root, &module_path[1..])
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
/// A standard package (`own_standard_package`) also reaches its own modules through their `std.*` spelling. Use this
/// inside per-module loops to avoid rebuilding the module key map on every iteration.
pub fn imported_module_deps_for<'m>(
    modules: &'m [ParsedModule],
    module_index: usize,
    module_idx_by_key: &HashMap<String, usize>,
    own_standard_package: Option<&'static StandardPackageNamespacePolicy>,
) -> Vec<(&'m str, &'m Program)> {
    // ---- Context: bounds and setup ----
    if module_index >= modules.len() {
        return Vec::new();
    }

    // ---- Context: walk the transitive local source-module import closure ----
    let mut dep_indexes: BTreeSet<usize> = BTreeSet::new();
    let mut pending: Vec<usize> =
        direct_local_dep_indexes(modules, module_index, module_idx_by_key, own_standard_package)
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
            own_standard_package,
        ));
    }

    // ---- Context: materialize dependency pairs for typechecker.check_with_imports ----
    dep_indexes
        .into_iter()
        .map(|idx| (modules[idx].name.as_str(), &modules[idx].ast))
        .collect()
}

/// Collect immediate local source dependencies for one collected module from bare and absolute `crate.*` imports.
/// The owning closure walk supplies the same collected-module index and filters its own back edges.
fn direct_local_dep_indexes(
    modules: &[ParsedModule],
    module_index: usize,
    module_idx_by_key: &HashMap<String, usize>,
    own_standard_package: Option<&'static StandardPackageNamespacePolicy>,
) -> BTreeSet<usize> {
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
                    own_standard_package,
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
                    own_standard_package,
                )
                .or_else(|| {
                    let mut parent_path = path.clone();
                    parent_path.segments.pop();
                    resolve_local_dep_index(
                        &modules[module_index].path_segments,
                        &parent_path,
                        module_idx_by_key,
                        own_standard_package,
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

/// Resolve one import path to the exact collected source module, including a safe nested-entry fallback.
fn resolve_local_dep_index(
    current_module_path: &[String],
    path: &ImportPath,
    module_idx_by_key: &HashMap<String, usize>,
    own_standard_package: Option<&'static StandardPackageNamespacePolicy>,
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

    // A standard package writes its public spelling (`std.registry`) for its own physical source module
    // (`registry`); only a root its policy assigns it makes this a source-graph edge.
    if path.parent_levels == 0
        && !path.is_absolute
        && path.segments.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT)
        && own_standard_package.is_some_and(|policy| {
            path.segments
                .get(1)
                .is_some_and(|root| policy.namespace_roots.contains(&root.as_str()))
        })
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
