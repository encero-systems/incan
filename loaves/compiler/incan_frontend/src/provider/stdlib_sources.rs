//! Where the standard library's sources are: the component catalog resolved against the stdlib root.
//!
//! The standard library is a ring of components (`loaves/stdlib/<component>/` in a checkout; the same layout below
//! the installed toolchain's stdlib root), each holding the `.incn` modules for the namespace roots that
//! `sdk-components.toml` assigns it. The kernel registry (`incan_lang::lang::stdlib`) says which namespaces exist and
//! spells a module's path relative to its owning component's `src/` — `stdlib/fs/path.incn` for `std.fs.path` — and
//! this module is the one place that turns that spelling into a file: it finds the root through the toolchain layout
//! policy, reads the catalog once per root, and joins the owner's source directory. Every reader of a standard-library
//! source — the typechecker's loader, the LSP, `incan check` on a stdlib module, the testing markers, the SDK
//! publisher — resolves through here, so the language never learns the packaging and the packaging never learns the
//! language.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use incan_lang::lang::stdlib;

use super::sdk::SdkInventoryError;
use super::sdk::{SDK_SOURCE_CATALOG_FILE, SdkSourceCatalog, SdkSourceComponent};

/// The catalog-resolved standard library below one root.
#[derive(Debug)]
pub struct StdlibSources {
    root: PathBuf,
    catalog: SdkSourceCatalog,
}

/// Catalogs already read, keyed by their root, so a compile that resolves hundreds of stdlib modules parses the
/// catalog once. Tests that point at their own roots get their own entries.
fn cache() -> &'static Mutex<HashMap<PathBuf, Arc<StdlibSources>>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<StdlibSources>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

impl StdlibSources {
    /// Read the catalog below `root` (the directory holding `sdk-components.toml` and the component directories).
    ///
    /// # Errors
    ///
    /// Returns the catalog's own validation error when the file is missing, malformed, or assigns a namespace root
    /// to two components.
    pub fn from_root(root: &Path) -> Result<Self, SdkInventoryError> {
        let catalog = SdkSourceCatalog::read_from_path(&root.join(SDK_SOURCE_CATALOG_FILE))?;
        Ok(Self {
            root: root.to_path_buf(),
            catalog,
        })
    }

    /// The standard library the active toolchain ships, resolved through the toolchain layout policy.
    ///
    /// `None` when no stdlib root can be found — a compiler binary far from any checkout or installation — or when
    /// the root's catalog does not read; callers treat both as "no built-in stdlib source" exactly as they treated a
    /// missing directory before the components existed.
    pub fn discover() -> Option<Arc<Self>> {
        let root = oven_model::toolchain_layout::find_stdlib_root()?;
        Self::cached(&root)
    }

    /// The standard library below `root`, read once per root for the life of the process.
    pub fn cached(root: &Path) -> Option<Arc<Self>> {
        let key = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let mut cache = cache().lock().ok()?;
        if let Some(sources) = cache.get(&key) {
            return Some(Arc::clone(sources));
        }
        let sources = Arc::new(Self::from_root(&key).ok()?);
        cache.insert(key, Arc::clone(&sources));
        Some(sources)
    }

    /// The directory holding the catalog and the component directories.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The catalog this root was read with.
    pub fn catalog(&self) -> &SdkSourceCatalog {
        &self.catalog
    }

    /// The component that owns a top-level namespace root such as `fs` or `json`.
    pub fn owner_of(&self, namespace_root: &str) -> Option<&SdkSourceComponent> {
        self.catalog
            .components
            .values()
            .find(|component| component.namespace_roots.contains(namespace_root))
    }

    /// A component's Incan source directory: `<project root>/src`.
    pub fn component_source_dir(&self, component_id: &str) -> Option<PathBuf> {
        self.catalog
            .components
            .get(component_id)
            .map(|component| component.project_root.join("src"))
    }

    /// The file for a module path such as `["std", "fs", "path"]`, or `None` when the registry has no source for it.
    ///
    /// The path is not checked for existence: a registered module whose file is missing is a broken toolchain, and
    /// the caller's read reports which file.
    pub fn module_source_path(&self, module_path: &[String]) -> Option<PathBuf> {
        self.source_path(&stdlib::stdlib_stub_path(module_path)?)
    }

