//! IR-based code generation facade
//!
//! This module provides `IrCodegen`, a unified API for generating Rust code from Incan AST using the IR pipeline:
//!
//! ```text
//! AST → AstLowering → IR → IrEmitter (quote!) → prettyplease → RustSource
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use incan::backend::IrCodegen;
//!
//! // Fallible API (recommended):
//! let codegen = IrCodegen::new();
//! let rust_code = codegen.try_generate(&ast)?;
//!
//! // Convenience API (returns error comments on failure):
//! let mut codegen = IrCodegen::new();
//! let rust_code = codegen.generate(&ast);
//! ```
//!
//! ## Error Handling
//!
//! The `try_generate*` family of methods return `Result<_, GenerationError>`,
//! allowing callers to handle lowering and emission errors explicitly.
//! The `generate*` methods are convenience wrappers that return error comments
//! on failure (useful for debugging but not recommended for production).

use std::collections::{HashMap, HashSet};
use std::env;
#[cfg(feature = "rust_inspect")]
use std::path::PathBuf;
use std::sync::Arc;

use crate::frontend::ast::{self, Declaration, Expr, ImportKind, ImportPath, Program};
use crate::frontend::decorator_resolution;
use crate::frontend::diagnostics::CompileError;
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::module::canonicalize_source_module_segments;
use crate::frontend::typechecker::stdlib_loader::StdlibAstCache;
use incan_core::lang::decorators::{self, DecoratorId};
use incan_core::lang::stdlib;
use incan_core::lang::traits::{self as core_traits, TraitId};

use super::scanners::{
    check_for_this_import as scan_check_for_this_import, collect_rust_crates as scan_collect_rust_crates,
    detect_serde_usage,
};
use super::{AstLowering, EmitError, EmitService, IrEmitter, LoweringErrors};

const SERDE_SERIALIZE_DERIVE: &str = "serde::Serialize";
const SERDE_DESERIALIZE_DERIVE: &str = "serde::Deserialize";

fn collect_model_field_aliases(main: &Program, deps: &[(&str, &Program)]) -> HashMap<String, HashMap<String, String>> {
    use crate::frontend::ast::Declaration;

    let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();

    let mut visit = |p: &Program| {
        for decl in &p.declarations {
            let Declaration::Model(m) = &decl.node else {
                continue;
            };

            let mut map: HashMap<String, String> = HashMap::new();
            for f in &m.fields {
                if let Some(alias) = &f.node.metadata.alias {
                    map.insert(alias.clone(), f.node.name.clone());
                }
            }

            if !map.is_empty() {
                out.entry(m.name.clone()).or_default().extend(map);
            }
        }
    };

    visit(main);
    for (_, dep) in deps {
        visit(dep);
    }

    out
}

/// Resolve a source import path to the generated Rust module path used for dependency emission.
fn generated_module_path_for_source_import(path: &ImportPath, current_module_path: &[String]) -> Option<Vec<String>> {
    let resolved_segments = if path.parent_levels > 0 {
        let keep = current_module_path.len().checked_sub(path.parent_levels)?;
        let mut resolved = current_module_path[..keep].to_vec();
        resolved.extend(path.segments.clone());
        resolved
    } else {
        path.segments.clone()
    };
    let mut segments = canonicalize_source_module_segments(&resolved_segments);

    if segments.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT) {
        segments[0] = stdlib::INCAN_STD_NAMESPACE.to_string();
    }

    Some(segments)
}

/// True when a dependency module should keep its public API even if the main module does not import every item.
fn should_preserve_dependency_public_items(module_path: &[String], preserve_non_stdlib_public_items: bool) -> bool {
    if matches!(
        module_path.first().map(String::as_str),
        Some(stdlib::STDLIB_ROOT | stdlib::INCAN_STD_NAMESPACE)
    ) {
        return true;
    }
    preserve_non_stdlib_public_items
}

/// Return whether a function carries the stdlib-backed web route decorator that lowers to a Rust proc-macro attribute.
///
/// Binary-style dependency emission prunes otherwise-unreferenced private items. Route handlers are different because
/// their Rust attribute expands into inventory registration after IR emission, so the function itself is a generated
/// entrypoint even when no Incan expression calls it directly.
fn has_web_route_passthrough_decorator(
    func: &ast::FunctionDecl,
    aliases: &HashMap<String, Vec<String>>,
    stdlib_cache: &mut StdlibAstCache,
) -> bool {
    func.decorators.iter().any(|decorator| {
        let resolved = decorator_resolution::resolve_decorator_path(&decorator.node, aliases);
        if resolved.len() < 2 {
            return false;
        }
        let module_segments = &resolved[..resolved.len() - 1];
        let name = &resolved[resolved.len() - 1];
        if name != "route" {
            return false;
        }
        let Some(meta) = stdlib_cache.lookup_function_meta(module_segments, name) else {
            return false;
        };
        meta.is_rust_extern && meta.rust_module_path.as_deref() == Some("incan_web_macros")
    })
}

/// Collect dependency-module declarations that are referenced through imports.
fn collect_externally_reachable_items_by_module(
    main: &Program,
    dependency_modules: &[(&str, &Program, Option<Vec<String>>)],
) -> HashMap<Vec<String>, HashSet<String>> {
    let module_paths: HashSet<Vec<String>> = dependency_modules
        .iter()
        .map(|(name, _, path_segments)| path_segments.clone().unwrap_or_else(|| vec![(*name).to_string()]))
        .collect();

    /// Record imported item names against the generated dependency module that owns them.
    fn record_imports(
        reachable: &mut HashMap<Vec<String>, HashSet<String>>,
        program: &Program,
        current_module_path: &[String],
        module_paths: &HashSet<Vec<String>>,
    ) {
        if crate::frontend::surface_semantics::uses_ambient_log_surface(program) {
            reachable
                .entry(vec!["std".to_string(), "logging".to_string()])
                .or_default()
                .insert("get_logger".to_string());
        }
        let mut module_import_bindings: HashMap<String, Vec<String>> = HashMap::new();
        for decl in &program.declarations {
            let Declaration::Import(import) = &decl.node else {
                continue;
            };
            match &import.kind {
                ImportKind::From { module, items } => {
                    let Some(module_path) = generated_module_path_for_source_import(module, current_module_path) else {
                        continue;
                    };
                    let reachable_items = reachable.entry(module_path).or_default();
                    for item in items {
                        reachable_items.insert(item.name.clone());
                    }
                }
                ImportKind::Module(path) => {
                    let Some(segments) = generated_module_path_for_source_import(path, current_module_path) else {
                        continue;
                    };
                    if module_paths.contains(&segments) {
                        if let Some(binding) = import.alias.clone().or_else(|| path.segments.last().cloned()) {
                            module_import_bindings.insert(binding, segments);
                        }
                        continue;
                    }
                    let Some(item_name) = segments.last() else {
                        continue;
                    };
                    for module_path in module_paths {
                        if segments.len() == module_path.len() + 1 && segments.starts_with(module_path) {
                            reachable
                                .entry(module_path.clone())
                                .or_default()
                                .insert(item_name.clone());
                            break;
                        }
                    }
                }
                ImportKind::PubLibrary { .. }
                | ImportKind::PubFrom { .. }
                | ImportKind::RustCrate { .. }
                | ImportKind::RustFrom { .. }
                | ImportKind::Python(_) => {}
            }
        }
        if !module_import_bindings.is_empty() {
            let _ = crate::frontend::ast_walk::any_expr_in_program(program, |expr| {
                if let Expr::Field(object, field) = expr
                    && let Expr::Ident(binding) = &object.node
                    && let Some(module_path) = module_import_bindings.get(binding)
                {
                    reachable.entry(module_path.clone()).or_default().insert(field.clone());
                }
                if let Expr::MethodCall(object, method, _, _) = expr
                    && let Expr::Ident(binding) = &object.node
                    && let Some(module_path) = module_import_bindings.get(binding)
                {
                    reachable.entry(module_path.clone()).or_default().insert(method.clone());
                }
                false
            });
        }
        if module_paths.contains(current_module_path) {
            let aliases = decorator_resolution::collect_import_aliases(program);
            let mut stdlib_cache = StdlibAstCache::new();
            for decl in &program.declarations {
                let Declaration::Function(func) = &decl.node else {
                    continue;
                };
                if has_web_route_passthrough_decorator(func, &aliases, &mut stdlib_cache) {
                    reachable
                        .entry(current_module_path.to_vec())
                        .or_default()
                        .insert(func.name.clone());
                }
            }
        }
    }

    let mut reachable = HashMap::new();
    record_imports(&mut reachable, main, &[String::from("main")], &module_paths);
    for (name, program, path_segments) in dependency_modules {
        let module_path = path_segments.clone().unwrap_or_else(|| vec![(*name).to_string()]);
        record_imports(&mut reachable, program, &module_path, &module_paths);
    }
    reachable
}

/// Dependency type facts gathered during codegen setup and reused by module emission.
///
/// Multi-file consumers only carry short nominal type names after typechecking/lowering, so emission cannot infer
/// imported-enum ownership rules from local IR alone. This metadata keeps a single codegen-owned source of truth for:
/// - dependency module qualification (`module_paths`)
/// - short-name collisions that must not be auto-qualified (`ambiguous_type_names`)
/// - imported enum names that are safe to treat as enum loop elements (`enum_type_names`)
/// - imported stdlib error types whose trait methods require Rust trait imports (`error_trait_type_names`)
#[derive(Debug, Clone, Default)]
struct DependencyTypeMetadata {
    module_paths: HashMap<String, Vec<String>>,
    ambiguous_type_names: HashSet<String>,
    enum_type_names: HashSet<String>,
    error_trait_type_names: HashSet<String>,
}

/// Collect dependency type metadata needed by IR emission for cross-module nominal types.
///
/// Enum loop ownership is the subtle case: imported enums lower to nominal `Struct(name)` references in consumer
/// modules, so the emitter cannot rely on local enum declarations when deciding whether `list[T]` loops should emit
/// `.iter().cloned()`. This helper records enum names from dependency modules while excluding ambiguous short names and
/// short names that are also used by non-enum dependency types.
fn collect_dependency_type_metadata(deps: &[(&str, &Program, Option<Vec<String>>)]) -> DependencyTypeMetadata {
    let mut paths: HashMap<String, Vec<String>> = HashMap::new();
    let mut ambiguous: HashSet<String> = HashSet::new();
    let mut enum_type_names: HashSet<String> = HashSet::new();
    let mut non_enum_type_names: HashSet<String> = HashSet::new();
    let mut error_trait_type_names: HashSet<String> = HashSet::new();
    let error_trait_name = core_traits::as_str(TraitId::Error);

    for (_name, program, path_segments) in deps {
        for decl in &program.declarations {
            let type_name = match &decl.node {
                Declaration::Model(m) => {
                    if m.traits.iter().any(|bound| bound.node.name == error_trait_name) {
                        error_trait_type_names.insert(m.name.clone());
                    }
                    Some((&m.name, false))
                }
                Declaration::Class(c) => {
                    if c.traits.iter().any(|bound| bound.node.name == error_trait_name) {
                        error_trait_type_names.insert(c.name.clone());
                    }
                    Some((&c.name, false))
                }
                Declaration::Enum(e) => Some((&e.name, true)),
                Declaration::TypeAlias(a) => Some((&a.name, false)),
                Declaration::Newtype(n) => Some((&n.name, false)),
                _ => None,
            };
            let Some((name, is_enum)) = type_name else {
                continue;
            };

            if is_enum {
                enum_type_names.insert(name.clone());
            } else {
                non_enum_type_names.insert(name.clone());
            }

            let Some(segs) = path_segments.as_ref() else {
                continue;
            };

            if let Some(existing) = paths.get(name) {
                if existing != segs {
                    ambiguous.insert(name.clone());
                }
            } else {
                paths.insert(name.clone(), segs.clone());
            }
        }
    }

    for name in &ambiguous {
        paths.remove(name);
    }
    enum_type_names.retain(|name| !ambiguous.contains(name) && !non_enum_type_names.contains(name));

    DependencyTypeMetadata {
        module_paths: paths,
        ambiguous_type_names: ambiguous,
        enum_type_names,
        error_trait_type_names,
    }
}

/// Return whether any loaded module derives serde serialize or deserialize through resolved JSON derive imports.
fn collect_serde_derives(main: &Program, deps: &[(&str, &Program)]) -> (bool, bool) {
    let mut has_serialize = false;
    let mut has_deserialize = false;

    let mut visit = |program: &Program| {
        let import_aliases = decorator_resolution::collect_import_aliases(program);
        for decl in &program.declarations {
            let decorators = match &decl.node {
                Declaration::Model(m) => Some(&m.decorators),
                Declaration::Class(c) => Some(&c.decorators),
                Declaration::Enum(e) => Some(&e.decorators),
                _ => None,
            };
            let Some(decorators) = decorators else {
                continue;
            };
            for dec in decorators {
                if decorators::from_str(dec.node.name.as_str()) != Some(DecoratorId::Derive) {
                    continue;
                }
                for arg in &dec.node.args {
                    let crate::frontend::ast::DecoratorArg::Positional(expr) = arg else {
                        continue;
                    };
                    let crate::frontend::ast::Expr::Ident(name) = &expr.node else {
                        continue;
                    };
                    let resolved = import_aliases
                        .get(name)
                        .cloned()
                        .unwrap_or_else(|| vec![name.to_string()]);
                    match resolved.as_slice() {
                        [std, serde, json] if std == "std" && serde == "serde" && json == "json" => {
                            has_serialize = true;
                            has_deserialize = true;
                        }
                        [std, serde, json, trait_name]
                            if std == "std" && serde == "serde" && json == "json" && trait_name == "Serialize" =>
                        {
                            has_serialize = true;
                        }
                        [std, serde, json, trait_name]
                            if std == "std" && serde == "serde" && json == "json" && trait_name == "Deserialize" =>
                        {
                            has_deserialize = true;
                        }
                        [serde, trait_name] if serde == "serde" && trait_name == "Serialize" => {
                            has_serialize = true;
                        }
                        [serde, trait_name] if serde == "serde" && trait_name == "Deserialize" => {
                            has_deserialize = true;
                        }
                        _ => {}
                    }
                }
            }
        }
    };

    visit(main);
    for (_, dep) in deps {
        visit(dep);
    }

    // Fallback: if no explicit serde derive was found but serde usage is detected (e.g. `json_stringify()` builtin), we
    // conservatively enable Serialize only.
    // Deserialize is NOT enabled here because implicit serde usage (like `json_stringify`)
    // only needs serialization, not deserialization.
    if !has_serialize && !has_deserialize {
        let serde_used = super::scanners::detect_serde_usage(main)
            || deps
                .iter()
                .any(|(_, program)| super::scanners::detect_serde_usage(program));
        if serde_used {
            has_serialize = true;
        }
    }

    (has_serialize, has_deserialize)
}

