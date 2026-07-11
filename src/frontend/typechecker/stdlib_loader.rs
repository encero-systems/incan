//! RFC 023: Stdlib `.incn` file loader.
//!
//! This module provides infrastructure for loading, parsing, and extracting function and trait signatures from stdlib
//! `.incn` files. It replaces hardcoded function registries with signatures derived from the actual source files.
//!
//! ## Design
//!
//! 1. **Discovery**: finds stdlib `.incn` files using `incan_core::lang::stdlib::stdlib_stub_path`.
//! 2. **Parsing**: lexes and parses the file through the normal Incan frontend pipeline.
//! 3. **Extraction**: walks the parsed AST to extract `FunctionInfo` entries for each `def` and `TraitInfo` entries for
//!    each `trait`.
//! 4. **Caching**: results are cached per module path in a `HashMap` to avoid redundant parsing.
//!
//! ## Re-export resolution
//!
//! Modules with submodules (e.g. `std.web`) resolve to a prelude file (e.g. `stdlib/web/prelude.incn`).
//! The prelude typically only contains `from std.web.<sub> import ...` re-export statements, not direct declarations.
//! To support `from std.web import route` (where `route` is declared in `std.web.routing`), the loader follows these
//! re-export imports and merges the referenced submodule metadata into the parent.
//!
//! ## Limitations
//!
//! - Function signatures are extracted from top-level `def` declarations. Public type metadata also preserves method
//!   signatures for class/model/enum imports.
//! - Trait signatures are extracted from top-level `trait` declarations with their methods and `with` supertraits (RFC
//!   042), using the same lightweight `ast_type_to_resolved` mapping as method signatures.
//! - Compact signatures retain default presence but not expressions. Paired source declarations preserve full default
//!   expressions for source-defined functions; Rust-imported runtime entries may have no declaration.
//! - Complex types beyond the common set (`int`, `str`, `bool`, `Option[T]`, etc.) are treated as `Named`.
//! - Parse failures are logged and the module is treated as unavailable for AST-derived signature lookup.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::frontend::ast;
use crate::frontend::symbols::{CallableParam, FunctionOverloadInfo, SymbolKind, VariableInfo};
use crate::frontend::symbols::{
    ClassInfo, EnumInfo, FieldInfo, FunctionInfo, MethodInfo, ModelInfo, NewtypeInfo, ResolvedType, StaticInfo,
    TraitInfo, TypeBoundInfo, TypeInfo, overloaded_function_emitted_name,
};
use crate::frontend::typechecker::helpers::render_resolved_type_as_rust_arg;
use incan_core::lang::conventions;
use incan_core::lang::decorators::{self, DecoratorId};
use incan_core::lang::rust_keywords;
use incan_core::lang::stdlib;
use incan_core::lang::surface::functions::{self as surface_functions, SurfaceFnId};
use incan_core::lang::types::collections::{self as collection_types, CollectionTypeId};
use incan_core::lang::types::numerics::{self as numeric_types, NumericTypeId};
use incan_core::lang::types::stringlike::{self as string_types, StringLikeId};

#[derive(Debug, Clone, Default)]
struct StdlibModuleData {
    functions: Vec<StdlibFunctionEntry>,
    traits: Vec<(String, TraitInfo)>,
    trait_declarations: Vec<(String, ast::TraitDecl)>,
    types: Vec<(String, TypeInfo)>,
    type_docstrings: HashMap<String, String>,
    constants: Vec<(String, VariableInfo)>,
    statics: Vec<(String, StaticInfo)>,
    derivable_traits: Vec<String>,
    function_meta: HashMap<String, FunctionMeta>,
    trait_meta: HashMap<String, TraitMeta>,
}

