//! Manifest-only closure projection for the compiler's Rust-backed Body IR caller contract.
//!
//! Cargo cannot distinguish same-version local packages at two roots in one lock. Compiler sources and included
//! resources retain their repository-relative layout while compiler-owned language edges select the active
//! byte-equivalent SDK copy. Only enumerated compiler roots qualify; build scripts and compile-time manifest-location
//! reads are refused.

use std::{fs, io, path::Path};

use oven_model::manifest::{DependencySource, DependencySpec};
use oven_model::toolchain_layout::{development_root, resolve_toolchain_crate_path};
use sha2::{Digest, Sha256};

use super::generator::ProjectGenerator;

impl ProjectGenerator {
    /// Preserve the CLI's checked-in registry patches when a generated host selects compiler source roots.
    /// Cargo only honors patches in the root manifest; dependency-local workspace patches cannot seal this closure.
    pub(super) fn compiler_closure_patches(&self) -> io::Result<Option<toml::Table>> {
        let mut compiler_host = false;
        for dependency in &self.dependencies {
            if let DependencySource::Path { path } = &dependency.source {
                compiler_host |= compiler_projection_root(path)?;
            }
        }
        if !compiler_host {
            return Ok(None);
        }
        let root = development_root();
        let workspace: toml::Value =
            toml::from_str(&fs::read_to_string(root.join("Cargo.toml"))?).map_err(io::Error::other)?;
        let Some(mut patches) = workspace.get("patch").and_then(toml::Value::as_table).cloned() else {
            return Ok(None);
        };
        for (_, registry) in patches.iter_mut() {
            let entries = registry
                .as_table_mut()
                .ok_or_else(|| io::Error::other("invalid compiler patches"))?;
            for (_, specification) in entries.iter_mut() {
                let table = specification
                    .as_table_mut()
                    .ok_or_else(|| io::Error::other("invalid compiler patch"))?;
                let relative = table
                    .get("path")
                    .and_then(toml::Value::as_str)
                    .ok_or_else(|| io::Error::other("compiler patch must be a checked-in path"))?;
                let path = fs::canonicalize(root.join(relative))?;
                if !path.starts_with(fs::canonicalize(root.join("loaves/third_party"))?) {
                    return Err(io::Error::other("compiler patch is outside approved third-party roots"));
                }
                table.insert("path".into(), path.to_string_lossy().into_owned().into());
            }
        }
        Ok(Some(patches))
    }

    /// Bind compiler-owned Body IR and frontend path crates to one byte-equivalent SDK language definition.
    ///
    /// Exact source bytes are retained; only workspace inheritance and compiler-owned path edges are projected.
    /// The compiler graph shares content-addressed package roots between the provider and Rust host.
    pub(super) fn project_body_ir_dependencies(
        &self,
        mut dependencies: Vec<DependencySpec>,
    ) -> io::Result<Vec<DependencySpec>> {
        let selected = resolve_toolchain_crate_path("incan_lang");
        let mut projected = std::collections::BTreeMap::new();
        let mut visiting = std::collections::BTreeSet::new();
        for dependency in &mut dependencies {
            let DependencySource::Path { path } = &mut dependency.source else {
                continue;
            };
            if compiler_projection_root(path)? {
                *path = project_compiler_library(path, &selected, &mut projected, &mut visiting)?;
            }
        }
        Ok(dependencies)
    }
}

/// Admit only source roots owned by the compiler's checked Body IR and in-process frontend closure.
///
/// Every generated project's path dependencies pass through here, so a path that does not resolve, or a toolchain with
/// no development checkout, is simply not a compiler root: it keeps its dependency exactly as declared.
fn compiler_projection_root(path: &Path) -> io::Result<bool> {
    let Ok(root) = fs::canonicalize(path) else {
        return Ok(false);
    };
    let development = development_root();
    Ok([
        "loaves/kernel/incan_semantics_core",
        "loaves/kernel/incan_syntax",
        "loaves/compiler/incan_frontend",
        "loaves/compiler/incan_format",
        "loaves/compiler/incan_semantics_stdlib",
        "loaves/compiler/rust_inspect",
        "loaves/compiler/incan_driver",
        "loaves/compiler/incan_emit",
        "loaves/compiler/incan_ir",
        "loaves/compiler/incan_provider",
        "loaves/compiler/incan_oven_facet",
    ]
    .iter()
    .filter_map(|relative| fs::canonicalize(development.join(relative)).ok())
    .any(|compiler_root| compiler_root == root))
}