/// Add serde derives to generated newtypes when the current program needs serde support.
fn add_serde_to_newtypes(ir_program: &mut super::IrProgram, add_serialize: bool, add_deserialize: bool) {
    use super::decl::IrDeclKind;
    use super::types::IrType;

    /// Return whether a newtype inner type can safely receive derived serde support.
    fn is_conservative_serde_safe_newtype_inner(ty: &IrType) -> bool {
        match ty {
            IrType::Unit
            | IrType::Bool
            | IrType::Int
            | IrType::Float
            | IrType::String
            | IrType::Bytes
            | IrType::StaticStr
            | IrType::StaticBytes
            | IrType::FrozenStr
            | IrType::FrozenBytes
            | IrType::StrRef => true,
            IrType::List(inner) | IrType::Set(inner) | IrType::Option(inner) => {
                is_conservative_serde_safe_newtype_inner(inner)
            }
            IrType::Dict(key, value) | IrType::Result(key, value) => {
                is_conservative_serde_safe_newtype_inner(key) && is_conservative_serde_safe_newtype_inner(value)
            }
            IrType::Tuple(items) => items.iter().all(is_conservative_serde_safe_newtype_inner),
            _ => false,
        }
    }

    for decl in &mut ir_program.declarations {
        if let IrDeclKind::Struct(s) = &mut decl.kind
            && s.fields.len() == 1
            && s.fields[0].name == "0"
        {
            if !s.type_params.is_empty() {
                continue;
            }
            if !is_conservative_serde_safe_newtype_inner(&s.fields[0].ty) {
                continue;
            }
            if add_serialize && !s.derives.iter().any(|d| d == SERDE_SERIALIZE_DERIVE) {
                s.derives.push(SERDE_SERIALIZE_DERIVE.to_string());
            }
            if add_deserialize && !s.derives.iter().any(|d| d == SERDE_DESERIALIZE_DERIVE) {
                s.derives.push(SERDE_DESERIALIZE_DERIVE.to_string());
            }
        }
    }
}

/// Error during Rust code generation.
///
/// This error type wraps all possible errors that can occur during code generation,
/// including AST lowering errors and IR emission errors.
///
/// ## Examples
///
/// ```rust,ignore
/// use incan::backend::{IrCodegen, GenerationError};
///
/// let codegen = IrCodegen::new();
/// match codegen.try_generate(&ast) {
///     Ok(code) => println!("{}", code),
///     Err(GenerationError::Lowering(errors)) => {
///         for err in errors.iter() {
///             eprintln!("Lowering error: {}", err);
///         }
///     }
///     Err(GenerationError::Emission(e)) => eprintln!("Emission failed: {}", e),
/// }
/// ```
#[derive(Debug)]
pub enum GenerationError {
    /// Errors during frontend typechecking.
    TypeCheck(Vec<CompileError>),
    /// Errors during AST to IR lowering (may contain multiple errors)
    Lowering(LoweringErrors),
    /// Error during IR to Rust emission
    Emission(EmitError),
}

impl std::fmt::Display for GenerationError {
    /// Format generation errors for CLI and integration-test diagnostics.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GenerationError::TypeCheck(errs) => {
                if errs.is_empty() {
                    write!(f, "typecheck failed")
                } else {
                    // We intentionally avoid rich source formatting here (no file/source context at this layer), but
                    // include every message so generated-project stdlib failures are actionable.
                    let messages = errs
                        .iter()
                        .map(|err| err.message.as_str())
                        .collect::<Vec<_>>()
                        .join("; ");
                    write!(f, "typecheck failed ({} errors): {}", errs.len(), messages)
                }
            }
            GenerationError::Lowering(e) => write!(f, "{}", e),
            GenerationError::Emission(e) => write!(f, "emission error: {}", e),
        }
    }
}

impl std::error::Error for GenerationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GenerationError::TypeCheck(_) => None,
            GenerationError::Lowering(e) => Some(e),
            GenerationError::Emission(e) => Some(e),
        }
    }
}

impl From<LoweringErrors> for GenerationError {
    fn from(e: LoweringErrors) -> Self {
        GenerationError::Lowering(e)
    }
}

impl From<EmitError> for GenerationError {
    fn from(e: EmitError) -> Self {
        GenerationError::Emission(e)
    }
}

/// IR-based Rust code generator
///
/// This is the unified entrypoint for code generation. It uses the typed IR and syn/quote for code emission.
pub struct IrCodegen<'a> {
    /// The current program being generated
    current_program: Option<&'a Program>,
    /// Dependency modules to include before main.
    ///
    /// Stores both the flat module name (used for build graph identity) and the nested module path
    /// segments (used for correct Rust qualification in codegen).
    dependency_modules: Vec<(&'a str, &'a Program, Option<Vec<String>>)>,
    /// Whether serde is needed for emitted Rust derives or helpers.
    // Serde still affects emitted Rust imports and derive augmentation in IR emission, so this remains an
    // emission-internal signal even after project-level requirement collection moved to provider manifests.
    needs_serde: bool,
    /// Fixtures available for test functions (name -> (has_teardown, dependencies))
    fixtures: HashMap<String, (bool, Vec<String>)>,
    /// Rust crates imported via `import rust::` or `from rust::`
    rust_crates: HashSet<String>,
    /// Whether to emit the Zen of Incan at the start of main (set by `import this`)
    emit_zen_in_main: bool,
    /// Functions imported from external Rust crates (name -> true for external)
    external_rust_functions: HashSet<String>,
    /// Declared Rust crate names from `incan.toml [rust-dependencies]` (RFC 013 / RFC 023).
    ///
    /// When set, internal typechecking (used to obtain `TypeCheckInfo` for lowering) will validate `rust.module()`
    /// crate segments against this set.
    declared_crate_names: Option<HashSet<String>>,
    /// Consumer-side `pub::` dependency metadata used by internal typechecking.
    library_manifest_index: Option<Arc<LibraryManifestIndex>>,
    /// Whether generated Rust should deny warning classes that normal emission suppresses at narrow scopes.
    strict_generated_lints: bool,
    /// Private IR items called by generated code that is appended outside normal IR emission.
    externally_reachable_items: HashSet<String>,
    /// Whether non-stdlib dependency modules keep public items that are not otherwise reachable.
    preserve_dependency_public_items: bool,
    /// Manifest/workspace root for rust-inspect-backed typechecking during IR generation.
    #[cfg(feature = "rust_inspect")]
    rust_inspect_manifest_dir: Option<PathBuf>,
}

impl<'a> IrCodegen<'a> {
    /// Create a new IR-based code generator
    pub fn new() -> Self {
        Self {
            current_program: None,
            dependency_modules: Vec::new(),
            needs_serde: false,
            external_rust_functions: HashSet::new(),
            fixtures: HashMap::new(),
            rust_crates: HashSet::new(),
            emit_zen_in_main: false,
            declared_crate_names: None,
            library_manifest_index: None,
            strict_generated_lints: false,
            externally_reachable_items: HashSet::new(),
            preserve_dependency_public_items: true,
            #[cfg(feature = "rust_inspect")]
            rust_inspect_manifest_dir: None,
        }
    }

    /// Enable strict generated Rust lint validation for `--emit-rust --strict`.
    pub fn set_strict_generated_lints(&mut self, enabled: bool) {
        self.strict_generated_lints = enabled;
    }

    /// Set private generated Rust entrypoints called by code injected after IR emission.
    pub fn set_externally_reachable_items(&mut self, names: HashSet<String>) {
        self.externally_reachable_items = names;
    }

    /// Set whether non-stdlib dependency modules preserve their public API surface during emission.
    ///
    /// Library builds keep this enabled so public dependency declarations remain available at the Rust crate boundary.
    /// Binary and test harness builds can disable it so unused dependency declarations are pruned instead of warning.
    pub fn set_preserve_dependency_public_items(&mut self, enabled: bool) {
        self.preserve_dependency_public_items = enabled;
    }

    /// Set declared Rust crate names from `incan.toml [rust-dependencies]`. (RFC 031)
    ///
    /// This is used for validating `rust.module()` paths during the internal typechecking that precedes IR lowering.
    pub fn set_declared_crate_names(&mut self, names: HashSet<String>) {
        self.declared_crate_names = Some(names);
    }

    /// Set the consumer-side library manifest index for `pub::` import validation.
    pub fn set_library_manifest_index(&mut self, index: LibraryManifestIndex) {
        self.library_manifest_index = Some(Arc::new(index));
    }

    /// Set the manifest/workspace root used for rust-inspect-backed typechecking during IR generation.
    #[cfg(feature = "rust_inspect")]
    pub fn set_rust_inspect_manifest_dir(&mut self, dir: PathBuf) {
        self.rust_inspect_manifest_dir = Some(dir);
    }

    /// Get the Rust crates imported via `import rust::` or `from rust::`
    pub fn rust_crates(&self) -> &HashSet<String> {
        &self.rust_crates
    }

    /// Register a fixture for test code generation
    pub fn add_fixture(&mut self, name: &str, has_teardown: bool, dependencies: Vec<String>) {
        self.fixtures.insert(name.to_string(), (has_teardown, dependencies));
    }

    /// Check if serde is needed.
    #[cfg(test)]
    fn needs_serde(&self) -> bool {
        self.needs_serde
    }

    fn configure_typechecker(&self, tc: &mut crate::frontend::typechecker::TypeChecker) {
        if let Some(names) = self.declared_crate_names.clone() {
            tc.set_declared_crate_names(names);
        }
        if let Some(index) = self.library_manifest_index.clone() {
            tc.set_library_manifest_index_shared(index);
        }
        #[cfg(feature = "rust_inspect")]
        if let Some(dir) = self.rust_inspect_manifest_dir.clone() {
            tc.set_rust_inspect_manifest_dir(dir);
        }
    }

    /// Add a dependency module (for multi-file compilation)
    pub fn add_module(&mut self, module_name: &'a str, module_ast: &'a Program) {
        self.dependency_modules.push((module_name, module_ast, None));
    }

    /// Add a dependency module with its nested module path segments.
    ///
    /// This is used by the CLI multi-file nested mode where a module like `api.routes` is emitted as
    /// `crate::api::routes` in Rust (even though we may use a flattened name like `api_routes` for internal identity).
    pub fn add_module_with_path_segments(
        &mut self,
        module_name: &'a str,
        module_ast: &'a Program,
        path_segments: Vec<String>,
    ) {
        self.dependency_modules
            .push((module_name, module_ast, Some(path_segments)));
    }

    /// Backfill nested module path segments for a dependency module by name.
    ///
    /// This is primarily used by tests or older call sites that only registered a flat
    /// module name via `add_module()`. If a matching module entry exists and has no
    /// path segments yet, this sets them.
    pub fn set_module_path_segments(&mut self, module_name: &str, path_segments: Vec<String>) {
        if let Some((_name, _ast, segs)) = self
            .dependency_modules
            .iter_mut()
            .find(|(name, _, _)| *name == module_name)
            && segs.is_none()
        {
            *segs = Some(path_segments);
        }
    }

    // =========================================================================
    // Feature Detection
    // =========================================================================