/// One cached stdlib function overload with its source declaration when available.
#[derive(Debug, Clone)]
struct StdlibFunctionEntry {
    name: String,
    info: FunctionInfo,
    declaration: Option<ast::FunctionDecl>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FunctionMeta {
    pub is_rust_extern: bool,
    pub rust_module_path: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TraitMeta {
    pub rust_module_path: Option<String>,
    pub rust_derive_paths: Vec<String>,
}

/// Public stdlib source metadata consumed by one LSP completion or hover request.
///
/// Resolved signatures and source declarations deliberately travel together so callers can render every overload and
/// its docstring without reparsing the module for each public item.
#[cfg(feature = "lsp")]
#[derive(Debug, Clone, Default)]
pub(crate) struct StdlibModuleLspMetadata {
    pub functions: Vec<StdlibFunctionLspMetadata>,
    pub traits: Vec<(String, TraitInfo)>,
    pub trait_declarations: Vec<(String, ast::TraitDecl)>,
    pub types: Vec<(String, TypeInfo)>,
    pub type_docstrings: HashMap<String, String>,
}

/// One resolved stdlib overload paired with its source declaration when the function is source-defined.
#[cfg(feature = "lsp")]
#[derive(Debug, Clone)]
pub(crate) struct StdlibFunctionLspMetadata {
    pub name: String,
    pub info: FunctionInfo,
    pub declaration: Option<ast::FunctionDecl>,
}

/// Cached stdlib module signatures keyed by dot-joined module path (e.g. `"std.testing"`).
#[derive(Debug, Clone, Default)]
pub(crate) struct StdlibAstCache {
    /// Map from module path (dot-joined) to extracted stdlib module data.
    cache: HashMap<String, StdlibModuleData>,
}

impl StdlibAstCache {
    pub fn new() -> Self {
        Self { cache: HashMap::new() }
    }

    /// Look up a function binding while preserving a same-name overload set.
    pub fn lookup_function_symbol(&mut self, module_path: &[String], function_name: &str) -> Option<SymbolKind> {
        self.ensure_loaded(module_path);
        let key = module_path.join(".");
        let functions = self
            .cache
            .get(&key)?
            .functions
            .iter()
            .filter(|entry| entry.name == function_name)
            .map(|entry| entry.info.clone())
            .collect::<Vec<_>>();
        match functions.as_slice() {
            [] => None,
            [info] => Some(SymbolKind::Function(info.clone())),
            _ => Some(SymbolKind::FunctionOverloads(
                functions
                    .into_iter()
                    .map(|info| FunctionOverloadInfo {
                        info,
                        span: ast::Span::default(),
                    })
                    .collect(),
            )),
        }
    }

    /// Look up a specific trait in a stdlib module.
    ///
    /// Returns `Some(TraitInfo)` if the module has been loaded and contains a trait with the given name.
    pub fn lookup_trait(&mut self, module_path: &[String], trait_name: &str) -> Option<TraitInfo> {
        self.ensure_loaded(module_path);
        let key = module_path.join(".");
        self.cache
            .get(&key)?
            .traits
            .iter()
            .find(|(name, _)| name == trait_name)
            .map(|(_, info)| info.clone())
    }

    /// Look up a specific type in a stdlib module.
    pub fn lookup_type(&mut self, module_path: &[String], type_name: &str) -> Option<TypeInfo> {
        self.ensure_loaded(module_path);
        let key = module_path.join(".");
        self.cache
            .get(&key)?
            .types
            .iter()
            .find(|(name, _)| name == type_name)
            .map(|(_, info)| info.clone())
    }

    /// Look up a method declaration on a stdlib type, following prelude re-exports.
    ///
    /// This is intentionally AST-shaped rather than `MethodInfo`-shaped so lowering can preserve default parameter
    /// expressions when imported type methods are called with omitted arguments.
    pub(crate) fn lookup_type_method_decl(
        &mut self,
        module_path: &[String],
        type_name: &str,
        method_name: &str,
    ) -> Option<ast::MethodDecl> {
        lookup_type_method_decl_inner(module_path, type_name, method_name, &mut HashSet::new())
    }

    /// Look up all stdlib function declarations for a source name, preserving overload order.
    ///
    /// This preserves source-level default expressions for lowering and emission. `lookup_function_symbol`
    /// intentionally returns compact type metadata and only records whether a parameter has a default.
    pub(crate) fn lookup_function_decls(
        &mut self,
        module_path: &[String],
        function_name: &str,
    ) -> Vec<ast::FunctionDecl> {
        self.ensure_loaded(module_path);
        let key = module_path.join(".");
        self.cache
            .get(&key)
            .into_iter()
            .flat_map(|data| &data.functions)
            .filter(|entry| entry.name == function_name)
            .filter_map(|entry| entry.declaration.clone())
            .collect()
    }

    /// Look up the first stdlib function declaration for legacy lowering paths that select overloads separately.
    pub(crate) fn lookup_function_decl(
        &mut self,
        module_path: &[String],
        function_name: &str,
    ) -> Option<ast::FunctionDecl> {
        self.lookup_function_decls(module_path, function_name)
            .into_iter()
            .next()
    }

    /// Look up a stdlib trait declaration, following prelude re-exports.
    pub(crate) fn lookup_trait_decl(&mut self, module_path: &[String], trait_name: &str) -> Option<ast::TraitDecl> {
        self.ensure_loaded(module_path);
        let key = module_path.join(".");
        self.cache
            .get(&key)?
            .trait_declarations
            .iter()
            .find(|(name, _)| name == trait_name)
            .map(|(_, declaration)| declaration.clone())
    }

    /// Return one cached snapshot of public declarations, signatures, and docs for LSP rendering.
    #[cfg(feature = "lsp")]
    pub(crate) fn lsp_metadata(&mut self, module_path: &[String]) -> StdlibModuleLspMetadata {
        self.ensure_loaded(module_path);
        let key = module_path.join(".");
        let Some(data) = self.cache.get(&key) else {
            return StdlibModuleLspMetadata::default();
        };
        StdlibModuleLspMetadata {
            functions: data
                .functions
                .iter()
                .map(|entry| StdlibFunctionLspMetadata {
                    name: entry.name.clone(),
                    info: entry.info.clone(),
                    declaration: entry.declaration.clone(),
                })
                .collect(),
            traits: data.traits.clone(),
            trait_declarations: data.trait_declarations.clone(),
            types: data.types.clone(),
            type_docstrings: data.type_docstrings.clone(),
        }
    }

    /// List public type signatures in a stdlib module.
    pub fn list_types(&mut self, module_path: &[String]) -> Vec<(String, TypeInfo)> {
        self.ensure_loaded(module_path);
        let key = module_path.join(".");
        self.cache.get(&key).map(|data| data.types.clone()).unwrap_or_default()
    }

    /// List public trait signatures in a stdlib module.
    pub fn list_traits(&mut self, module_path: &[String]) -> Vec<(String, TraitInfo)> {
        self.ensure_loaded(module_path);
        let key = module_path.join(".");
        self.cache.get(&key).map(|data| data.traits.clone()).unwrap_or_default()
    }

    /// Look up a specific const binding in a stdlib module.
    pub fn lookup_constant(&mut self, module_path: &[String], const_name: &str) -> Option<VariableInfo> {
        self.ensure_loaded(module_path);
        let key = module_path.join(".");
        self.cache
            .get(&key)?
            .constants
            .iter()
            .find(|(name, _)| name == const_name)
            .map(|(_, info)| info.clone())
    }

    /// Look up a specific static binding in a stdlib module.
    pub fn lookup_static(&mut self, module_path: &[String], static_name: &str) -> Option<StaticInfo> {
        self.ensure_loaded(module_path);
        let key = module_path.join(".");
        self.cache
            .get(&key)?
            .statics
            .iter()
            .find(|(name, _)| name == static_name)
            .map(|(_, info)| info.clone())
    }

    /// Look up metadata for a specific function in a stdlib module.
    pub fn lookup_function_meta(&mut self, module_path: &[String], function_name: &str) -> Option<FunctionMeta> {
        self.ensure_loaded(module_path);
        let key = module_path.join(".");
        self.cache.get(&key)?.function_meta.get(function_name).cloned()
    }

    /// Look up metadata for a specific trait in a stdlib module.
    pub fn lookup_trait_meta(&mut self, module_path: &[String], trait_name: &str) -> Option<TraitMeta> {
        self.ensure_loaded(module_path);
        let key = module_path.join(".");
        self.cache.get(&key)?.trait_meta.get(trait_name).cloned()
    }

    /// Return the traits listed by a module-level `__derives__ = [...]` declaration.
    pub fn lookup_derivable_traits(&mut self, module_path: &[String]) -> Option<Vec<String>> {
        self.ensure_loaded(module_path);
        let key = module_path.join(".");
        let traits = &self.cache.get(&key)?.derivable_traits;
        (!traits.is_empty()).then(|| traits.clone())
    }

    /// Return the already-loaded stdlib module path that exports `trait_name`, if known.
    ///
    /// This intentionally scans only cached modules. Callers use it after ordinary import/type lookup has loaded the
    /// relevant stdlib module, avoiding a broad filesystem scan from the typechecker hot path.
    pub fn loaded_trait_module_path(&self, trait_name: &str) -> Option<Vec<String>> {
        self.cache.iter().find_map(|(module_path, data)| {
            data.traits
                .iter()
                .any(|(name, _)| name == trait_name)
                .then(|| module_path.split('.').map(str::to_string).collect())
        })
    }

    /// Ensure a module is loaded into the cache, loading it on first access.
    fn ensure_loaded(&mut self, module_path: &[String]) {
        let key = module_path.join(".");
        self.cache
            .entry(key)
            .or_insert_with(|| load_stdlib_module_data(module_path).unwrap_or_default());
    }
}

/// Load and parse a stdlib `.incn` file, extracting module data.
///
/// For modules with submodules (whose stub path resolves to a prelude file), this function also follows
/// `from std.<ns>.<submodule> import <name>` re-exports: it loads each referenced submodule and merges
/// the imported names' metadata into the parent module. This enables `from std.web import route` to resolve
/// decorator metadata even though `route` is declared in `std.web.routing`, not the prelude itself.
///
/// Returns `None` if the file cannot be found or parsed.
fn load_stdlib_module_data(module_path: &[String]) -> Option<StdlibModuleData> {
    load_stdlib_module_data_inner(module_path, &mut HashSet::new())
}

/// Load one stdlib module while tracking the current re-export chain.
///
/// Returns `None` for recursive re-entry so cyclic stdlib preludes cannot overflow the loader stack.
fn load_stdlib_module_data_inner(module_path: &[String], loading: &mut HashSet<String>) -> Option<StdlibModuleData> {
    let key = module_path.join(".");
    if !loading.insert(key.clone()) {
        return None;
    }

    let data = load_stdlib_module_data_unguarded(module_path, loading);
    loading.remove(&key);
    data
}

/// Load one stdlib module without inserting it into the active-cycle guard.
///
/// Call through `load_stdlib_module_data_inner` unless the caller has already marked `module_path` as in progress.
fn load_stdlib_module_data_unguarded(
    module_path: &[String],
    loading: &mut HashSet<String>,
) -> Option<StdlibModuleData> {
    let relative = stdlib::stdlib_stub_path(module_path)?;
    let abs_path = find_stdlib_file(&relative)?;

    let source = std::fs::read_to_string(&abs_path)
        .map_err(|e| {
            tracing::debug!(path = %abs_path.display(), error = %e, "failed to read stdlib file");
        })
        .ok()?;

    let tokens = crate::frontend::lexer::lex(&source)
        .map_err(|e| {
            tracing::debug!(path = %abs_path.display(), error = ?e, "failed to lex stdlib file");
        })
        .ok()?;

    let program = crate::frontend::parser::parse(&tokens)
        .map_err(|e| {
            tracing::debug!(path = %abs_path.display(), error = ?e, "failed to parse stdlib file");
        })
        .ok()?;

    let mut functions = extract_function_entries(&program);
    assign_function_overload_emitted_names(&mut functions);
    let mut traits = extract_trait_signatures(&program);
    let mut trait_declarations = extract_trait_declarations(&program);
    let mut types = extract_type_signatures(&program);
    let mut type_docstrings = extract_type_docstrings(&program);
    let mut constants = extract_const_signatures(&program);
    let mut statics = extract_static_signatures(&program);
    let mut function_meta = extract_function_meta(&program);
    let mut trait_meta = extract_trait_meta(&program);

    // ---- Follow prelude re-exports ----
    // If this module is a prelude (has submodules), its declarations are just `from ... import ...` re-exports.
    // We recursively load each referenced submodule and merge the imported names' metadata so that
    // `lookup_function_meta(["std", "web"], "route")` finds `route` even though it's declared in
    // `std.web.routing`.
    let mut reexport_targets = ReexportMetadataTargets {
        functions: &mut functions,
        traits: &mut traits,
        trait_declarations: &mut trait_declarations,
        types: &mut types,
        type_docstrings: &mut type_docstrings,
        constants: &mut constants,
        statics: &mut statics,
        function_meta: &mut function_meta,
        trait_meta: &mut trait_meta,
    };
    merge_reexported_metadata(module_path, &program, &mut reexport_targets, loading);

    Some(StdlibModuleData {
        functions,
        traits,
        trait_declarations,
        types,
        type_docstrings,
        constants,
        statics,
        derivable_traits: extract_derivable_traits(&program),
        function_meta,
        trait_meta,
    })
}

struct ReexportMetadataTargets<'a> {
    functions: &'a mut Vec<StdlibFunctionEntry>,
    traits: &'a mut Vec<(String, TraitInfo)>,
    trait_declarations: &'a mut Vec<(String, ast::TraitDecl)>,
    types: &'a mut Vec<(String, TypeInfo)>,
    type_docstrings: &'a mut HashMap<String, String>,
    constants: &'a mut Vec<(String, VariableInfo)>,
    statics: &'a mut Vec<(String, StaticInfo)>,
    function_meta: &'a mut HashMap<String, FunctionMeta>,
    trait_meta: &'a mut HashMap<String, TraitMeta>,
}

/// Scan a program's import declarations and merge metadata from referenced stdlib submodules.
///
/// For each `from std... import name1, name2` statement, loads the referenced module and copies the
/// corresponding function/trait signatures and metadata into the parent module's collections.
fn merge_reexported_metadata(
    current_module_path: &[String],
    program: &ast::Program,
    targets: &mut ReexportMetadataTargets<'_>,
    loading: &mut HashSet<String>,
) {
    for decl in &program.declarations {
        let ast::Declaration::Import(import) = &decl.node else {
            continue;
        };
        let ast::ImportKind::From { module, items } = &import.kind else {
            continue;
        };

        // Only follow stdlib re-exports (paths starting with "std").
        if module.segments.first().is_none_or(|s| s != stdlib::STDLIB_ROOT) {
            continue;
        }
        if module.segments.len() < 2 {
            continue;
        }
        if module.segments == [stdlib::STDLIB_ROOT] {
            continue;
        }
        if current_module_path == module.segments.as_slice() {
            continue;
        }
        if !is_stdlib_metadata_reexport(current_module_path, program, import) {
            continue;
        }

        let Some(sub_data) = load_stdlib_module_data_inner(&module.segments, loading) else {
            continue;
        };

        for item in items {
            let effective_name = item.alias.as_deref().unwrap_or(&item.name);

            // Merge every function overload and its matching source declaration under the re-exported source name.
            for entry in sub_data.functions.iter().filter(|entry| entry.name == item.name) {
                let already_present = targets.functions.iter().any(|existing| {
                    existing.name == effective_name && existing.info.emitted_name == entry.info.emitted_name
                });
                if !already_present {
                    let mut entry = entry.clone();
                    entry.name = effective_name.to_string();
                    targets.functions.push(entry);
                }
            }

            // Merge trait signature.
            if let Some((_, info)) = sub_data.traits.iter().find(|(n, _)| n == &item.name)
                && !targets.traits.iter().any(|(n, _)| n == effective_name)
            {
                targets.traits.push((effective_name.to_string(), info.clone()));
            }
            if let Some((_, declaration)) = sub_data.trait_declarations.iter().find(|(name, _)| name == &item.name)
                && !targets
                    .trait_declarations
                    .iter()
                    .any(|(name, _)| name == effective_name)
            {
                targets
                    .trait_declarations
                    .push((effective_name.to_string(), declaration.clone()));
            }

            // Merge type signature.
            if let Some((_, info)) = sub_data.types.iter().find(|(n, _)| n == &item.name)
                && !targets.types.iter().any(|(n, _)| n == effective_name)
            {
                targets.types.push((effective_name.to_string(), info.clone()));
            }
            if let Some(docstring) = sub_data.type_docstrings.get(&item.name) {
                targets
                    .type_docstrings
                    .entry(effective_name.to_string())
                    .or_insert_with(|| docstring.clone());
            }

            // Merge const signature.
            if let Some((_, info)) = sub_data.constants.iter().find(|(n, _)| n == &item.name)
                && !targets.constants.iter().any(|(n, _)| n == effective_name)
            {
                targets.constants.push((effective_name.to_string(), info.clone()));
            }

            // Merge static signature.
            if let Some((_, info)) = sub_data.statics.iter().find(|(n, _)| n == &item.name)
                && !targets.statics.iter().any(|(n, _)| n == effective_name)
            {
                targets.statics.push((effective_name.to_string(), info.clone()));
            }

            // Merge function meta.
            if let Some(meta) = sub_data.function_meta.get(&item.name) {
                targets
                    .function_meta
                    .entry(effective_name.to_string())
                    .or_insert_with(|| meta.clone());
            }

            // Merge trait meta.
            if let Some(meta) = sub_data.trait_meta.get(&item.name) {
                targets
                    .trait_meta
                    .entry(effective_name.to_string())
                    .or_insert_with(|| meta.clone());
            }
        }
    }
}

/// Return whether a stdlib import should contribute public metadata to the containing module.
///
/// Existing stdlib prelude files are facade modules, so their imports define public module surface. Ordinary stdlib
/// implementation modules should only expose explicit `pub from ... import ...` re-exports; otherwise private
/// dependencies such as `std.io` importing `std.traits.error.Error` leak as user-visible stdlib members.
fn is_stdlib_metadata_reexport(
    current_module_path: &[String],
    program: &ast::Program,
    import: &ast::ImportDecl,
) -> bool {
    import.visibility == ast::Visibility::Public
        || stdlib_module_uses_prelude_stub(current_module_path)
        || stdlib_module_is_import_facade(program)
}

/// Return whether the stdlib registry resolves this module to a `prelude.incn` facade.
fn stdlib_module_uses_prelude_stub(module_path: &[String]) -> bool {
    stdlib::stdlib_stub_path(module_path)
        .is_some_and(|path| path.ends_with("/prelude.incn") || path == "stdlib/prelude.incn")
}

/// Return whether a stdlib module is only an import facade.
///
/// Some public modules, such as `std.datetime.civil`, are real `.incn` files rather than `prelude.incn` registry
/// stubs, but they still exist solely to aggregate submodules. Treating these import-only files as facades preserves
/// that public surface without letting ordinary implementation modules leak their private dependencies.
fn stdlib_module_is_import_facade(program: &ast::Program) -> bool {
    program
        .declarations
        .iter()
        .all(|decl| matches!(decl.node, ast::Declaration::Docstring(_) | ast::Declaration::Import(_)))
}

/// Find the absolute path for a stdlib file given its relative path (e.g. `"stdlib/testing.incn"`).
///
/// Search order:
/// 1. `$INCAN_STDLIB_DIR/<relative>` if the env var is set
/// 2. compile-time workspace path: `$CARGO_MANIFEST_DIR/crates/incan_stdlib/<relative>`
/// 3. compile-time workspace path: `$CARGO_MANIFEST_DIR/<relative>`
/// 4. paths relative to current executable (repo and install layouts)
/// 5. `$CWD/crates/incan_stdlib/<relative>`
/// 6. `$CWD/<relative>`
/// 7. `$INCAN_STDLIB_PATH/<relative>` for installed layouts
fn find_stdlib_file(relative: &str) -> Option<PathBuf> {
    // 1. Explicit override root.
    if let Ok(dir) = std::env::var("INCAN_STDLIB_DIR") {
        let p = PathBuf::from(dir).join(relative);
        if p.exists() {
            return Some(p);
        }
    }

    // 2-3. Compile-time workspace paths (reliable in dev and local source builds).
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let crate_local = manifest_dir.join("crates/incan_stdlib").join(relative);
    if crate_local.exists() {
        return Some(crate_local);
    }
    let workspace_local = manifest_dir.join(relative);
    if workspace_local.exists() {
        return Some(workspace_local);
    }

    // 4. Relative to executable location, including installed symlink launchers.
    for base in crate::toolchain_layout::current_executable_search_bases() {
        let candidate_crate_local = base.join("crates/incan_stdlib").join(relative);
        if candidate_crate_local.exists() {
            return Some(candidate_crate_local);
        }
        let candidate_local = base.join(relative);
        if candidate_local.exists() {
            return Some(candidate_local);
        }
    }

    // 5-6. Relative to current working directory.
    if let Ok(cwd) = std::env::current_dir() {
        let crate_local = cwd.join("crates/incan_stdlib").join(relative);
        if crate_local.exists() {
            return Some(crate_local);
        }
        let local = cwd.join(relative);
        if local.exists() {
            return Some(local);
        }
    }

    // 7. Installed stdlib path (runtime, for production installs).
    if let Ok(stdlib_root) = std::env::var("INCAN_STDLIB_PATH") {
        let installed_path = PathBuf::from(stdlib_root).join(relative);
        if installed_path.exists() {
            return Some(installed_path);
        }
    }

    tracing::debug!(relative_path = %relative, "stdlib file not found in any search path");
    None
}

/// Find a method declaration on a stdlib type, following prelude-style re-exports.
fn lookup_type_method_decl_inner(
    module_path: &[String],
    type_name: &str,
    method_name: &str,
    loading: &mut HashSet<String>,
) -> Option<ast::MethodDecl> {
    let key = module_path.join(".");
    if !loading.insert(key.clone()) {
        return None;
    }
    let result = load_stdlib_program(module_path).and_then(|program| {
        find_method_decl_in_program(&program, type_name, method_name)
            .or_else(|| find_reexported_type_method_decl(module_path, &program, type_name, method_name, loading))
    });
    loading.remove(&key);
    result
}

/// Parse a stdlib stub module into an AST program for metadata lookups that need source expressions.
fn load_stdlib_program(module_path: &[String]) -> Option<ast::Program> {
    let relative = match stdlib::stdlib_stub_path(module_path) {
        Some(path) => path,
        None => {
            tracing::warn!("Stdlib module not found: {}", module_path.join("::"));
            return None;
        }
    };
    let path = match find_stdlib_file(&relative) {
        Some(path) => path,
        None => {
            tracing::warn!("Failed to find stdlib file: {}", relative);
            return None;
        }
    };
    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(e) => {
            tracing::warn!("Failed to read stdlib file {}: {}", relative, e);
            return None;
        }
    };
    let tokens = match crate::frontend::lexer::lex(&source) {
        Ok(tokens) => tokens,
        Err(e) => {
            tracing::warn!("Lexer error in stdlib {}: {:?}", relative, e);
            return None;
        }
    };
    let ast = match crate::frontend::parser::parse(&tokens) {
        Ok(ast) => ast,
        Err(e) => {
            tracing::warn!("Parser error in stdlib {}: {:?}", relative, e);
            return None;
        }
    };
    Some(ast)
}

