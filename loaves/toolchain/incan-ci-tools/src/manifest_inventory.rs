//! Reproducible inventory of the Cargo facts that Oven Rust facets must replace.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;
use serde::Serialize;
use serde_json::{Map, Value, json};
use thiserror::Error;

const SCHEMA_VERSION: u64 = 1;
const INHERITABLE_PACKAGE_KEYS: [&str; 16] = [
    "authors",
    "categories",
    "description",
    "documentation",
    "edition",
    "exclude",
    "homepage",
    "include",
    "keywords",
    "license",
    "license-file",
    "publish",
    "readme",
    "repository",
    "rust-version",
    "version",
];
const DEPENDENCY_TABLES: [(&str, &str); 3] = [
    ("dependencies", "normal"),
    ("dev-dependencies", "dev"),
    ("build-dependencies", "build"),
];

/// Failures produced while discovering, parsing, rendering, or checking the inventory.
#[derive(Debug, Error)]
pub enum InventoryError {
    /// A filesystem operation failed.
    #[error("{context}: {source}")]
    Io {
        /// Description of the attempted operation.
        context: String,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// A manifest contained invalid TOML.
    #[error("failed to parse {path}: {source}")]
    Toml {
        /// Manifest path being parsed.
        path: String,
        /// TOML parser diagnostic.
        source: toml::de::Error,
    },
    /// Git could not enumerate tracked manifests or locate the repository.
    #[error("git {operation} failed: {detail}")]
    Git {
        /// Git operation that failed.
        operation: &'static str,
        /// Captured diagnostic or exit status.
        detail: String,
    },
    /// A Cargo manifest violates an inventory invariant.
    #[error("{0}")]
    Contract(String),
    /// The committed inventory is absent or stale.
    #[error("{path} is stale; rerun incan-cargo-manifest-inventory")]
    Stale {
        /// Repository-relative inventory path.
        path: String,
    },
    /// Stable JSON rendering failed.
    #[error("failed to render inventory JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// JSON bytes unexpectedly failed UTF-8 validation.
    #[error("rendered inventory was not UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

/// Command-line options for inventory generation and verification.
#[derive(Debug, Parser)]
#[command(about = "Inventory Cargo facts needed by Oven Rust facets")]
struct Arguments {
    /// Fail when the committed JSON differs from a fresh inventory.
    #[arg(long)]
    check: bool,
    /// Print the rendered per-manifest Markdown table.
    #[arg(long)]
    markdown: bool,
    /// Write or check a non-default JSON path.
    #[arg(long)]
    output: Option<PathBuf>,
}

/// Parse process arguments and execute the manifest inventory command.
pub fn run_from_environment() -> Result<(), InventoryError> {
    run(Arguments::parse())
}

/// Execute one inventory command using the repository found from the current directory.
fn run(arguments: Arguments) -> Result<(), InventoryError> {
    let root = repository_root()?;
    let output = arguments
        .output
        .unwrap_or_else(|| root.join("scripts/self_build/cargo_manifest_inventory.json"));
    let inventory = build_inventory(&root)?;
    if arguments.markdown {
        print!("{}", render_markdown(&inventory));
        return Ok(());
    }
    let text = render_json(&inventory)?;
    if arguments.check {
        let current = fs::read_to_string(&output).map_err(|source| InventoryError::Io {
            context: format!("failed to read {}", output.display()),
            source,
        })?;
        if current != text {
            return Err(InventoryError::Stale {
                path: portable(&root, &output),
            });
        }
        println!(
            "{} is current ({} manifests)",
            portable(&root, &output),
            inventory["summary"]["manifest_count"]
        );
        return Ok(());
    }
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|source| InventoryError::Io {
            context: format!("failed to create {}", parent.display()),
            source,
        })?;
    }
    fs::write(&output, text).map_err(|source| InventoryError::Io {
        context: format!("failed to write {}", output.display()),
        source,
    })?;
    println!(
        "wrote {} ({} manifests)",
        portable(&root, &output),
        inventory["summary"]["manifest_count"]
    );
    Ok(())
}