    /// Scan a program for external Rust function imports
    fn collect_external_rust_functions(&mut self, program: &Program) {
        use crate::frontend::ast::{Declaration, ImportKind};

        for decl in &program.declarations {
            if let Declaration::Import(import) = &decl.node {
                match &import.kind {
                    // from rust::crate import items
                    ImportKind::RustFrom { items, .. } => {
                        for item in items {
                            let func_name = item.alias.as_ref().unwrap_or(&item.name);
                            self.external_rust_functions.insert(func_name.clone());
                        }
                    }
                    // Legacy: from rust::crate import items (parsed as From with rust:: module)
                    ImportKind::From { module, items }
                        if !module.segments.is_empty() && module.segments.first() == Some(&"rust".to_string()) =>
                    {
                        for item in items {
                            let func_name = item.alias.as_ref().unwrap_or(&item.name);
                            self.external_rust_functions.insert(func_name.clone());
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// Scan a program for serde-backed derives.
    ///
    /// This remains an internal compatibility hook because serde-backed derives and legacy
    /// `json_stringify` usage can still require serde emission without import-activated provider
    /// metadata.
    fn update_serde_requirement(&mut self, program: &Program) {
        if detect_serde_usage(program) {
            self.needs_serde = true;
        }
    }

    // (helper methods removed in favor of centralized scanners)

    /// Collect rust crates from imports
    fn collect_rust_crates(&mut self, program: &Program) {
        let crates = scan_collect_rust_crates(program);
        for c in crates {
            self.rust_crates.insert(c);
        }
    }

    /// Check for `import this`
    fn check_for_this_import(&mut self, program: &Program) {
        if scan_check_for_this_import(program) {
            self.emit_zen_in_main = true;
        }
    }

    // =========================================================================
    // Code Generation - Main Entry Points
    // =========================================================================

    /// Generate Rust code from an Incan program (single-file mode)
    ///
    /// This is the main entry point for code generation. It:
    /// 1. Scans for feature usage (serde, async, web, etc.)
    /// 2. Lowers the AST to IR
    /// 3. Emits Rust code using syn/quote
    /// 4. Formats with prettyplease
    ///
    /// **Note**: This is a convenience method that returns error comments on failure.
    /// For production use, prefer [`try_generate`](Self::try_generate) which returns
    /// a proper `Result`.
    #[tracing::instrument(skip_all)]
    pub fn generate(mut self, program: &'a Program) -> String {
        match self.try_generate_internal(program) {
            Ok(code) => code,
            Err(e) => format!("// Generation error: {}\n", e),
        }
    }

    /// Generate Rust code from an Incan program (single-file mode, fallible)
    ///
    /// This is the recommended entry point for code generation. It:
    /// 1. Scans for feature usage (serde, async, web, etc.)
    /// 2. Lowers the AST to IR
    /// 3. Emits Rust code using syn/quote
    /// 4. Formats with prettyplease
    ///
    /// ## Errors
    ///
    /// Returns `GenerationError::Lowering` if AST lowering fails, or
    /// `GenerationError::Emission` if IR emission fails.
    ///
    /// ## Examples
    ///
    /// ```rust,ignore
    /// use incan::backend::IrCodegen;
    ///
    /// let codegen = IrCodegen::new();
    /// let rust_code = codegen.try_generate(&ast)?;
    /// ```
    #[tracing::instrument(skip_all)]
    pub fn try_generate(mut self, program: &'a Program) -> Result<String, GenerationError> {
        self.try_generate_internal(program)
    }

    /// Internal implementation of try_generate (takes &mut self)
    fn try_generate_internal(&mut self, program: &'a Program) -> Result<String, GenerationError> {
        self.current_program = Some(program);

        // Scan for emission-relevant features
        self.update_serde_requirement(program);
        self.collect_rust_crates(program);
        self.check_for_this_import(program);
        self.collect_external_rust_functions(program);

        // Scan dependencies
        for (_mod_name, dep_ast, _mod_path_segments) in &self.dependency_modules.clone() {
            self.update_serde_requirement(dep_ast);
            self.collect_rust_crates(dep_ast);
            self.collect_external_rust_functions(dep_ast);
        }

        // Use the IR pipeline: AST → IR → Rust
        self.try_generate_via_ir(program, &HashSet::new())
    }

    /// Generate code via the IR pipeline (fallible version)
    fn try_generate_via_ir(
        &self,
        program: &Program,
        internal_module_roots: &HashSet<String>,
    ) -> Result<String, GenerationError> {
        self.try_generate_via_ir_with_union_config(program, internal_module_roots, HashMap::new(), false)
    }

    /// Generate code via the IR pipeline with optional crate-root union sharing for multi-file source modules.
    fn try_generate_via_ir_with_union_config(
        &self,
        program: &Program,
        internal_module_roots: &HashSet<String>,
        generated_union_types: HashMap<String, super::types::IrType>,
        qualify_union_types_from_crate: bool,
    ) -> Result<String, GenerationError> {
        let deps: Vec<(&str, &Program)> = self
            .dependency_modules
            .iter()
            .map(|(name, ast, _)| (*name, *ast))
            .collect();

        // RFC 021: Make alias-aware lowering work across module boundaries by seeding alias maps
        // for models declared in dependency modules as well.
        let global_aliases = collect_model_field_aliases(program, &deps);
        let dependency_type_metadata = collect_dependency_type_metadata(&self.dependency_modules);
        let (needs_serialize, needs_deserialize) = collect_serde_derives(program, &deps);

        // Typecheck to obtain reusable type information for lowering.
        //
        // Strict policy: if typechecking fails, do NOT proceed to lowering/codegen.
        let type_info_opt = {
            use crate::frontend::typechecker::TypeChecker;
            let mut tc = TypeChecker::new();
            self.configure_typechecker(&mut tc);
            match tc.check_with_imports(program, &deps) {
                Ok(()) => tc.type_info().clone(),
                Err(errs) => return Err(GenerationError::TypeCheck(errs)),
            }
        };

        // Lower AST to IR using typechecker output when available
        let mut lowering = AstLowering::new_with_type_info(type_info_opt);
        lowering.set_current_source_module_name(
            program
                .source_path
                .as_deref()
                .and_then(crate::frontend::module::logical_module_name_from_source_path),
        );
        lowering.seed_dependency_trait_decls(&self.dependency_modules);
        lowering.seed_struct_field_aliases(global_aliases.clone());
        let mut ir_program = lowering.lower_program(program)?;
        if self.needs_serde {
            add_serde_to_newtypes(&mut ir_program, needs_serialize, needs_deserialize);
        }

        // RFC 023: Infer trait bounds for generic functions.
        super::trait_bound_inference::infer_trait_bounds(&mut ir_program);

        // Build unified function registry including imported module functions
        let mut unified_registry = ir_program.function_registry.clone();
        let mut dependency_ir_programs = Vec::new();
        for (_, dep_ast, _) in &self.dependency_modules {
            // For dependencies, use best-effort lowering without type info to
            // preserve prior behavior and avoid redundant typechecking.
            let mut dep_lowering = AstLowering::new();
            dep_lowering.set_current_source_module_name(
                dep_ast
                    .source_path
                    .as_deref()
                    .and_then(crate::frontend::module::logical_module_name_from_source_path),
            );
            dep_lowering.seed_struct_field_aliases(global_aliases.clone());
            let dep_ir = dep_lowering.lower_program(dep_ast)?;
            unified_registry.merge(&dep_ir.function_registry);
            dependency_ir_programs.push(dep_ir);
        }

        // Emit IR to Rust code
        let use_emit_service = env::var("INCAN_EMIT_SERVICE").ok().as_deref() == Some("1");
        if use_emit_service {
            let mut svc = EmitService::new_from_program(&ir_program);
            // Configure inner emitter
            let inner = svc.inner_mut();
            inner.set_internal_module_roots(internal_module_roots.clone());
            if self.emit_zen_in_main {
                inner.set_emit_zen(true);
            }
            inner.set_type_module_paths(
                dependency_type_metadata.module_paths.clone(),
                dependency_type_metadata.ambiguous_type_names.clone(),
            );
            inner.set_dependency_enum_types(dependency_type_metadata.enum_type_names.clone());
            inner.set_external_error_trait_types(dependency_type_metadata.error_trait_type_names.clone());
            inner.set_needs_serde(self.needs_serde);
            inner.set_external_rust_functions(self.external_rust_functions.clone());
            inner.set_strict_generated_lints(self.strict_generated_lints);
            inner.set_externally_reachable_items(self.externally_reachable_items.clone());
            inner.set_qualify_union_types_from_crate(qualify_union_types_from_crate);
            inner.set_generated_union_types(generated_union_types);
            for dep_ir in &dependency_ir_programs {
                inner.seed_dependency_nominal_metadata_from_program(dep_ir);
            }
            Ok(svc.emit_program(&ir_program)?)
        } else {
            let mut emitter = IrEmitter::new(&unified_registry);
            emitter.set_internal_module_roots(internal_module_roots.clone());
            if self.emit_zen_in_main {
                emitter.set_emit_zen(true);
            }
            emitter.set_type_module_paths(
                dependency_type_metadata.module_paths.clone(),
                dependency_type_metadata.ambiguous_type_names.clone(),
            );
            emitter.set_dependency_enum_types(dependency_type_metadata.enum_type_names.clone());
            emitter.set_external_error_trait_types(dependency_type_metadata.error_trait_type_names.clone());
            emitter.set_needs_serde(self.needs_serde);
            emitter.set_external_rust_functions(self.external_rust_functions.clone());
            emitter.set_strict_generated_lints(self.strict_generated_lints);
            emitter.set_externally_reachable_items(self.externally_reachable_items.clone());
            emitter.set_qualify_union_types_from_crate(qualify_union_types_from_crate);
            emitter.set_generated_union_types(generated_union_types);
            for dep_ir in &dependency_ir_programs {
                emitter.seed_dependency_nominal_metadata_from_program(dep_ir);
            }
            Ok(emitter.emit_program(&ir_program)?)
        }
    }

    /// Generate Rust code for a dependency module (not the main module)
    ///
    /// **Note**: This is a convenience method that returns error comments on failure.
    /// For production use, prefer [`try_generate_module`](Self::try_generate_module).
    pub fn generate_module(&mut self, module_name: &str, program: &Program) -> String {
        match self.try_generate_module(module_name, program) {
            Ok(code) => code,
            Err(e) => format!("// Generation error: {}\n", e),
        }
    }

    /// Generate Rust code for a dependency module (not the main module, fallible)
    ///
    /// ## Errors
    ///
    /// Returns `GenerationError::Lowering` if AST lowering fails, or
    /// `GenerationError::Emission` if IR emission fails.
    pub fn try_generate_module(&mut self, _module_name: &str, program: &Program) -> Result<String, GenerationError> {
        // Use the IR pipeline for module generation too
        let mut lowering = AstLowering::new();
        lowering.set_current_source_module_name(
            program
                .source_path
                .as_deref()
                .and_then(crate::frontend::module::logical_module_name_from_source_path),
        );
        let mut ir_program = lowering.lower_program(program)?;

        // RFC 023: Infer trait bounds for generic functions.
        super::trait_bound_inference::infer_trait_bounds(&mut ir_program);

        // Best-effort: treat registered dependency module names as internal roots.
        // (This is most relevant for the non-nested multi-file API.)
        let internal_roots: HashSet<String> = self
            .dependency_modules
            .iter()
            .map(|(name, _, _)| (*name).to_string())
            .collect();

        let use_emit_service = env::var("INCAN_EMIT_SERVICE").ok().as_deref() == Some("1");
        if use_emit_service {
            let mut svc = EmitService::new_from_program(&ir_program);
            let inner = svc.inner_mut();
            inner.set_internal_module_roots(internal_roots);
            inner.set_externally_reachable_items(self.externally_reachable_items.clone());
            Ok(svc.emit_program(&ir_program)?)
        } else {
            let mut emitter = IrEmitter::new(&ir_program.function_registry);
            emitter.set_internal_module_roots(internal_roots);
            if self.emit_zen_in_main {
                emitter.set_emit_zen(true);
            }
            emitter.set_needs_serde(self.needs_serde);
            emitter.set_externally_reachable_items(self.externally_reachable_items.clone());
            Ok(emitter.emit_program(&ir_program)?)
        }
    }

    /// Generate Rust code for a multi-file project
    ///
    /// **Note**: This is a convenience method that returns error comments on failure.
    /// For production use, prefer [`try_generate_multi_file`](Self::try_generate_multi_file).
    pub fn generate_multi_file(
        mut self,
        program: &'a Program,
        module_names: &[&str],
    ) -> (String, HashMap<String, String>) {
        match self.try_generate_multi_file_internal(program, module_names) {
            Ok(result) => result,
            Err(e) => (format!("// Generation error: {}\n", e), HashMap::new()),
        }
    }

    /// Generate Rust code for a multi-file project (fallible)
    ///
    /// ## Errors
    ///
    /// Returns `GenerationError::Lowering` if AST lowering fails for any module, or
    /// `GenerationError::Emission` if IR emission fails for any module.
    pub fn try_generate_multi_file(
        mut self,
        program: &'a Program,
        module_names: &[&str],
    ) -> Result<(String, HashMap<String, String>), GenerationError> {
        self.try_generate_multi_file_internal(program, module_names)
    }

    /// Generate flat dependency modules with generated-use pruning.
    ///
    /// Dependency modules keep imported/reachable declarations for binary-style emission and can preserve non-stdlib
    /// public items when library surfaces are being generated.
    fn try_generate_multi_file_internal(
        &mut self,
        program: &'a Program,
        module_names: &[&str],
    ) -> Result<(String, HashMap<String, String>), GenerationError> {
        self.current_program = Some(program);

        // Scan all modules for emission-relevant features
        self.update_serde_requirement(program);
        self.collect_rust_crates(program);

        for (_mod_name, dep_ast, _mod_path_segments) in &self.dependency_modules.clone() {
            self.update_serde_requirement(dep_ast);
            self.collect_rust_crates(dep_ast);
        }

        let internal_roots: HashSet<String> = module_names.iter().map(|s| (*s).to_string()).collect();

        let deps: Vec<(&str, &Program)> = self
            .dependency_modules
            .iter()
            .map(|(name, ast, _)| (*name, *ast))
            .collect();
        let global_aliases = collect_model_field_aliases(program, &deps);
        let dependency_type_metadata = collect_dependency_type_metadata(&self.dependency_modules);
        let dependency_reachable_items =
            collect_externally_reachable_items_by_module(program, &self.dependency_modules);

        // Generate module files
        let mut lowered_modules = Vec::new();
        for (name, ast, path_segments) in &self.dependency_modules {
            if !module_names.contains(name) {
                continue;
            }
            let module_type_info = {
                use crate::frontend::typechecker::TypeChecker;
                let mut tc = TypeChecker::new();
                self.configure_typechecker(&mut tc);
                match tc.check_with_imports_allow_private(ast, &deps) {
                    Ok(()) => tc.type_info().clone(),
                    Err(errs) => return Err(GenerationError::TypeCheck(errs)),
                }
            };
            let mut lowering = AstLowering::new_with_type_info(module_type_info);
            lowering.set_current_source_module_name(Some(
                path_segments
                    .clone()
                    .unwrap_or_else(|| vec![name.to_string()])
                    .join("."),
            ));
            lowering.seed_dependency_trait_decls(&self.dependency_modules);
            lowering.seed_struct_field_aliases(global_aliases.clone());
            let mut ir = lowering.lower_program(ast)?;
            // Do not auto-add serde derives to dependency modules.
            // Global serde usage in the main module must not mutate unrelated dependency
            // newtypes (e.g., stdlib wrapper types like std.web.request.Query/Path).
            super::trait_bound_inference::infer_trait_bounds(&mut ir);
            let module_path = path_segments.clone().unwrap_or_else(|| vec![name.to_string()]);
            lowered_modules.push((name.to_string(), module_path, ir));
        }
        for idx in 0..lowered_modules.len() {
            let (left, rest) = lowered_modules.split_at_mut(idx);
            let Some((_, current_ir, tail)) = rest
                .split_first_mut()
                .map(|((name, _path, ir), tail)| (name.clone(), ir, tail))
            else {
                continue;
            };
            let external_programs: Vec<&super::IrProgram> = left
                .iter()
                .map(|(_, _, ir)| ir)
                .chain(tail.iter().map(|(_, _, ir)| ir))
                .collect();
            super::trait_bound_inference::propagate_trait_bounds_from_programs(current_ir, &external_programs);
        }
        let mut shared_union_types = HashMap::new();
        for (_, _, ir) in &lowered_modules {
            shared_union_types.extend(IrEmitter::collect_union_types_from_program(ir));
        }

        // Generate main file after dependency lowering so it can own shared crate-root union wrappers.
        let main_code =
            self.try_generate_via_ir_with_union_config(program, &internal_roots, shared_union_types, true)?;

        let mut modules = HashMap::new();
        for (name, module_path, ir) in &lowered_modules {
            let reachable_items = dependency_reachable_items.get(module_path).cloned().unwrap_or_default();
            let preserve_public_items =
                should_preserve_dependency_public_items(module_path, self.preserve_dependency_public_items);
            let use_emit_service = env::var("INCAN_EMIT_SERVICE").ok().as_deref() == Some("1");
            let module_code = if use_emit_service {
                let mut svc = EmitService::new_from_program(ir);
                let inner = svc.inner_mut();
                inner.set_internal_module_roots(internal_roots.clone());
                inner.set_preserve_public_items(preserve_public_items);
                inner.set_externally_reachable_items(reachable_items.clone());
                inner.set_type_module_paths(
                    dependency_type_metadata.module_paths.clone(),
                    dependency_type_metadata.ambiguous_type_names.clone(),
                );
                inner.set_dependency_enum_types(dependency_type_metadata.enum_type_names.clone());
                inner.set_external_error_trait_types(dependency_type_metadata.error_trait_type_names.clone());
                inner.set_external_rust_functions(self.external_rust_functions.clone());
                inner.set_qualify_union_types_from_crate(true);
                inner.set_emit_generated_union_definitions(false);
                for (_, _, dep_ir) in &lowered_modules {
                    inner.seed_dependency_nominal_metadata_from_program(dep_ir);
                }
                svc.emit_program(ir)?
            } else {
                let mut emitter = IrEmitter::new(&ir.function_registry);
                emitter.set_internal_module_roots(internal_roots.clone());
                emitter.set_preserve_public_items(preserve_public_items);
                emitter.set_externally_reachable_items(reachable_items);
                emitter.set_type_module_paths(
                    dependency_type_metadata.module_paths.clone(),
                    dependency_type_metadata.ambiguous_type_names.clone(),
                );
                emitter.set_dependency_enum_types(dependency_type_metadata.enum_type_names.clone());
                emitter.set_external_error_trait_types(dependency_type_metadata.error_trait_type_names.clone());
                emitter.set_external_rust_functions(self.external_rust_functions.clone());
                emitter.set_qualify_union_types_from_crate(true);
                emitter.set_emit_generated_union_definitions(false);
                for (_, _, dep_ir) in &lowered_modules {
                    emitter.seed_dependency_nominal_metadata_from_program(dep_ir);
                }
                emitter.emit_program(ir)?
            };
            modules.insert(name.clone(), module_code);
        }

        Ok((main_code, modules))
    }

    /// Generate Rust code for a multi-file project with nested module paths
    ///
    /// **Note**: This is a convenience method that returns error comments on failure.
    /// For production use, prefer [`try_generate_multi_file_nested`](Self::try_generate_multi_file_nested).
    pub fn generate_multi_file_nested(
        mut self,
        program: &'a Program,
        module_paths: &[Vec<String>],
    ) -> (String, HashMap<Vec<String>, String>) {
        match self.try_generate_multi_file_nested_internal(program, module_paths) {
            Ok(result) => result,
            Err(e) => (format!("// Generation error: {}\n", e), HashMap::new()),
        }
    }

    /// Generate Rust code for a multi-file project with nested module paths (fallible)
    ///
    /// ## Errors
    ///
    /// Returns `GenerationError::Lowering` if AST lowering fails for any module, or
    /// `GenerationError::Emission` if IR emission fails for any module.
    pub fn try_generate_multi_file_nested(
        mut self,
        program: &'a Program,
        module_paths: &[Vec<String>],
    ) -> Result<(String, HashMap<Vec<String>, String>), GenerationError> {
        self.try_generate_multi_file_nested_internal(program, module_paths)
    }

    /// Generate nested dependency modules with generated-use pruning.
    ///
    /// Dependency modules keep imported/reachable declarations for binary-style emission and can preserve non-stdlib
    /// public items when library surfaces are being generated.
    fn try_generate_multi_file_nested_internal(
        &mut self,
        program: &'a Program,
        module_paths: &[Vec<String>],
    ) -> Result<(String, HashMap<Vec<String>, String>), GenerationError> {
        self.current_program = Some(program);

        // Backfill nested module path segments for dependency modules when they were registered
        // via the legacy `add_module()` API (flat names only).
        //
        // The CLI typically registers both: a flat name like "api_routes" and the nested path
        // segments ["api", "routes"]. Tests may register only the flat name.
        for path in module_paths {
            let flat = path.join("_");
            if let Some((_name, _ast, segs)) = self
                .dependency_modules
                .iter_mut()
                .find(|(name, _, _)| *name == flat.as_str())
                && segs.is_none()
            {
                *segs = Some(path.clone());
            }
        }

        // Scan all modules for emission-relevant features
        self.update_serde_requirement(program);
        self.collect_rust_crates(program);

        for (_mod_name, dep_ast, _mod_path_segments) in &self.dependency_modules.clone() {
            self.update_serde_requirement(dep_ast);
            self.collect_rust_crates(dep_ast);
        }

        let internal_roots: HashSet<String> = module_paths.iter().filter_map(|p| p.first().cloned()).collect();

        let deps: Vec<(&str, &Program)> = self
            .dependency_modules
            .iter()
            .map(|(name, ast, _)| (*name, *ast))
            .collect();
        let global_aliases = collect_model_field_aliases(program, &deps);
        let dependency_type_metadata = collect_dependency_type_metadata(&self.dependency_modules);
        let dependency_reachable_items =
            collect_externally_reachable_items_by_module(program, &self.dependency_modules);

        // Generate module files by path
        let mut lowered_modules = Vec::new();
        for (name, ast, _) in &self.dependency_modules {
            // Find matching path by comparing joined segments with module name
            // Module name is path segments joined with "_" (e.g., "db_models")
            for path in module_paths {
                let path_name = path.join("_");
                if path_name != *name {
                    continue;
                }
                let module_type_info = {
                    use crate::frontend::typechecker::TypeChecker;
                    let mut tc = TypeChecker::new();
                    self.configure_typechecker(&mut tc);
                    match tc.check_with_imports_allow_private(ast, &deps) {
                        Ok(()) => tc.type_info().clone(),
                        Err(errs) => return Err(GenerationError::TypeCheck(errs)),
                    }
                };
                let mut lowering = AstLowering::new_with_type_info(module_type_info);
                lowering.set_current_source_module_name(Some(path.join(".")));
                lowering.seed_dependency_trait_decls(&self.dependency_modules);
                lowering.seed_struct_field_aliases(global_aliases.clone());
                let mut ir = lowering.lower_program(ast)?;
                // Do not auto-add serde derives to dependency modules.
                // Global serde usage in the main module must not mutate unrelated dependency
                // newtypes (e.g., stdlib wrapper types like std.web.request.Query/Path).
                super::trait_bound_inference::infer_trait_bounds(&mut ir);
                lowered_modules.push((path.clone(), ir));
                break;
            }
        }
        for idx in 0..lowered_modules.len() {
            let (left, rest) = lowered_modules.split_at_mut(idx);
            let Some((_, current_ir, tail)) = rest
                .split_first_mut()
                .map(|((path, ir), tail)| (path.clone(), ir, tail))
            else {
                continue;
            };
            let external_programs: Vec<&super::IrProgram> = left
                .iter()
                .map(|(_, ir)| ir)
                .chain(tail.iter().map(|(_, ir)| ir))
                .collect();
            super::trait_bound_inference::propagate_trait_bounds_from_programs(current_ir, &external_programs);
        }
        let mut shared_union_types = HashMap::new();
        for (_, ir) in &lowered_modules {
            shared_union_types.extend(IrEmitter::collect_union_types_from_program(ir));
        }

        // Generate main file after dependency lowering so it can own shared crate-root union wrappers.
        let main_code =
            self.try_generate_via_ir_with_union_config(program, &internal_roots, shared_union_types, true)?;

        let mut modules = HashMap::new();
        for (path, ir) in &lowered_modules {
            let reachable_items = dependency_reachable_items.get(path).cloned().unwrap_or_default();
            let preserve_public_items =
                should_preserve_dependency_public_items(path, self.preserve_dependency_public_items);
            let use_emit_service = env::var("INCAN_EMIT_SERVICE").ok().as_deref() == Some("1");
            let module_code = if use_emit_service {
                let mut svc = EmitService::new_from_program(ir);
                let inner = svc.inner_mut();
                inner.set_internal_module_roots(internal_roots.clone());
                inner.set_preserve_public_items(preserve_public_items);
                inner.set_externally_reachable_items(reachable_items.clone());
                inner.set_type_module_paths(
                    dependency_type_metadata.module_paths.clone(),
                    dependency_type_metadata.ambiguous_type_names.clone(),
                );
                inner.set_dependency_enum_types(dependency_type_metadata.enum_type_names.clone());
                inner.set_external_error_trait_types(dependency_type_metadata.error_trait_type_names.clone());
                inner.set_external_rust_functions(self.external_rust_functions.clone());
                inner.set_qualify_union_types_from_crate(true);
                inner.set_emit_generated_union_definitions(false);
                for (_, dep_ir) in &lowered_modules {
                    inner.seed_dependency_nominal_metadata_from_program(dep_ir);
                }
                svc.emit_program(ir)?
            } else {
                let mut emitter = IrEmitter::new(&ir.function_registry);
                emitter.set_internal_module_roots(internal_roots.clone());
                emitter.set_preserve_public_items(preserve_public_items);
                emitter.set_externally_reachable_items(reachable_items);
                emitter.set_type_module_paths(
                    dependency_type_metadata.module_paths.clone(),
                    dependency_type_metadata.ambiguous_type_names.clone(),
                );
                emitter.set_dependency_enum_types(dependency_type_metadata.enum_type_names.clone());
                emitter.set_external_error_trait_types(dependency_type_metadata.error_trait_type_names.clone());
                emitter.set_external_rust_functions(self.external_rust_functions.clone());
                emitter.set_qualify_union_types_from_crate(true);
                emitter.set_emit_generated_union_definitions(false);
                for (_, dep_ir) in &lowered_modules {
                    emitter.seed_dependency_nominal_metadata_from_program(dep_ir);
                }
                emitter.emit_program(ir)?
            };
            modules.insert(path.clone(), module_code);
        }

        Ok((main_code, modules))
    }
}

impl Default for IrCodegen<'_> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::library_manifest_index::{
        LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
    };
    use crate::frontend::{lexer, parser};
    use crate::library_manifest::{
        ConstExport, FunctionExport, LibraryManifest, ModelExport, ParamExport, ParamKindExport, TypeRef,
    };
    use std::collections::HashMap;
    #[cfg(feature = "rust_inspect")]
    use std::fs;

    fn must_ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => panic!("unexpected error: {err:?}"),
        }
    }

    fn must_some<T>(value: Option<T>, context: &str) -> T {
        match value {
            Some(v) => v,
            None => panic!("{context}"),
        }
    }

    fn generate(source: &str) -> String {
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));
        must_ok(IrCodegen::new().try_generate(&ast))
    }