/// Find a method declaration directly in a parsed stdlib program.
fn find_method_decl_in_program(program: &ast::Program, type_name: &str, method_name: &str) -> Option<ast::MethodDecl> {
    program.declarations.iter().find_map(|decl| match &decl.node {
        ast::Declaration::Model(model) if model.name == type_name => find_method_decl(&model.methods, method_name),
        ast::Declaration::Class(class) if class.name == type_name => find_method_decl(&class.methods, method_name),
        ast::Declaration::Newtype(newtype) if newtype.name == type_name => {
            find_method_decl(&newtype.methods, method_name)
        }
        ast::Declaration::Enum(enum_decl) if enum_decl.name == type_name => {
            find_method_decl(&enum_decl.methods, method_name)
        }
        _ => None,
    })
}

/// Find one named method in a type declaration's method list.
fn find_method_decl(methods: &[ast::Spanned<ast::MethodDecl>], method_name: &str) -> Option<ast::MethodDecl> {
    methods
        .iter()
        .find(|method| method.node.name == method_name)
        .map(|method| method.node.clone())
}

/// Follow stdlib `from std.x.y import Type` re-exports while searching for the owning type declaration.
fn find_reexported_type_method_decl(
    current_module_path: &[String],
    program: &ast::Program,
    type_name: &str,
    method_name: &str,
    loading: &mut HashSet<String>,
) -> Option<ast::MethodDecl> {
    program.declarations.iter().find_map(|decl| {
        let ast::Declaration::Import(import) = &decl.node else {
            return None;
        };
        let ast::ImportKind::From { module, items } = &import.kind else {
            return None;
        };
        if module.segments.first().map(String::as_str) != Some(stdlib::STDLIB_ROOT) {
            return None;
        }
        if !is_stdlib_metadata_reexport(current_module_path, program, import) {
            return None;
        }
        items.iter().find_map(|item| {
            let effective_name = item.alias.as_ref().unwrap_or(&item.name);
            if effective_name != type_name {
                return None;
            }
            lookup_type_method_decl_inner(&module.segments, &item.name, method_name, loading)
        })
    })
}

/// Extract paired function metadata from a parsed stdlib `.incn` program.
///
/// Only top-level `def` declarations are extracted. Methods and other declarations are ignored.
fn extract_function_entries(program: &ast::Program) -> Vec<StdlibFunctionEntry> {
    let mut fns = Vec::new();
    let stdlib_imports = stdlib_import_aliases(program);
    for decl in &program.declarations {
        if let ast::Declaration::Function(func) = &decl.node {
            if !matches!(func.visibility, ast::Visibility::Public) {
                continue;
            }
            let info = function_decl_to_info(func, &stdlib_imports);
            fns.push(StdlibFunctionEntry {
                name: func.name.clone(),
                info,
                declaration: Some(func.clone()),
            });
            continue;
        }

        if let ast::Declaration::Import(import) = &decl.node {
            let ast::ImportKind::RustFrom { items, .. } = &import.kind else {
                continue;
            };
            for item in items {
                let local_name = item.alias.as_deref().unwrap_or(&item.name);
                if let Some(info) = imported_runtime_function_info(local_name)
                    && !fns.iter().any(|entry| entry.name == local_name)
                {
                    fns.push(StdlibFunctionEntry {
                        name: local_name.to_string(),
                        info,
                        declaration: None,
                    });
                }
            }
        }
    }
    fns
}

/// Assign provider Rust names to every member of a same-name source overload set.
fn assign_function_overload_emitted_names(functions: &mut [StdlibFunctionEntry]) {
    let mut counts = HashMap::<String, usize>::new();
    for entry in functions.iter() {
        *counts.entry(entry.name.clone()).or_default() += 1;
    }
    for entry in functions {
        if counts.get(entry.name.as_str()).copied().unwrap_or_default() > 1 {
            entry.info.emitted_name = Some(overloaded_function_emitted_name(&entry.name, &entry.info));
        }
    }
}

/// Extract public const bindings from a parsed stdlib `.incn` program.
fn extract_const_signatures(program: &ast::Program) -> Vec<(String, VariableInfo)> {
    let mut consts = Vec::new();
    for decl in &program.declarations {
        let ast::Declaration::Const(konst) = &decl.node else {
            continue;
        };
        if konst.name == "__derives__" {
            continue;
        }
        let ty = konst
            .ty
            .as_ref()
            .map(|ty| ast_type_to_resolved(&ty.node, &[]))
            .unwrap_or(ResolvedType::Unknown);
        consts.push((
            konst.name.clone(),
            VariableInfo {
                ty,
                is_mutable: false,
                is_used: false,
            },
        ));
    }
    consts
}

/// Extract public static bindings from a parsed stdlib `.incn` program.
fn extract_static_signatures(program: &ast::Program) -> Vec<(String, StaticInfo)> {
    let mut statics = Vec::new();
    for decl in &program.declarations {
        let ast::Declaration::Static(static_decl) = &decl.node else {
            continue;
        };
        if !matches!(static_decl.visibility, ast::Visibility::Public) {
            continue;
        }
        let ty = ast_type_to_resolved(&static_decl.ty.node, &[]);
        statics.push((
            static_decl.name.clone(),
            StaticInfo {
                ty,
                is_public: true,
                is_imported: true,
                is_used: false,
            },
        ));
    }
    statics
}

/// Extract RFC 024 module-level derivable trait declarations.
fn extract_derivable_traits(program: &ast::Program) -> Vec<String> {
    for decl in &program.declarations {
        let ast::Declaration::Const(konst) = &decl.node else {
            continue;
        };
        if konst.name != "__derives__" {
            continue;
        }
        let ast::Expr::List(entries) = &konst.value.node else {
            return Vec::new();
        };
        return entries
            .iter()
            .filter_map(|entry| match entry {
                ast::ListEntry::Element(expr) => match &expr.node {
                    ast::Expr::Ident(name) => Some(name.clone()),
                    _ => None,
                },
                ast::ListEntry::Spread(_) => None,
            })
            .collect();
    }
    Vec::new()
}

/// Map one `with` supertrait bound to `(trait_name, type_arguments)` for stdlib trait metadata (RFC 042).
///
/// Uses the declaring trait's type parameter names so bounds like `DataSet[T]` become generic supertrait entries with
/// [`ResolvedType::TypeVar`] arguments. Bounds that do not resolve to a plain trait name or generic trait application
/// are skipped (malformed stdlib sources are treated as having no edge for that bound).
fn supertrait_entry_from_trait_bound(
    bound: &ast::TraitBound,
    declaring_trait_type_params: &[String],
) -> Option<(String, Vec<ResolvedType>)> {
    let ty = if bound.type_args.is_empty() {
        ast::Type::Simple(bound.name.clone())
    } else {
        ast::Type::Generic(bound.name.clone(), bound.type_args.clone())
    };
    match ast_type_to_resolved(&ty, declaring_trait_type_params) {
        ResolvedType::Named(n) => Some((n, Vec::new())),
        ResolvedType::Generic(n, args) => Some((n, args)),
        _ => None,
    }
}