/// Resolve the checkout root through Git so the command works from any subdirectory.
fn repository_root() -> Result<PathBuf, InventoryError> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|source| InventoryError::Io {
            context: "failed to execute git rev-parse".to_owned(),
            source,
        })?;
    if !output.status.success() {
        return Err(InventoryError::Git {
            operation: "rev-parse",
            detail: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
}

/// Return every tracked Cargo manifest in sorted repository-relative order.
fn find_manifests(root: &Path) -> Result<Vec<PathBuf>, InventoryError> {
    let output = Command::new("git")
        .args(["ls-files", "--", "Cargo.toml", "**/Cargo.toml"])
        .current_dir(root)
        .output()
        .map_err(|source| InventoryError::Io {
            context: "failed to execute git ls-files".to_owned(),
            source,
        })?;
    if !output.status.success() {
        return Err(InventoryError::Git {
            operation: "ls-files",
            detail: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(manifests_from_git_output(
        root,
        &String::from_utf8_lossy(&output.stdout),
    ))
}

/// Convert tracked Git output into sorted absolute manifest paths.
fn manifests_from_git_output(root: &Path, output: &str) -> Vec<PathBuf> {
    let mut paths = output
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| root.join(line))
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

/// Render a path as a repository-relative POSIX path.
fn portable(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Parse one TOML document into its generic value tree.
fn load_toml(path: &Path) -> Result<toml::Value, InventoryError> {
    let text = fs::read_to_string(path).map_err(|source| InventoryError::Io {
        context: format!("failed to read {}", path.display()),
        source,
    })?;
    toml::from_str(&text).map_err(|source| InventoryError::Toml {
        path: path.display().to_string(),
        source,
    })
}

/// Return a TOML table or an empty table when the key is absent or not a table.
fn child_table<'a>(value: &'a toml::Value, key: &str) -> Option<&'a toml::Table> {
    value.get(key).and_then(toml::Value::as_table)
}

/// Classify a manifest into the self-build migration population it belongs to.
fn classify(root: &Path, path: &Path, document: &toml::Value, member_directories: &BTreeSet<String>) -> String {
    let relative = portable(root, path);
    let directory = path.parent().map(|parent| portable(root, parent)).unwrap_or_default();
    if path.parent() == Some(root) {
        "workspace-root"
    } else if member_directories.contains(&directory) {
        "workspace-member"
    } else if relative.starts_with("loaves/third_party/") {
        "third-party-patch"
    } else if relative.contains("/fixtures/") {
        "fixture"
    } else if relative.starts_with("examples/") {
        "example-companion"
    } else if document.get("workspace").is_some() {
        "standalone"
    } else {
        "unclassified"
    }
    .to_owned()
}

/// Resolve `key.workspace = true` package values against the root package table.
fn resolve_package(
    package: &toml::Table,
    workspace_package: &toml::Table,
) -> Result<(toml::Table, Vec<String>), InventoryError> {
    let mut resolved = toml::Table::new();
    let mut inherited = Vec::new();
    for (key, value) in package {
        let inherits = value
            .as_table()
            .and_then(|table| table.get("workspace"))
            .and_then(toml::Value::as_bool)
            == Some(true);
        if inherits {
            if !INHERITABLE_PACKAGE_KEYS.contains(&key.as_str()) {
                return Err(InventoryError::Contract(format!(
                    "package key `{key}` cannot inherit from the workspace"
                )));
            }
            let inherited_value = workspace_package.get(key).ok_or_else(|| {
                InventoryError::Contract(format!(
                    "package key `{key}` claims workspace inheritance but the root does not declare it"
                ))
            })?;
            resolved.insert(key.clone(), inherited_value.clone());
            inherited.push(key.clone());
        } else {
            resolved.insert(key.clone(), value.clone());
        }
    }
    inherited.sort();
    Ok((resolved, inherited))
}

/// Convert a TOML value to the corresponding JSON value without reparsing text.
fn toml_json(value: &toml::Value) -> Result<Value, InventoryError> {
    Ok(serde_json::to_value(value)?)
}

/// Convert an optional TOML value to JSON, retaining absence as JSON null.
fn optional_toml_json(value: Option<&toml::Value>) -> Result<Value, InventoryError> {
    value.map_or(Ok(Value::Null), toml_json)
}

/// Normalize one dependency declaration into the Rust-facet evidence contract.
fn normalize_dependency(
    alias: &str,
    entry: &toml::Value,
    workspace_dependencies: &toml::Table,
    workspace_packages: &BTreeSet<String>,
) -> Result<Value, InventoryError> {
    let mut table = if let Some(version) = entry.as_str() {
        toml::Table::from_iter([("version".to_owned(), toml::Value::String(version.to_owned()))])
    } else if let Some(table) = entry.as_table() {
        table.clone()
    } else {
        return Err(InventoryError::Contract(format!(
            "dependency `{alias}` has an unsupported shape"
        )));
    };
    let inherited = table.remove("workspace").and_then(|value| value.as_bool()) == Some(true);
    if inherited {
        let root_entry = workspace_dependencies.get(alias).ok_or_else(|| {
            InventoryError::Contract(format!(
                "dependency `{alias}` inherits from the workspace but the root does not declare it"
            ))
        })?;
        let member_features = table
            .remove("features")
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        let member_optional = table.remove("optional");
        if !table.is_empty() {
            return Err(InventoryError::Contract(format!(
                "dependency `{alias}` sets {:?} alongside workspace = true",
                table.keys().collect::<Vec<_>>()
            )));
        }
        table = if let Some(version) = root_entry.as_str() {
            toml::Table::from_iter([("version".to_owned(), toml::Value::String(version.to_owned()))])
        } else {
            root_entry.as_table().cloned().ok_or_else(|| {
                InventoryError::Contract(format!("workspace dependency `{alias}` has an unsupported shape"))
            })?
        };
        let mut features = table
            .remove("features")
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        features.extend(member_features);
        features.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
        features.dedup();
        table.insert("features".to_owned(), toml::Value::Array(features));
        if let Some(optional) = member_optional {
            table.insert("optional".to_owned(), optional);
        }
    }
    let package = table.get("package").and_then(toml::Value::as_str).unwrap_or(alias);
    let origin = if table.contains_key("path") {
        "path"
    } else if table.contains_key("git") {
        "git"
    } else {
        "registry"
    };
    let generated_path = table
        .get("path")
        .and_then(toml::Value::as_str)
        .map(|path| {
            Path::new(path).components().any(|part| {
                matches!(
                    part.as_os_str().to_string_lossy().as_ref(),
                    ".git"
                        | ".incan"
                        | ".lane"
                        | ".ralph-cache"
                        | "node_modules"
                        | "target"
                        | "incan_generated_shared_target"
                )
            })
        })
        .unwrap_or(false);
    let kind = if workspace_packages.contains(package) {
        "workspace-internal"
    } else if origin == "path" && generated_path {
        "generated"
    } else {
        "third-party"
    };
    let mut features = table
        .get("features")
        .and_then(toml::Value::as_array)
        .cloned()
        .unwrap_or_default();
    features.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
    let mut record = Map::new();
    record.insert("package".to_owned(), json!(package));
    record.insert("kind".to_owned(), json!(kind));
    record.insert("origin".to_owned(), json!(origin));
    record.insert("workspace_inherited".to_owned(), json!(inherited));
    record.insert("version_req".to_owned(), optional_toml_json(table.get("version"))?);
    record.insert("features".to_owned(), toml_json(&toml::Value::Array(features))?);
    record.insert(
        "default_features".to_owned(),
        json!(
            table
                .get("default-features")
                .and_then(toml::Value::as_bool)
                .unwrap_or(true)
        ),
    );
    record.insert(
        "optional".to_owned(),
        json!(table.get("optional").and_then(toml::Value::as_bool).unwrap_or(false)),
    );
    if package != alias {
        record.insert("renamed_from".to_owned(), json!(alias));
    }
    if let Some(path) = table.get("path") {
        record.insert("path".to_owned(), toml_json(path)?);
    }
    if origin == "git" {
        let mut git = Map::new();
        for key in ["git", "branch", "tag", "rev"] {
            if let Some(value) = table.get(key) {
                git.insert(key.to_owned(), toml_json(value)?);
            }
        }
        record.insert("git".to_owned(), Value::Object(git));
    }
    Ok(Value::Object(record))
}

/// Normalize normal, development, build, and target-specific dependency tables.
fn dependency_tables(
    document: &toml::Value,
    workspace_dependencies: &toml::Table,
    workspace_packages: &BTreeSet<String>,
) -> Result<Value, InventoryError> {
    let mut tables = Map::new();
    for (key, role) in DEPENDENCY_TABLES {
        let mut normalized = Map::new();
        if let Some(entries) = child_table(document, key) {
            let mut entries = entries.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(alias, _)| *alias);
            for (alias, entry) in entries {
                normalized.insert(
                    alias.clone(),
                    normalize_dependency(alias, entry, workspace_dependencies, workspace_packages)?,
                );
            }
        }
        tables.insert(role.to_owned(), Value::Object(normalized));
    }
    let mut targets = Map::new();
    if let Some(target_table) = child_table(document, "target") {
        let mut target_entries = target_table.iter().collect::<Vec<_>>();
        target_entries.sort_by_key(|(predicate, _)| *predicate);
        for (predicate, sections) in target_entries {
            let mut roles = Map::new();
            for (key, role) in DEPENDENCY_TABLES {
                if let Some(entries) = child_table(sections, key)
                    && !entries.is_empty()
                {
                    let mut normalized = Map::new();
                    let mut entries = entries.iter().collect::<Vec<_>>();
                    entries.sort_by_key(|(alias, _)| *alias);
                    for (alias, entry) in entries {
                        normalized.insert(
                            alias.clone(),
                            normalize_dependency(alias, entry, workspace_dependencies, workspace_packages)?,
                        );
                    }
                    roles.insert(role.to_owned(), Value::Object(normalized));
                }
            }
            if !roles.is_empty() {
                targets.insert(predicate.clone(), Value::Object(roles));
            }
        }
    }
    tables.insert("target".to_owned(), Value::Object(targets));
    Ok(Value::Object(tables))
}

/// Collect leading crate-level attributes, joining multiline declarations.
fn crate_level_attributes(source: &Path) -> Result<Vec<String>, InventoryError> {
    if !source.is_file() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(source).map_err(|source_error| InventoryError::Io {
        context: format!("failed to read {}", source.display()),
        source: source_error,
    })?;
    let mut attributes = Vec::new();
    let mut pending = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if !pending.is_empty() {
            pending.push(line);
            if line.ends_with(']') {
                attributes.push(pending.join(" "));
                pending.clear();
            }
        } else if line.is_empty() || line.starts_with("//") {
            continue;
        } else if line.starts_with("#![") {
            if line.ends_with(']') {
                attributes.push(line.to_owned());
            } else {
                pending.push(line);
            }
        } else {
            break;
        }
    }
    Ok(attributes
        .into_iter()
        .map(|attribute| attribute.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect())
}

/// Resolve explicit and conventional library and binary crate roots.
fn target_roots(root: &Path, manifest_dir: &Path, document: &toml::Value) -> Vec<(String, String)> {
    let mut roots = Vec::new();
    let package_name = document
        .get("package")
        .and_then(|value| value.get("name"))
        .and_then(toml::Value::as_str)
        .unwrap_or_default();
    let library = child_table(document, "lib");
    let library_path = library
        .and_then(|table| table.get("path"))
        .and_then(toml::Value::as_str)
        .map(|path| manifest_dir.join(path))
        .or_else(|| {
            let conventional = manifest_dir.join("src/lib.rs");
            conventional.is_file().then_some(conventional)
        });
    if let Some(path) = library_path {
        roots.push(("lib".to_owned(), portable(root, &path)));
    }
    if let Some(binaries) = document.get("bin").and_then(toml::Value::as_array) {
        for binary in binaries {
            let name = binary.get("name").and_then(toml::Value::as_str).unwrap_or(package_name);
            let path = binary
                .get("path")
                .and_then(toml::Value::as_str)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| format!("src/bin/{name}.rs"));
            roots.push((format!("bin:{name}"), portable(root, &manifest_dir.join(path))));
        }
    } else {
        let conventional = manifest_dir.join("src/main.rs");
        if conventional.is_file() {
            roots.push((format!("bin:{package_name}"), portable(root, &conventional)));
        }
    }
    roots
}

/// Describe whether Cargo would execute a package build script.
fn build_script(root: &Path, manifest_dir: &Path, package: &toml::Table) -> Result<Value, InventoryError> {
    let declared = package.get("build");
    if declared.and_then(toml::Value::as_bool) == Some(false) {
        return Ok(json!({"present": false, "declared": false, "path": null}));
    }
    let candidate = manifest_dir.join(declared.and_then(toml::Value::as_str).unwrap_or("build.rs"));
    Ok(json!({
        "present": candidate.is_file(),
        "declared": declared.map(toml_json).transpose()?,
        "path": candidate.is_file().then(|| portable(root, &candidate)),
    }))
}

/// Build the complete record for one manifest.
fn manifest_record(
    root: &Path,
    path: &Path,
    workspace: &toml::Table,
    member_directories: &BTreeSet<String>,
    workspace_packages: &BTreeSet<String>,
) -> Result<Value, InventoryError> {
    let document = load_toml(path)?;
    let manifest_dir = path
        .parent()
        .ok_or_else(|| InventoryError::Contract(format!("{} has no parent directory", path.display())))?;
    let category = classify(root, path, &document, member_directories);
    let empty = toml::Table::new();
    let package_table = child_table(&document, "package").unwrap_or(&empty);
    let workspace_package = workspace
        .get("package")
        .and_then(toml::Value::as_table)
        .unwrap_or(&empty);
    let workspace_dependencies = workspace
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .unwrap_or(&empty);
    let (package, inherited) = if matches!(category.as_str(), "workspace-member" | "workspace-root") {
        resolve_package(package_table, workspace_package)?
    } else {
        (package_table.clone(), Vec::new())
    };
    let library = child_table(&document, "lib");
    let mut record = Map::new();
    record.insert("path".to_owned(), json!(portable(root, path)));
    record.insert("category".to_owned(), json!(category));
    record.insert(
        "cargo_lock_beside".to_owned(),
        json!(manifest_dir.join("Cargo.lock").is_file()),
    );
    record.insert(
        "is_virtual_workspace".to_owned(),
        json!(document.get("package").is_none() && document.get("workspace").is_some()),
    );
    record.insert(
        "declares_own_workspace".to_owned(),
        json!(document.get("workspace").is_some() && manifest_dir != root),
    );
    if !package.is_empty() {
        let package_value = toml::Value::Table(package.clone());
        let mut package_record = Map::new();
        for (json_key, toml_key) in [
            ("name", "name"),
            ("version", "version"),
            ("edition", "edition"),
            ("rust_version", "rust-version"),
            ("license", "license"),
        ] {
            package_record.insert(json_key.to_owned(), optional_toml_json(package.get(toml_key))?);
        }
        package_record.insert(
            "publish".to_owned(),
            package.get("publish").map_or(Ok(json!(true)), toml_json)?,
        );
        for key in ["description", "resolver"] {
            package_record.insert(key.to_owned(), optional_toml_json(package.get(key))?);
        }
        let mut auto_discovery_off = ["autolib", "autobins", "autoexamples", "autotests", "autobenches"]
            .into_iter()
            .filter(|key| package.get(*key).and_then(toml::Value::as_bool) == Some(false))
            .collect::<Vec<_>>();
        auto_discovery_off.sort();
        package_record.insert("auto_discovery_off".to_owned(), json!(auto_discovery_off));
        record.insert("package".to_owned(), Value::Object(package_record));
        record.insert("inherited_package_keys".to_owned(), json!(inherited));
        record.insert(
            "lib".to_owned(),
            if let Some(lib) = library {
                json!({
                    "name": lib.get("name").map(toml_json).transpose()?,
                    "path": lib.get("path").map(toml_json).transpose()?,
                    "proc_macro": lib.get("proc-macro").and_then(toml::Value::as_bool).unwrap_or(false),
                    "crate_type": lib.get("crate-type").map(toml_json).transpose()?,
                    "doctest": lib.get("doctest").and_then(toml::Value::as_bool).unwrap_or(true),
                })
            } else {
                Value::Null
            },
        );
        for (record_key, document_key) in [
            ("bins", "bin"),
            ("explicit_tests", "test"),
            ("explicit_examples", "example"),
            ("explicit_benches", "bench"),
        ] {
            let values = document
                .get(document_key)
                .and_then(toml::Value::as_array)
                .map(|targets| {
                    targets
                        .iter()
                        .map(|target| {
                            if record_key == "bins" {
                                Ok(json!({
                                    "name": target.get("name").map(toml_json).transpose()?,
                                    "path": target.get("path").map(toml_json).transpose()?,
                                }))
                            } else {
                                optional_toml_json(target.get("name"))
                            }
                        })
                        .collect::<Result<Vec<_>, InventoryError>>()
                })
                .transpose()?
                .unwrap_or_default();
            record.insert(record_key.to_owned(), Value::Array(values));
        }
        let features = child_table(&document, "features").cloned().unwrap_or_default();
        let sorted_features = features
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<BTreeMap<_, _>>()
            .into_iter()
            .collect::<toml::Table>();
        record.insert("features".to_owned(), toml_json(&toml::Value::Table(sorted_features))?);
        record.insert(
            "default_features".to_owned(),
            features.get("default").map_or(Ok(json!([])), toml_json)?,
        );
        record.insert(
            "dependencies".to_owned(),
            dependency_tables(&document, workspace_dependencies, workspace_packages)?,
        );
        record.insert("build_script".to_owned(), build_script(root, manifest_dir, &package)?);
        record.insert("lints".to_owned(), optional_toml_json(document.get("lints"))?);
        record.insert(
            "package_metadata".to_owned(),
            optional_toml_json(package_value.get("metadata"))?,
        );
        let roots = target_roots(root, manifest_dir, &document);
        record.insert(
            "target_roots".to_owned(),
            Value::Object(
                roots
                    .iter()
                    .map(|(target, source)| (target.clone(), json!(source)))
                    .collect(),
            ),
        );
        let mut attributes = Map::new();
        let sorted_roots = roots.into_iter().collect::<BTreeMap<_, _>>();
        for (target, source) in sorted_roots {
            attributes.insert(target, json!(crate_level_attributes(&root.join(source))?));
        }
        record.insert("crate_level_attributes".to_owned(), Value::Object(attributes));
    }
    if let Some(section) = child_table(&document, "workspace") {
        let mut dependencies = Map::new();
        if let Some(entries) = section.get("dependencies").and_then(toml::Value::as_table) {
            let mut entries = entries.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(alias, _)| *alias);
            for (alias, entry) in entries {
                dependencies.insert(
                    alias.clone(),
                    if let Some(version) = entry.as_str() {
                        json!({"version": version})
                    } else {
                        toml_json(entry)?
                    },
                );
            }
        }
        record.insert(
            "workspace".to_owned(),
            json!({
                "members": section.get("members").map(toml_json).transpose()?.unwrap_or_else(|| json!([])),
                "exclude": section.get("exclude").map(toml_json).transpose()?.unwrap_or_else(|| json!([])),
                "resolver": section.get("resolver").map(toml_json).transpose()?,
                "package": section.get("package").map(toml_json).transpose()?,
                "dependencies": dependencies,
            }),
        );
    }
    for key in ["patch", "profile"] {
        if let Some(value) = document.get(key) {
            record.insert(key.to_owned(), toml_json(value)?);
        }
    }
    Ok(Value::Object(record))
}

/// Build the complete stable inventory document for a checkout.
pub fn build_inventory(root: &Path) -> Result<Value, InventoryError> {
    let manifests = find_manifests(root)?;
    let root_document = load_toml(&root.join("Cargo.toml"))?;
    let workspace = child_table(&root_document, "workspace")
        .ok_or_else(|| InventoryError::Contract("root Cargo.toml has no workspace table".to_owned()))?;
    let member_directories = workspace
        .get("members")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    let mut workspace_packages = BTreeSet::new();
    for member in &member_directories {
        let member_manifest = root.join(member).join("Cargo.toml");
        if !member_manifest.is_file() {
            return Err(InventoryError::Contract(format!(
                "workspace member `{member}` has no Cargo.toml"
            )));
        }
        let document = load_toml(&member_manifest)?;
        let name = document
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(toml::Value::as_str)
            .ok_or_else(|| InventoryError::Contract(format!("workspace member `{member}` has no package name")))?;
        workspace_packages.insert(name.to_owned());
    }
    let records = manifests
        .iter()
        .map(|path| manifest_record(root, path, workspace, &member_directories, &workspace_packages))
        .collect::<Result<Vec<_>, _>>()?;
    let summary = inventory_summary(&records, &workspace_packages);
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "generated_by": "incan-cargo-manifest-inventory",
        "summary": summary,
        "manifests": records,
    }))
}