    fn assert_no_generated_unused_lint_allows(code: &str) {
        assert!(!code.contains("#[allow(dead_code)]"), "{code}");
        assert!(!code.contains("#[allow(unused_imports)]"), "{code}");
        assert!(!code.contains("#[allow(dead_code, unused_variables)]"), "{code}");
    }

    #[test]
    fn partial_function_codegen_emits_wrapper_with_defaulted_preset() {
        let code = generate(
            r#"
pub def route(method: str, path: str) -> str:
  return method

pub get = partial route(method="GET")

pub def use() -> str:
  return get(path="/health")
"#,
        );
        assert!(code.contains("pub fn get("), "{code}");
        assert!(code.contains("\"GET\""), "{code}");
        assert!(code.contains("route("), "{code}");
        assert!(
            code.contains("get(\"GET\".to_string(), \"/health\".to_string())"),
            "{code}"
        );
    }

    #[test]
    fn local_partial_codegen_fills_omitted_preset_argument() {
        let code = generate(
            r#"
def route(method: str, path: str) -> str:
  return method + path

pub def use() -> str:
  get = partial route(method="GET")
  return get(path="/health")
"#,
        );
        assert!(code.contains("|method, path|"), "{code}");
        assert!(
            code.contains("get(\"GET\".to_string(), \"/health\".to_string())"),
            "{code}"
        );
    }

    #[test]
    fn partial_model_constructor_codegen_emits_wrapper_with_defaulted_preset() {
        let code = generate(
            r#"
pub model Reader:
  layer: str
  format: str

pub BronzeReader = partial Reader(layer="bronze", format="delta")

pub def use() -> Reader:
  return BronzeReader()
"#,
        );
        assert!(code.contains("pub fn BronzeReader("), "{code}");
        assert!(code.contains("\"bronze\""), "{code}");
        assert!(code.contains("\"delta\""), "{code}");
        assert!(code.contains("Reader {"), "{code}");
    }

    #[test]
    fn trait_method_partial_codegen_emits_default_method_wrapper() {
        let code = generate(
            r#"
trait Named:
  def label(self, prefix: str) -> str:
    return prefix
  short = partial label(prefix="name")

model User with Named:
  name: str

pub def use(user: User) -> str:
  return user.short()
"#,
        );
        assert!(code.contains("fn short"), "{code}");
        assert!(code.contains("return self.label(prefix);"), "{code}");
        assert!(code.contains("user.short(\"name\".to_string())"), "{code}");
    }

    #[test]
    fn method_partial_codegen_resolves_alias_target() {
        let code = generate(
            r#"
model User:
  name: str
  def label(self, prefix: str) -> str:
    return prefix
  display = label
  short = partial display(prefix="name")

pub def use(user: User) -> str:
  return user.short()
"#,
        );
        assert!(code.contains("fn short"), "{code}");
        assert!(code.contains("return self.label(&prefix);"), "{code}");
        assert!(code.contains("user.short(\"name\".to_string())"), "{code}");
    }