    /// The file for a registry spelling such as `stdlib/fs/path.incn` (the `stdlib/` prefix is optional).
    ///
    /// The first segment after the prefix is the namespace root; the catalog says which component owns it, and the
    /// rest of the path is relative to that component's `src/`.
    pub fn source_path(&self, relative: &str) -> Option<PathBuf> {
        let relative = Path::new(relative);
        let relative = relative.strip_prefix("stdlib").unwrap_or(relative);
        let mut components = relative.components();
        let first = components.next()?.as_os_str().to_str()?;
        let namespace_root = first.strip_suffix(".incn").unwrap_or(first);
        let owner = self.owner_of(namespace_root)?;
        Some(owner.project_root.join("src").join(relative))
    }

    /// The file for a registry spelling when it exists on disk.
    pub fn existing_source_path(&self, relative: &str) -> Option<PathBuf> {
        let path = self.source_path(relative)?;
        path.is_file().then_some(path)
    }

    /// Every component's Incan source directory, in catalog order.
    pub fn component_source_dirs(&self) -> Vec<PathBuf> {
        self.catalog
            .components
            .values()
            .map(|component| component.project_root.join("src"))
            .collect()
    }
}

/// Resolve a registry spelling against the active toolchain's standard library, when both exist.
///
/// The convenience most readers want: [`StdlibSources::discover`] then [`StdlibSources::existing_source_path`].
pub fn find_stdlib_source_file(relative: &str) -> Option<PathBuf> {
    StdlibSources::discover()?.existing_source_path(relative)
}

/// The active toolchain's stdlib root, when one exists.
///
/// The directory below which the catalog and the components live; readers that enumerate every source or digest the
/// whole library start here.
pub fn find_stdlib_root() -> Option<PathBuf> {
    oven_model::toolchain_layout::find_stdlib_root()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::StdlibSources;

    fn write(path: &std::path::Path, content: &str) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, content)
    }

    fn fixture_root(temp: &std::path::Path) -> std::io::Result<()> {
        write(
            &temp.join("sdk-components.toml"),
            "[sdk]\nid = \"incan\"\nversion = \"0.1.0\"\ncompiler-requirement = \">=0.1.0\"\n\n[profiles]\ndefault = [\"stdlib-core\", \"stdlib-system\"]\n\n[components.stdlib-core]\nproject = \"core\"\nmandatory = true\ndependencies = []\nnamespace-roots = [\"prelude\", \"result\"]\n\n[components.stdlib-system]\nproject = \"system\"\ndependencies = [\"stdlib-core\"]\nnamespace-roots = [\"fs\", \"io\"]\n",
        )?;
        write(&temp.join("core/src/prelude.incn"), "\"\"\"prelude\"\"\"\n")?;
        write(&temp.join("core/src/result.incn"), "\"\"\"result\"\"\"\n")?;
        write(&temp.join("system/src/io.incn"), "\"\"\"io\"\"\"\n")?;
        write(&temp.join("system/src/fs/prelude.incn"), "\"\"\"fs\"\"\"\n")?;
        write(&temp.join("system/src/fs/path.incn"), "\"\"\"path\"\"\"\n")
    }

    #[test]
    fn a_module_resolves_to_its_owning_component() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        fixture_root(temp.path())?;
        let sources = StdlibSources::from_root(temp.path())?;
        assert_eq!(
            sources.source_path("stdlib/fs/path.incn"),
            Some(temp.path().join("system/src/fs/path.incn"))
        );
        assert_eq!(
            sources.source_path("result.incn"),
            Some(temp.path().join("core/src/result.incn"))
        );
        assert_eq!(
            sources.source_path("stdlib/regex.incn"),
            None,
            "an unassigned root has no owner"
        );
        assert!(sources.existing_source_path("stdlib/fs/path.incn").is_some());
        assert!(sources.existing_source_path("stdlib/io/missing.incn").is_none());
        assert_eq!(
            sources.owner_of("io").map(|component| component.id.as_str()),
            Some("stdlib-system")
        );
        assert_eq!(
            sources.component_source_dir("stdlib-core"),
            Some(temp.path().join("core/src"))
        );
        Ok(())
    }

    #[test]
    fn the_cache_reads_a_root_once_and_keeps_roots_apart() -> Result<(), Box<dyn std::error::Error>> {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        fixture_root(first.path())?;
        fixture_root(second.path())?;
        let a = StdlibSources::cached(first.path()).ok_or("first root did not read")?;
        let again = StdlibSources::cached(first.path()).ok_or("first root did not read twice")?;
        let b = StdlibSources::cached(second.path()).ok_or("second root did not read")?;
        assert!(std::sync::Arc::ptr_eq(&a, &again));
        assert!(!std::sync::Arc::ptr_eq(&a, &b));
        assert!(StdlibSources::cached(&first.path().join("nowhere")).is_none());
        Ok(())
    }
}
