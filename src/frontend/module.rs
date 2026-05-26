//! Canonical module resolution for multi-file Incan projects.
//!
//! This module owns source-module import classification, on-disk path resolution, and logical module identity
//! derivation. CLI, LSP, and test-runner orchestration may parse sources differently, but they should all ask this
//! module which imports point at local source modules and what logical module name those files carry.

use std::path::{Path, PathBuf};

use super::ast::{Declaration, ImportDecl, ImportKind, ImportPath, Program, Span, Visibility};
use incan_core::lang::stdlib;

/// A resolved local source module import.
///
/// `file_path` is the concrete source file to read. `path_segments` is the canonical logical module identity derived
/// from the source root and normalized so directory entrypoints such as `mod.incn` and `__init__.incn` do not become
/// semantic module names. `module_name` is the underscore-joined key used by existing typechecker/codegen adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceModuleRef {
    pub file_path: PathBuf,
    pub module_name: String,
    pub path_segments: Vec<String>,
}

impl SourceModuleRef {
    /// Build a resolved source-module reference from an on-disk file path and logical path segments.
    fn new(file_path: PathBuf, path_segments: Vec<String>) -> Self {
        let module_name = path_segments.join("_");
        Self {
            file_path,
            module_name,
            path_segments,
        }
    }
}

/// Classification for an import that may affect source-module graph traversal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceModuleImportResolution {
    /// Import resolved to a local Incan source file.
    Local(SourceModuleRef),
    /// Import points at an Incan stdlib source module. Callers that materialize stdlib source decide how to load it.
    Stdlib { module_path: Vec<String> },
    /// Import is not a source-backed Incan module, or no matching local source file exists.
    External,
}

/// One resolved import declaration inside a parsed program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedProgramSourceImport {
    pub span: Span,
    pub resolution: SourceModuleImportResolution,
}

/// Resolve every import declaration in `program` through the canonical source-module resolver.
///
/// `base_dir` is the directory of `program`'s source file. `source_root` is optional but should be supplied by CLI and
/// test-runner flows that already resolved the project source root; when it is absent, manifest/layout discovery is
/// used as a fallback for crate-root imports and source-root fallback behavior.
pub(crate) fn resolve_program_source_imports(
    program: &Program,
    base_dir: &Path,
    source_root: Option<&Path>,
) -> Vec<ResolvedProgramSourceImport> {
    program
        .declarations
        .iter()
        .flat_map(|decl| {
            let Declaration::Import(import) = &decl.node else {
                return Vec::new();
            };
            let mut resolved = vec![ResolvedProgramSourceImport {
                span: decl.span,
                resolution: resolve_source_module_import(base_dir, source_root, import),
            }];

            if let ImportKind::From { module, items } = &import.kind
                && module.parent_levels == 0
                && !module.is_absolute
                && module
                    .segments
                    .first()
                    .is_some_and(|segment| segment == stdlib::STDLIB_ROOT)
            {
                for item in items {
                    let mut item_module_path = module.segments.clone();
                    item_module_path.push(item.name.clone());
                    if stdlib::is_known_stdlib_module(&item_module_path) {
                        resolved.push(ResolvedProgramSourceImport {
                            span: decl.span,
                            resolution: SourceModuleImportResolution::Stdlib {
                                module_path: item_module_path,
                            },
                        });
                    }
                }
            }

            resolved
        })
        .collect()
}

/// Resolve one import declaration into local source, stdlib source, or external/non-source classification.
pub(crate) fn resolve_source_module_import(
    base_dir: &Path,
    source_root: Option<&Path>,
    import: &ImportDecl,
) -> SourceModuleImportResolution {
    let Some((path, candidates)) = source_import_candidates(import) else {
        return SourceModuleImportResolution::External;
    };

    if path.parent_levels == 0
        && !path.is_absolute
        && path
            .segments
            .first()
            .is_some_and(|segment| segment == stdlib::STDLIB_ROOT)
    {
        return SourceModuleImportResolution::Stdlib {
            module_path: path.segments.clone(),
        };
    }

    let identity_root = effective_source_root(base_dir, source_root);

    let primary_base = primary_import_base_dir(base_dir, source_root, path);
    if let Some(module_ref) = resolve_first_source_candidate(&primary_base, identity_root.as_deref(), &candidates) {
        return SourceModuleImportResolution::Local(module_ref);
    }

    if !path.is_absolute
        && path.parent_levels == 0
        && let Some(root) = identity_root
        && root != primary_base
        && let Some(module_ref) = resolve_first_source_candidate(&root, Some(&root), &candidates)
    {
        return SourceModuleImportResolution::Local(module_ref);
    }

    SourceModuleImportResolution::External
}

/// Resolve an `import` / `from ... import ...` into an on-disk Incan module file path.
///
/// This compatibility wrapper returns only local source files. New orchestration code should prefer
/// [`resolve_source_module_import`] or [`resolve_program_source_imports`] so stdlib and external imports are classified
/// explicitly.
pub fn resolve_import_path(base_dir: &Path, import: &ImportDecl) -> Option<PathBuf> {
    match resolve_source_module_import(base_dir, None, import) {
        SourceModuleImportResolution::Local(module_ref) => Some(module_ref.file_path),
        SourceModuleImportResolution::Stdlib { .. } | SourceModuleImportResolution::External => None,
    }
}