    #[test]
    fn normal_codegen_does_not_emit_blanket_generated_lint_allows() {
        let code = generate(
            r#"
def helper(value: int) -> int:
  return value

def main() -> None:
  return
"#,
        );

        assert!(!code.contains("#![allow(unused_imports, dead_code, unused_variables)]"));
        assert!(!code.contains("use incan_stdlib::prelude::*;"));
        assert!(!code.contains("use incan_derive::{FieldInfo, IncanClass};"));
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn top_level_callable_alias_lowers_calls_to_target_and_public_reexport() {
        let code = generate(
            r#"
pub def avg(x: int) -> int:
  return x

mean = avg
pub average = alias avg

def main() -> int:
  return mean(10)
"#,
        );
        assert!(code.contains("pub fn avg(x: i64) -> i64"), "{code}");
        assert!(code.contains("pub use avg as average;"), "{code}");
        assert!(code.contains("return avg(10);"), "{code}");
        assert!(!code.contains("fn mean"), "{code}");
    }

    #[test]
    fn top_level_qualified_alias_preserves_target_path() {
        let code = generate(
            r#"
import std.math as math

pub root = math.sqrt
"#,
        );
        assert!(code.contains("pub use crate::__incan_std::math as math;"), "{code}");
        assert!(code.contains("pub use math::sqrt as root;"), "{code}");
    }

    #[test]
    fn normal_codegen_keeps_used_private_helpers_without_dead_code_allows() {
        let code = generate(
            r#"
def helper(value: int) -> int:
  return value

def main() -> None:
  print(helper(1))
"#,
        );

        assert!(code.contains("fn helper(value: i64) -> i64"), "{code}");
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn normal_codegen_prunes_unused_private_helpers() {
        let code = generate(
            r#"
def helper(value: int) -> int:
  return value

def main() -> None:
  print("done")
"#,
        );

        assert!(!code.contains("fn helper"), "{code}");
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn normal_codegen_prunes_unused_dependency_public_items_for_binary_mode() {
        let constants_module = parse_program(
            r#"
pub def api_version() -> str:
  return "v1"

pub def max_page_size() -> int:
  return 100

pub def default_timeout() -> int:
  return 30
"#,
        );
        let main_module = parse_program(
            r#"
from shared.constants import api_version, max_page_size

def main() -> None:
  print(api_version())
  print(max_page_size())
"#,
        );
        let constants_path = vec!["shared".to_string(), "constants".to_string()];
        let mut codegen = IrCodegen::new();
        codegen.set_preserve_dependency_public_items(false);
        codegen.add_module_with_path_segments("shared_constants", &constants_module, constants_path.clone());

        let (_main_code, rust_modules) =
            must_ok(codegen.try_generate_multi_file_nested(&main_module, std::slice::from_ref(&constants_path)));
        let constants_code = must_some(
            rust_modules.get(&constants_path),
            "missing generated shared.constants module",
        );

        assert!(
            constants_code.contains("pub fn api_version() -> String"),
            "{constants_code}"
        );
        assert!(
            constants_code.contains("pub fn max_page_size() -> i64"),
            "{constants_code}"
        );
        assert!(!constants_code.contains("default_timeout"), "{constants_code}");
        assert_no_generated_unused_lint_allows(constants_code);
    }

    #[test]
    fn normal_codegen_preserves_stdlib_dependency_public_items_for_generated_projects() {
        let gzip_module = parse_program(
            r#"
pub def compress(data: bytes) -> bytes:
  return data

pub def decompress(data: bytes) -> bytes:
  return data
"#,
        );
        let main_module = parse_program(
            r#"
from std.compression.gzip import decompress

def main() -> None:
  _ = decompress(b"data")
"#,
        );
        let gzip_path = vec!["__incan_std".to_string(), "compression".to_string(), "gzip".to_string()];
        let mut codegen = IrCodegen::new();
        codegen.set_preserve_dependency_public_items(false);
        codegen.add_module_with_path_segments("__incan_std_compression_gzip", &gzip_module, gzip_path.clone());

        let (_main_code, rust_modules) =
            must_ok(codegen.try_generate_multi_file_nested(&main_module, std::slice::from_ref(&gzip_path)));
        let gzip_code = must_some(
            rust_modules.get(&gzip_path),
            "missing generated std.compression.gzip module",
        );

        assert!(gzip_code.contains("pub fn compress"), "{gzip_code}");
        assert!(gzip_code.contains("pub fn decompress"), "{gzip_code}");
        assert_no_generated_unused_lint_allows(gzip_code);
    }

    #[test]
    fn normal_codegen_can_preserve_dependency_public_items_for_library_mode() {
        let constants_module = parse_program(
            r#"
pub def api_version() -> str:
  return "v1"

pub def default_timeout() -> int:
  return 30
"#,
        );
        let main_module = parse_program(
            r#"
from shared.constants import api_version

def main() -> None:
  print(api_version())
"#,
        );
        let constants_path = vec!["shared".to_string(), "constants".to_string()];
        let mut codegen = IrCodegen::new();
        codegen.set_preserve_dependency_public_items(true);
        codegen.add_module_with_path_segments("shared_constants", &constants_module, constants_path.clone());

        let (_main_code, rust_modules) =
            must_ok(codegen.try_generate_multi_file_nested(&main_module, std::slice::from_ref(&constants_path)));
        let constants_code = must_some(
            rust_modules.get(&constants_path),
            "missing generated shared.constants module",
        );

        assert!(
            constants_code.contains("pub fn api_version() -> String"),
            "{constants_code}"
        );
        assert!(
            constants_code.contains("pub fn default_timeout() -> i64"),
            "{constants_code}"
        );
        assert_no_generated_unused_lint_allows(constants_code);
    }

    #[test]
    fn normal_codegen_keeps_external_generated_entrypoints() {
        let tokens = must_ok(lexer::lex(
            r#"
def test_generated_entrypoint() -> None:
  return
"#,
        ));
        let ast = must_ok(parser::parse(&tokens));
        let mut codegen = IrCodegen::new();
        codegen.set_externally_reachable_items(std::collections::HashSet::from([String::from(
            "test_generated_entrypoint",
        )]));
        let code = must_ok(codegen.try_generate(&ast));

        assert!(code.contains("fn test_generated_entrypoint"), "{code}");
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn normal_codegen_prunes_unused_rust_imports() {
        let code = generate(
            r#"
import rust::std::collections::HashMap

def main() -> None:
  print("done")
"#,
        );

        assert!(!code.contains("use std::collections::HashMap;"), "{code}");
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn normal_codegen_keeps_used_rust_import_aliases() {
        let code = generate(
            r#"
import rust::std::f64::consts as consts

def main() -> None:
  _ = consts.PI
"#,
        );

        assert!(code.contains("use std::f64::consts as consts;"), "{code}");
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn generated_use_analysis_keeps_rust_extension_trait_imports() {
        use crate::backend::ir::decl::{
            FunctionParam, IrFunction, IrImportItem, IrImportOrigin, IrImportQualifier, IrRustTraitImport, Visibility,
        };
        use crate::backend::ir::expr::{
            IrCallArg, IrCallArgKind, IrExprKind, MethodCallArgPolicy, VarAccess, VarRefKind,
        };
        use crate::backend::ir::{IrDecl, IrDeclKind, IrProgram, IrStmt, IrStmtKind, IrType, Mutability, TypedExpr};

        let mut program = IrProgram::new();
        program.declarations.push(IrDecl::new(IrDeclKind::Import {
            visibility: Visibility::Private,
            origin: IrImportOrigin::Standard,
            qualifier: IrImportQualifier::None,
            path: vec![String::from("rand")],
            alias: None,
            items: vec![
                IrImportItem {
                    name: String::from("Rng"),
                    alias: None,
                    rust_trait_import: Some(IrRustTraitImport {
                        trait_path: String::from("rand::Rng"),
                        definition_path: None,
                        methods: vec![String::from("gen_range")],
                    }),
                },
                IrImportItem {
                    name: String::from("thread_rng"),
                    alias: None,
                    rust_trait_import: None,
                },
            ],
        }));
        let rng_ty = IrType::Struct(String::from("rand::rngs::ThreadRng"));
        program.declarations.push(IrDecl::new(IrDeclKind::Function(IrFunction {
            name: String::from("main"),
            params: Vec::<FunctionParam>::new(),
            return_type: IrType::Unit,
            body: vec![
                IrStmt::new(IrStmtKind::Let {
                    name: String::from("rng"),
                    ty: rng_ty.clone(),
                    type_annotation: None,
                    mutability: Mutability::Mutable,
                    value: TypedExpr::new(
                        IrExprKind::Call {
                            func: Box::new(TypedExpr::new(
                                IrExprKind::Var {
                                    name: String::from("thread_rng"),
                                    access: VarAccess::Move,
                                    ref_kind: VarRefKind::ExternalRustName,
                                },
                                IrType::Function {
                                    params: Vec::new(),
                                    ret: Box::new(rng_ty.clone()),
                                },
                            )),
                            type_args: Vec::new(),
                            args: Vec::new(),
                            callable_signature: None,
                            canonical_path: None,
                        },
                        rng_ty.clone(),
                    ),
                }),
                IrStmt::new(IrStmtKind::Expr(TypedExpr::new(
                    IrExprKind::MethodCall {
                        receiver: Box::new(TypedExpr::new(
                            IrExprKind::Var {
                                name: String::from("rng"),
                                access: VarAccess::Read,
                                ref_kind: VarRefKind::Value,
                            },
                            rng_ty,
                        )),
                        method: String::from("gen_range"),
                        dispatch: None,
                        type_args: Vec::new(),
                        args: vec![IrCallArg {
                            name: None,
                            kind: IrCallArgKind::Positional,
                            expr: TypedExpr::new(
                                IrExprKind::Range {
                                    start: Some(Box::new(TypedExpr::new(IrExprKind::Int(1), IrType::Int))),
                                    end: Some(Box::new(TypedExpr::new(IrExprKind::Int(7), IrType::Int))),
                                    inclusive: false,
                                },
                                IrType::Unknown,
                            ),
                        }],
                        callable_signature: None,
                        arg_policy: MethodCallArgPolicy::Default,
                    },
                    IrType::Int,
                ))),
            ],
            is_async: false,
            is_generator: false,
            visibility: Visibility::Private,
            type_params: Vec::new(),
            is_extern: false,
            rust_attributes: Vec::new(),
            lint_allows: Vec::new(),
        })));

        let mut emitter = IrEmitter::new(&program.function_registry);
        let code = must_ok(emitter.emit_program(&program));

        assert!(code.contains("use ::rand::Rng;"), "{code}");
        assert!(code.contains("use ::rand::thread_rng;"), "{code}");
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn generated_use_analysis_keeps_only_selected_same_name_rust_extension_trait_import() {
        use crate::backend::ir::decl::{
            FunctionParam, IrFunction, IrImportItem, IrImportOrigin, IrImportQualifier, IrRustTraitImport, IrStruct,
            Visibility,
        };
        use crate::backend::ir::expr::{IrExprKind, IrMethodDispatch, MethodCallArgPolicy, VarAccess, VarRefKind};
        use crate::backend::ir::{IrDecl, IrDeclKind, IrProgram, IrStmt, IrStmtKind, IrType, Mutability, TypedExpr};

        let mut program = IrProgram::new();
        program.declarations.push(IrDecl::new(IrDeclKind::Import {
            visibility: Visibility::Private,
            origin: IrImportOrigin::Standard,
            qualifier: IrImportQualifier::None,
            path: vec![String::from("demo")],
            alias: None,
            items: vec![
                IrImportItem {
                    name: String::from("AlphaRender"),
                    alias: None,
                    rust_trait_import: Some(IrRustTraitImport {
                        trait_path: String::from("demo::AlphaRender"),
                        definition_path: None,
                        methods: vec![String::from("render")],
                    }),
                },
                IrImportItem {
                    name: String::from("BetaRender"),
                    alias: None,
                    rust_trait_import: Some(IrRustTraitImport {
                        trait_path: String::from("demo::BetaRender"),
                        definition_path: None,
                        methods: vec![String::from("render")],
                    }),
                },
            ],
        }));
        program.declarations.push(IrDecl::new(IrDeclKind::Struct(IrStruct {
            name: String::from("Widget"),
            fields: Vec::new(),
            derives: Vec::new(),
            visibility: Visibility::Private,
            type_params: Vec::new(),
            derive_rust_modules: std::collections::HashMap::new(),
            lint_allows: Vec::new(),
        })));
        let widget_ty = IrType::Struct(String::from("Widget"));
        program.declarations.push(IrDecl::new(IrDeclKind::Function(IrFunction {
            name: String::from("main"),
            params: Vec::<FunctionParam>::new(),
            return_type: IrType::Unit,
            body: vec![
                IrStmt::new(IrStmtKind::Let {
                    name: String::from("widget"),
                    ty: widget_ty.clone(),
                    type_annotation: None,
                    mutability: Mutability::Immutable,
                    value: TypedExpr::new(
                        IrExprKind::Struct {
                            name: String::from("Widget"),
                            fields: Vec::new(),
                        },
                        widget_ty.clone(),
                    ),
                }),
                IrStmt::new(IrStmtKind::Expr(TypedExpr::new(
                    IrExprKind::MethodCall {
                        receiver: Box::new(TypedExpr::new(
                            IrExprKind::Var {
                                name: String::from("widget"),
                                access: VarAccess::Read,
                                ref_kind: VarRefKind::Value,
                            },
                            widget_ty,
                        )),
                        method: String::from("render"),
                        dispatch: Some(IrMethodDispatch::RustExtensionTraitImport {
                            binding: String::from("AlphaRender"),
                        }),
                        type_args: Vec::new(),
                        args: Vec::new(),
                        callable_signature: None,
                        arg_policy: MethodCallArgPolicy::Default,
                    },
                    IrType::String,
                ))),
            ],
            is_async: false,
            is_generator: false,
            visibility: Visibility::Private,
            type_params: Vec::new(),
            is_extern: false,
            rust_attributes: Vec::new(),
            lint_allows: Vec::new(),
        })));

        let mut emitter = IrEmitter::new(&program.function_registry);
        let code = must_ok(emitter.emit_program(&program));

        assert!(code.contains("use ::demo::AlphaRender;"), "{code}");
        assert!(!code.contains("use ::demo::BetaRender;"), "{code}");
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn generated_use_analysis_keeps_rust_trait_candidates_without_metadata() {
        use crate::backend::ir::decl::{
            FunctionParam, IrFunction, IrImportItem, IrImportOrigin, IrImportQualifier, Visibility,
        };
        use crate::backend::ir::expr::{
            IrCallArg, IrCallArgKind, IrExprKind, MethodCallArgPolicy, VarAccess, VarRefKind,
        };
        use crate::backend::ir::{IrDecl, IrDeclKind, IrProgram, IrStmt, IrStmtKind, IrType, Mutability, TypedExpr};

        let mut program = IrProgram::new();
        program.declarations.push(IrDecl::new(IrDeclKind::Import {
            visibility: Visibility::Private,
            origin: IrImportOrigin::Standard,
            qualifier: IrImportQualifier::None,
            path: vec![String::from("rand")],
            alias: None,
            items: vec![
                IrImportItem {
                    name: String::from("Rng"),
                    alias: None,
                    rust_trait_import: None,
                },
                IrImportItem {
                    name: String::from("thread_rng"),
                    alias: None,
                    rust_trait_import: None,
                },
            ],
        }));
        let rng_ty = IrType::Struct(String::from("rand::rngs::ThreadRng"));
        program.declarations.push(IrDecl::new(IrDeclKind::Function(IrFunction {
            name: String::from("main"),
            params: Vec::<FunctionParam>::new(),
            return_type: IrType::Unit,
            body: vec![
                IrStmt::new(IrStmtKind::Let {
                    name: String::from("rng"),
                    ty: rng_ty.clone(),
                    type_annotation: None,
                    mutability: Mutability::Mutable,
                    value: TypedExpr::new(
                        IrExprKind::Call {
                            func: Box::new(TypedExpr::new(
                                IrExprKind::Var {
                                    name: String::from("thread_rng"),
                                    access: VarAccess::Move,
                                    ref_kind: VarRefKind::ExternalRustName,
                                },
                                IrType::Function {
                                    params: Vec::new(),
                                    ret: Box::new(rng_ty.clone()),
                                },
                            )),
                            type_args: Vec::new(),
                            args: Vec::new(),
                            callable_signature: None,
                            canonical_path: None,
                        },
                        rng_ty.clone(),
                    ),
                }),
                IrStmt::new(IrStmtKind::Expr(TypedExpr::new(
                    IrExprKind::MethodCall {
                        receiver: Box::new(TypedExpr::new(
                            IrExprKind::Var {
                                name: String::from("rng"),
                                access: VarAccess::Read,
                                ref_kind: VarRefKind::Value,
                            },
                            rng_ty,
                        )),
                        method: String::from("gen_range"),
                        dispatch: None,
                        type_args: Vec::new(),
                        args: vec![IrCallArg {
                            name: None,
                            kind: IrCallArgKind::Positional,
                            expr: TypedExpr::new(
                                IrExprKind::Range {
                                    start: Some(Box::new(TypedExpr::new(IrExprKind::Int(1), IrType::Int))),
                                    end: Some(Box::new(TypedExpr::new(IrExprKind::Int(7), IrType::Int))),
                                    inclusive: false,
                                },
                                IrType::Unknown,
                            ),
                        }],
                        callable_signature: None,
                        arg_policy: MethodCallArgPolicy::Default,
                    },
                    IrType::Int,
                ))),
            ],
            is_async: false,
            is_generator: false,
            visibility: Visibility::Private,
            type_params: Vec::new(),
            is_extern: false,
            rust_attributes: Vec::new(),
            lint_allows: Vec::new(),
        })));

        let mut emitter = IrEmitter::new(&program.function_registry);
        let code = must_ok(emitter.emit_program(&program));

        assert!(code.contains("use ::rand::Rng;"), "{code}");
        assert!(code.contains("use ::rand::thread_rng;"), "{code}");
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn generated_use_analysis_keeps_rust_trait_for_associated_method_on_rust_type() {
        use crate::backend::ir::decl::{
            FunctionParam, IrFunction, IrImportItem, IrImportOrigin, IrImportQualifier, IrRustTraitImport, Visibility,
        };
        use crate::backend::ir::expr::{
            IrCallArg, IrCallArgKind, IrExprKind, MethodCallArgPolicy, VarAccess, VarRefKind,
        };
        use crate::backend::ir::{IrDecl, IrDeclKind, IrProgram, IrStmt, IrStmtKind, IrType, TypedExpr};

        let mut program = IrProgram::new();
        program.declarations.push(IrDecl::new(IrDeclKind::Import {
            visibility: Visibility::Private,
            origin: IrImportOrigin::Standard,
            qualifier: IrImportQualifier::None,
            path: vec![String::from("sha2")],
            alias: None,
            items: vec![
                IrImportItem {
                    name: String::from("Digest"),
                    alias: None,
                    rust_trait_import: Some(IrRustTraitImport {
                        trait_path: String::from("sha2::Digest"),
                        definition_path: Some(String::from("digest::digest::Digest")),
                        methods: vec![String::from("digest")],
                    }),
                },
                IrImportItem {
                    name: String::from("Sha256"),
                    alias: None,
                    rust_trait_import: None,
                },
            ],
        }));
        program.declarations.push(IrDecl::new(IrDeclKind::Function(IrFunction {
            name: String::from("main"),
            params: Vec::<FunctionParam>::new(),
            return_type: IrType::Unit,
            body: vec![IrStmt::new(IrStmtKind::Expr(TypedExpr::new(
                IrExprKind::MethodCall {
                    receiver: Box::new(TypedExpr::new(
                        IrExprKind::Var {
                            name: String::from("Sha256"),
                            access: VarAccess::Copy,
                            ref_kind: VarRefKind::ExternalRustName,
                        },
                        IrType::Unknown,
                    )),
                    method: String::from("digest"),
                    dispatch: None,
                    type_args: Vec::new(),
                    args: vec![IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: TypedExpr::new(IrExprKind::Bytes(b"abc".to_vec()), IrType::Bytes),
                    }],
                    callable_signature: None,
                    arg_policy: MethodCallArgPolicy::Default,
                },
                IrType::Bytes,
            )))],
            is_async: false,
            is_generator: false,
            visibility: Visibility::Private,
            type_params: Vec::new(),
            is_extern: false,
            rust_attributes: Vec::new(),
            lint_allows: Vec::new(),
        })));

        let mut emitter = IrEmitter::new(&program.function_registry);
        let code = must_ok(emitter.emit_program(&program));

        assert!(code.contains("use ::sha2::Digest;"), "{code}");
        assert!(code.contains("use ::sha2::Sha256;"), "{code}");
        assert!(code.contains("Sha256::digest"), "{code}");
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn normal_codegen_expects_only_unread_private_model_fields() {
        let code = generate(
            r#"
model User:
  name: str
  age: int

def main() -> None:
  let user = User(name="Ada", age=42)
  print(user.name)
"#,
        );

        assert!(code.contains("name: String"), "{code}");
        assert!(
            code.contains(
                "#[expect(dead_code, reason = \"retained for Incan private field semantics\")]\n    age: i64"
            ),
            "{code}"
        );
        assert_no_generated_unused_lint_allows(&code);
    }

    #[test]
    fn generated_rust_warning_clean() -> Result<(), Box<dyn std::error::Error>> {
        use crate::backend::project::ProjectGenerator;
        use std::process::Command;

        let code = generate(
            r#"
import rust::std::f64::consts as consts

model User:
  name: str
  age: int

def helper(value: int) -> int:
  return value

def main() -> None:
  let user = User(name="Ada", age=42)
  print(user.name)
  print(helper(1))
  _ = consts.PI
"#,
        );
        assert_no_generated_unused_lint_allows(&code);

        let tmp = tempfile::tempdir()?;
        let generator = ProjectGenerator::new(tmp.path(), "warning_clean_codegen", true);
        generator.generate(&code)?;

        let output = Command::new("cargo")
            .arg("check")
            .current_dir(tmp.path())
            .env("CARGO_NET_OFFLINE", "true")
            .env("RUSTFLAGS", "-Dwarnings")
            .output()?;

        assert!(
            output.status.success(),
            "generated Rust should pass cargo check with -Dwarnings\nstderr:\n{}\nstdout:\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        Ok(())
    }

    #[test]
    fn normal_codegen_uses_underscore_for_unused_parameters() {
        let code = generate(
            r#"
def helper(value: int, unused: int) -> int:
  return value

def main() -> None:
  print(helper(1, 2))
"#,
        );

        assert!(code.contains("fn helper(value: i64, _: i64) -> i64"), "{code}");
        assert!(!code.contains("#[allow(unused_variables)]"), "{code}");
    }

    #[test]
    fn normal_codegen_uses_underscore_for_unused_locals() {
        let code = generate(
            r#"
def main() -> None:
  let unused = "value"
  print("done")
"#,
        );

        assert!(code.contains("let _unused = \"value\".to_string();"), "{code}");
        assert!(!code.contains("let unused = \"value\".to_string();"), "{code}");
        assert!(!code.contains("#[allow(unused_variables)]"), "{code}");
    }

    #[test]
    fn normal_codegen_unused_local_scan_respects_shadowing() {
        let code = generate(
            r#"
def main() -> None:
  let unused = "outer"
  if true:
    let unused = "inner"
    print(unused)
"#,
        );

        assert!(code.contains("let _unused = \"outer\".to_string();"), "{code}");
        assert!(code.contains("let unused = \"inner\".to_string();"), "{code}");
        assert!(!code.contains("#[allow(unused_variables)]"), "{code}");
    }

    #[test]
    fn strict_codegen_emits_denies_without_generated_scoped_allows() {
        let ast = parse_program(
            r#"
def helper(value: int) -> int:
  return value

def main() -> None:
  return
"#,
        );
        let mut codegen = IrCodegen::new();
        codegen.set_strict_generated_lints(true);
        let code = must_ok(codegen.try_generate(&ast));

        assert!(code.contains("#![deny(unused_imports, dead_code, unused_variables)]"));
        assert!(!code.contains("#![allow("));
        assert!(!code.contains("#[allow(dead_code"));
        assert!(!code.contains("#[allow(unused_variables"));
    }

    #[test]
    fn built_in_derive_macros_are_path_qualified() {
        let code = generate(
            r#"
model User:
  name: str

def main() -> None:
  let user = User(name="Ada")
  print(user.name)
"#,
        );

        assert!(code.contains("#[derive(Debug, Clone, incan_derive::FieldInfo, incan_derive::IncanClass)]"));
        assert!(!code.contains("use incan_derive::{FieldInfo, IncanClass};"));
    }

    /// Parse an Incan program into an AST
    fn parse_program(source: &str) -> Program {
        let tokens = must_ok(lexer::lex(source));
        must_ok(parser::parse(&tokens))
    }

    fn parse_program_result(source: &str) -> Result<Program, Box<dyn std::error::Error>> {
        let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        Ok(ast)
    }

    fn read_stdlib_program(path: &str) -> Result<Program, Box<dyn std::error::Error>> {
        let source = std::fs::read_to_string(path)?;
        parse_program_result(&source)
    }

    /// Parse and scan a source snippet to determine whether serde runtime support is required.
    fn detects_serde(source: &str) -> bool {
        let ast = parse_program(source);
        let mut codegen = IrCodegen::new();
        codegen.update_serde_requirement(&ast);
        codegen.needs_serde()
    }

    #[cfg(feature = "rust_inspect")]
    fn seeded_rust_inspect_workspace() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "ra_seeded_codegen_probe"
version = "0.1.0"
edition = "2021"
"#,
        )?;
        Ok(tmp)
    }

    #[cfg(feature = "rust_inspect")]
    fn reqwest_shaped_rust_inspect_workspace() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "reqwest"
version = "0.0.0"
edition = "2021"
"#,
        )?;
        fs::create_dir_all(tmp.path().join("src"))?;
        fs::write(
            tmp.path().join("src").join("lib.rs"),
            r#"
pub struct Client;

pub struct RequestBuilder;

pub trait IntoUrl {}

impl IntoUrl for &str {}

impl Client {
    pub fn new() -> Client {
        Client
    }

    pub fn post<U: IntoUrl>(&self, _url: U) -> RequestBuilder {
        RequestBuilder
    }
}

impl RequestBuilder {
    pub fn json<T: ?Sized>(self, _json: &T) -> RequestBuilder {
        self
    }
}
"#,
        )?;
        Ok(tmp)
    }

    #[cfg(feature = "rust_inspect")]
    fn prewarm_metadata(manifest_dir: &std::path::Path, paths: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
        let inspector =
            crate::rust_inspect::Inspector::new(crate::rust_inspect::InspectorConfig::new(manifest_dir.to_path_buf()));
        inspector.prewarm(paths.iter().map(|p| (*p).to_string()).collect::<Vec<_>>(), &|_| ())?;
        Ok(())
    }

    fn db_module_program() -> Program {
        parse_program(
            r#"
model Database:
  id: int
"#,
        )
    }

    fn main_module_program() -> Program {
        parse_program(
            r#"
def main() -> None:
  return
"#,
        )
    }

    fn library_index_with_widgets_exports() -> LibraryManifestIndex {
        let mut artifact_root = std::env::temp_dir();
        artifact_root.push("incan_test_widgets_artifacts");
        artifact_root.push("target");
        artifact_root.push("lib");

        let mut manifest = LibraryManifest::new("widgets_core", "0.1.0");
        manifest.exports.models.push(ModelExport {
            name: "Widget".to_string(),
            type_params: Vec::new(),
            traits: Vec::new(),
            trait_adoptions: Vec::new(),
            derives: Vec::new(),
            fields: Vec::new(),
            methods: Vec::new(),
        });
        manifest.exports.functions.push(FunctionExport {
            name: "make_widget".to_string(),
            type_params: Vec::new(),
            params: vec![ParamExport {
                name: "name".to_string(),
                ty: TypeRef::Named {
                    name: "str".to_string(),
                },
                kind: ParamKindExport::Normal,
                has_default: false,
            }],
            return_type: TypeRef::Named {
                name: "Widget".to_string(),
            },
            is_async: false,
        });
        manifest.exports.consts.push(ConstExport {
            name: "DEFAULT_NAME".to_string(),
            ty: TypeRef::Named {
                name: "str".to_string(),
            },
        });
        LibraryManifestIndex::from_entries(HashMap::from([(
            "widgets".to_string(),
            LibraryManifestIndexEntry::Loaded {
                manifest: Box::new(manifest),
                metadata: LibraryArtifactMetadata::from_crate_root("widgets", "widgets_core", artifact_root),
            },
        )]))
    }

    fn generate_nested_store_code(store_source: &str) -> String {
        let db_module = db_module_program();
        let store_module = parse_program(store_source);
        let main_module = main_module_program();

        let mut codegen = IrCodegen::new();
        codegen.add_module("db_schema", &db_module);
        codegen.add_module("store_json_store", &store_module);

        let db_path = vec!["db".to_string(), "schema".to_string()];
        let store_path = vec!["store".to_string(), "json_store".to_string()];
        let (_main_code, rust_modules) =
            must_ok(codegen.try_generate_multi_file_nested(&main_module, &[db_path.clone(), store_path.clone()]));

        must_some(rust_modules.get(&store_path), "missing generated nested store module").to_string()
    }

    fn generate_non_nested_store_code(store_source: &str, db_module_name: &str) -> String {
        let db_module = db_module_program();
        let store_module = parse_program(store_source);
        let main_module = main_module_program();

        let mut codegen = IrCodegen::new();
        codegen.add_module(db_module_name, &db_module);
        codegen.add_module("store", &store_module);

        let (_main_code, modules) = must_ok(codegen.try_generate_multi_file(&main_module, &[db_module_name, "store"]));

        must_some(modules.get("store"), "missing generated non-nested store module").to_string()
    }

    #[test]
    fn test_simple_function() {
        let code = generate(
            r#"
pub def add(a: int, b: int) -> int:
  return a + b
"#,
        );
        assert!(code.contains("fn add(a: i64, b: i64) -> i64"));
        assert!(code.contains("a + b"));
    }

    #[test]
    fn test_model_generation() {
        let code = generate(
            r#"
pub model User:
  pub name: str
  pub age: int
"#,
        );
        assert!(code.contains("struct User"));
        assert!(code.contains("name: String"));
        assert!(code.contains("age: i64"));
    }

    #[test]
    fn test_serde_detection() {
        let source = r#"
from std.serde import json

@derive(json)
model Config:
  name: str
"#;
        assert!(detects_serde(source));
    }

    #[test]
    fn test_serde_detection_single_derive() {
        let source = r#"
from std.serde.json import Serialize

@derive(Serialize)
model User:
  id: int
"#;
        assert!(detects_serde(source));
    }

    #[test]
    fn test_no_serde_when_not_used() {
        let source = r#"
@derive(Clone, Debug)
model User:
  id: int
"#;
        assert!(!detects_serde(source));
    }

    #[test]
    fn test_serde_detection_json_stringify_builtin() {
        let source = r#"
def main() -> None:
  _ = json_stringify(123)
"#;
        assert!(detects_serde(source));
    }

    #[test]
    fn test_serde_detection_json_stringify_in_if_condition() {
        let source = r#"
def main() -> None:
  if json_stringify(1) == "1":
    pass
"#;
        assert!(detects_serde(source));
    }

    #[test]
    fn test_serde_detection_json_stringify_in_elif_body() {
        let source = r#"
def main() -> None:
  if true:
    pass
  elif false:
    _ = json_stringify(1)
"#;
        assert!(detects_serde(source));
    }

    #[test]
    fn test_serde_detection_json_stringify_in_while_condition() {
        let source = r#"
def main() -> None:
  while json_stringify(1) == "1":
    break
"#;
        assert!(detects_serde(source));
    }

    #[test]
    fn test_serde_detection_json_stringify_in_for_iterator() {
        let source = r#"
def main() -> None:
  for item in [json_stringify(1)]:
    _ = item
"#;
        assert!(detects_serde(source));
    }

    #[test]
    fn test_fstring_generation() {
        let code = generate(
            r#"
pub def greet(name: str) -> str:
  return f"Hello, {name}!"
"#,
        );
        assert!(code.contains(r#"incan_stdlib::strings::fstring"#));
        assert!(code.contains(r#"["Hello, ", "!"]"#));
    }

    #[test]
    fn test_struct_instantiation() {
        let code = generate(
            r#"
model Point:
  x: int
  y: int

def main() -> None:
  p = Point(x=10, y=20)
"#,
        );
        assert!(code.contains("Point {"));
        assert!(code.contains("x: 10"));
        assert!(code.contains("y: 20"));
    }

    #[test]
    fn test_enum_generation() {
        let code = generate(
            r#"
pub enum Status:
  Active
  Inactive
"#,
        );
        assert!(code.contains("enum Status"));
        assert!(code.contains("Active"));
        assert!(code.contains("Inactive"));
    }

    #[test]
    fn test_multi_file_imports_use_crate_prefix() {
        let store_code = generate_nested_store_code(
            r#"
from db.schema import Database

pub def touch(db: Database) -> None:
  return
"#,
        );
        assert!(store_code.contains("use crate::db::schema::Database;"));
        assert!(!store_code.contains("use db::schema::Database;"));
    }

    #[test]
    fn test_multi_file_model_aliases_work_across_modules() {
        // DB module defines a model with an alias. Store module should be able to use the alias
        // in member access and constructor calls and still emit canonical Rust field names.
        let db_module = parse_program(
            r#"
model Account:
  type_ [alias="type"]: str
"#,
        );
        let store_module = parse_program(
            r#"
from db.schema import Account

pub def get_type(a: Account) -> str:
  return a.type

pub def make() -> Account:
  return Account(type="x")
"#,
        );
        let main_module = main_module_program();

        let mut codegen = IrCodegen::new();
        codegen.add_module("db_schema", &db_module);
        codegen.add_module("store_json_store", &store_module);

        let db_path = vec!["db".to_string(), "schema".to_string()];
        let store_path = vec!["store".to_string(), "json_store".to_string()];
        let (_main_code, rust_modules) =
            must_ok(codegen.try_generate_multi_file_nested(&main_module, &[db_path.clone(), store_path.clone()]));
        let store_code = must_some(rust_modules.get(&store_path), "missing generated store module").to_string();

        assert!(
            store_code.contains(".type_"),
            "expected canonical field access; got:\n{store_code}"
        );
        assert!(
            store_code.contains("Account { type_:"),
            "expected canonical struct field init; got:\n{store_code}"
        );
        assert!(
            !store_code.contains(".type;"),
            "should not emit Rust keyword field access"
        );
        assert!(
            !store_code.contains("Account { type:"),
            "should not emit Rust keyword field init"
        );
    }

    #[test]
    fn test_multi_file_model_aliases_work_with_import_alias() {
        let db_module = parse_program(
            r#"
model Account:
  type_ [alias="type"]: str
"#,
        );
        let store_module = parse_program(
            r#"
from db.schema import Account as A

pub def get_type(a: A) -> str:
  return a.type

pub def make() -> A:
  return A(type="x")
"#,
        );
        let main_module = main_module_program();

        let mut codegen = IrCodegen::new();
        codegen.add_module("db_schema", &db_module);
        codegen.add_module("store_json_store", &store_module);

        let db_path = vec!["db".to_string(), "schema".to_string()];
        let store_path = vec!["store".to_string(), "json_store".to_string()];
        let (_main_code, rust_modules) =
            must_ok(codegen.try_generate_multi_file_nested(&main_module, &[db_path.clone(), store_path.clone()]));
        let store_code = must_some(rust_modules.get(&store_path), "missing generated aliased store module").to_string();

        assert!(
            store_code.contains(".type_"),
            "expected canonical field access; got:\n{store_code}"
        );
        assert!(
            store_code.contains("A { type_:"),
            "expected canonical struct field init; got:\n{store_code}"
        );
    }

    #[test]
    fn test_multi_file_self_alias_resolution_in_dependency_module() {
        let db_module = parse_program(
            r#"
pub model Account:
  type_ [alias="type"]: str

  def get_type(self) -> str:
    return self.type
"#,
        );
        let main_module = main_module_program();

        let mut codegen = IrCodegen::new();
        codegen.add_module("db_schema", &db_module);

        let db_path = vec!["db".to_string(), "schema".to_string()];
        let (_main_code, rust_modules) =
            must_ok(codegen.try_generate_multi_file_nested(&main_module, std::slice::from_ref(&db_path)));
        let db_code = must_some(rust_modules.get(&db_path), "missing generated db module").to_string();

        assert!(
            db_code.contains("self.type_"),
            "expected canonical field access in dependency module; got:\n{db_code}"
        );
        assert!(
            !db_code.contains("self.type;"),
            "should not emit Rust keyword field access"
        );
    }

    #[test]
    fn test_same_named_stdlib_helpers_do_not_contaminate_nested_module_signatures()
    -> Result<(), Box<dyn std::error::Error>> {
        let main_module = parse_program_result(
            r#"
from std.testing import timeout
from std.async.time import timeout as async_timeout

def main() -> None:
  return
"#,
        )?;
        let testing_module = read_stdlib_program("crates/incan_stdlib/stdlib/testing.incn")?;
        let async_task_module = read_stdlib_program("crates/incan_stdlib/stdlib/async/task.incn")?;
        let async_time_module = read_stdlib_program("crates/incan_stdlib/stdlib/async/time.incn")?;
        let traits_error_module = read_stdlib_program("crates/incan_stdlib/stdlib/traits/error.incn")?;

        let testing_path = vec!["__incan_std".to_string(), "testing".to_string()];
        let async_task_path = vec!["__incan_std".to_string(), "async".to_string(), "task".to_string()];
        let async_time_path = vec!["__incan_std".to_string(), "async".to_string(), "time".to_string()];
        let traits_error_path = vec!["__incan_std".to_string(), "traits".to_string(), "error".to_string()];

        let mut codegen = IrCodegen::new();
        codegen.add_module_with_path_segments("__incan_std_testing", &testing_module, testing_path.clone());
        codegen.add_module_with_path_segments("__incan_std_async_task", &async_task_module, async_task_path.clone());
        codegen.add_module_with_path_segments("__incan_std_async_time", &async_time_module, async_time_path.clone());
        codegen.add_module_with_path_segments(
            "__incan_std_traits_error",
            &traits_error_module,
            traits_error_path.clone(),
        );

        let (_main_code, rust_modules) = codegen.try_generate_multi_file_nested(
            &main_module,
            &[
                testing_path.clone(),
                async_task_path,
                async_time_path,
                traits_error_path,
            ],
        )?;
        let testing_code = rust_modules
            .get(&testing_path)
            .ok_or_else(|| std::io::Error::other("missing generated std.testing module"))?;

        assert!(
            testing_code.contains("pub fn timeout(duration: String)"),
            "std.testing.timeout should remain a non-generic marker wrapper; got:\n{testing_code}"
        );
        assert!(
            !testing_code.contains("RuntimeFuture"),
            "std.testing wrapper should not inherit std.async.time.timeout bounds; got:\n{testing_code}"
        );
        Ok(())
    }

    #[test]
    fn imported_stdlib_trait_default_expands_in_dependency_impl() -> Result<(), Box<dyn std::error::Error>> {
        let main_module = parse_program_result(
            r#"
from std.io import BytesIO

def main() -> None:
  return
"#,
        )?;
        let io_module = read_stdlib_program("crates/incan_stdlib/stdlib/io.incn")?;
        let traits_error_module = read_stdlib_program("crates/incan_stdlib/stdlib/traits/error.incn")?;

        let io_path = vec!["__incan_std".to_string(), "io".to_string()];
        let traits_error_path = vec!["__incan_std".to_string(), "traits".to_string(), "error".to_string()];

        let mut codegen = IrCodegen::new();
        codegen.add_module_with_path_segments("__incan_std_io", &io_module, io_path.clone());
        codegen.add_module_with_path_segments(
            "__incan_std_traits_error",
            &traits_error_module,
            traits_error_path.clone(),
        );

        let (_main_code, rust_modules) =
            codegen.try_generate_multi_file_nested(&main_module, &[io_path.clone(), traits_error_path])?;
        let io_code = rust_modules
            .get(&io_path)
            .ok_or_else(|| std::io::Error::other("missing generated std.io module"))?;

        assert!(
            io_code.contains("impl Error for IoError"),
            "expected IoError to adopt std.traits.error.Error; got:\n{io_code}"
        );
        assert!(
            io_code.contains("fn source(&self) -> Option<String>"),
            "expected imported Error.source default method to expand into IoError impl; got:\n{io_code}"
        );
        Ok(())
    }

    #[test]
    fn test_rust_imports_do_not_use_crate_prefix() {
        let code = generate(
            r#"
from rust::time import Duration

pub def touch(duration: Duration) -> None:
  return
"#,
        );
        assert!(code.contains("use ::time::Duration;"));
        assert!(!code.contains("use crate::time::Duration;"));
    }

    #[test]
    fn test_rust_style_external_crate_import_is_not_forced_under_crate() {
        let code = generate(
            r#"
import serde::Serialize

pub def touch(value: Serialize) -> None:
  return
"#,
        );
        assert!(code.contains("use serde::Serialize;"));
        assert!(!code.contains("use crate::serde::Serialize;"));
    }

    #[test]
    fn test_relative_from_import_uses_super_prefix() {
        let store_code = generate_nested_store_code(
            r#"
from ..db.schema import Database

pub def touch(db: Database) -> None:
  return
"#,
        );
        assert!(store_code.contains("use super::db::schema::Database;"));
        assert!(!store_code.contains("use crate::db::schema::Database;"));
    }

    #[test]
    fn test_multi_file_imports_rust_style_module_import_uses_crate_prefix() {
        let store_code = generate_nested_store_code(
            r#"
import db::schema::Database

pub def touch(db: Database) -> None:
  return
"#,
        );
        assert!(store_code.contains("use crate::db::schema::Database;"));
        assert!(!store_code.contains("use db::schema::Database;"));
    }

    #[test]
    fn test_non_nested_multi_file_api_sets_internal_module_roots() {
        let store_code = generate_non_nested_store_code(
            r#"
from db import Database

pub def touch(db: Database) -> None:
  return
"#,
            "db",
        );
        assert!(store_code.contains("use crate::db::Database;"));
        assert!(!store_code.contains("use db::Database;"));
    }

    #[test]
    fn test_non_nested_multi_file_nested_modules_use_crate_prefix() {
        let store_code = generate_non_nested_store_code(
            r#"
from db.schema import Database

pub def touch(db: Database) -> None:
  return
"#,
            "db_schema",
        );
        assert!(store_code.contains("use crate::db::schema::Database;"));
        assert!(!store_code.contains("use db::schema::Database;"));
    }

    #[test]
    fn test_pub_from_import_emits_dependency_crate_item_paths() {
        let ast = parse_program(
            r#"
from pub::widgets import Widget as PublicWidget, make_widget

def main() -> None:
  w: PublicWidget = make_widget("ok")
"#,
        );
        let mut codegen = IrCodegen::new();
        codegen.set_library_manifest_index(library_index_with_widgets_exports());
        let code = must_ok(codegen.try_generate(&ast));
        assert!(code.contains("use widgets::Widget as PublicWidget;"));
        assert!(code.contains("use widgets::make_widget;"));
        assert!(!code.contains("pub use widgets::Widget as PublicWidget;"));
        assert!(!code.contains("pub use widgets::make_widget;"));
        assert!(!code.contains("pub::widgets"));
    }

    #[test]
    fn test_pub_import_expressions_codegen() {
        let source = r#"
from pub::widgets import Widget, make_widget, DEFAULT_NAME

def main() -> None:
  mut w: Widget = make_widget(DEFAULT_NAME)
"#;
        let ast = parse_program(source);
        let mut codegen = IrCodegen::new();
        codegen.set_library_manifest_index(library_index_with_widgets_exports());
        let code = must_ok(codegen.try_generate(&ast));
        assert!(
            code.contains("let _w: Widget = make_widget(DEFAULT_NAME);"),
            "Generated code did not match expected. Code was:\n{code}"
        );
    }

    #[test]
    fn test_pub_module_import_alias_emits_use_alias() {
        let ast = parse_program(
            r#"
import pub::widgets as widgets_alias

def main() -> None:
  widgets_alias.make_widget("ok")
"#,
        );
        let mut codegen = IrCodegen::new();
        codegen.set_library_manifest_index(library_index_with_widgets_exports());
        let code = must_ok(codegen.try_generate(&ast));
        assert!(code.contains("use widgets as widgets_alias;"));
        assert!(!code.contains("pub use widgets as widgets_alias;"));
        assert!(!code.contains("use pub::widgets"));
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_borrows_rust_backed_free_function_args_from_metadata() -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use incan_core::interop::{RustFunctionSig, RustItemKind, RustItemMetadata, RustParam, RustVisibility};

        let source = r#"
from rust::demo import Thing
from rust::demo import takes_ref

pub def forward(value: Thing) -> None:
  takes_ref(value)
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let tmp = seeded_rust_inspect_workspace()?;
        let manifest_dir = tmp.path().to_path_buf();
        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(manifest_dir.clone());
        tc.rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::takes_ref".to_string(),
                    definition_path: Some("demo::takes_ref".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Function(RustFunctionSig {
                        params: vec![RustParam {
                            name: Some("value".to_string()),
                            type_display: "&demo::Thing".to_string(),
                        }],
                        return_type: "()".to_string(),
                        is_async: false,
                        is_unsafe: false,
                    }),
                },
            )
            .map_err(|e| std::io::Error::other(format!("seed rust-inspect function: {e}")))?;
        tc.check_program(&ast)
            .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|err| std::io::Error::other(format!("lowering failed: {err:?}")))?;

        let mut codegen = IrCodegen::new();
        codegen.collect_external_rust_functions(&ast);

        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        emitter.set_external_rust_functions(codegen.external_rust_functions.clone());
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|err| std::io::Error::other(format!("emit failed: {err:?}")))?;

        assert!(
            code.contains("takes_ref(&value);"),
            "expected borrowed rust free-function arg in generated code; got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_borrows_rust_backed_method_args_from_metadata() -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use incan_core::interop::{
            RustFunctionSig, RustItemKind, RustItemMetadata, RustMethodSig, RustParam, RustTypeInfo, RustVisibility,
        };

        let source = r#"
from rust::demo import Builder

model Payload:
  name: str

pub def forward(payload: Payload) -> int:
  builder = Builder.new()
  return builder.json(payload)
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let tmp = seeded_rust_inspect_workspace()?;
        let manifest_dir = tmp.path().to_path_buf();
        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(manifest_dir.clone());
        tc.rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::Builder".to_string(),
                    definition_path: Some("demo::Builder".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Type(RustTypeInfo {
                        methods: vec![
                            RustMethodSig {
                                name: "new".to_string(),
                                signature: RustFunctionSig {
                                    params: Vec::new(),
                                    return_type: "demo::Builder".to_string(),
                                    is_async: false,
                                    is_unsafe: false,
                                },
                            },
                            RustMethodSig {
                                name: "json".to_string(),
                                signature: RustFunctionSig {
                                    params: vec![RustParam {
                                        name: Some("value".to_string()),
                                        type_display: "&T".to_string(),
                                    }],
                                    return_type: "i64".to_string(),
                                    is_async: false,
                                    is_unsafe: false,
                                },
                            },
                        ],
                        implemented_traits: Vec::new(),
                        fields: Vec::new(),
                        variants: Vec::new(),
                    }),
                },
            )
            .map_err(|e| std::io::Error::other(format!("seed rust-inspect type: {e}")))?;
        tc.check_program(&ast)
            .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|err| std::io::Error::other(format!("lowering failed: {err:?}")))?;

        let mut codegen = IrCodegen::new();
        codegen.collect_external_rust_functions(&ast);

        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        emitter.set_external_rust_functions(codegen.external_rust_functions.clone());
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|err| std::io::Error::other(format!("emit failed: {err:?}")))?;

        assert!(
            code.contains("builder.json(&payload);"),
            "expected borrowed rust method arg in generated code; got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_borrows_reqwest_json_payload_returned_from_registry_client()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;

        let source = r#"
from rust::reqwest import Client

model Payload:
  name: str

pub def forward(payload: Payload) -> None:
  builder = Client.new().post("https://example.invalid")
  _ = builder.json(payload)
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let tmp = reqwest_shaped_rust_inspect_workspace()?;
        let manifest_dir = tmp.path().to_path_buf();
        prewarm_metadata(&manifest_dir, &["reqwest::Client"])?;

        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(manifest_dir);
        tc.check_program(&ast)
            .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|err| std::io::Error::other(format!("lowering failed: {err:?}")))?;

        let mut codegen = IrCodegen::new();
        codegen.collect_external_rust_functions(&ast);

        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        emitter.set_external_rust_functions(codegen.external_rust_functions.clone());
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|err| std::io::Error::other(format!("emit failed: {err:?}")))?;

        assert!(
            code.contains("builder.json(&payload);"),
            "expected registry-returned reqwest RequestBuilder::json payload to be borrowed; got:\n{code}"
        );
        assert!(
            code.contains(r#"Client::new().post("https://example.invalid")"#),
            "expected generic reqwest Client::post string literal to keep inferable &str shape; got:\n{code}"
        );
        assert!(
            !code.contains(r#".post("https://example.invalid".into())"#),
            "generic reqwest Client::post must not force ambiguous `.into()` on string literals; got:\n{code}"
        );
        Ok(())
    }

    #[test]
    fn test_codegen_keeps_nested_rust_associated_calls_type_like_when_outer_receiver_is_unknown()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;

        let source = r#"
from rust::datafusion::execution::context import SessionContext
from rust::datafusion::dataframe import DataFrameWriteOptions

pub def f(uri: str) -> None:
  ctx = SessionContext.new()
  _ = ctx.write_csv(uri, DataFrameWriteOptions.new(), None)
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let mut tc = TypeChecker::new();
        tc.check_program(&ast)
            .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|err| std::io::Error::other(format!("lowering failed: {err:?}")))?;

        let mut codegen = IrCodegen::new();
        codegen.collect_external_rust_functions(&ast);

        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        emitter.set_external_rust_functions(codegen.external_rust_functions.clone());
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|err| std::io::Error::other(format!("emit failed: {err:?}")))?;

        assert!(
            code.contains("ctx.write_csv(&uri, DataFrameWriteOptions::new(), None::<_>);"),
            "expected nested rust associated call to keep :: syntax; got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_borrows_async_rust_backed_free_function_args_from_metadata()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use incan_core::interop::{RustFunctionSig, RustItemKind, RustItemMetadata, RustParam, RustVisibility};

        let source = r#"
from std.async import sleep
from rust::demo import State
from rust::demo import Plan
from rust::demo import consume

pub async def run(state: State, plan: Plan) -> None:
  await sleep(0.01)
  await consume(state, plan)
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let tmp = seeded_rust_inspect_workspace()?;
        let manifest_dir = tmp.path().to_path_buf();
        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(manifest_dir.clone());
        tc.rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: "demo::consume".to_string(),
                    definition_path: Some("demo::consume".to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Function(RustFunctionSig {
                        params: vec![
                            RustParam {
                                name: Some("state".to_string()),
                                type_display: "&demo::State".to_string(),
                            },
                            RustParam {
                                name: Some("plan".to_string()),
                                type_display: "&demo::Plan".to_string(),
                            },
                        ],
                        return_type: "()".to_string(),
                        is_async: true,
                        is_unsafe: false,
                    }),
                },
            )
            .map_err(|e| std::io::Error::other(format!("seed rust-inspect function: {e}")))?;
        tc.check_program(&ast)
            .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|err| std::io::Error::other(format!("lowering failed: {err:?}")))?;

        let mut codegen = IrCodegen::new();
        codegen.collect_external_rust_functions(&ast);

        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        emitter.set_external_rust_functions(codegen.external_rust_functions.clone());
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|err| std::io::Error::other(format!("emit failed: {err:?}")))?;

        assert!(
            code.contains("consume(&state, &plan).await"),
            "expected borrowed async rust free-function args in generated code; got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_borrows_async_rust_backed_free_function_args_from_real_rust_inspect()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use crate::rust_inspect::write_async_result_probe_crate;

        let source = r#"
from std.async import sleep
from rust::ra_async_result_probe import State
from rust::ra_async_result_probe import Plan
from rust::ra_async_result_probe import consume

pub async def run(state: State, plan: Plan) -> None:
  await sleep(0.01)
  await consume(state, plan)
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let tmp = tempfile::tempdir()?;
        write_async_result_probe_crate(tmp.path())?;

        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(tmp.path().to_path_buf());
        prewarm_metadata(
            tmp.path(),
            &[
                "ra_async_result_probe::State",
                "ra_async_result_probe::Plan",
                "ra_async_result_probe::consume",
            ],
        )?;
        tc.check_program(&ast)
            .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|err| std::io::Error::other(format!("lowering failed: {err:?}")))?;

        let mut codegen = IrCodegen::new();
        codegen.collect_external_rust_functions(&ast);

        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        emitter.set_external_rust_functions(codegen.external_rust_functions.clone());
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|err| std::io::Error::other(format!("emit failed: {err:?}")))?;