/// Expand one selected compiler manifest and recursively retain its compiler-owned dependency roots.
fn project_compiler_library(
    root: &Path,
    language: &Path,
    projected: &mut std::collections::BTreeMap<std::path::PathBuf, std::path::PathBuf>,
    visiting: &mut std::collections::BTreeSet<std::path::PathBuf>,
) -> io::Result<std::path::PathBuf> {
    let root = fs::canonicalize(root)?;
    if let Some(path) = projected.get(&root) {
        return Ok(path.clone());
    }
    if !visiting.insert(root.clone()) {
        return Err(io::Error::other(
            "compiler source projection refuses cyclic path dependencies",
        ));
    }
    let mut manifest = read_effective_manifest(&root.join("Cargo.toml"))?;
    project_compiler_edges(&root, &mut manifest, language, projected, visiting)?;
    if root.join("build.rs").exists() {
        return Err(io::Error::other("compiler source projection refuses build scripts"));
    }
    let sources = source_records(&root.join("src"))?;
    validate_projection_locations(&sources)?;
    let table = manifest
        .as_table_mut()
        .ok_or_else(|| io::Error::other("compiler manifest must be a table"))?;
    table.insert("workspace".into(), toml::Value::Table(toml::Table::new()));
    table.remove("dev-dependencies");
    let result = materialize_projection(&root, &manifest, sources)?;
    visiting.remove(&root);
    projected.insert(root, result.clone());
    Ok(result)
}

/// Rebind only proven language sources and recursively selected compiler crates, leaving other paths anchored.
fn project_compiler_edges(
    root: &Path,
    manifest: &mut toml::Value,
    language: &Path,
    projected: &mut std::collections::BTreeMap<std::path::PathBuf, std::path::PathBuf>,
    visiting: &mut std::collections::BTreeSet<std::path::PathBuf>,
) -> io::Result<()> {
    let Some(dependencies) = manifest.get_mut("dependencies").and_then(toml::Value::as_table_mut) else {
        return Ok(());
    };
    for (name, specification) in dependencies {
        let Some(table) = specification.as_table_mut() else {
            continue;
        };
        let Some(declared) = table.get("path").and_then(toml::Value::as_str) else {
            continue;
        };
        let declared = root.join(declared);
        let selected = if name == "incan_lang" {
            verify_language_source(&declared, language)?;
            language.to_path_buf()
        } else if compiler_projection_root(&declared)? {
            project_compiler_library(&declared, language, projected, visiting)?
        } else {
            declared
        };
        table.insert(
            "path".into(),
            toml::Value::String(selected.to_string_lossy().into_owned()),
        );
    }
    Ok(())
}

/// Require the declaring compiler and selected SDK language package to have identical definitions and versions.
fn verify_language_source(original: &Path, selected: &Path) -> io::Result<()> {
    if fs::canonicalize(original)? == fs::canonicalize(selected)? {
        return Ok(());
    }
    if source_records(&original.join("src"))? != source_records(&selected.join("src"))? {
        return Err(io::Error::other(
            "Body IR caller requires byte-equivalent compiler and SDK incan_lang sources",
        ));
    }
    let original = read_effective_manifest(&original.join("Cargo.toml"))?;
    let selected = read_effective_manifest(&selected.join("Cargo.toml"))?;
    if original.get("package").and_then(|p| p.get("version")) != selected.get("package").and_then(|p| p.get("version"))
    {
        return Err(io::Error::other(
            "Body IR caller compiler and SDK incan_lang versions differ",
        ));
    }
    Ok(())
}