/// Return import path metadata plus candidate source-module paths for an import.
///
/// `from a.b import C` has one source module candidate, `a.b`. `import a.b.C` is ambiguous in existing syntax because
/// it can mean a module import or an item-style import; try the full path first, then the parent module. Keeping both
/// candidates here prevents CLI, LSP, and test-runner paths from drifting on that ambiguity.
fn source_import_candidates(import: &ImportDecl) -> Option<(&ImportPath, Vec<Vec<String>>)> {
    match &import.kind {
        ImportKind::From { module, .. } if !module.segments.is_empty() => Some((module, vec![module.segments.clone()])),
        ImportKind::Module(path) if !path.segments.is_empty() => {
            let mut candidates = vec![path.segments.clone()];
            if path.segments.len() > 1 {
                let parent = path.segments[..path.segments.len() - 1].to_vec();
                if parent != path.segments {
                    candidates.push(parent);
                }
            }
            Some((path, candidates))
        }
        ImportKind::RustCrate { .. }
        | ImportKind::RustFrom { .. }
        | ImportKind::PubLibrary { .. }
        | ImportKind::PubFrom { .. }
        | ImportKind::Python(_)
        | ImportKind::Module(_)
        | ImportKind::From { .. } => None,
    }
}

/// Determine the primary base directory for resolving an import path.
fn primary_import_base_dir(base_dir: &Path, source_root: Option<&Path>, path: &ImportPath) -> PathBuf {
    if path.is_absolute {
        return effective_source_root(base_dir, source_root)
            .unwrap_or_else(|| discover_source_root_from_layout(base_dir));
    }

    let mut base = base_dir.to_path_buf();
    for _ in 0..path.parent_levels {
        base = base.parent().map(Path::to_path_buf).unwrap_or(base);
    }
    base
}

/// Find the effective source root for imports when one is known or discoverable.
fn effective_source_root(base_dir: &Path, source_root: Option<&Path>) -> Option<PathBuf> {
    source_root
        .map(Path::to_path_buf)
        .or_else(|| resolve_source_root_for_imports(base_dir))
}

/// Infer a source root from conventional project layout when no manifest-driven root is available.
fn discover_source_root_from_layout(base_dir: &Path) -> PathBuf {
    let mut project_root = base_dir.to_path_buf();
    while !project_root.join("Cargo.toml").exists() && !project_root.join("src").exists() {
        if !project_root.pop() {
            break;
        }
    }
    if project_root.join("src").exists() {
        project_root.join("src")
    } else {
        project_root
    }
}

/// Resolve the first candidate path that exists under `base`.
fn resolve_first_source_candidate(
    base: &Path,
    identity_root: Option<&Path>,
    candidates: &[Vec<String>],
) -> Option<SourceModuleRef> {
    for candidate in candidates {
        if candidate.is_empty() {
            continue;
        }
        if let Some(file_path) = resolve_module_path_from_base(base, candidate) {
            let logical_segments = identity_root
                .and_then(|root| logical_module_segments_from_file(root, &file_path))
                .or_else(|| logical_module_segments_from_file(base, &file_path))
                .unwrap_or_else(|| candidate.clone());
            return Some(SourceModuleRef::new(file_path, logical_segments));
        }
    }
    None
}

/// Canonicalize source-module path segments.
///
/// `mod` and `__init__` are file-layout entrypoints for directory-backed modules, not semantic module names. Normalize
/// them away so the logical module identity stays consistent anywhere the compiler converts source-backed module paths
/// into logical module IDs.
pub(crate) fn canonicalize_source_module_segments(segments: &[String]) -> Vec<String> {
    match segments.last().map(String::as_str) {
        Some("mod" | "__init__") => segments[..segments.len().saturating_sub(1)].to_vec(),
        _ => segments.to_vec(),
    }
}

/// Derive the logical module path segments for an on-disk source module relative to `base`.
///
/// Examples:
/// - `src/foo.incn` => `["foo"]`
/// - `src/foo/bar.incn` => `["foo", "bar"]`
/// - `src/foo/mod.incn` => `["foo"]`
/// - `src/foo/bar/mod.incn` => `["foo", "bar"]`
pub(crate) fn logical_module_segments_from_file(base: &Path, module_file: &Path) -> Option<Vec<String>> {
    let relative = if let Ok(relative) = module_file.strip_prefix(base) {
        relative.to_path_buf()
    } else {
        let canonical_base = base.canonicalize().ok()?;
        let canonical_file = module_file.canonicalize().ok()?;
        canonical_file.strip_prefix(&canonical_base).ok()?.to_path_buf()
    };
    let mut segments = Vec::new();

    for component in relative.components() {
        let part = component.as_os_str().to_str()?;
        let path = Path::new(part);
        let stem = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or(part);
        segments.push(stem.to_string());
    }

    Some(canonicalize_source_module_segments(&segments))
}

/// Derive a logical module name from parser-supplied source metadata.
///
/// This is a fallback for single-file and test flows that have a source path but have not already built the full
/// source-module graph. Multi-file orchestration should prefer explicit `path_segments` from [`SourceModuleRef`].
pub(crate) fn logical_module_name_from_source_path(source_path: &str) -> Option<String> {
    let path = Path::new(source_path);
    let mut segments: Vec<String> = Vec::new();
    let mut seen_src = false;
    for component in path.components() {
        let value = component.as_os_str().to_string_lossy();
        if seen_src {
            segments.push(value.to_string());
        } else if value == "src" {
            seen_src = true;
        }
    }
    if !seen_src {
        return None;
    }
    let file = segments.pop()?;
    let stem = Path::new(&file).file_stem()?.to_string_lossy().to_string();
    if stem != "mod" && stem != "__init__" {
        segments.push(stem);
    }
    if segments.is_empty() {
        Some("main".to_string())
    } else {
        Some(segments.join("."))
    }
}