        assert!(
            code.contains("consume(&state, &plan).await"),
            "expected borrowed async rust free-function args from real metadata; got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_borrows_async_rust_backed_free_function_args_from_generated_lock_workspace()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::backend::project::ProjectGenerator;
        use crate::frontend::typechecker::TypeChecker;
        use crate::manifest::{DependencySource, DependencySpec};
        use crate::rust_inspect::write_hyphenated_function_probe_crate;

        let source = r#"
from std.async import sleep
from rust::foo_bar import State
from rust::foo_bar import Plan
from rust::foo_bar::consumer import consume

pub async def run(state: State, plan: Plan) -> None:
  await sleep(0.01)
  await consume(state, plan)
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let tmp = tempfile::tempdir()?;
        let dep_root = tmp.path().join("foo-bar-dep");
        write_hyphenated_function_probe_crate(&dep_root)?;

        let lock_root = tmp.path().join("generated_lock");
        let mut generator = ProjectGenerator::new(&lock_root, "lock_probe", true);
        generator.set_dependencies(vec![DependencySpec {
            crate_name: "foo-bar".to_string(),
            version: None,
            features: vec![],
            default_features: true,
            source: DependencySource::Path { path: dep_root.clone() },
            optional: false,
            package: None,
        }]);
        generator.generate("fn main() {}\n")?;

        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(lock_root.clone());
        prewarm_metadata(
            &lock_root,
            &["foo_bar::State", "foo_bar::Plan", "foo_bar::consumer::consume"],
        )?;
        tc.check_program(&ast)
            .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|err| std::io::Error::other(format!("lowering failed: {err:?}")))?;