/// Aggregate per-manifest records into the migration summary.
fn inventory_summary(records: &[Value], workspace_packages: &BTreeSet<String>) -> Value {
    let members = records
        .iter()
        .filter(|record| record["category"] == "workspace-member")
        .collect::<Vec<_>>();
    let mut categories = BTreeMap::<String, usize>::new();
    for record in records {
        if let Some(category) = record["category"].as_str() {
            *categories.entry(category.to_owned()).or_default() += 1;
        }
    }
    let mut third_party = BTreeMap::<String, BTreeSet<String>>::new();
    for record in &members {
        for role in ["normal", "dev", "build"] {
            if let Some(dependencies) = record["dependencies"][role].as_object() {
                for dependency in dependencies.values() {
                    if dependency["kind"] == "third-party"
                        && let Some(package) = dependency["package"].as_str()
                    {
                        third_party
                            .entry(package.to_owned())
                            .or_default()
                            .insert(role.to_owned());
                    }
                }
            }
        }
    }
    let third_party_count = third_party.len();
    let member_names = |predicate: fn(&Value) -> bool| {
        members
            .iter()
            .filter(|record| predicate(record))
            .filter_map(|record| record["package"]["name"].as_str())
            .collect::<BTreeSet<_>>()
    };
    let mut bins = BTreeMap::<String, Value>::new();
    for record in &members {
        if record["bins"].as_array().is_some_and(|values| !values.is_empty())
            && let Some(name) = record["package"]["name"].as_str()
        {
            bins.insert(
                name.to_owned(),
                Value::Array(
                    record["bins"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|binary| binary["name"].clone())
                        .collect(),
                ),
            );
        }
    }
    let mut lint_attributes = Map::new();
    for record in records {
        let mut targets = Map::new();
        if let Some(attributes_by_target) = record["crate_level_attributes"].as_object() {
            for (target, attributes) in attributes_by_target {
                let matching = attributes
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|attribute| attribute.as_str().is_some_and(is_lint_attribute))
                    .cloned()
                    .collect::<Vec<_>>();
                targets.insert(target.clone(), Value::Array(matching));
            }
        }
        if targets
            .values()
            .any(|attributes| attributes.as_array().is_some_and(|values| !values.is_empty()))
            && let Some(path) = record["path"].as_str()
        {
            lint_attributes.insert(path.to_owned(), Value::Object(targets));
        }
    }
    json!({
        "manifest_count": records.len(),
        "by_category": categories,
        "workspace_member_count": members.len(),
        "workspace_packages": workspace_packages,
        "proc_macro_members": member_names(|record| record["lib"]["proc_macro"] == true),
        "members_with_bins": bins,
        "members_with_features": member_names(|record| record["features"].as_object().is_some_and(|value| !value.is_empty())),
        "members_with_dev_dependencies": member_names(|record| record["dependencies"]["dev"].as_object().is_some_and(|value| !value.is_empty())),
        "members_with_build_scripts": member_names(|record| record["build_script"]["present"] == true),
        "manifests_with_lints_table": records.iter().filter(|record| !record["lints"].is_null()).filter_map(|record| record["path"].as_str()).collect::<BTreeSet<_>>(),
        "manifests_with_build_scripts": records.iter().filter(|record| record["build_script"]["present"] == true).filter_map(|record| record["path"].as_str()).collect::<BTreeSet<_>>(),
        "editions": records.iter().filter_map(|record| record["package"]["edition"].as_str()).collect::<BTreeSet<_>>(),
        "third_party_packages_used_by_members": third_party,
        "third_party_package_count_members": third_party_count,
        "crate_level_lint_attributes": lint_attributes,
    })
}