/// Resolves an Incan module file under `base` from import path segments (e.g. `foo.bar` → `foo/bar`).
///
/// Tries, in order: `segments.incn`, `segments.incan`, `segments/mod.incn`, `segments/mod.incan`,
/// `segments/__init__.incn`, `segments/__init__.incan`. Returns the first path that exists on disk, canonicalized when
/// possible. Returns `None` if none match.
fn resolve_module_path_from_base(base: &Path, path: &[String]) -> Option<PathBuf> {
    // Build file path from segments
    let mut file_path = base.to_path_buf();
    for segment in path {
        file_path = file_path.join(segment);
    }

    // Try .incn extension first (preferred)
    let mut with_ext = file_path.clone();
    with_ext.set_extension("incn");
    if with_ext.exists() {
        return Some(with_ext.canonicalize().unwrap_or(with_ext));
    }

    // Try .incan extension (legacy/alternate)
    with_ext.set_extension("incan");
    if with_ext.exists() {
        return Some(with_ext.canonicalize().unwrap_or(with_ext));
    }

    // Try as directory with mod.incn
    let mod_file = file_path.join("mod.incn");
    if mod_file.exists() {
        return Some(mod_file.canonicalize().unwrap_or(mod_file));
    }

    // Try as directory with mod.incan (legacy/alternate)
    let mod_file_legacy = file_path.join("mod.incan");
    if mod_file_legacy.exists() {
        return Some(mod_file_legacy.canonicalize().unwrap_or(mod_file_legacy));
    }

    // Try as directory with __init__.incn (Python-style entrypoint)
    let init_file = file_path.join("__init__.incn");
    if init_file.exists() {
        return Some(init_file.canonicalize().unwrap_or(init_file));
    }

    // Try as directory with __init__.incan (legacy/alternate)
    let init_file_legacy = file_path.join("__init__.incan");
    if init_file_legacy.exists() {
        return Some(init_file_legacy.canonicalize().unwrap_or(init_file_legacy));
    }

    None
}

/// Returns the project's configured or conventional source root for resolving unqualified module imports.
///
/// Walks up from `start_dir` to find `incan.toml`, then uses `[build] source-root` when set; otherwise `src/` if it
/// exists, else the project root. Returns `None` if no manifest is found.
fn resolve_source_root_for_imports(start_dir: &Path) -> Option<PathBuf> {
    let manifest = crate::manifest::ProjectManifest::discover(start_dir).ok().flatten()?;
    let project_root = manifest.project_root().to_path_buf();
    if let Some(custom) = manifest.build.as_ref().and_then(|build| build.source_root.as_deref()) {
        return Some(project_root.join(custom));
    }
    let src_root = project_root.join("src");
    if src_root.exists() {
        Some(src_root)
    } else {
        Some(project_root)
    }
}

/// Extract symbols exported by a module.
///
/// Visibility is enforced for declarations (`pub` only), and module-level `from ... import ...` statements are treated
/// as re-exports by name (including aliases).
///
/// This follows Python semantics: `from foo import bar` at module level makes `bar` part of the module's public
/// surface. The primary use case is stdlib prelude files (e.g., `std.web/prelude.incn`) that re-export items from
/// submodules, but it applies uniformly to all modules. The consumer of this list (`validate_import_visibility` in
/// the typechecker) uses it to check whether `from some_module import X` is valid.
pub fn exported_symbols(ast: &Program) -> Vec<ExportedSymbol> {
    let mut exports = Vec::new();

    for decl in &ast.declarations {
        match &decl.node {
            Declaration::Const(c) => {
                if matches!(c.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Const(c.name.clone()));
                }
            }
            Declaration::Static(s) => {
                if matches!(s.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Static(s.name.clone()));
                }
            }
            Declaration::Model(m) => {
                if matches!(m.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Type(m.name.clone()));
                }
            }
            Declaration::Class(c) => {
                if matches!(c.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Type(c.name.clone()));
                }
            }
            Declaration::Enum(e) => {
                if matches!(e.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Type(e.name.clone()));
                    // Also export variants
                    for variant in &e.variants {
                        exports.push(ExportedSymbol::Variant {
                            enum_name: e.name.clone(),
                            variant_name: variant.node.name.clone(),
                        });
                    }
                    for alias in &e.variant_aliases {
                        exports.push(ExportedSymbol::Variant {
                            enum_name: e.name.clone(),
                            variant_name: alias.node.name.clone(),
                        });
                    }
                }
            }
            Declaration::TypeAlias(a) => {
                if matches!(a.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Type(a.name.clone()));
                }
            }
            Declaration::Alias(a) => {
                if matches!(a.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Reexported(a.name.clone()));
                }
            }
            Declaration::Newtype(n) => {
                if matches!(n.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Type(n.name.clone()));
                }
            }
            Declaration::Trait(t) => {
                if matches!(t.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Trait(t.name.clone()));
                }
            }
            Declaration::Function(f) => {
                if matches!(f.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Function(f.name.clone()));
                }
            }
            Declaration::Partial(p) => {
                if matches!(p.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Function(p.name.clone()));
                }
            }
            Declaration::Import(import) => {
                // Both `from module import X` and `from rust::crate import X` are treated as re-exports. This lets
                // stdlib files like `response.incn` expose axum types (`from rust::axum import Json`) to importers
                // without needing a newtype wrapper.
                let items = match &import.kind {
                    ImportKind::From { items, .. } => Some(items.as_slice()),
                    ImportKind::RustFrom { items, .. } => Some(items.as_slice()),
                    ImportKind::PubFrom { items, .. } => Some(items.as_slice()),
                    _ => None,
                };
                if let Some(items) = items {
                    for item in items {
                        let exported_name = item.alias.as_ref().unwrap_or(&item.name);
                        exports.push(ExportedSymbol::Reexported(exported_name.clone()));
                    }
                }
            }
            Declaration::Docstring(_) | Declaration::TestModule(_) => {}
        }
    }

    exports
}