/// Refuse manifest-relative production inputs; test-only checkout probes are absent from library builds.
///
/// Relative include files remain at their exact source-relative locations in the snapshot. The unprojected
/// oven_model retains its real manifest root because its development-root contract is location-sensitive.
fn validate_projection_locations(sources: &[(String, Vec<u8>)]) -> io::Result<()> {
    for (name, bytes) in sources {
        let test = name.contains("/tests/") || name.starts_with("tests/") || name.ends_with("_tests.rs");
        if !test
            && name.ends_with(".rs")
            && reads_manifest_directory(
                String::from_utf8_lossy(bytes)
                    .parse()
                    .map_err(|error| io::Error::other(format!("cannot tokenize compiler source {name}: {error}")))?,
            )
        {
            return Err(io::Error::other(format!(
                "compiler source projection refuses manifest-relative source {name}"
            )));
        }
    }
    Ok(())
}

/// Detect compile-time manifest-root reads while permitting subprocess environment sanitization and assignment.
/// Only `env!` and `option_env!` capture the projected package's location; naming a child's environment key does not.
fn reads_manifest_directory(source: proc_macro2::TokenStream) -> bool {
    use proc_macro2::TokenTree;
    let mut tokens = source.into_iter().peekable();
    while let Some(token) = tokens.next() {
        match token {
            TokenTree::Ident(name) if name == "env" || name == "option_env" => {
                if matches!(tokens.peek(), Some(TokenTree::Punct(punctuation)) if punctuation.as_char() == '!') {
                    tokens.next();
                    if let Some(TokenTree::Group(arguments)) = tokens.next() {
                        let key: String = arguments
                            .stream()
                            .to_string()
                            .chars()
                            .filter(|character| character.is_alphanumeric() || *character == '_')
                            .collect();
                        if key.contains("CARGO_MANIFEST_DIR") {
                            return true;
                        }
                    }
                }
            }
            TokenTree::Group(group) if reads_manifest_directory(group.stream()) => return true,
            _ => {}
        }
    }
    false
}

/// Return the directory content-addressed compiler-source projections live under.
///
/// Cargo identifies a path dependency by its location, so a projection keeps one path across builds or Cargo rebuilds
/// the whole compiler closure every time: below `INCAN_HOME` (else `~/.incan`), never a per-process temporary
/// directory. The temporary directory remains only for an environment with neither home.
fn projection_cache_root() -> std::path::PathBuf {
    std::env::var_os("INCAN_HOME")
        .filter(|path| !path.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            crate::oven_store::user_home()
                .filter(|path| !path.is_empty())
                .map(|path| std::path::PathBuf::from(path).join(".incan"))
        })
        .map(|root| root.join("cache").join("body-ir-source"))
        .unwrap_or_else(|| std::env::temp_dir().join("incan-body-ir-source"))
}

/// Publish exact compiler sources and included resources in their repository-relative layout under a shared lock.
/// The package retains its original depth so frozen repository-relative includes resolve inside the snapshot; manifests
/// and resource bytes contribute to the content identity.
fn materialize_projection(
    root: &Path,
    manifest: &toml::Value,
    sources: Vec<(String, Vec<u8>)>,
) -> io::Result<std::path::PathBuf> {
    let development = fs::canonicalize(development_root())?;
    let relative_root = root.strip_prefix(&development).map_err(io::Error::other)?;
    // The frozen emitter includes this repository-relative resource. Preserve its path and exact bytes alongside
    // the source instead of rewriting the include or allowing it to escape the content-addressed snapshot.
    let resources = if relative_root == Path::new("loaves/compiler/incan_emit") {
        vec![(
            "loaves/stdlib/zen.txt",
            fs::read(development.join("loaves/stdlib/zen.txt"))?,
        )]
    } else {
        Vec::new()
    };
    let rendered = toml::to_string(&manifest).map_err(io::Error::other)?;
    let mut digest = Sha256::new();
    digest.update(relative_root.to_string_lossy().as_bytes());
    digest.update(rendered.as_bytes());
    for (name, bytes) in &sources {
        digest.update(name.as_bytes());
        digest.update([0]);
        digest.update(bytes.len().to_le_bytes());
        digest.update(bytes);
    }
    for (name, bytes) in &resources {
        digest.update(name.as_bytes());
        digest.update([0]);
        digest.update(bytes.len().to_le_bytes());
        digest.update(bytes);
    }
    let shadow = projection_cache_root().join(hex::encode(digest.finalize()));
    fs::create_dir_all(&shadow)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(shadow.join(".projection.lock"))?;
    lock.lock()?;
    for (name, bytes) in resources {
        let destination = shadow.join(name);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(destination, bytes)?;
    }
    let package = shadow.join(relative_root);
    fs::create_dir_all(package.join("src"))?;
    for (name, bytes) in sources {
        let destination = package.join("src").join(name);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        if fs::read(&destination).ok().as_deref() != Some(bytes.as_slice()) {
            fs::write(destination, bytes)?;
        }
    }
    let manifest_path = package.join("Cargo.toml");
    if fs::read_to_string(&manifest_path).ok().as_deref() != Some(rendered.as_str()) {
        fs::write(manifest_path, rendered)?;
    }
    Ok(package)
}