/// Return whether one crate attribute declares a lint level.
fn is_lint_attribute(attribute: &str) -> bool {
    ["#![deny(", "#![warn(", "#![forbid(", "#![allow("]
        .iter()
        .any(|prefix| attribute.starts_with(prefix))
}

/// Render stable, one-space-indented JSON text.
pub fn render_json(inventory: &Value) -> Result<String, InventoryError> {
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b" ");
    let mut bytes = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, formatter);
    inventory.serialize(&mut serializer)?;
    let mut rendered = String::from_utf8(bytes)?;
    rendered.push('\n');
    Ok(rendered)
}

/// Render one Markdown row per manifest with the facts a Rust facet must carry.
pub fn render_markdown(inventory: &Value) -> String {
    let mut lines = vec![
        "| manifest | category | package | version | edition | targets | features (default) | deps | dev-deps | build.rs | `[lints]` | crate-level `#![…]` |".to_owned(),
        "|---|---|---|---|---|---|---|---|---|---|---|---|".to_owned(),
    ];
    for record in inventory["manifests"].as_array().into_iter().flatten() {
        if record.get("package").is_none() {
            let members = record["workspace"]["members"].as_array().map_or(0, Vec::len);
            lines.push(format!(
                "| `{}` | {} | (virtual workspace, {members} members) | {} | {} | – | – | – | – | – | – | – |",
                text(&record["path"]),
                text(&record["category"]),
                display_or_dash(&record["workspace"]["package"]["version"]),
                display_or_dash(&record["workspace"]["package"]["edition"]),
            ));
            continue;
        }
        let mut targets = Vec::new();
        if !record["lib"].is_null() {
            let mut kind = if record["lib"]["proc_macro"] == true {
                "proc-macro".to_owned()
            } else {
                "lib".to_owned()
            };
            if let Some(crate_types) = record["lib"]["crate_type"].as_array() {
                kind.push_str(&format!(
                    " ({})",
                    crate_types.iter().map(text).collect::<Vec<_>>().join(", ")
                ));
            }
            targets.push(kind);
        } else if record["target_roots"].get("lib").is_some() {
            targets.push("lib (conventional)".to_owned());
        }
        if let Some(binaries) = record["bins"].as_array() {
            for binary in binaries {
                targets.push(format!("bin `{}`", text(&binary["name"])));
            }
            if binaries.is_empty()
                && let Some(roots) = record["target_roots"].as_object()
            {
                for target in roots.keys().filter(|target| target.starts_with("bin:")) {
                    targets.push(format!("bin `{}` (conventional)", &target[4..]));
                }
            }
        }
        lines.push(format!(
            "| `{}` | {} | `{}` | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            text(&record["path"]),
            text(&record["category"]),
            text(&record["package"]["name"]),
            display_or_dash(&record["package"]["version"]),
            display_or_dash(&record["package"]["edition"]),
            if targets.is_empty() {
                "–".to_owned()
            } else {
                targets.join(", ")
            },
            feature_text(record),
            dependency_text(record, "normal"),
            dependency_text(record, "dev"),
            build_script_text(record),
            if record["lints"].is_null() { "–" } else { "yes" },
            lint_attribute_text(record),
        ));
    }
    format!("{}\n", lines.join("\n"))
}