/// Kind of declaration surfaced by [`exported_type_like_docs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExportedTypeLikeKind {
    Model,
    Class,
    Enum,
    Trait,
    Newtype,
}

/// Body docstring attached to a **public** type-like declaration, for documentation tooling and IDE features.
///
/// This reads the AST fields populated by the parser (`ModelDecl::docstring`, `ClassDecl::docstring`, etc.). It does
/// **not** include freestanding [`Declaration::Docstring`] items (module-level narrative) or `const` / `static` docs,
/// which use the preceding docstring declaration pattern instead.
///
/// Visibility matches [`exported_symbols`]: only `pub` declarations are listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedTypeLikeDoc {
    pub name: String,
    pub kind: ExportedTypeLikeKind,
    /// Raw docstring payload from the lexer/parser (same string the formatter round-trips).
    pub docstring: Option<String>,
}

/// Collect body docstrings for public model/class/enum/trait/newtype declarations.
///
/// Order follows declaration order in `ast`. Entries with `docstring: None` are still included so callers can tell a
/// public type exists without attached body docs.
pub fn exported_type_like_docs(ast: &Program) -> Vec<ExportedTypeLikeDoc> {
    let mut out = Vec::new();
    for decl in &ast.declarations {
        match &decl.node {
            Declaration::Model(m) => {
                if matches!(m.visibility, Visibility::Public) {
                    out.push(ExportedTypeLikeDoc {
                        name: m.name.clone(),
                        kind: ExportedTypeLikeKind::Model,
                        docstring: m.docstring.clone(),
                    });
                }
            }
            Declaration::Class(c) => {
                if matches!(c.visibility, Visibility::Public) {
                    out.push(ExportedTypeLikeDoc {
                        name: c.name.clone(),
                        kind: ExportedTypeLikeKind::Class,
                        docstring: c.docstring.clone(),
                    });
                }
            }
            Declaration::Enum(e) => {
                if matches!(e.visibility, Visibility::Public) {
                    out.push(ExportedTypeLikeDoc {
                        name: e.name.clone(),
                        kind: ExportedTypeLikeKind::Enum,
                        docstring: e.docstring.clone(),
                    });
                }
            }
            Declaration::Trait(t) => {
                if matches!(t.visibility, Visibility::Public) {
                    out.push(ExportedTypeLikeDoc {
                        name: t.name.clone(),
                        kind: ExportedTypeLikeKind::Trait,
                        docstring: t.docstring.clone(),
                    });
                }
            }
            Declaration::Newtype(n) => {
                if matches!(n.visibility, Visibility::Public) {
                    out.push(ExportedTypeLikeDoc {
                        name: n.name.clone(),
                        kind: ExportedTypeLikeKind::Newtype,
                        docstring: n.docstring.clone(),
                    });
                }
            }
            _ => {}
        }
    }
    out
}

#[derive(Debug, Clone)]
pub enum ExportedSymbol {
    Type(String),
    Trait(String),
    Function(String),
    Const(String),
    Static(String),
    Reexported(String),
    Variant { enum_name: String, variant_name: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::ast::{
        ClassDecl, ConstDecl, Declaration, EnumDecl, Expr, FunctionDecl, ImportDecl, ImportItem, ImportKind,
        ImportPath, IntLiteral, Literal, ModelDecl, NewtypeDecl, Program, Span, Spanned, StaticDecl, TraitDecl, Type,
        VariantDecl, Visibility,
    };
    use crate::frontend::{lexer, parser};

    /// Shared with `tests/integration_tests.rs` (GitHub #247 export + CLI fmt coverage).
    const BLOCK_DOCSTRING_PUBLIC_TYPE_LIKE: &str =
        include_str!("../../tests/fixtures/block_docstring_public_type_like.incn");

    fn make_spanned<T>(node: T) -> Spanned<T> {
        Spanned {
            node,
            span: Span::default(),
            leading_blank_lines: 0,
        }
    }

    fn relative_from_import(module: &str) -> ImportDecl {
        ImportDecl {
            visibility: Visibility::Private,
            kind: ImportKind::From {
                module: ImportPath {
                    segments: vec![module.to_string()],
                    is_absolute: false,
                    parent_levels: 0,
                },
                items: vec![],
            },
            alias: None,
        }
    }

    #[test]
    fn resolve_import_path_falls_back_to_project_source_root_for_tests_dir() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path();
        std::fs::write(
            root.join("incan.toml"),
            r#"[project]
name = "demo"
version = "0.1.0"
"#,
        )?;
        let tests_dir = root.join("tests");
        let src_dir = root.join("src");
        std::fs::create_dir_all(&tests_dir)?;
        std::fs::create_dir_all(&src_dir)?;
        let dataset = src_dir.join("dataset.incn");
        std::fs::write(&dataset, "pub trait DataSet[T]:\n    pass\n")?;

        let resolved = resolve_import_path(&tests_dir, &relative_from_import("dataset"));
        assert_eq!(resolved, Some(dataset.canonicalize().unwrap_or(dataset)));
        Ok(())
    }