/// Extract trait signatures from a parsed stdlib `.incn` program.
///
/// Top-level `trait` declarations are extracted with their method signatures and `with` supertrait bounds. `@requires`
/// decorators are not resolved (`requires` stays empty) since stdlib traits typically don't use them.
fn extract_trait_signatures(program: &ast::Program) -> Vec<(String, TraitInfo)> {
    let mut traits = Vec::new();
    for decl in &program.declarations {
        if let ast::Declaration::Trait(tr) = &decl.node {
            let tp_names: Vec<String> = tr.type_params.iter().map(|tp| tp.name.clone()).collect();
            let mut method_overloads =
                extract_method_overloads_with_rust_imports(&tr.methods, &tp_names, &HashMap::new(), &HashMap::new());
            let mut methods = methods_from_overloads(&method_overloads);
            let method_aliases = apply_method_aliases(&tr.method_aliases, &mut methods, &mut method_overloads);
            let supertraits: Vec<(String, Vec<ResolvedType>)> = tr
                .traits
                .iter()
                .filter_map(|b| supertrait_entry_from_trait_bound(&b.node, &tp_names))
                .collect();
            traits.push((
                tr.name.clone(),
                TraitInfo {
                    type_params: tp_names,
                    supertraits,
                    methods,
                    method_aliases,
                    properties: std::collections::HashMap::new(),
                    requires: Vec::new(),
                },
            ));
        }
    }
    traits
}

/// Preserve source trait declarations alongside their resolved signatures.
fn extract_trait_declarations(program: &ast::Program) -> Vec<(String, ast::TraitDecl)> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| match &declaration.node {
            ast::Declaration::Trait(trait_decl) => Some((trait_decl.name.clone(), trait_decl.clone())),
            _ => None,
        })
        .collect()
}

/// Extract public type metadata from a parsed stdlib `.incn` program.
///
/// Stdlib imports are source-visible type imports, not just function imports. Keeping type metadata here lets generic
/// rusttypes such as `TimeoutJoinOutcome[T]` retain their Rust backing when imported from `std.async.time`.
fn extract_type_signatures(program: &ast::Program) -> Vec<(String, TypeInfo)> {
    let rust_imports = rust_import_aliases(program);
    let stdlib_imports = stdlib_import_aliases(program);
    let mut types = Vec::new();
    for decl in &program.declarations {
        match &decl.node {
            ast::Declaration::Model(model) if model.visibility == ast::Visibility::Public => {
                let tp_names = type_param_names(&model.type_params);
                let mut method_overloads = extract_method_overloads_with_rust_imports(
                    &model.methods,
                    &tp_names,
                    &rust_imports,
                    &stdlib_imports,
                );
                let mut methods = methods_from_overloads(&method_overloads);
                let method_aliases = apply_method_aliases(&model.method_aliases, &mut methods, &mut method_overloads);
                types.push((
                    model.name.clone(),
                    TypeInfo::Model(ModelInfo {
                        type_params: tp_names.clone(),
                        traits: model.traits.iter().map(|bound| bound.node.name.clone()).collect(),
                        trait_adoptions: trait_adoption_infos_from_bounds(&model.traits, &tp_names, &stdlib_imports),
                        derives: Vec::new(),
                        fields: extract_field_signatures(&model.name, &model.fields, &tp_names, &rust_imports),
                        field_order: model.fields.iter().map(|field| field.node.name.clone()).collect(),
                        properties: std::collections::HashMap::new(),
                        method_overloads,
                        methods,
                        method_aliases,
                    }),
                ));
            }
            ast::Declaration::Class(class) if class.visibility == ast::Visibility::Public => {
                let tp_names = type_param_names(&class.type_params);
                let mut method_overloads = extract_method_overloads_with_rust_imports(
                    &class.methods,
                    &tp_names,
                    &rust_imports,
                    &stdlib_imports,
                );
                let mut methods = methods_from_overloads(&method_overloads);
                let method_aliases = apply_method_aliases(&class.method_aliases, &mut methods, &mut method_overloads);
                types.push((
                    class.name.clone(),
                    TypeInfo::Class(ClassInfo {
                        type_params: tp_names.clone(),
                        extends: class.extends.clone(),
                        traits: class.traits.iter().map(|bound| bound.node.name.clone()).collect(),
                        trait_adoptions: trait_adoption_infos_from_bounds(&class.traits, &tp_names, &stdlib_imports),
                        derives: Vec::new(),
                        fields: extract_field_signatures(&class.name, &class.fields, &tp_names, &rust_imports),
                        field_order: class.fields.iter().map(|field| field.node.name.clone()).collect(),
                        properties: std::collections::HashMap::new(),
                        method_overloads,
                        methods,
                        method_aliases,
                    }),
                ));
            }
            ast::Declaration::TypeAlias(alias) if alias.visibility == ast::Visibility::Public => {
                types.push((alias.name.clone(), TypeInfo::TypeAlias));
            }
            ast::Declaration::Newtype(nt) if nt.visibility == ast::Visibility::Public => {
                let tp_names = type_param_names(&nt.type_params);
                let underlying = ast_type_to_resolved_with_rust_imports(&nt.underlying.node, &tp_names, &rust_imports);
                let method_rebindings = nt
                    .rebindings
                    .iter()
                    .filter_map(|rebinding| {
                        rebinding_target_method_name(&rebinding.node.target.node)
                            .map(|target| (rebinding.node.name.clone(), target))
                    })
                    .collect();
                let mut method_overloads =
                    extract_method_overloads_with_rust_imports(&nt.methods, &tp_names, &rust_imports, &stdlib_imports);
                let mut methods = methods_from_overloads(&method_overloads);
                let method_aliases = apply_method_aliases(&nt.method_aliases, &mut methods, &mut method_overloads);
                types.push((
                    nt.name.clone(),
                    TypeInfo::Newtype(NewtypeInfo {
                        type_params: tp_names.clone(),
                        is_rusttype: nt.is_rusttype,
                        has_interop: !nt.interop_edges.is_empty(),
                        underlying,
                        constraints: Vec::new(),
                        implicit_coercion_enabled: true,
                        method_rebindings,
                        traits: nt.traits.iter().map(|trait_ref| trait_ref.node.name.clone()).collect(),
                        trait_adoptions: trait_adoption_infos_from_bounds(&nt.traits, &tp_names, &stdlib_imports),
                        method_aliases,
                        methods,
                        method_overloads,
                    }),
                ));
            }
            ast::Declaration::Enum(en) if en.visibility == ast::Visibility::Public => {
                let tp_names = type_param_names(&en.type_params);
                let method_overloads =
                    extract_method_overloads_with_rust_imports(&en.methods, &tp_names, &rust_imports, &stdlib_imports);
                let methods = methods_from_overloads(&method_overloads);
                let variant_fields = en
                    .variants
                    .iter()
                    .map(|variant| {
                        let fields = variant
                            .node
                            .fields
                            .iter()
                            .map(|field| ast_type_to_resolved_with_rust_imports(&field.node, &tp_names, &rust_imports))
                            .collect();
                        (variant.node.name.clone(), fields)
                    })
                    .collect();
                types.push((
                    en.name.clone(),
                    TypeInfo::Enum(EnumInfo {
                        type_params: tp_names.clone(),
                        traits: en.traits.iter().map(|t| t.node.name.clone()).collect(),
                        trait_adoptions: trait_adoption_infos_from_bounds(&en.traits, &tp_names, &stdlib_imports),
                        variants: en.variants.iter().map(|variant| variant.node.name.clone()).collect(),
                        variant_fields,
                        variant_aliases: en
                            .variant_aliases
                            .iter()
                            .map(|alias| (alias.node.name.clone(), alias.node.target.clone()))
                            .collect(),
                        value_enum: None,
                        derives: Vec::new(),
                        method_overloads,
                        methods,
                    }),
                ));
            }
            _ => {}
        }
    }
    types
}

/// Extract public type docstrings during the module's original parse.
fn extract_type_docstrings(program: &ast::Program) -> HashMap<String, String> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| match &declaration.node {
            ast::Declaration::Model(model) if model.visibility == ast::Visibility::Public => {
                model.docstring.clone().map(|docstring| (model.name.clone(), docstring))
            }
            ast::Declaration::Class(class) if class.visibility == ast::Visibility::Public => {
                class.docstring.clone().map(|docstring| (class.name.clone(), docstring))
            }
            ast::Declaration::Newtype(newtype) if newtype.visibility == ast::Visibility::Public => newtype
                .docstring
                .clone()
                .map(|docstring| (newtype.name.clone(), docstring)),
            ast::Declaration::Enum(enum_decl) if enum_decl.visibility == ast::Visibility::Public => enum_decl
                .docstring
                .clone()
                .map(|docstring| (enum_decl.name.clone(), docstring)),
            _ => None,
        })
        .collect()
}

/// Extract just the declared names from AST type parameters.
fn type_param_names(type_params: &[ast::TypeParam]) -> Vec<String> {
    type_params.iter().map(|tp| tp.name.clone()).collect()
}

/// Convert AST field declarations into typechecker field metadata.
fn extract_field_signatures(
    owner: &str,
    fields: &[ast::Spanned<ast::FieldDecl>],
    type_params: &[String],
    rust_imports: &HashMap<String, String>,
) -> HashMap<String, FieldInfo> {
    fields
        .iter()
        .map(|field| {
            (
                field.node.name.clone(),
                FieldInfo {
                    ty: ast_type_to_resolved_with_rust_imports(&field.node.ty.node, type_params, rust_imports),
                    visibility: field.node.visibility,
                    owner: Some(owner.to_string()),
                    has_default: field.node.default.is_some(),
                    alias: field.node.metadata.alias.clone(),
                    description: field.node.metadata.description.clone(),
                },
            )
        })
        .collect()
}

/// Extract the effective Rust method name from a rusttype member rebinding target.
fn rebinding_target_method_name(target: &ast::Expr) -> Option<String> {
    match target {
        ast::Expr::Ident(name) => Some(name.clone()),
        ast::Expr::Field(_, member) => Some(member.clone()),
        _ => None,
    }
}

/// Convert stdlib `with` bounds into trait adoption metadata with resolved generic arguments.
fn trait_adoption_infos_from_bounds(
    bounds: &[ast::Spanned<ast::TraitBound>],
    type_params: &[String],
    stdlib_imports: &HashMap<String, Vec<String>>,
) -> Vec<TypeBoundInfo> {
    bounds
        .iter()
        .map(|bound| TypeBoundInfo {
            name: bound.node.name.clone(),
            source_name: None,
            type_args: bound
                .node
                .type_args
                .iter()
                .map(|arg| ast_type_to_resolved(&arg.node, type_params))
                .collect(),
            module_path: stdlib_imports.get(&bound.node.name).cloned(),
        })
        .collect()
}

/// Extract method metadata grouped by source name, preserving same-name trait-backed overloads.
fn extract_method_overloads_with_rust_imports(
    methods: &[ast::Spanned<ast::MethodDecl>],
    type_params: &[String],
    rust_imports: &HashMap<String, String>,
    stdlib_imports: &HashMap<String, Vec<String>>,
) -> HashMap<String, Vec<MethodInfo>> {
    let mut overloads: HashMap<String, Vec<MethodInfo>> = HashMap::new();
    for method in methods {
        overloads
            .entry(method.node.name.clone())
            .or_default()
            .push(method_info_from_ast_method(
                &method.node,
                type_params,
                rust_imports,
                stdlib_imports,
            ));
    }
    overloads
}

/// Collapse overload groups into the legacy single-method map for non-overload call paths.
fn methods_from_overloads(method_overloads: &HashMap<String, Vec<MethodInfo>>) -> HashMap<String, MethodInfo> {
    method_overloads
        .iter()
        .filter_map(|(name, methods)| methods.last().cloned().map(|method| (name.clone(), method)))
        .collect()
}