/// Render a JSON scalar as unquoted table text.
fn text(value: &Value) -> String {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| value.to_string())
}

/// Render a nullable scalar, using an en dash for missing values.
fn display_or_dash(value: &Value) -> String {
    if value.is_null() { "–".to_owned() } else { text(value) }
}

/// Summarize one dependency role by internal, third-party, and generated counts.
fn dependency_text(record: &Value, role: &str) -> String {
    let Some(table) = record["dependencies"][role].as_object() else {
        return "–".to_owned();
    };
    if table.is_empty() {
        return "–".to_owned();
    }
    let internal = table
        .values()
        .filter(|dependency| dependency["kind"] == "workspace-internal")
        .count();
    let generated = table
        .values()
        .filter(|dependency| dependency["kind"] == "generated")
        .count();
    let external = table.len() - internal - generated;
    let mut output = format!("{internal} ws / {external} 3p");
    if generated > 0 {
        output.push_str(&format!(" / {generated} generated"));
    }
    output
}

/// Summarize feature names and the default feature set.
fn feature_text(record: &Value) -> String {
    let Some(features) = record["features"].as_object() else {
        return "–".to_owned();
    };
    if features.is_empty() {
        return "–".to_owned();
    }
    let names = features
        .keys()
        .filter(|name| name.as_str() != "default")
        .cloned()
        .collect::<Vec<_>>();
    let default = record["default_features"]
        .as_array()
        .into_iter()
        .flatten()
        .map(text)
        .collect::<Vec<_>>();
    format!(
        "{}{} (default = {})",
        names.len(),
        if names.is_empty() {
            String::new()
        } else {
            format!(": {}", names.join(", "))
        },
        if default.is_empty() {
            "∅".to_owned()
        } else {
            default.join(", ")
        }
    )
}

