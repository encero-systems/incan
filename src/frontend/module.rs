//! Module resolution for multi-file Incan projects
//!
//! Resolves import paths like `import models::User` to actual file paths
//! and manages loading/parsing of dependent modules.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::ast::{Declaration, ImportDecl, ImportKind, Program, Span, Visibility};
use super::diagnostics::{CompileError, errors};
use super::lexer;
use super::parser;
use incan_core::lang::stdlib;

/// Represents a resolved module with its AST and metadata
#[derive(Debug)]
pub struct ResolvedModule {
    pub path: PathBuf,
    pub source: String,
    pub ast: Program,
}

/// Collects all modules needed for compilation
pub struct ModuleCollector {
    /// Base directory for resolving relative imports
    base_dir: PathBuf,
    /// Already loaded modules (path -> module)
    loaded: HashMap<PathBuf, ResolvedModule>,
    /// Modules currently being loaded (for cycle detection)
    loading: HashSet<PathBuf>,
}

impl ModuleCollector {
    pub fn new(entry_file: &Path) -> Self {
        let base_dir = entry_file
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));

        Self {
            base_dir,
            loaded: HashMap::new(),
            loading: HashSet::new(),
        }
    }

    /// Load the entry file and all its dependencies
    pub fn collect(&mut self, entry_file: &Path) -> Result<Vec<ResolvedModule>, Vec<CompileError>> {
        let canonical = entry_file.canonicalize().unwrap_or_else(|_| entry_file.to_path_buf());

        self.load_module(&canonical)?;

        // Return modules in dependency order (dependencies first)
        let mut result = Vec::new();
        let entry_key = canonical.clone();

        // First add all non-entry modules
        for (path, module) in self.loaded.drain() {
            if path != entry_key {
                result.push(module);
            }
        }

        // Entry module is handled separately
        Ok(result)
    }

    /// Load a single module and its dependencies
    fn load_module(&mut self, path: &Path) -> Result<(), Vec<CompileError>> {
        // Already loaded?
        if self.loaded.contains_key(path) {
            return Ok(());
        }

        // Cycle detection
        if self.loading.contains(path) {
            return Err(vec![errors::circular_import(path, Span::default())]);
        }

        self.loading.insert(path.to_path_buf());

        // Read and parse
        let source = fs::read_to_string(path).map_err(|e| vec![errors::cannot_read_file(path, &e, Span::default())])?;

        let tokens = lexer::lex(&source)?;
        let ast = parser::parse_with_module_path(&tokens, path.to_str())?;

        // Find and load dependencies
        for decl in &ast.declarations {
            if let Declaration::Import(import) = &decl.node
                && let Some(dep_path) = self.resolve_import(import)
            {
                self.load_module(&dep_path)?;
            }
        }

        self.loading.remove(path);
        self.loaded.insert(
            path.to_path_buf(),
            ResolvedModule {
                path: path.to_path_buf(),
                source,
                ast,
            },
        );

        Ok(())
    }

    /// Resolve an import to a file path (relative to this collector's base directory).
    fn resolve_import(&self, import: &ImportDecl) -> Option<PathBuf> {
        resolve_import_path(&self.base_dir, import)
    }

    /// Get all loaded modules
    pub fn modules(&self) -> impl Iterator<Item = &ResolvedModule> {
        self.loaded.values()
    }

    /// Take ownership of loaded modules
    pub fn into_modules(self) -> HashMap<PathBuf, ResolvedModule> {
        self.loaded
    }
}

/// Resolve an `import` / `from ... import ...` into an on-disk Incan module file path.
///
/// This is used by both the CLI and the LSP to typecheck multi-file projects.
pub fn resolve_import_path(base_dir: &Path, import: &ImportDecl) -> Option<PathBuf> {
    let (path, is_absolute, parent_levels) = match &import.kind {
        ImportKind::Module(p) if !p.segments.is_empty() => (p.segments.clone(), p.is_absolute, p.parent_levels),
        ImportKind::From { module, .. } if !module.segments.is_empty() => {
            (module.segments.clone(), module.is_absolute, module.parent_levels)
        }
        // External namespace imports don't resolve to on-disk Incan source files.
        ImportKind::RustCrate { .. }
        | ImportKind::RustFrom { .. }
        | ImportKind::PubLibrary { .. }
        | ImportKind::PubFrom { .. } => return None,
        ImportKind::Python(_) | ImportKind::Module(_) | ImportKind::From { .. } => return None,
    };

    // Skip standard library imports (std::*)
    if let Some(first) = path.first()
        && first == stdlib::STDLIB_ROOT
    {
        return None;
    }

    // Calculate base directory based on relative path
    let mut base = base_dir.to_path_buf();

    // Handle absolute paths (crate::...)
    if is_absolute {
        // Find project root (look for Cargo.toml or src/ directory)
        let mut project_root = base.clone();
        while !project_root.join("Cargo.toml").exists() && !project_root.join("src").exists() {
            if !project_root.pop() {
                break;
            }
        }
        // If we found a src directory, use it as base
        if project_root.join("src").exists() {
            base = project_root.join("src");
        } else {
            base = project_root;
        }
    } else {
        // Handle parent navigation (super:: or ..)
        for _ in 0..parent_levels {
            base = base.parent().map(|p| p.to_path_buf()).unwrap_or(base);
        }
    }

    if path.is_empty() {
        return None;
    }

    if let Some(resolved) = resolve_module_path_from_base(&base, &path) {
        return Some(resolved);
    }

    // For simple relative imports (e.g. `from dataset import ...`) in non-source directories
    // like `tests/` or `examples/`, also attempt resolution from the project source root.
    if !is_absolute
        && parent_levels == 0
        && let Some(source_root) = resolve_source_root_for_imports(base_dir)
        && source_root != base
        && let Some(resolved) = resolve_module_path_from_base(&source_root, &path)
    {
        return Some(resolved);
    }

    None
}