/// Project same-type method aliases into stdlib import metadata.
fn apply_method_aliases(
    aliases: &[ast::Spanned<ast::MethodAliasDecl>],
    methods: &mut HashMap<String, MethodInfo>,
    overloads: &mut HashMap<String, Vec<MethodInfo>>,
) -> HashMap<String, String> {
    let mut method_aliases = HashMap::new();
    for alias in aliases {
        let target = alias.node.target.clone();
        method_aliases.insert(alias.node.name.clone(), target.clone());

        if let Some(target_overloads) = overloads.get(&target).cloned() {
            let alias_overloads: Vec<_> = target_overloads
                .into_iter()
                .map(|mut info| {
                    info.alias_of = Some(target.clone());
                    info
                })
                .collect();
            if let Some(last) = alias_overloads.last().cloned() {
                methods.insert(alias.node.name.clone(), last);
            }
            overloads.insert(alias.node.name.clone(), alias_overloads);
        } else if let Some(mut info) = methods.get(&target).cloned() {
            info.alias_of = Some(target.clone());
            methods.insert(alias.node.name.clone(), info.clone());
            overloads.insert(alias.node.name.clone(), vec![info]);
        }
    }
    method_aliases
}

/// Convert one AST method declaration into lightweight semantic method metadata.
fn method_info_from_ast_method(
    method: &ast::MethodDecl,
    type_params: &[String],
    rust_imports: &HashMap<String, String>,
    stdlib_imports: &HashMap<String, Vec<String>>,
) -> MethodInfo {
    let method_type_params: Vec<String> = method.type_params.iter().map(|tp| tp.name.clone()).collect();
    let method_type_param_bounds: HashMap<String, Vec<String>> = method
        .type_params
        .iter()
        .map(|tp| {
            (
                tp.name.clone(),
                tp.bounds.iter().map(|bound| bound.name.clone()).collect(),
            )
        })
        .collect();
    let mut all_type_params = type_params.to_vec();
    all_type_params.extend(method_type_params.iter().cloned());
    let method_type_param_bound_details = method
        .type_params
        .iter()
        .map(|tp| {
            (
                tp.name.clone(),
                tp.bounds
                    .iter()
                    .map(|bound| crate::frontend::symbols::TypeBoundInfo {
                        name: bound.name.clone(),
                        source_name: None,
                        type_args: bound
                            .type_args
                            .iter()
                            .map(|arg| {
                                ast_type_to_resolved_with_rust_imports(&arg.node, &all_type_params, rust_imports)
                            })
                            .collect(),
                        module_path: stdlib_imports.get(&bound.name).cloned(),
                    })
                    .collect(),
            )
        })
        .collect();
    let params: Vec<CallableParam> = method
        .params
        .iter()
        .map(|p| {
            CallableParam::named_with_default(
                p.node.name.clone(),
                ast_type_to_resolved_with_rust_imports(&p.node.ty.node, &all_type_params, rust_imports),
                p.node.kind,
                p.node.default.is_some(),
            )
        })
        .collect();
    let return_type = ast_type_to_resolved_with_rust_imports(&method.return_type.node, &all_type_params, rust_imports);
    let trait_target = method.trait_target.as_ref().map(|target| TypeBoundInfo {
        name: target.node.name.clone(),
        source_name: None,
        type_args: target
            .node
            .type_args
            .iter()
            .map(|arg| ast_type_to_resolved_with_rust_imports(&arg.node, &all_type_params, rust_imports))
            .collect(),
        module_path: stdlib_imports.get(&target.node.name).cloned(),
    });
    MethodInfo {
        type_params: method_type_params,
        type_param_bounds: method_type_param_bounds,
        type_param_bound_details: method_type_param_bound_details,
        trait_target,
        receiver: method.receiver,
        params,
        return_type,
        is_async: method.is_async(),
        has_body: method.body.is_some(),
        alias_of: None,
    }
}

/// Convert an AST `FunctionDecl` to a typechecker `FunctionInfo`.
fn function_decl_to_info(func: &ast::FunctionDecl, stdlib_imports: &HashMap<String, Vec<String>>) -> FunctionInfo {
    // Extract just the type parameter names for type resolution.
    let tp_names: Vec<String> = func.type_params.iter().map(|tp| tp.name.clone()).collect();
    let tp_bounds: HashMap<String, Vec<String>> = func
        .type_params
        .iter()
        .map(|tp| {
            (
                tp.name.clone(),
                tp.bounds.iter().map(|bound| bound.name.clone()).collect(),
            )
        })
        .collect();
    let tp_bound_details: HashMap<String, Vec<TypeBoundInfo>> = func
        .type_params
        .iter()
        .map(|tp| {
            (
                tp.name.clone(),
                tp.bounds
                    .iter()
                    .map(|bound| TypeBoundInfo {
                        name: bound.name.clone(),
                        source_name: None,
                        type_args: bound
                            .type_args
                            .iter()
                            .map(|arg| ast_type_to_resolved(&arg.node, &tp_names))
                            .collect(),
                        module_path: stdlib_imports.get(&bound.name).cloned(),
                    })
                    .collect(),
            )
        })
        .collect();

    let params: Vec<CallableParam> = func
        .params
        .iter()
        .map(|p| {
            CallableParam::named_with_default(
                p.node.name.clone(),
                ast_type_to_resolved(&p.node.ty.node, &tp_names),
                p.node.kind,
                p.node.default.is_some(),
            )
        })
        .collect();

    let return_type = ast_type_to_resolved(&func.return_type.node, &tp_names);

    FunctionInfo {
        params,
        return_type,
        is_async: func.is_async(),
        type_params: tp_names,
        type_param_bounds: tp_bounds,
        type_param_bound_details: tp_bound_details,
        emitted_name: None,
    }
}

/// Build a lightweight `FunctionInfo` for the remaining generic Rust leaves that still need direct stdlib imports.
///
/// We intentionally keep this list narrow: only helpers whose Rust-side bounds are not yet representable by the
/// language surface stay on this path. Public stdlib functions that can be declared locally should prefer real `.incn`
/// definitions so their signatures come straight from the AST.
fn imported_runtime_function_info(name: &str) -> Option<FunctionInfo> {
    let (params, return_type, is_async) = match surface_functions::from_str(name)? {
        SurfaceFnId::Timeout => (
            vec![
                CallableParam::named("seconds", ResolvedType::Float, ast::ParamKind::Normal),
                CallableParam::named("task", ResolvedType::Unknown, ast::ParamKind::Normal),
            ],
            ResolvedType::Unknown,
            true,
        ),
        SurfaceFnId::TimeoutMs => (
            vec![
                CallableParam::named("milliseconds", ResolvedType::Int, ast::ParamKind::Normal),
                CallableParam::named("task", ResolvedType::Unknown, ast::ParamKind::Normal),
            ],
            ResolvedType::Unknown,
            true,
        ),
        SurfaceFnId::RaceTimeout => (
            vec![
                CallableParam::named("seconds", ResolvedType::Float, ast::ParamKind::Normal),
                CallableParam::named("task", ResolvedType::Unknown, ast::ParamKind::Normal),
            ],
            ResolvedType::Unknown,
            true,
        ),
        SurfaceFnId::Spawn => (
            vec![CallableParam::named(
                "task",
                ResolvedType::Unknown,
                ast::ParamKind::Normal,
            )],
            ResolvedType::Unknown,
            false,
        ),
        SurfaceFnId::SpawnBlocking => (
            vec![CallableParam::named(
                "task",
                ResolvedType::Unknown,
                ast::ParamKind::Normal,
            )],
            ResolvedType::Unknown,
            true,
        ),
        _ => return None,
    };

    Some(FunctionInfo {
        params,
        return_type,
        is_async,
        type_params: Vec::new(),
        type_param_bounds: HashMap::new(),
        type_param_bound_details: HashMap::new(),
        emitted_name: None,
    })
}

/// Extract function metadata from a stdlib module's AST.
///
/// Walks top-level function declarations and records:
/// - Whether the function has the `@rust.extern` decorator (indicating delegation to a Rust backing module).
/// - The `rust.module()` path declared on the program, if any.
///
/// This metadata is stored in [`StdlibAstCache`] and used during lowering to decide whether a decorator reference
/// should be emitted as a Rust attribute (passthrough) or compiled normally.
fn extract_function_meta(program: &ast::Program) -> HashMap<String, FunctionMeta> {
    let mut meta = HashMap::new();
    let rust_module_path = program.rust_module_path.as_ref().map(|sp| sp.node.clone());
    for decl in &program.declarations {
        if let ast::Declaration::Function(func) = &decl.node {
            let is_rust_extern = func
                .decorators
                .iter()
                .any(|d| decorators::from_str(&d.node.path.segments.join(".")) == Some(DecoratorId::RustExtern));
            meta.insert(
                func.name.clone(),
                FunctionMeta {
                    is_rust_extern,
                    rust_module_path: rust_module_path.clone(),
                },
            );
        }
    }
    meta
}

/// Extract trait metadata from a stdlib module's AST.
///
/// Walks top-level trait declarations and records the `rust.module()` backing path (if any).
/// This metadata is used by `AstLowering::resolve_derive_module_path` to map `@derive(Trait)` references to their Rust
/// proc-macro crate paths for derive passthrough.
fn extract_trait_meta(program: &ast::Program) -> HashMap<String, TraitMeta> {
    let mut meta = HashMap::new();
    let rust_module_path = program.rust_module_path.as_ref().map(|sp| sp.node.clone());
    for decl in &program.declarations {
        if let ast::Declaration::Trait(tr) = &decl.node {
            let rust_derive_paths = rust_derive_paths_from_decorators(&tr.decorators);
            meta.insert(
                tr.name.clone(),
                TraitMeta {
                    rust_module_path: rust_module_path.clone(),
                    rust_derive_paths,
                },
            );
        }
    }
    meta
}

/// Extract RFC 024 `@rust.derive(...)` path strings from stdlib trait decorators.
fn rust_derive_paths_from_decorators(decorators: &[ast::Spanned<ast::Decorator>]) -> Vec<String> {
    let mut out = Vec::new();
    for decorator in decorators {
        if decorators::from_str(&decorator.node.path.segments.join(".")) != Some(DecoratorId::RustDerive) {
            continue;
        }
        for arg in &decorator.node.args {
            let ast::DecoratorArg::Positional(expr) = arg else {
                continue;
            };
            let ast::Expr::Literal(ast::Literal::String(path)) = &expr.node else {
                continue;
            };
            if !out.iter().any(|existing| existing == path) {
                out.push(path.clone());
            }
        }
    }
    out
}

/// Convert an AST `Type` to a `ResolvedType`.
///
/// Type parameter names (from the enclosing function's `type_params`) are resolved to `TypeVar`.
/// Primitive type names are resolved to their concrete `ResolvedType` variants.
/// Unknown types are resolved to `Named(name)`.
fn ast_type_to_resolved(ty: &ast::Type, type_params: &[String]) -> ResolvedType {
    ast_type_to_resolved_with_rust_imports(ty, type_params, &HashMap::new())
}