        let mut codegen = IrCodegen::new();
        codegen.collect_external_rust_functions(&ast);

        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        emitter.set_external_rust_functions(codegen.external_rust_functions.clone());
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|err| std::io::Error::other(format!("emit failed: {err:?}")))?;

        assert!(
            code.contains("consume(&state, &plan).await"),
            "expected borrowed async rust free-function args from generated lock workspace; got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_nested_module_codegen_borrows_async_rust_args_from_generated_lock_workspace()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::backend::project::ProjectGenerator;
        use crate::manifest::{DependencySource, DependencySpec};
        use crate::rust_inspect::write_hyphenated_function_probe_crate;

        let main_module = parse_program(
            r#"
def main() -> None:
  return
"#,
        );
        let dep_module = parse_program(
            r#"
from std.async import sleep
from rust::foo_bar import State
from rust::foo_bar import Plan
from rust::foo_bar::consumer import consume

pub async def run(state: State, plan: Plan) -> None:
  await sleep(0.01)
  await consume(state, plan)
"#,
        );

        let tmp = tempfile::tempdir()?;
        let dep_root = tmp.path().join("foo-bar-dep");
        write_hyphenated_function_probe_crate(&dep_root)?;

        let lock_root = tmp.path().join("generated_lock");
        let mut generator = ProjectGenerator::new(&lock_root, "lock_probe", true);
        generator.set_dependencies(vec![DependencySpec {
            crate_name: "foo-bar".to_string(),
            version: None,
            features: vec![],
            default_features: true,
            source: DependencySource::Path { path: dep_root.clone() },
            optional: false,
            package: None,
        }]);
        generator.generate("fn main() {}\n")?;

        let worker_path = vec!["worker".to_string()];
        let mut codegen = IrCodegen::new();
        codegen.set_rust_inspect_manifest_dir(lock_root);
        codegen.add_module_with_path_segments("worker", &dep_module, worker_path.clone());

        let (_main_code, rust_modules) =
            must_ok(codegen.try_generate_multi_file_nested(&main_module, std::slice::from_ref(&worker_path)));
        let worker_code = must_some(rust_modules.get(&worker_path), "missing generated worker module");

        assert!(
            worker_code.contains("consume(&state, &plan).await"),
            "expected borrowed async rust free-function args in generated nested module; got:\n{worker_code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_codegen_borrows_async_rust_args_after_rust_method_return() -> Result<(), Box<dyn std::error::Error>> {
        use crate::frontend::typechecker::TypeChecker;
        use crate::rust_inspect::write_async_result_probe_crate;

        let source = r#"
from std.async import sleep
from rust::ra_async_result_probe import SessionContext
from rust::ra_async_result_probe import Plan
from rust::ra_async_result_probe import consume

pub async def run(plan: Plan) -> None:
  ctx = SessionContext.new()
  state = ctx.state()
  await sleep(0.01)
  await consume(state, plan)
"#;
        let tokens = must_ok(lexer::lex(source));
        let ast = must_ok(parser::parse(&tokens));

        let tmp = tempfile::tempdir()?;
        write_async_result_probe_crate(tmp.path())?;

        let mut tc = TypeChecker::new();
        tc.set_rust_inspect_manifest_dir(tmp.path().to_path_buf());
        prewarm_metadata(
            tmp.path(),
            &[
                "ra_async_result_probe::SessionContext",
                "ra_async_result_probe::Plan",
                "ra_async_result_probe::consume",
            ],
        )?;
        tc.check_program(&ast)
            .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

        let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
        let ir_program = lowering
            .lower_program(&ast)
            .map_err(|err| std::io::Error::other(format!("lowering failed: {err:?}")))?;

        let mut codegen = IrCodegen::new();
        codegen.collect_external_rust_functions(&ast);

        let mut emitter = IrEmitter::new(&ir_program.function_registry);
        emitter.set_external_rust_functions(codegen.external_rust_functions.clone());
        let code = emitter
            .emit_program(&ir_program)
            .map_err(|err| std::io::Error::other(format!("emit failed: {err:?}")))?;

        assert!(
            code.contains("consume(&state, &plan).await"),
            "expected borrowed async rust free-function args after rust method return; got:\n{code}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn test_ir_codegen_uses_configured_rust_inspect_workspace_for_async_borrows()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::rust_inspect::write_hyphenated_function_probe_crate;

        let tmp = tempfile::tempdir()?;
        let dep_root = tmp.path().join("foo-bar-dep");
        write_hyphenated_function_probe_crate(&dep_root)?;

        let host_root = tmp.path().join("host");
        std::fs::create_dir_all(host_root.join("src"))?;
        std::fs::write(
            host_root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"host\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies.foo_bar]\npackage = \"foo-bar\"\npath = \"{}\"\n",
                dep_root.display()
            ),
        )?;
        std::fs::write(host_root.join("src/lib.rs"), "pub fn touch() {}\n")?;

        let source = r#"
from std.async import sleep
from rust::foo_bar import State
from rust::foo_bar import Plan
from rust::foo_bar::consumer import consume

pub async def run(state: State, plan: Plan) -> None:
  await sleep(0.01)
  await consume(state, plan)
"#;
        let ast = parse_program(source);
        let mut codegen = IrCodegen::new();
        codegen.set_rust_inspect_manifest_dir(host_root);
        let code = must_ok(codegen.try_generate(&ast));

        assert!(
            code.contains("consume(&state, &plan).await"),
            "expected IrCodegen to preserve borrowed async args via the configured metadata workspace; got:\n{code}"
        );
        Ok(())
    }

    #[test]
    fn test_codegen_emits_explicit_function_call_type_args() {
        let source = r#"
def id[T](x: T) -> T:
  return x

pub def run() -> int:
  return id[int](1)
"#;
        let ast = parse_program(source);
        let code = must_ok(IrCodegen::new().try_generate(&ast));
        assert!(
            code.contains("id::<i64>(1)") || code.contains("id :: < i64 > (1)"),
            "expected explicit function type args to emit Rust turbofish, got:\n{code}"
        );
    }

    #[test]
    fn test_codegen_emits_explicit_method_call_type_args() {
        let source = r#"
class Box:
  def pick[T](self, value: T) -> T:
    return value

pub def run() -> int:
  let b = Box()
  return b.pick[int](1)
"#;
        let ast = parse_program(source);
        let code = must_ok(IrCodegen::new().try_generate(&ast));
        assert!(
            code.contains("pick::<i64>") || code.contains("pick :: < i64 >"),
            "expected explicit method type args to emit Rust turbofish, got:\n{code}"
        );
    }

    #[test]
    fn test_codegen_emits_full_turbofish_for_mixed_explicit_and_inferred_type_args() {
        let source = r#"
def pair_map[T, U](x: T, y: U) -> int:
  return 0

pub def run() -> int:
  return pair_map[int, _](1, 2)
"#;
        let ast = parse_program(source);
        let code = must_ok(IrCodegen::new().try_generate(&ast));
        assert!(
            code.contains("pair_map::<i64, i64>") || code.contains("pair_map :: < i64 , i64 >"),
            "expected full turbofish for mixed explicit/`_` call-site generics, got:\n{code}"
        );
    }
}