/// Resolve selected workspace fields for one manifest using the native path planner's identical projection.
fn read_effective_manifest(path: &Path) -> io::Result<toml::Value> {
    let manifest = toml::from_str(&fs::read_to_string(path)?).map_err(io::Error::other)?;
    oven_rustc::rustc::effective_path_manifest(path, manifest).map_err(io::Error::other)
}

/// Collect exact source bytes in stable relative-path order; non-regular entries are never projected.
fn source_records(root: &Path) -> io::Result<Vec<(String, Vec<u8>)>> {
    /// Retain relative names and bytes without following symbolic links.
    fn collect(root: &Path, directory: &Path, records: &mut Vec<(String, Vec<u8>)>) -> io::Result<()> {
        let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                collect(root, &path, records)?;
            } else if entry.file_type()?.is_file() {
                let relative = path.strip_prefix(root).map_err(io::Error::other)?;
                records.push((relative.to_string_lossy().into_owned(), fs::read(path)?));
            } else {
                return Err(io::Error::other(
                    "Body IR projection refuses non-regular source entries",
                ));
            }
        }
        Ok(())
    }
    let mut records = Vec::new();
    collect(root, root, &mut records)?;
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::validate_projection_locations;

    /// A compiler host inherits the CLI's registry patch; ordinary user projects retain their own resolution.
    #[test]
    fn compiler_host_preserves_the_cli_registry_patch() -> Result<(), Box<dyn std::error::Error>> {
        use super::{DependencySource, DependencySpec, ProjectGenerator, development_root};
        let temporary = tempfile::tempdir()?;
        let mut generator = ProjectGenerator::new(temporary.path(), "host", true);
        assert!(generator.compiler_closure_patches()?.is_none());
        generator.set_dependencies(vec![DependencySpec {
            crate_name: "incan_driver".into(),
            version: None,
            features: vec!["rust_inspect".into()],
            default_features: false,
            source: DependencySource::Path {
                path: development_root().join("loaves/compiler/incan_driver"),
            },
            optional: false,
            package: None,
        }]);
        let patches = generator
            .compiler_closure_patches()?
            .ok_or("missing compiler host patches")?;
        let path = patches
            .get("crates-io")
            .and_then(|registry| registry.get("ra_ap_proc_macro_api"))
            .and_then(|package| package.get("path"))
            .and_then(toml::Value::as_str)
            .ok_or("missing checked-in proc-macro API patch")?;
        assert_eq!(
            std::fs::canonicalize(path)?,
            std::fs::canonicalize(development_root().join("loaves/third_party/ra_ap_proc_macro_api"))?
        );
        Ok(())
    }

    /// Moving a package must reject actual manifest-root captures, including concatenated environment keys.
    #[test]
    fn projection_refuses_manifest_root_captures() {
        for source in [
            "fn root() { env!(\"CARGO_MANIFEST_DIR\"); }",
            "fn root() { option_env!(concat!(\"CARGO\", \"_MANIFEST_DIR\")); }",
        ] {
            assert!(validate_projection_locations(&[("lib.rs".into(), source.as_bytes().to_vec())]).is_err());
        }
    }

    /// A child environment key is not a compile-time source-location dependency.
    #[test]
    fn projection_preserves_child_environment_controls() -> Result<(), Box<dyn std::error::Error>> {
        validate_projection_locations(&[("lib.rs".into(), b"fn clean(command: &mut Command) { command.env_remove(\"CARGO_MANIFEST_DIR\"); command.env(\"CARGO_MANIFEST_DIR\", selected); }".to_vec())])?;
        Ok(())
    }
}