/// Convert an AST type to a resolved type while honoring Rust import aliases from the same stdlib module.
fn ast_type_to_resolved_with_rust_imports(
    ty: &ast::Type,
    type_params: &[String],
    rust_imports: &HashMap<String, String>,
) -> ResolvedType {
    match ty {
        ast::Type::Unit => ResolvedType::Unit,
        ast::Type::SelfType => ResolvedType::SelfType,
        ast::Type::Qualified(segments) => {
            let Some((head, tail)) = segments.split_first() else {
                return ResolvedType::Unknown;
            };
            if let Some(base) = rust_imports.get(head) {
                let mut parts = vec![base.clone()];
                parts.extend(tail.iter().map(|segment| rust_keywords::escape_keyword(segment)));
                ResolvedType::RustPath(parts.join("::"))
            } else {
                ResolvedType::Unknown
            }
        }
        ast::Type::Simple(name) => {
            // Check if it's a type parameter first.
            if type_params.contains(name) {
                return ResolvedType::TypeVar(name.clone());
            }
            if let Some(path) = rust_imports.get(name) {
                return ResolvedType::RustPath(path.clone());
            }

            // Resolve through incan_core registries (numerics, strings, unit).
            if let Some(id) = numeric_types::from_str(name) {
                return match name.as_str() {
                    "int" => ResolvedType::Int,
                    "float" => ResolvedType::Float,
                    "bool" => ResolvedType::Bool,
                    _ => match id {
                        NumericTypeId::Bool => ResolvedType::Bool,
                        _ => ResolvedType::Numeric(id),
                    },
                };
            }
            if let Some(id) = string_types::from_str(name) {
                return match id {
                    StringLikeId::Str => ResolvedType::Str,
                    StringLikeId::Bytes => ResolvedType::Bytes,
                    // Frozen variants are named types in this context.
                    _ => ResolvedType::Named(name.clone()),
                };
            }
            if name == conventions::NONE_TYPE_NAME {
                return ResolvedType::Unit;
            }

            ResolvedType::Named(name.clone())
        }
        ast::Type::ConstrainedPrimitive(name, _) => {
            let base = ast::Type::Simple(name.clone());
            ast_type_to_resolved_with_rust_imports(&base, type_params, rust_imports)
        }
        ast::Type::Generic(name, args) => {
            let resolved_args: Vec<ResolvedType> = args
                .iter()
                .map(|a| ast_type_to_resolved_with_rust_imports(&a.node, type_params, rust_imports))
                .collect();

            if let Some(path) = rust_imports.get(name) {
                let rendered_args = resolved_args
                    .iter()
                    .map(render_resolved_type_as_rust_arg)
                    .collect::<Vec<_>>()
                    .join(", ");
                return ResolvedType::RustPath(format!("{path}<{rendered_args}>"));
            }

            let collection_id = collection_types::from_str(name);
            match collection_id {
                Some(CollectionTypeId::Option) => {
                    let canonical = collection_types::as_str(CollectionTypeId::Option).to_string();
                    if let Some(inner) = resolved_args.into_iter().next() {
                        ResolvedType::Generic(canonical, vec![inner])
                    } else {
                        ResolvedType::Named(canonical)
                    }
                }
                Some(CollectionTypeId::Result) => {
                    let canonical = collection_types::as_str(CollectionTypeId::Result).to_string();
                    if resolved_args.len() == 2 {
                        ResolvedType::Generic(canonical, resolved_args)
                    } else {
                        ResolvedType::Named(canonical)
                    }
                }
                Some(CollectionTypeId::List) => {
                    let canonical = collection_types::as_str(CollectionTypeId::List).to_string();
                    if let Some(inner) = resolved_args.into_iter().next() {
                        ResolvedType::Generic(canonical, vec![inner])
                    } else {
                        ResolvedType::Named(canonical)
                    }
                }
                _ => ResolvedType::Generic(name.clone(), resolved_args),
            }
        }
        ast::Type::Function(params, ret) => {
            let param_types: Vec<CallableParam> = params
                .iter()
                .map(|p| {
                    CallableParam::positional(ast_type_to_resolved_with_rust_imports(
                        &p.node,
                        type_params,
                        rust_imports,
                    ))
                })
                .collect();
            let ret_type = ast_type_to_resolved_with_rust_imports(&ret.node, type_params, rust_imports);
            ResolvedType::Function(param_types, Box::new(ret_type))
        }
        ast::Type::Ref(inner) => ResolvedType::Ref(Box::new(ast_type_to_resolved_with_rust_imports(
            &inner.node,
            type_params,
            rust_imports,
        ))),
        ast::Type::RefMut(inner) => ResolvedType::RefMut(Box::new(ast_type_to_resolved_with_rust_imports(
            &inner.node,
            type_params,
            rust_imports,
        ))),
        ast::Type::Tuple(elems) => {
            let elem_types: Vec<ResolvedType> = elems
                .iter()
                .map(|e| ast_type_to_resolved_with_rust_imports(&e.node, type_params, rust_imports))
                .collect();
            ResolvedType::Tuple(elem_types)
        }
        ast::Type::IntLiteral(value) => ResolvedType::TypeVar(value.repr.clone()),
        ast::Type::Infer => ResolvedType::CallSiteInfer,
    }
}

/// Collect local aliases for Rust imports declared in a stdlib module.
fn rust_import_aliases(program: &ast::Program) -> HashMap<String, String> {
    let mut aliases = HashMap::new();
    for decl in &program.declarations {
        let ast::Declaration::Import(import) = &decl.node else {
            continue;
        };
        match &import.kind {
            ast::ImportKind::RustFrom {
                crate_name,
                path,
                items,
                ..
            } => {
                for item in items {
                    let local_name = item.alias.as_deref().unwrap_or(&item.name);
                    let mut full_path = vec![rust_keywords::escape_keyword(crate_name)];
                    full_path.extend(path.iter().map(|segment| rust_keywords::escape_keyword(segment)));
                    full_path.push(rust_keywords::escape_keyword(&item.name));
                    aliases.insert(local_name.to_string(), full_path.join("::"));
                }
            }
            ast::ImportKind::RustCrate { crate_name, path, .. } => {
                let local_name = import
                    .alias
                    .as_deref()
                    .or_else(|| path.last().map(String::as_str))
                    .unwrap_or(crate_name);
                let mut full_path = vec![rust_keywords::escape_keyword(crate_name)];
                full_path.extend(path.iter().map(|segment| rust_keywords::escape_keyword(segment)));
                aliases.insert(local_name.to_string(), full_path.join("::"));
            }
            _ => {}
        }
    }
    aliases
}

/// Collect local aliases for stdlib `from std... import ...` items declared in a stdlib module.
fn stdlib_import_aliases(program: &ast::Program) -> HashMap<String, Vec<String>> {
    let mut aliases = HashMap::new();
    for decl in &program.declarations {
        let ast::Declaration::Import(import) = &decl.node else {
            continue;
        };
        let ast::ImportKind::From { module, items } = &import.kind else {
            continue;
        };
        if module
            .segments
            .first()
            .is_none_or(|segment| segment != stdlib::STDLIB_ROOT)
        {
            continue;
        }
        for item in items {
            let local_name = item.alias.as_deref().unwrap_or(&item.name);
            aliases.insert(local_name.to_string(), module.segments.clone());
        }
    }
    aliases
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_testing_module() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "testing".to_string()];
        let module = load_stdlib_module_data(&path);

        // The stdlib/testing.incn file should be findable and parseable.
        let module = module.ok_or("failed to load stdlib/testing.incn")?;
        let fns = module.functions;
        assert!(!fns.is_empty(), "should have extracted function signatures");

        // Check a few known functions.
        let fail_fn = fns.iter().find(|entry| entry.name == "fail");
        assert!(fail_fn.is_some(), "should find 'fail' function");
        let fail_info = &fail_fn.ok_or("fail not found")?.info;
        assert_eq!(fail_info.params.len(), 1);
        assert_eq!(fail_info.params[0].name(), Some("msg"));
        assert!(matches!(fail_info.params[0].ty, ResolvedType::Str));
        assert!(fail_info.type_params.is_empty());
        assert!(matches!(fail_info.return_type, ResolvedType::Unit));

        let fail_t_fn = fns.iter().find(|entry| entry.name == "fail_t");
        assert!(fail_t_fn.is_some(), "should find 'fail_t' function");
        let fail_t_info = &fail_t_fn.ok_or("fail_t not found")?.info;
        assert_eq!(fail_t_info.params.len(), 1);
        assert_eq!(fail_t_info.params[0].name(), Some("msg"));
        assert!(matches!(fail_t_info.params[0].ty, ResolvedType::Str));
        assert_eq!(fail_t_info.type_params, vec!["T".to_string()]);
        assert!(matches!(fail_t_info.return_type, ResolvedType::TypeVar(ref s) if s == "T"));

        let assert_eq_fn = fns.iter().find(|entry| entry.name == "assert_eq");
        assert!(assert_eq_fn.is_some(), "should find 'assert_eq' function");
        let assert_eq_info = &assert_eq_fn.ok_or("assert_eq not found")?.info;
        assert_eq!(assert_eq_info.params.len(), 3);
        assert_eq!(assert_eq_info.type_params, vec!["T".to_string()]);
        assert!(matches!(assert_eq_info.params[0].ty, ResolvedType::TypeVar(ref s) if s == "T"));
        assert!(matches!(assert_eq_info.params[1].ty, ResolvedType::TypeVar(ref s) if s == "T"));
        assert!(matches!(assert_eq_info.params[2].ty, ResolvedType::Str));

        Ok(())
    }

    #[test]
    fn test_load_collections_module_exports_public_types() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "collections".to_string()];
        let module = load_stdlib_module_data(&path).ok_or("failed to load stdlib/collections.incn")?;
        let names = module
            .types
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<std::collections::BTreeSet<_>>();

        for expected in [
            "PriorityOrder",
            "Deque",
            "Counter",
            "DefaultDict",
            "OrderedDict",
            "OrderedSet",
            "SortedDict",
            "SortedSet",
            "ChainMap",
            "PriorityQueue",
        ] {
            assert!(names.contains(expected), "std.collections should export {expected}");
        }

        let deque = module
            .types
            .iter()
            .find(|(name, _)| name == "Deque")
            .ok_or("Deque export not found")?;
        let TypeInfo::Model(deque_info) = &deque.1 else {
            return Err("Deque should be an AST-loaded model export".into());
        };
        assert!(deque_info.methods.contains_key("appendleft"));
        assert!(deque_info.methods.contains_key("popleft"));

        Ok(())
    }

    #[test]
    fn test_load_encoding_modules_export_source_owned_surface() -> Result<(), Box<dyn std::error::Error>> {
        let prelude_path = vec!["std".to_string(), "encoding".to_string()];
        let prelude = load_stdlib_module_data(&prelude_path).ok_or("failed to load stdlib/encoding/prelude.incn")?;
        assert!(
            prelude.types.iter().any(|(name, _)| name == "EncodingError"),
            "std.encoding should export source-owned EncodingError"
        );

        for (module_name, expected_functions) in [
            (
                "hex",
                vec![
                    "encode",
                    "decode",
                    "b16encode",
                    "b16decode",
                    "encode_stream",
                    "decode_stream",
                ],
            ),
            (
                "base64",
                vec![
                    "encode",
                    "decode",
                    "decode_lenient",
                    "b64encode",
                    "b64decode",
                    "b64decode_lenient",
                    "b64encode_stream",
                    "b64decode_stream",
                    "urlsafe_b64encode",
                    "urlsafe_b64decode",
                    "urlsafe_b64encode_stream",
                    "urlsafe_b64decode_stream",
                ],
            ),
            (
                "base32",
                vec![
                    "encode",
                    "decode",
                    "decode_lenient",
                    "b32encode",
                    "b32decode",
                    "b32decode_lenient",
                    "b32hexencode",
                    "b32hexdecode",
                    "b32encode_stream",
                    "b32decode_stream",
                    "b32hexencode_stream",
                    "b32hexdecode_stream",
                    "encode_stream",
                    "decode_stream",
                ],
            ),
            (
                "base85",
                vec![
                    "a85encode",
                    "a85decode",
                    "b85encode",
                    "b85decode",
                    "z85encode",
                    "z85decode",
                    "a85encode_stream",
                    "a85decode_stream",
                    "b85encode_stream",
                    "b85decode_stream",
                    "z85encode_stream",
                    "z85decode_stream",
                    "encode_stream",
                    "decode_stream",
                ],
            ),
            (
                "base58",
                vec![
                    "encode",
                    "decode",
                    "b58encode",
                    "b58decode",
                    "b58encode_stream",
                    "b58decode_stream",
                    "encode_stream",
                    "decode_stream",
                ],
            ),
            (
                "bech32",
                vec!["bech32_encode", "bech32_decode", "bech32m_encode", "bech32m_decode"],
            ),
        ] {
            let path = vec!["std".to_string(), "encoding".to_string(), module_name.to_string()];
            let module = load_stdlib_module_data(&path)
                .ok_or_else(|| format!("failed to load stdlib/encoding/{module_name}.incn"))?;
            for expected in expected_functions {
                assert!(
                    module.functions.iter().any(|entry| entry.name == expected),
                    "std.encoding.{module_name} should export {expected}"
                );
            }
        }

        Ok(())
    }

    #[test]
    fn test_load_uuid_module_exports_public_surface() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "uuid".to_string()];
        let module = load_stdlib_module_data(&path).ok_or("failed to load stdlib/uuid.incn")?;
        let names = module
            .types
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<std::collections::BTreeSet<_>>();

        for expected in ["UUID", "UuidError", "UuidVersion", "UuidVariant"] {
            assert!(names.contains(expected), "std.uuid should export {expected}");
        }

        let functions = module
            .functions
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        for removed in [
            "parse",
            "from_int",
            "from_bytes",
            "v1",
            "v3",
            "v4",
            "v5",
            "v6",
            "v7",
            "v8",
            "nil",
            "max",
        ] {
            assert!(
                !functions.contains(removed),
                "std.uuid constructors should live on UUID, not as module function {removed}"
            );
        }
        assert!(
            !functions.contains("_hex_value"),
            "private std.uuid helpers must not become importable stdlib functions"
        );

        let constants = module
            .constants
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        for expected in [
            "NIL",
            "MAX",
            "NAMESPACE_DNS",
            "NAMESPACE_URL",
            "NAMESPACE_OID",
            "NAMESPACE_X500",
        ] {
            assert!(constants.contains(expected), "std.uuid should export const {expected}");
        }

        Ok(())
    }

    #[test]
    fn test_load_regex_module_exports_rfc059_surface() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "regex".to_string()];
        let module = load_stdlib_module_data(&path).ok_or("failed to load stdlib/regex.incn")?;
        let names = module
            .types
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<std::collections::BTreeSet<_>>();

        for expected in ["Regex", "Match", "Captures", "RegexError"] {
            assert!(names.contains(expected), "std.regex should export {expected}");
        }

        let regex = module
            .types
            .iter()
            .find(|(name, _)| name == "Regex")
            .ok_or("Regex export not found")?;
        let regex_methods = match &regex.1 {
            TypeInfo::Newtype(info) => &info.methods,
            TypeInfo::Model(info) => &info.methods,
            TypeInfo::Class(info) => &info.methods,
            _ => return Err("Regex should expose importable method metadata".into()),
        };
        for expected in [
            "is_match",
            "find",
            "find_iter",
            "captures",
            "captures_iter",
            "full_match",
            "split",
            "splitn",
            "replace",
            "replace_all",
            "replacen",
        ] {
            assert!(
                regex_methods.contains_key(expected),
                "std.regex Regex should expose method {expected}"
            );
        }

        let find = regex_methods.get("find").ok_or("missing Regex.find")?;
        assert_eq!(
            find.return_type,
            ResolvedType::Generic("Option".to_string(), vec![ResolvedType::Named("Match".to_string())]),
            "Regex.find should return Option[Match]"
        );
        let full_match = regex_methods.get("full_match").ok_or("missing Regex.full_match")?;
        assert_eq!(
            full_match.return_type,
            ResolvedType::Generic("Option".to_string(), vec![ResolvedType::Named("Captures".to_string())]),
            "Regex.full_match should return Option[Captures]"
        );

        let captures = module
            .types
            .iter()
            .find(|(name, _)| name == "Captures")
            .ok_or("Captures export not found")?;
        let captures_methods = match &captures.1 {
            TypeInfo::Newtype(info) => &info.methods,
            TypeInfo::Model(info) => &info.methods,
            TypeInfo::Class(info) => &info.methods,
            _ => return Err("Captures should expose importable method metadata".into()),
        };
        for expected in ["full_match", "group", "span", "groups", "groupdict"] {
            assert!(
                captures_methods.contains_key(expected),
                "std.regex Captures should expose method {expected}"
            );
        }

        Ok(())
    }

    #[test]
    fn extract_type_signatures_preserves_same_name_method_overloads() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