    #[test]
    fn resolve_import_path_prefers_base_dir_before_source_root_fallback() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path();
        std::fs::write(
            root.join("incan.toml"),
            r#"[project]
name = "demo"
version = "0.1.0"
"#,
        )?;
        let tests_dir = root.join("tests");
        let src_dir = root.join("src");
        std::fs::create_dir_all(&tests_dir)?;
        std::fs::create_dir_all(&src_dir)?;
        let tests_dataset = tests_dir.join("dataset.incn");
        let src_dataset = src_dir.join("dataset.incn");
        std::fs::write(&tests_dataset, "pub trait LocalDataSet[T]:\n    pass\n")?;
        std::fs::write(&src_dataset, "pub trait SourceDataSet[T]:\n    pass\n")?;

        let resolved = resolve_import_path(&tests_dir, &relative_from_import("dataset"));
        assert_eq!(resolved, Some(tests_dataset.canonicalize().unwrap_or(tests_dataset)));
        Ok(())
    }

    #[test]
    fn resolve_import_path_uses_manifest_source_root_when_configured() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path();
        std::fs::write(
            root.join("incan.toml"),
            r#"[project]
name = "demo"
version = "0.1.0"

[build]
source-root = "library"
"#,
        )?;
        let tests_dir = root.join("tests");
        let library_dir = root.join("library");
        std::fs::create_dir_all(&tests_dir)?;
        std::fs::create_dir_all(&library_dir)?;
        let dataset = library_dir.join("dataset.incn");
        std::fs::write(&dataset, "pub trait DataSet[T]:\n    pass\n")?;

        let resolved = resolve_import_path(&tests_dir, &relative_from_import("dataset"));
        assert_eq!(resolved, Some(dataset.canonicalize().unwrap_or(dataset)));
        Ok(())
    }

    #[test]
    fn logical_module_segments_strip_directory_entrypoint_suffixes() -> Result<(), Box<dyn std::error::Error>> {
        let base = PathBuf::from("src");

        let dataset = logical_module_segments_from_file(&base, &base.join("dataset").join("mod.incn"))
            .ok_or("dataset/mod.incn should resolve logical path")?;
        assert_eq!(dataset, vec!["dataset".to_string()]);

        let nested = logical_module_segments_from_file(&base, &base.join("dataset").join("ops").join("mod.incn"))
            .ok_or("dataset/ops/mod.incn should resolve logical path")?;
        assert_eq!(nested, vec!["dataset".to_string(), "ops".to_string()]);

        let leaf = logical_module_segments_from_file(&base, &base.join("dataset").join("ops.incn"))
            .ok_or("dataset/ops.incn should resolve logical path")?;
        assert_eq!(leaf, vec!["dataset".to_string(), "ops".to_string()]);

        Ok(())
    }

    fn relative_module_import(segments: &[&str]) -> ImportDecl {
        ImportDecl {
            visibility: Visibility::Private,
            kind: ImportKind::Module(ImportPath {
                segments: segments.iter().map(|segment| (*segment).to_string()).collect(),
                is_absolute: false,
                parent_levels: 0,
            }),
            alias: None,
        }
    }

    #[test]
    fn resolve_source_module_import_classifies_stdlib_imports() {
        let import = relative_module_import(&["std", "testing"]);
        let resolved = resolve_source_module_import(Path::new("src"), None, &import);

        assert_eq!(
            resolved,
            SourceModuleImportResolution::Stdlib {
                module_path: vec!["std".to_string(), "testing".to_string()]
            }
        );
    }

    #[test]
    fn resolve_source_module_import_prefers_full_module_import_path() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("dataset"))?;
        let parent = src.join("dataset.incn");
        let nested = src.join("dataset").join("ops.incn");
        std::fs::write(&parent, "pub const PARENT: int = 1\n")?;
        std::fs::write(&nested, "pub const NESTED: int = 2\n")?;

        let import = relative_module_import(&["dataset", "ops"]);
        let resolved = resolve_source_module_import(&src, Some(&src), &import);

        let SourceModuleImportResolution::Local(module_ref) = resolved else {
            return Err("expected local source module resolution".into());
        };
        assert_eq!(module_ref.file_path, nested.canonicalize()?);
        assert_eq!(module_ref.path_segments, vec!["dataset".to_string(), "ops".to_string()]);
        Ok(())
    }

    #[test]
    fn resolve_source_module_import_falls_back_to_parent_for_item_style_module_import()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("db"))?;
        let schema = src.join("db").join("schema.incn");
        std::fs::write(&schema, "pub model Database:\n    id: int\n")?;

        let import = relative_module_import(&["db", "schema", "Database"]);
        let resolved = resolve_source_module_import(&src, Some(&src), &import);

        let SourceModuleImportResolution::Local(module_ref) = resolved else {
            return Err("expected parent source module resolution".into());
        };
        assert_eq!(module_ref.file_path, schema.canonicalize()?);
        assert_eq!(module_ref.path_segments, vec!["db".to_string(), "schema".to_string()]);
        Ok(())
    }

    #[test]
    fn resolve_source_module_import_uses_source_root_for_nested_module_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let src = tmp.path().join("src");
        let package = src.join("pkg");
        std::fs::create_dir_all(&package)?;
        let sibling = package.join("bar.incn");
        std::fs::write(&sibling, "pub const VALUE: int = 1\n")?;

        let import = relative_module_import(&["bar"]);
        let resolved = resolve_source_module_import(&package, Some(&src), &import);

        let SourceModuleImportResolution::Local(module_ref) = resolved else {
            return Err("expected local source module resolution".into());
        };
        assert_eq!(module_ref.file_path, sibling.canonicalize()?);
        assert_eq!(module_ref.path_segments, vec!["pkg".to_string(), "bar".to_string()]);
        assert_eq!(module_ref.module_name, "pkg_bar");
        Ok(())
    }

    // ========================================
    // exported_type_like_docs tests
    // ========================================

    fn assert_docstring_has_marker_lines(doc: Option<&str>, ctx: &str) -> Result<(), Vec<String>> {
        let Some(doc) = doc else {
            return Err(vec![format!("{ctx}: expected body docstring")]);
        };
        let trimmed = doc.trim();
        if !trimmed.contains("Line A documents the class API.") {
            return Err(vec![format!("{ctx}: missing marker A in {trimmed:?}")]);
        }
        if !trimmed.contains("Line B keeps interior newlines after trim().") {
            return Err(vec![format!("{ctx}: missing marker B in {trimmed:?}")]);
        }
        Ok(())
    }

    #[test]
    fn test_exported_type_like_docs_reads_body_docstrings_from_parse() -> Result<(), Vec<String>> {
        let tokens = lexer::lex(BLOCK_DOCSTRING_PUBLIC_TYPE_LIKE)
            .map_err(|e| e.iter().map(|x| x.message.clone()).collect::<Vec<_>>())?;
        let ast = parser::parse(&tokens).map_err(|e| e.iter().map(|x| x.message.clone()).collect::<Vec<_>>())?;
        let docs = exported_type_like_docs(&ast);
        assert_eq!(docs.len(), 5, "expected one entry per public type-like decl");

        let mut by_name: std::collections::HashMap<&str, &ExportedTypeLikeDoc> = std::collections::HashMap::new();
        for d in &docs {
            by_name.insert(d.name.as_str(), d);
        }

        let m = by_name
            .get("CliModelProbe")
            .ok_or_else(|| vec!["missing CliModelProbe entry".to_string()])?;
        assert_eq!(m.kind, ExportedTypeLikeKind::Model);
        assert_docstring_has_marker_lines(m.docstring.as_deref(), "CliModelProbe")?;

        let c = by_name
            .get("CliClassProbe")
            .ok_or_else(|| vec!["missing CliClassProbe entry".to_string()])?;
        assert_eq!(c.kind, ExportedTypeLikeKind::Class);
        assert_docstring_has_marker_lines(c.docstring.as_deref(), "CliClassProbe")?;

        let e = by_name
            .get("CliEnumProbe")
            .ok_or_else(|| vec!["missing CliEnumProbe entry".to_string()])?;
        assert_eq!(e.kind, ExportedTypeLikeKind::Enum);
        assert_docstring_has_marker_lines(e.docstring.as_deref(), "CliEnumProbe")?;

        let t = by_name
            .get("CliTraitProbe")
            .ok_or_else(|| vec!["missing CliTraitProbe entry".to_string()])?;
        assert_eq!(t.kind, ExportedTypeLikeKind::Trait);
        assert_docstring_has_marker_lines(t.docstring.as_deref(), "CliTraitProbe")?;

        let n = by_name
            .get("CliNewtypeProbe")
            .ok_or_else(|| vec!["missing CliNewtypeProbe entry".to_string()])?;
        assert_eq!(n.kind, ExportedTypeLikeKind::Newtype);
        assert_docstring_has_marker_lines(n.docstring.as_deref(), "CliNewtypeProbe")?;

        Ok(())
    }

    #[test]
    fn test_exported_type_like_docs_skips_private_even_with_docstring() -> Result<(), Vec<String>> {
        let source = r#"class Secret:
    """
    Line A documents the class API.
    Line B keeps interior newlines after trim().
    """
    x: int
"#;
        let tokens = lexer::lex(source).map_err(|e| e.iter().map(|x| x.message.clone()).collect::<Vec<_>>())?;
        let ast = parser::parse(&tokens).map_err(|e| e.iter().map(|x| x.message.clone()).collect::<Vec<_>>())?;
        assert!(exported_type_like_docs(&ast).is_empty());
        Ok(())
    }

    // ========================================
    // exported_symbols tests
    // ========================================

    #[test]
    fn test_exported_symbols_empty_program() {
        let program = Program {
            declarations: vec![],
            rust_module_path: None,
            source_path: None,
            warnings: vec![],
        };
        let exports = exported_symbols(&program);
        assert!(exports.is_empty());
    }

    #[test]
    fn test_exported_symbols_model() {
        let model = ModelDecl {
            visibility: Visibility::Public,
            decorators: vec![],
            name: "User".to_string(),
            type_params: vec![],
            traits: vec![],
            docstring: None,
            fields: vec![],
            method_aliases: vec![],
            method_partials: vec![],
            properties: vec![],
            methods: vec![],
        };
        let program = Program {
            declarations: vec![make_spanned(Declaration::Model(model))],
            rust_module_path: None,
            source_path: None,
            warnings: vec![],
        };
        let exports = exported_symbols(&program);
        assert_eq!(exports.len(), 1);
        match &exports[0] {
            ExportedSymbol::Type(name) => assert_eq!(name, "User"),
            _ => panic!("Expected Type export"),
        }
    }

    #[test]
    fn test_exported_symbols_class() {
        let class = ClassDecl {
            visibility: Visibility::Public,
            decorators: vec![],
            name: "MyClass".to_string(),
            type_params: vec![],
            extends: None,
            traits: vec![],
            docstring: None,
            fields: vec![],
            method_aliases: vec![],
            method_partials: vec![],
            properties: vec![],
            methods: vec![],
        };
        let program = Program {
            declarations: vec![make_spanned(Declaration::Class(class))],
            rust_module_path: None,
            source_path: None,
            warnings: vec![],
        };
        let exports = exported_symbols(&program);
        assert_eq!(exports.len(), 1);
        match &exports[0] {
            ExportedSymbol::Type(name) => assert_eq!(name, "MyClass"),
            _ => panic!("Expected Type export"),
        }
    }

    #[test]
    fn test_exported_symbols_enum_with_variants() {
        let enum_decl = EnumDecl {
            visibility: Visibility::Public,
            decorators: vec![],
            name: "Color".to_string(),
            type_params: vec![],
            value_type: None,
            traits: vec![],
            docstring: None,
            variants: vec![
                make_spanned(VariantDecl {
                    name: "Red".to_string(),
                    fields: vec![],
                    value: None,
                }),
                make_spanned(VariantDecl {
                    name: "Green".to_string(),
                    fields: vec![],
                    value: None,
                }),
                make_spanned(VariantDecl {
                    name: "Blue".to_string(),
                    fields: vec![],
                    value: None,
                }),
            ],
            variant_aliases: vec![],
            methods: vec![],
        };
        let program = Program {
            declarations: vec![make_spanned(Declaration::Enum(enum_decl))],
            rust_module_path: None,
            source_path: None,
            warnings: vec![],
        };
        let exports = exported_symbols(&program);
        // 1 type + 3 variants = 4 exports
        assert_eq!(exports.len(), 4);

        // First should be the type
        match &exports[0] {
            ExportedSymbol::Type(name) => assert_eq!(name, "Color"),
            _ => panic!("Expected Type export"),
        }

        // Rest are variants
        match &exports[1] {
            ExportedSymbol::Variant {
                enum_name,
                variant_name,
            } => {
                assert_eq!(enum_name, "Color");
                assert_eq!(variant_name, "Red");
            }
            _ => panic!("Expected Variant export"),
        }
    }

    #[test]
    fn test_exported_symbols_newtype() {
        let newtype = NewtypeDecl {
            visibility: Visibility::Public,
            decorators: vec![],
            name: "UserId".to_string(),
            type_params: vec![],
            is_rusttype: false,
            underlying: make_spanned(Type::Simple("i64".to_string())),
            traits: vec![],
            docstring: None,
            rebindings: vec![],
            method_aliases: vec![],
            method_partials: vec![],
            associated_types: vec![],
            interop_edges: vec![],
            methods: vec![],
        };
        let program = Program {
            declarations: vec![make_spanned(Declaration::Newtype(newtype))],
            rust_module_path: None,
            source_path: None,
            warnings: vec![],
        };
        let exports = exported_symbols(&program);
        assert_eq!(exports.len(), 1);
        match &exports[0] {
            ExportedSymbol::Type(name) => assert_eq!(name, "UserId"),
            _ => panic!("Expected Type export"),
        }
    }

    #[test]
    fn test_exported_symbols_trait() {
        let trait_decl = TraitDecl {
            visibility: Visibility::Public,
            decorators: vec![],
            name: "Printable".to_string(),
            type_params: vec![],
            traits: vec![],
            docstring: None,
            method_aliases: vec![],
            method_partials: vec![],
            properties: vec![],
            methods: vec![],
        };
        let program = Program {
            declarations: vec![make_spanned(Declaration::Trait(trait_decl))],
            rust_module_path: None,
            source_path: None,
            warnings: vec![],
        };
        let exports = exported_symbols(&program);
        assert_eq!(exports.len(), 1);
        match &exports[0] {
            ExportedSymbol::Trait(name) => assert_eq!(name, "Printable"),
            _ => panic!("Expected Trait export"),
        }
    }

    #[test]
    fn test_exported_symbols_function() {
        let func = FunctionDecl {
            visibility: Visibility::Public,
            decorators: vec![],
            surface_modifiers: vec![],
            name: "calculate".to_string(),
            type_params: vec![],
            params: vec![],
            return_type: make_spanned(Type::Unit),
            body: vec![],
        };
        let program = Program {
            declarations: vec![make_spanned(Declaration::Function(func))],
            rust_module_path: None,
            source_path: None,
            warnings: vec![],
        };
        let exports = exported_symbols(&program);
        assert_eq!(exports.len(), 1);
        match &exports[0] {
            ExportedSymbol::Function(name) => assert_eq!(name, "calculate"),
            _ => panic!("Expected Function export"),
        }
    }

    #[test]
    fn test_exported_symbols_partial() -> Result<(), Vec<String>> {
        let source = r#"
pub def route(method: str, path: str) -> str:
  return path

pub get = partial route(method="GET")
"#;
        let tokens = lexer::lex(source).map_err(|e| e.iter().map(|x| x.message.clone()).collect::<Vec<_>>())?;
        let ast = parser::parse(&tokens).map_err(|e| e.iter().map(|x| x.message.clone()).collect::<Vec<_>>())?;
        let exports = exported_symbols(&ast);
        assert!(
            exports
                .iter()
                .any(|export| matches!(export, ExportedSymbol::Function(name) if name == "get")),
            "expected public partial callable export, got {exports:?}"
        );
        Ok(())
    }

    #[test]
    fn test_exported_symbols_ignores_module_imports() {
        let import = ImportDecl {
            visibility: Visibility::Private,
            kind: ImportKind::Module(ImportPath {
                segments: vec!["std".to_string()],
                is_absolute: false,
                parent_levels: 0,
            }),
            alias: None,
        };
        let program = Program {
            declarations: vec![make_spanned(Declaration::Import(import))],
            rust_module_path: None,
            source_path: None,
            warnings: vec![],
        };
        let exports = exported_symbols(&program);
        assert!(exports.is_empty());
    }

    #[test]
    fn test_exported_symbols_reexports_from_import_items() {
        let import = ImportDecl {
            visibility: Visibility::Private,
            kind: ImportKind::From {
                module: ImportPath {
                    segments: vec!["std".to_string(), "web".to_string(), "routing".to_string()],
                    is_absolute: false,
                    parent_levels: 0,
                },
                items: vec![
                    ImportItem {
                        name: "route".to_string(),
                        alias: None,
                    },
                    ImportItem {
                        name: "GET".to_string(),
                        alias: Some("METHOD_GET".to_string()),
                    },
                ],
            },
            alias: None,
        };
        let program = Program {
            declarations: vec![make_spanned(Declaration::Import(import))],
            rust_module_path: None,
            source_path: None,
            warnings: vec![],
        };
        let exports = exported_symbols(&program);
        assert_eq!(exports.len(), 2);
        match &exports[0] {
            ExportedSymbol::Reexported(name) => assert_eq!(name, "route"),
            _ => panic!("Expected Reexported export"),
        }
        match &exports[1] {
            ExportedSymbol::Reexported(name) => assert_eq!(name, "METHOD_GET"),
            _ => panic!("Expected Reexported export"),
        }
    }

    #[test]
    fn test_exported_symbols_ignores_docstrings() {
        let program = Program {
            declarations: vec![make_spanned(Declaration::Docstring("Module documentation".to_string()))],
            rust_module_path: None,
            source_path: None,
            warnings: vec![],
        };
        let exports = exported_symbols(&program);
        assert!(exports.is_empty());
    }

    #[test]
    fn test_exported_symbols_const() {
        let konst = ConstDecl {
            visibility: crate::frontend::ast::Visibility::Public,
            name: "X".to_string(),
            ty: Some(make_spanned(Type::Simple("int".to_string()))),
            value: make_spanned(Expr::Literal(Literal::Int(IntLiteral::synthetic(1)))),
        };
        let program = Program {
            declarations: vec![make_spanned(Declaration::Const(konst))],
            rust_module_path: None,
            source_path: None,
            warnings: vec![],
        };
        let exports = exported_symbols(&program);
        assert_eq!(exports.len(), 1);
        match &exports[0] {
            ExportedSymbol::Const(name) => assert_eq!(name, "X"),
            _ => panic!("Expected Const export"),
        }
    }

    #[test]
    fn test_exported_symbols_static() {
        let static_decl = StaticDecl {
            visibility: crate::frontend::ast::Visibility::Public,
            name: "COUNTER".to_string(),
            ty: make_spanned(Type::Simple("int".to_string())),
            value: make_spanned(Expr::Literal(Literal::Int(IntLiteral::synthetic(0)))),
        };
        let program = Program {
            declarations: vec![make_spanned(Declaration::Static(static_decl))],
            rust_module_path: None,
            source_path: None,
            warnings: vec![],
        };
        let exports = exported_symbols(&program);
        assert_eq!(exports.len(), 1);
        match &exports[0] {
            ExportedSymbol::Static(name) => assert_eq!(name, "COUNTER"),
            _ => panic!("Expected Static export"),
        }
    }

    #[test]
    fn test_exported_symbols_multiple_declarations() {
        let model = ModelDecl {
            visibility: Visibility::Public,
            decorators: vec![],
            name: "User".to_string(),
            type_params: vec![],
            traits: vec![],
            docstring: None,
            fields: vec![],
            method_aliases: vec![],
            method_partials: vec![],
            properties: vec![],
            methods: vec![],
        };
        let func = FunctionDecl {
            visibility: Visibility::Public,
            decorators: vec![],
            surface_modifiers: vec![],
            name: "create_user".to_string(),
            type_params: vec![],
            params: vec![],
            return_type: make_spanned(Type::Unit),
            body: vec![],
        };
        let trait_decl = TraitDecl {
            visibility: Visibility::Public,
            decorators: vec![],
            name: "Serializable".to_string(),
            type_params: vec![],
            traits: vec![],
            docstring: None,
            method_aliases: vec![],
            method_partials: vec![],
            properties: vec![],
            methods: vec![],
        };
        let program = Program {
            declarations: vec![
                make_spanned(Declaration::Model(model)),
                make_spanned(Declaration::Function(func)),
                make_spanned(Declaration::Trait(trait_decl)),
            ],
            rust_module_path: None,
            source_path: None,
            warnings: vec![],
        };
        let exports = exported_symbols(&program);
        assert_eq!(exports.len(), 3);
    }

    // ========================================
    // ExportedSymbol tests
    // ========================================

    #[test]
    fn test_exported_symbol_type_clone() {
        let sym = ExportedSymbol::Type("MyType".to_string());
        let cloned = sym.clone();
        match cloned {
            ExportedSymbol::Type(name) => assert_eq!(name, "MyType"),
            _ => panic!("Clone changed variant"),
        }
    }

    #[test]
    fn test_exported_symbol_variant_clone() {
        let sym = ExportedSymbol::Variant {
            enum_name: "Status".to_string(),
            variant_name: "Active".to_string(),
        };
        let cloned = sym.clone();
        match cloned {
            ExportedSymbol::Variant {
                enum_name,
                variant_name,
            } => {
                assert_eq!(enum_name, "Status");
                assert_eq!(variant_name, "Active");
            }
            _ => panic!("Clone changed variant"),
        }
    }

    #[test]
    fn test_exported_symbol_debug() {
        let sym = ExportedSymbol::Function("test".to_string());
        let debug_str = format!("{:?}", sym);
        assert!(debug_str.contains("Function"));
        assert!(debug_str.contains("test"));
    }
}