/// Render build-script presence and explicit disablement.
fn build_script_text(record: &Value) -> String {
    if let Some(path) = record["build_script"]["path"].as_str() {
        path.to_owned()
    } else if record["build_script"]["declared"] == false {
        "declared off".to_owned()
    } else {
        "–".to_owned()
    }
}

/// Render the unique crate-level lint declarations for one manifest.
fn lint_attribute_text(record: &Value) -> String {
    let mut attributes = BTreeSet::new();
    if let Some(targets) = record["crate_level_attributes"].as_object() {
        for attribute in targets
            .values()
            .filter_map(Value::as_array)
            .flatten()
            .filter_map(Value::as_str)
            .filter(|attribute| is_lint_attribute(attribute))
        {
            attributes.insert(attribute.trim_start_matches("#![").trim_end_matches(']'));
        }
    }
    if attributes.is_empty() {
        "–".to_owned()
    } else {
        attributes.into_iter().collect::<Vec<_>>().join("; ")
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::error::Error;
    use std::fs;
    use std::path::Path;

    use super::{build_inventory, crate_level_attributes, manifests_from_git_output, render_json};
    use serde_json::json;

    /// Tracked discovery ignores absent and untracked manifests because Git supplies the source list.
    #[test]
    fn tracked_manifest_output_is_sorted_and_rooted() {
        let root = Path::new("/repo");
        assert_eq!(
            manifests_from_git_output(root, "z/Cargo.toml\nCargo.toml\n\n"),
            vec![root.join("Cargo.toml"), root.join("z/Cargo.toml")]
        );
    }

    /// Crate attribute collection joins multiline policies and stops at the first code item.
    #[test]
    fn crate_attributes_preserve_leading_policy() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("lib.rs");
        fs::write(
            &source,
            "//! docs\n#![deny(\n clippy::unwrap_used,\n)]\n#![warn(missing_docs)]\npub fn later() {}\n#![allow(dead_code)]\n",
        )?;
        assert_eq!(
            crate_level_attributes(&source)?,
            vec!["#![deny( clippy::unwrap_used, )]", "#![warn(missing_docs)]"]
        );
        Ok(())
    }

    /// Stable JSON retains the legacy one-space indentation and trailing newline contract.
    #[test]
    fn json_rendering_is_stable() -> Result<(), Box<dyn Error>> {
        assert_eq!(render_json(&json!({"a": [1]}))?, "{\n \"a\": [\n  1\n ]\n}\n");
        Ok(())
    }

    /// The committed repository inventory is byte-for-byte reproducible from tracked manifests.
    #[test]
    fn repository_inventory_matches_committed() -> Result<(), Box<dyn Error>> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .ok_or("CI tools manifest is not nested three levels below the repository root")?;
        let output = root.join("scripts/self_build/cargo_manifest_inventory.json");
        let rendered = render_json(&build_inventory(root)?)?;
        if env::var_os("UPDATE_CARGO_MANIFEST_INVENTORY").is_some() {
            fs::write(&output, rendered)?;
        } else {
            assert_eq!(fs::read_to_string(&output)?, rendered);
        }
        Ok(())
    }
}