pub trait Convert[T]:
  def convert(self) -> T: ...

pub enum Token with Convert[int], Convert[float]:
  Number

  def convert(self) -> int:
    return 1

  def convert(self) -> float:
    return 1.0
"#;
        let tokens = crate::frontend::lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let program =
            crate::frontend::parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let types = extract_type_signatures(&program);
        let Some((_, TypeInfo::Enum(info))) = types.iter().find(|(name, _)| name == "Token") else {
            return Err("missing Token enum metadata".into());
        };

        let overloads = info
            .method_overloads
            .get("convert")
            .ok_or("missing convert overload metadata")?;
        assert_eq!(overloads.len(), 2);
        assert!(
            overloads
                .iter()
                .any(|method| matches!(method.return_type, ResolvedType::Int))
        );
        assert!(
            overloads
                .iter()
                .any(|method| matches!(method.return_type, ResolvedType::Float))
        );
        Ok(())
    }

    #[test]
    fn test_cache_lookup() -> Result<(), Box<dyn std::error::Error>> {
        let mut cache = StdlibAstCache::new();
        let path = vec!["std".to_string(), "testing".to_string()];

        // First lookup loads the module.
        let fail_info = cache.lookup_function_symbol(&path, "fail");
        assert!(fail_info.is_some(), "should find 'fail' from cache");

        // Second lookup uses the cache.
        let assert_eq_info = cache.lookup_function_symbol(&path, "assert_eq");
        assert!(assert_eq_info.is_some(), "should find 'assert_eq' from cache");

        // Unknown function returns None.
        let unknown = cache.lookup_function_symbol(&path, "nonexistent_function");
        assert!(unknown.is_none(), "should not find unknown function");

        Ok(())
    }

    #[test]
    fn test_std_fs_path_lookup_uses_ast_cache_not_web_surface_type() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from rust::std::path import PathBuf as RustPathBuf
from rust::std::fs import File as RustFile

pub type Path = rusttype RustPathBuf:
  """Filesystem path wrapper."""

pub type File = rusttype RustFile:
  """Filesystem file handle wrapper."""