/// Resolves an Incan module file under `base` from import path segments (e.g. `foo.bar` → `foo/bar`).
///
/// Tries, in order: `segments.incn`, `segments.incan`, `segments/mod.incn`, `segments/mod.incan`. Returns the first
/// path that exists on disk, canonicalized when possible. Returns `None` if none match.
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
                }
            }
            Declaration::TypeAlias(a) => {
                if matches!(a.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Type(a.name.clone()));
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
            Declaration::Docstring(_) => {}
        }
    }

    exports
}

#[derive(Debug, Clone)]
pub enum ExportedSymbol {
    Type(String),
    Trait(String),
    Function(String),
    Const(String),
    Reexported(String),
    Variant { enum_name: String, variant_name: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::ast::{
        ClassDecl, ConstDecl, Declaration, EnumDecl, Expr, FunctionDecl, ImportDecl, ImportItem, ImportKind,
        ImportPath, Literal, ModelDecl, NewtypeDecl, Program, Span, Spanned, TraitDecl, Type, VariantDecl, Visibility,
    };

    fn make_spanned<T>(node: T) -> Spanned<T> {
        Spanned {
            node,
            span: Span::default(),
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

    // ========================================
    // ModuleCollector tests
    // ========================================

    #[test]
    fn test_module_collector_new() {
        let path = std::path::Path::new("/test/project/main.incn");
        let collector = ModuleCollector::new(path);
        assert!(collector.loaded.is_empty());
        assert!(collector.loading.is_empty());
    }

    #[test]
    fn test_module_collector_new_with_relative_path() {
        let path = std::path::Path::new("main.incn");
        let collector = ModuleCollector::new(path);
        // Should default to "." as base_dir when parent is none
        assert!(collector.loaded.is_empty());
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

    // ========================================
    // exported_symbols tests
    // ========================================

    #[test]
    fn test_exported_symbols_empty_program() {
        let program = Program {
            declarations: vec![],
            rust_module_path: None,
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
            fields: vec![],
            methods: vec![],
        };
        let program = Program {
            declarations: vec![make_spanned(Declaration::Model(model))],
            rust_module_path: None,
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
            fields: vec![],
            methods: vec![],
        };
        let program = Program {
            declarations: vec![make_spanned(Declaration::Class(class))],
            rust_module_path: None,
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
            variants: vec![
                make_spanned(VariantDecl {
                    name: "Red".to_string(),
                    fields: vec![],
                }),
                make_spanned(VariantDecl {
                    name: "Green".to_string(),
                    fields: vec![],
                }),
                make_spanned(VariantDecl {
                    name: "Blue".to_string(),
                    fields: vec![],
                }),
            ],
        };
        let program = Program {
            declarations: vec![make_spanned(Declaration::Enum(enum_decl))],
            rust_module_path: None,
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
            docstring: None,
            rebindings: vec![],
            interop_edges: vec![],
            methods: vec![],
        };
        let program = Program {
            declarations: vec![make_spanned(Declaration::Newtype(newtype))],
            rust_module_path: None,
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
            methods: vec![],
        };
        let program = Program {
            declarations: vec![make_spanned(Declaration::Trait(trait_decl))],
            rust_module_path: None,
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
            value: make_spanned(Expr::Literal(Literal::Int(1))),
        };
        let program = Program {
            declarations: vec![make_spanned(Declaration::Const(konst))],
            rust_module_path: None,
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
    fn test_exported_symbols_multiple_declarations() {
        let model = ModelDecl {
            visibility: Visibility::Public,
            decorators: vec![],
            name: "User".to_string(),
            type_params: vec![],
            traits: vec![],
            fields: vec![],
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
            methods: vec![],
        };
        let program = Program {
            declarations: vec![
                make_spanned(Declaration::Model(model)),
                make_spanned(Declaration::Function(func)),
                make_spanned(Declaration::Trait(trait_decl)),
            ],
            rust_module_path: None,
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

    // ========================================
    // ResolvedModule tests
    // ========================================

    #[test]
    fn test_resolved_module_debug() {
        let module = ResolvedModule {
            path: std::path::PathBuf::from("/test/module.incn"),
            source: "fn main(): ()".to_string(),
            ast: Program {
                declarations: vec![],
                rust_module_path: None,
                warnings: vec![],
            },
        };
        let debug_str = format!("{:?}", module);
        assert!(debug_str.contains("ResolvedModule"));
        assert!(debug_str.contains("module.incn"));
    }
}