"#;
        let tokens = crate::frontend::lexer::lex(source)
            .map_err(|errs| format!("synthetic std.fs source should lex: {errs:?}"))?;
        let program = crate::frontend::parser::parse(&tokens)
            .map_err(|errs| format!("synthetic std.fs source should parse: {errs:?}"))?;
        let module_data = StdlibModuleData {
            functions: extract_function_entries(&program),
            traits: extract_trait_signatures(&program),
            trait_declarations: extract_trait_declarations(&program),
            types: extract_type_signatures(&program),
            type_docstrings: extract_type_docstrings(&program),
            constants: extract_const_signatures(&program),
            statics: extract_static_signatures(&program),
            derivable_traits: extract_derivable_traits(&program),
            function_meta: extract_function_meta(&program),
            trait_meta: extract_trait_meta(&program),
        };
        let mut cache = StdlibAstCache::new();
        cache.cache.insert("std.fs".to_string(), module_data);
        let path = vec!["std".to_string(), "fs".to_string()];
        let fs_path = cache
            .lookup_type(&path, "Path")
            .ok_or("std.fs Path should resolve through StdlibAstCache::lookup_type")?;
        match fs_path {
            TypeInfo::Newtype(info) => {
                assert!(info.is_rusttype);
                assert_eq!(
                    info.underlying,
                    ResolvedType::RustPath("std::path::PathBuf".to_string())
                );
            }
            other => return Err(format!("std.fs Path should be an AST-loaded rusttype, got {other:?}").into()),
        }
        assert!(
            cache.lookup_type(&path, "File").is_some(),
            "std.fs File should resolve through the same AST cache path"
        );

        Ok(())
    }

    #[test]
    fn test_load_std_fs_prelude_handles_reexport_cycles() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "fs".to_string()];
        let module = load_stdlib_module_data(&path).ok_or("failed to load std.fs prelude")?;
        assert!(
            module.types.iter().any(|(name, _)| name == "Path"),
            "std.fs should re-export Path from std.fs.path"
        );
        assert!(
            module.types.iter().any(|(name, _)| name == "File"),
            "std.fs should re-export File from std.fs.file"
        );
        assert!(
            module.types.iter().any(|(name, _)| name == "OpenFileMode"),
            "std.fs should re-export OpenFileMode from std.fs.file"
        );
        assert!(
            module.types.iter().any(|(name, _)| name == "PathStat"),
            "std.fs should re-export PathStat from std.fs.metadata"
        );
        Ok(())
    }

    #[test]
    fn test_load_async_time_module() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "async".to_string(), "time".to_string()];
        let module = load_stdlib_module_data(&path);
        let module = module.ok_or("failed to load stdlib/async/time.incn")?;
        let fns = &module.functions;
        let sleep_fn = fns.iter().find(|entry| entry.name == "sleep");
        assert!(sleep_fn.is_some(), "should find 'sleep' function");
        let timeout_fn = fns.iter().find(|entry| entry.name == "timeout");
        assert!(timeout_fn.is_some(), "should find 'timeout' function");
        let timeout_join_outcome = module
            .types
            .iter()
            .find(|(name, _)| name == "TimeoutJoinOutcome")
            .ok_or("TimeoutJoinOutcome type not found")?;
        let TypeInfo::Newtype(info) = &timeout_join_outcome.1 else {
            return Err("TimeoutJoinOutcome should load as a newtype".into());
        };
        assert!(info.is_rusttype);
        assert_eq!(info.type_params, vec!["T".to_string()]);
        assert_eq!(
            info.underlying,
            ResolvedType::RustPath("incan_stdlib::r#async::time::TimeoutJoinOutcome<T>".to_string())
        );
        Ok(())
    }

    #[test]
    fn test_load_async_prelude_module() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "async".to_string()];
        let module = load_stdlib_module_data(&path);
        let fns = module.ok_or("failed to load stdlib/async/prelude.incn")?.functions;
        let sleep_fn = fns.iter().find(|entry| entry.name == "sleep");
        assert!(sleep_fn.is_some(), "should resolve prelude re-export 'sleep'");
        let spawn_fn = fns.iter().find(|entry| entry.name == "spawn");
        assert!(spawn_fn.is_some(), "should resolve prelude re-export 'spawn'");
        Ok(())
    }

    #[test]
    fn test_load_async_channel_module() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "async".to_string(), "channel".to_string()];
        let module = load_stdlib_module_data(&path);

        let fns = module.ok_or("failed to load stdlib/async/channel.incn")?.functions;

        let channel_fn = fns.iter().find(|entry| entry.name == "channel");
        assert!(channel_fn.is_some(), "should find 'channel' function");
        let oneshot_fn = fns.iter().find(|entry| entry.name == "oneshot");
        assert!(oneshot_fn.is_some(), "should find 'oneshot' function");

        Ok(())
    }

    #[test]
    fn test_load_async_task_module() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "async".to_string(), "task".to_string()];
        let module = load_stdlib_module_data(&path);
        let fns = module.ok_or("failed to load stdlib/async/task.incn")?.functions;

        assert!(fns.iter().any(|entry| entry.name == "spawn"));
        assert!(fns.iter().any(|entry| entry.name == "spawn_blocking"));
        assert!(fns.iter().any(|entry| entry.name == "yield_now"));
        Ok(())
    }

    #[test]
    fn test_load_async_race_module_exports_public_race_surface() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "async".to_string(), "race".to_string()];
        let module = load_stdlib_module_data(&path);
        let module = module.ok_or("failed to load stdlib/async/race.incn")?;
        let fns = module.functions;

        let exported_names: Vec<&str> = fns.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(exported_names, vec!["arm", "race", "race_timeout"]);
        assert!(module.types.iter().any(|(name, _)| name == "RaceArm"));
        Ok(())
    }

    #[test]
    fn test_load_async_select_module_is_removed() {
        let path = vec!["std".to_string(), "async".to_string(), "select".to_string()];
        assert!(load_stdlib_module_data(&path).is_none());
    }

    // ---- Phase 6: Derive trait extraction tests ----

    use incan_core::lang::derives::{self as derive_reg, DeriveId};
    use incan_core::lang::traits::{self as core_traits, TraitId};

    /// Helper: canonical derive name from the registry (avoids stringly-typed vocab checks).
    fn derive_name(id: DeriveId) -> &'static str {
        derive_reg::as_str(id)
    }

    #[test]
    fn test_load_derives_comparison_traits() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "derives".to_string(), "comparison".to_string()];
        let module = load_stdlib_module_data(&path);
        let module = module.ok_or("failed to load stdlib/derives/comparison.incn")?;

        // Should have no top-level functions, only traits.
        assert!(
            module.functions.is_empty(),
            "comparison.incn has no top-level functions"
        );
        assert!(!module.traits.is_empty(), "should have extracted trait signatures");

        // Eq trait: __eq__ (extern, no body) and __ne__ (default, has body).
        let eq_name = derive_name(DeriveId::Eq);
        let eq_trait = module.traits.iter().find(|(name, _)| name == eq_name);
        assert!(eq_trait.is_some(), "should find Eq trait");
        let eq_info = &eq_trait.ok_or("Eq not found")?.1;
        assert!(eq_info.type_params.is_empty());
        assert!(eq_info.methods.contains_key("__eq__"), "Eq should have __eq__");
        assert!(eq_info.methods.contains_key("__ne__"), "Eq should have __ne__");
        let ne = &eq_info.methods["__ne__"];
        assert!(ne.has_body, "__ne__ is a default method with a body");
        assert!(matches!(ne.return_type, ResolvedType::Bool));

        // Ord trait: __lt__ (extern) + __le__, __gt__, __ge__ (defaults).
        let ord_name = derive_name(DeriveId::Ord);
        let ord_trait = module.traits.iter().find(|(name, _)| name == ord_name);
        assert!(ord_trait.is_some(), "should find Ord trait");
        let ord_info = &ord_trait.ok_or("Ord not found")?.1;
        assert_eq!(
            ord_info.supertraits,
            vec![(eq_name.to_string(), Vec::new())],
            "Ord should declare Eq as a supertrait (see stdlib/derives/comparison.incn)"
        );
        assert_eq!(ord_info.methods.len(), 4);
        assert!(!ord_info.methods["__lt__"].has_body, "__lt__ is abstract (extern)");
        assert!(ord_info.methods["__le__"].has_body, "__le__ is a default method");
        assert!(ord_info.methods["__gt__"].has_body, "__gt__ is a default method");
        assert!(ord_info.methods["__ge__"].has_body, "__ge__ is a default method");

        // Hash trait: single __hash__ (extern).
        let hash_name = derive_name(DeriveId::Hash);
        let hash_trait = module.traits.iter().find(|(name, _)| name == hash_name);
        assert!(hash_trait.is_some(), "should find Hash trait");
        let hash_info = &hash_trait.ok_or("Hash not found")?.1;
        assert_eq!(hash_info.methods.len(), 1);
        assert!(hash_info.methods.contains_key("__hash__"));
        assert!(matches!(hash_info.methods["__hash__"].return_type, ResolvedType::Int));

        Ok(())
    }

    #[test]
    fn test_load_derives_copying_traits() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "derives".to_string(), "copying".to_string()];
        let module = load_stdlib_module_data(&path);
        let module = module.ok_or("failed to load stdlib/derives/copying.incn")?;

        assert!(module.functions.is_empty(), "copying.incn has no top-level functions");

        // Clone trait: single clone(self) -> Self method.
        let clone_name = derive_name(DeriveId::Clone);
        let clone_trait = module.traits.iter().find(|(name, _)| name == clone_name);
        assert!(clone_trait.is_some(), "should find Clone trait");
        let clone_info = &clone_trait.ok_or("Clone not found")?.1;
        assert_eq!(clone_info.methods.len(), 1);
        assert!(clone_info.methods.contains_key("clone"));
        assert!(matches!(
            clone_info.methods["clone"].return_type,
            ResolvedType::SelfType
        ));

        // Copy trait: marker trait with no methods.
        let copy_name = derive_name(DeriveId::Copy);
        let copy_trait = module.traits.iter().find(|(name, _)| name == copy_name);
        assert!(copy_trait.is_some(), "should find Copy trait");
        let copy_info = &copy_trait.ok_or("Copy not found")?.1;
        assert!(copy_info.methods.is_empty(), "Copy is a marker trait with no methods");

        // Default trait: default() -> Self (no receiver, associated function).
        let default_name = derive_name(DeriveId::Default);
        let default_trait = module.traits.iter().find(|(name, _)| name == default_name);
        assert!(default_trait.is_some(), "should find Default trait");
        let default_info = &default_trait.ok_or("Default not found")?.1;
        assert_eq!(default_info.methods.len(), 1);
        let default_method = &default_info.methods["default"];
        assert!(
            default_method.receiver.is_none(),
            "default() is an associated function (no receiver)"
        );
        assert!(matches!(default_method.return_type, ResolvedType::SelfType));

        Ok(())
    }

    #[test]
    fn test_load_derives_string_traits() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "derives".to_string(), "string".to_string()];
        let module = load_stdlib_module_data(&path);
        let module = module.ok_or("failed to load stdlib/derives/string.incn")?;

        assert!(module.functions.is_empty(), "string.incn has no top-level functions");

        // Debug trait: __repr__(self) -> str.
        let debug_name = derive_name(DeriveId::Debug);
        let debug_trait = module.traits.iter().find(|(name, _)| name == debug_name);
        assert!(debug_trait.is_some(), "should find Debug trait");
        let debug_info = &debug_trait.ok_or("Debug not found")?.1;
        assert_eq!(debug_info.methods.len(), 1);
        assert!(debug_info.methods.contains_key("__repr__"));
        assert!(matches!(debug_info.methods["__repr__"].return_type, ResolvedType::Str));

        // Display trait: __str__(self) -> str (abstract, no body).
        let display_name = derive_name(DeriveId::Display);
        let display_trait = module.traits.iter().find(|(name, _)| name == display_name);
        assert!(display_trait.is_some(), "should find Display trait");
        let display_info = &display_trait.ok_or("Display not found")?.1;
        assert_eq!(display_info.methods.len(), 1);
        assert!(display_info.methods.contains_key("__str__"));
        assert!(
            !display_info.methods["__str__"].has_body,
            "__str__ is abstract (no body)"
        );

        Ok(())
    }

    #[test]
    fn test_load_derives_collection_traits() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "derives".to_string(), "collection".to_string()];
        let module = load_stdlib_module_data(&path);
        let module = module.ok_or("failed to load stdlib/derives/collection.incn")?;

        for name in [
            "Contains",
            "Bool",
            "Len",
            core_traits::as_str(TraitId::Iterable),
            core_traits::as_str(TraitId::Iterator),
            core_traits::as_str(TraitId::Sum),
        ] {
            assert!(
                module.traits.iter().any(|(trait_name, _)| trait_name == name),
                "should find {name} trait"
            );
        }

        let iterator_info = module
            .traits
            .iter()
            .find(|(name, _)| name == core_traits::as_str(TraitId::Iterator))
            .ok_or("Iterator not found")?
            .1
            .clone();
        assert!(iterator_info.methods.contains_key("map"));
        assert!(iterator_info.methods.contains_key("flat_map"));
        assert!(iterator_info.methods.contains_key("sum"));

        Ok(())
    }

    #[test]
    fn test_load_serde_json_derivable_traits() -> Result<(), Box<dyn std::error::Error>> {
        let path = vec!["std".to_string(), "serde".to_string(), "json".to_string()];
        let module = load_stdlib_module_data(&path).ok_or("failed to load stdlib/serde/json.incn")?;

        assert_eq!(
            module.derivable_traits,
            vec!["Serialize".to_string(), "Deserialize".to_string()]
        );
        let serialize_meta = module.trait_meta.get("Serialize").ok_or("Serialize metadata missing")?;
        assert_eq!(serialize_meta.rust_derive_paths, vec!["serde::Serialize".to_string()]);
        let deserialize_meta = module
            .trait_meta
            .get("Deserialize")
            .ok_or("Deserialize metadata missing")?;
        assert_eq!(
            deserialize_meta.rust_derive_paths,
            vec!["serde::Deserialize".to_string()]
        );

        Ok(())
    }

    #[test]
    fn test_cache_trait_lookup() -> Result<(), Box<dyn std::error::Error>> {
        let mut cache = StdlibAstCache::new();
        let path = vec!["std".to_string(), "derives".to_string(), "comparison".to_string()];

        // First lookup loads the module and finds the trait.
        let eq_name = derive_name(DeriveId::Eq);
        let eq_info = cache.lookup_trait(&path, eq_name);
        assert!(eq_info.is_some(), "should find Eq trait from cache");

        // Second lookup uses the cache.
        let ord_name = derive_name(DeriveId::Ord);
        let ord_info = cache.lookup_trait(&path, ord_name);
        assert!(ord_info.is_some(), "should find Ord trait from cache");
        let ord_info = ord_info.ok_or("Ord missing")?;
        assert_eq!(
            ord_info.supertraits,
            vec![(derive_name(DeriveId::Eq).to_string(), Vec::new())],
            "cached Ord metadata should include Eq supertrait"
        );

        // Function lookup on a trait-only module returns None.
        let no_fn = cache.lookup_function_symbol(&path, eq_name);
        assert!(no_fn.is_none(), "should not find Eq as a function");

        // Unknown trait returns None.
        let unknown = cache.lookup_trait(&path, "NonExistent");
        assert!(unknown.is_none(), "should not find unknown trait");

        Ok(())
    }

    #[test]
    fn test_ast_type_conversion() {
        let type_params = vec!["T".to_string()];

        assert!(matches!(
            ast_type_to_resolved(&ast::Type::Simple("int".to_string()), &type_params),
            ResolvedType::Int
        ));
        assert!(matches!(
            ast_type_to_resolved(&ast::Type::Simple("str".to_string()), &type_params),
            ResolvedType::Str
        ));
        assert!(matches!(
            ast_type_to_resolved(&ast::Type::Simple("bool".to_string()), &type_params),
            ResolvedType::Bool
        ));
        assert!(matches!(
            ast_type_to_resolved(&ast::Type::Simple("T".to_string()), &type_params),
            ResolvedType::TypeVar(ref s) if s == "T"
        ));
        assert!(matches!(
            ast_type_to_resolved(&ast::Type::Unit, &type_params),
            ResolvedType::Unit
        ));
    }
}
