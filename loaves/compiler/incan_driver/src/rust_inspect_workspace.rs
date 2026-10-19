//! The direct Rust inspection root: an authored Loaf and probe source under the project's lock state, so the
//! inspector can see the exact Rust items a program imports through the selected native closure.
//!
//! Everything here is about producing that root and the query surface it answers. Reading Rust metadata out of it
//! is the `rust_inspect` crate's job.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{CliError, CliResult};
use ::rust_inspect::Inspector;
use ::rust_inspect::InspectorConfig;
use incan_frontend::ast::{ImportKind, Program};
use incan_frontend::parsed_module::ParsedModule;

/// Return the workspace directory used for Rust inspection metadata.
fn rust_inspect_workspace_dir(project_root: &Path, project_name: &str, fingerprint: &str) -> PathBuf {
    let mut safe_name = project_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if safe_name.is_empty() {
        safe_name.push_str("project");
    }
    let suffix = fingerprint
        .rsplit_once(':')
        .map(|(_, hash)| hash)
        .unwrap_or(fingerprint)
        .chars()
        .take(16)
        .collect::<String>();
    oven_model::lock::compiler_lock_state_dir(project_root)
        .join("rust_inspect")
        .join(format!("{safe_name}-{suffix}"))
}

/// Prepare only an authored Loaf and Rust probe for the direct inspector, without generating any build metadata.
///
/// Selected source authority is installed separately by the caller. Dependencies are deliberately absent here:
/// this stub cannot resolve them or replace the frozen closure's aliases, features, or source selections.
#[cfg(feature = "rust_inspect")]
pub fn ensure_direct_rust_inspect_workspace(
    project_root: &Path,
    project_name: &str,
    edition: Option<&str>,
    derive_paths: &[String],
    selection_inputs: &std::collections::BTreeMap<String, String>,
) -> CliResult<PathBuf> {
    let declaration = toml::Value::Table(toml::map::Map::from_iter([
        (
            "project".to_string(),
            toml::Value::Table(toml::map::Map::from_iter([
                (
                    "name".to_string(),
                    toml::Value::String(format!("{project_name}_rust_inspect")),
                ),
                ("version".to_string(), toml::Value::String("0.0.0".to_string())),
            ])),
        ),
        (
            "rust".to_string(),
            toml::Value::Table(toml::map::Map::from_iter([
                (
                    "name".to_string(),
                    toml::Value::String(format!("{}_rust_inspect", project_name.replace('-', "_"))),
                ),
                ("type".to_string(), toml::Value::String("lib".to_string())),
                (
                    "edition".to_string(),
                    toml::Value::String(edition.unwrap_or("2024").to_string()),
                ),
            ])),
        ),
    ]));
    let declaration = toml::to_string(&declaration).map_err(|error| CliError::failure(error.to_string()))?;
    let source = direct_rust_inspect_probe_source(derive_paths);
    let selection = serde_json::to_string(selection_inputs).map_err(|error| CliError::failure(error.to_string()))?;
    let fingerprint = oven_store::digest_bytes(format!("{declaration}\0{source}\0{selection}").as_bytes());
    let root = rust_inspect_workspace_dir(project_root, project_name, &fingerprint);
    fs::create_dir_all(root.join("src")).map_err(|error| CliError::failure(error.to_string()))?;
    fs::write(root.join("loaf.toml"), declaration).map_err(|error| CliError::failure(error.to_string()))?;
    fs::write(root.join("src/main.rs"), &source).map_err(|error| CliError::failure(error.to_string()))?;
    fs::write(root.join("src/lib.rs"), source).map_err(|error| CliError::failure(error.to_string()))?;
    fs::write(root.join(::rust_inspect::OVEN_LOAF_ONLY_INSPECTION_MARKER), "1\n")
        .map_err(|error| CliError::failure(error.to_string()))?;
    Ok(root)
}

/// Render the direct inspection root from its requested derive probes, shared by generation and retained validation.
#[cfg(feature = "rust_inspect")]
pub(crate) fn direct_rust_inspect_probe_source(derive_paths: &[String]) -> String {
    let mut source = String::new();
    for (index, path) in derive_paths.iter().enumerate() {
        source.push_str(&format!("#[derive({path})]\nstruct __IncanDeriveProbe{index};\n"));
    }
    source.push_str("fn main() {}\n");
    source
}

/// Generate a rust-inspect workspace whose Cargo package identity matches the canonical lock owner.

/// Collect canonical rust-inspect query paths from parsed `rust::` imports.
pub fn collect_rust_inspect_query_paths(modules: &[ParsedModule]) -> Vec<String> {
    collect_rust_inspect_query_paths_from_programs(modules.iter().map(|module| &module.ast))
}

/// Collect canonical rust-inspect query paths from parsed programs.
///
/// Loaf publication also parses compiler-owned provider source that is metadata-only in the consumer module
/// graph. Keeping the import walk program-based lets that explicit publisher preserve Rust ownership signatures
/// without making normal Oven consumers re-emit provider source.
pub fn collect_rust_inspect_query_paths_from_programs<'a>(
    programs: impl IntoIterator<Item = &'a Program>,
) -> Vec<String> {
    /// Read a presence-and-truthiness flag (`1`, `true`, `on`) from the environment.
    fn env_flag_enabled(name: &str) -> bool {
        std::env::var_os(name).is_some_and(|value| {
            let value = value.to_string_lossy();
            matches!(value.as_ref(), "1" | "true" | "TRUE" | "on" | "ON")
        })
    }

    // Default policy: prewarm explicit non-stdlib `from rust::... import Item` imports. These are the exact paths
    // semantic/codegen hot paths may query later, including Rust types with uppercase names.
    //
    // We still avoid crate/module imports and the standard library facets by default. Full eager prewarm can force
    // broad rust-analyzer walks and persist negative module lookups that are not safe metadata items.
    // Set `INCAN_RUST_INSPECT_PREWARM_ALL=1` to restore full eager prewarm for debugging/regressions.
    let prewarm_all = env_flag_enabled("INCAN_RUST_INSPECT_PREWARM_ALL");
    let mut paths: BTreeSet<String> = BTreeSet::new();
    for program in programs {
        for decl in &program.declarations {
            let incan_frontend::ast::Declaration::Import(import) = &decl.node else {
                continue;
            };
            match &import.kind {
                ImportKind::RustCrate { crate_name, path, .. } if prewarm_all => {
                    let mut segments = Vec::with_capacity(path.len() + 1);
                    segments.push(crate_name.replace('-', "_"));
                    segments.extend(path.iter().cloned());
                    if !segments.is_empty() {
                        paths.insert(segments.join("::"));
                    }
                }
                ImportKind::RustCrate { .. } => {}
                ImportKind::RustFrom {
                    crate_name,
                    path,
                    items,
                    ..
                } => {
                    let mut base = Vec::with_capacity(path.len() + 1);
                    base.push(crate_name.replace('-', "_"));
                    base.extend(path.iter().cloned());
                    let base = base.join("::");
                    if base.is_empty() {
                        continue;
                    }
                    if !prewarm_all && incan_lang::lang::stdlib::facets::path_names_a_facet(&base) {
                        continue;
                    }
                    let primitive_ns = matches!(base.as_str(), "std::primitive" | "core::primitive");
                    for item in items {
                        if !primitive_ns {
                            paths.insert(format!("{base}::{}", item.name));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    paths.into_iter().collect()
}

/// Collect exact Rust derive paths used by concrete Incan type declarations.
///
/// The explicit Cargo-backed inspection bootstrap emits one synthetic Rust type per path. Expanding that type records
/// a candidate trait and associated-type contract for ABI selection; native rustc remains authoritative for the real
/// Incan-authored declaration. This collector admits direct `rust::` item imports used by `@derive(...)` or
/// `@rust.derive(...)` plus syntactically valid explicit Rust macro paths. The preparation boundary later limits those
/// paths to declared dependency namespaces before generating the probe workspace.
pub fn collect_rust_inspect_derive_probe_paths(modules: &[ParsedModule]) -> Vec<String> {
    collect_rust_inspect_derive_probe_uses(modules)
        .into_iter()
        .map(|(path, _)| path)
        .collect()
}

/// Retain canonical derive demand and its original argument span for the shared manifest dependency resolver.
/// This collector identifies authored uses only; declaration and original native ownership remain separate checks.
pub(crate) fn collect_rust_inspect_derive_probe_uses(
    modules: &[ParsedModule],
) -> Vec<(String, incan_frontend::ast::Span)> {
    use incan_frontend::ast::{Declaration, Decorator, DecoratorArg, Expr, Literal};

    /// Return decorators only for declarations that can emit a concrete Rust type.
    fn declaration_decorators(declaration: &Declaration) -> Option<&[incan_frontend::ast::Spanned<Decorator>]> {
        match declaration {
            Declaration::Model(item) => Some(&item.decorators),
            Declaration::Class(item) => Some(&item.decorators),
            Declaration::Newtype(item) => Some(&item.decorators),
            Declaration::Enum(item) => Some(&item.decorators),
            Declaration::Import(_)
            | Declaration::Const(_)
            | Declaration::Static(_)
            | Declaration::Trait(_)
            | Declaration::Alias(_)
            | Declaration::Partial(_)
            | Declaration::TypeAlias(_)
            | Declaration::Function(_)
            | Declaration::TestModule(_)
            | Declaration::VocabBlock(_)
            | Declaration::Capability(_)
            | Declaration::Docstring(_) => None,
        }
    }

    /// Validate a canonical Rust item path, including raw-identifier segments, before generating a derive probe.
    fn is_valid_rust_path(path: &str) -> bool {
        path.split("::").all(|segment| {
            let segment = segment.strip_prefix("r#").unwrap_or(segment);
            let mut chars = segment.chars();
            chars
                .next()
                .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
                && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        })
    }

    let mut probes = std::collections::BTreeMap::new();
    for module in modules {
        let mut imported_paths = HashMap::new();
        for declaration in &module.ast.declarations {
            let Declaration::Import(import) = &declaration.node else {
                continue;
            };
            let ImportKind::RustFrom {
                crate_name,
                path,
                items,
                ..
            } = &import.kind
            else {
                continue;
            };
            let mut base = Vec::with_capacity(path.len() + 1);
            base.push(crate_name.replace('-', "_"));
            base.extend(path.iter().cloned());
            for item in items {
                let binding = item.alias.as_ref().unwrap_or(&item.name);
                let mut segments = base.clone();
                segments.push(item.name.clone());
                imported_paths.insert(binding.clone(), segments.join("::"));
            }
        }
        for declaration in &module.ast.declarations {
            let Some(decorators) = declaration_decorators(&declaration.node) else {
                continue;
            };
            for decorator in decorators {
                let Some(decorator_id) = incan_lang::lang::decorators::from_segments(&decorator.node.path.segments)
                else {
                    continue;
                };
                for argument in &decorator.node.args {
                    let DecoratorArg::Positional(argument) = argument else {
                        continue;
                    };
                    match (decorator_id, &argument.node) {
                        (incan_lang::lang::decorators::DecoratorId::Derive, Expr::Ident(name))
                        | (incan_lang::lang::decorators::DecoratorId::RustDerive, Expr::Ident(name)) => {
                            if let Some(path) = imported_paths.get(name) {
                                probes.entry(path.clone()).or_insert(argument.span);
                            }
                        }
                        (
                            incan_lang::lang::decorators::DecoratorId::RustDerive,
                            Expr::Literal(Literal::String(path)),
                        ) if path.contains("::") && is_valid_rust_path(path) => {
                            probes.entry(path.clone()).or_insert(argument.span);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    probes.into_iter().collect()
}

/// Return whether rust-inspect prewarm should run for the supplied environment value.
fn parse_rust_inspect_prewarm_env(raw: Option<&str>) -> bool {
    let Some(raw) = raw else {
        return false;
    };
    matches!(raw.trim(), "1" | "true" | "TRUE" | "on" | "ON" | "yes" | "YES")
}

/// Return whether Rust inspection prewarming is enabled.
fn rust_inspect_prewarm_enabled() -> bool {
    parse_rust_inspect_prewarm_env(std::env::var("INCAN_RUST_INSPECT_PREWARM").ok().as_deref())
}


/// Surface rust-inspect preparation progress from explicit CLI prewarm phases.
fn print_rust_inspect_prewarm_progress(message: String) {
    if message.starts_with("rust-inspect prewarm") {
        eprintln!("{message}");
    }
}


/// Mark one compiler-authored Rust inspection projection for receipt-bound direct-Rustc loading.
pub fn mark_oven_direct_rust_inspection(manifest_dir: &Path) -> CliResult<()> {
    let marker = manifest_dir.join(::rust_inspect::OVEN_DIRECT_INSPECTION_MARKER);
    fs::write(&marker, b"receipt-bound direct-rustc inspection\n").map_err(|error| {
        CliError::failure(format!(
            "failed to mark Oven Rust inspection projection {}: {error}",
            marker.display()
        ))
    })
}

/// Prepare rust-inspect metadata access before typechecking hot paths.
///
/// Metadata extraction defaults to lazy lookup, because eager extraction across every imported Rust path can
/// dominate a cold build. Set `INCAN_RUST_INSPECT_PREWARM=1` to opt into eager prewarm; `force_direct_prewarm`
/// requests it for one explicit command. The root must be a marked direct inspection projection.
pub fn prewarm_rust_inspect_workspace(
    manifest_dir: &Path,
    query_paths: &[String],
    force_direct_prewarm: bool,
) -> CliResult<()> {
    if !manifest_dir
        .join(::rust_inspect::OVEN_DIRECT_INSPECTION_MARKER)
        .is_file()
    {
        return Err(CliError::failure(format!(
            "{} is not a direct Oven Rust inspection projection",
            manifest_dir.display()
        )));
    }
    if query_paths.is_empty() || !(force_direct_prewarm || rust_inspect_prewarm_enabled()) {
        return Ok(());
    }
    let inspector = Inspector::new(InspectorConfig::new(manifest_dir.to_path_buf()));
    inspector
        .prewarm(query_paths.iter().cloned(), &print_rust_inspect_prewarm_progress)
        .map_err(|err| {
            CliError::failure(format!(
                "failed to prewarm direct Oven rust-inspect cache from {}: {err}",
                manifest_dir.display()
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use incan_provider::test_support::parsed_module_for_test;

    #[test]
    fn rust_inspect_prewarm_env_defaults_to_disabled() {
        assert!(!parse_rust_inspect_prewarm_env(None));
        assert!(!parse_rust_inspect_prewarm_env(Some("")));
        assert!(parse_rust_inspect_prewarm_env(Some("1")));
        assert!(parse_rust_inspect_prewarm_env(Some("true")));
        assert!(parse_rust_inspect_prewarm_env(Some("on")));
        assert!(parse_rust_inspect_prewarm_env(Some("YES")));
        assert!(!parse_rust_inspect_prewarm_env(Some("0")));
        assert!(!parse_rust_inspect_prewarm_env(Some("false")));
        assert!(!parse_rust_inspect_prewarm_env(Some(" OFF ")));
        assert!(!parse_rust_inspect_prewarm_env(Some("no")));
        assert!(!parse_rust_inspect_prewarm_env(Some("unexpected")));
    }


    #[test]
    fn rust_inspect_query_paths_include_explicit_rust_item_imports() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
from rust::datafusion::execution::context import SessionContext
from rust::datafusion::prelude import CsvReadOptions, read_csv
from rust::incan_std_async::runtime import block_on
from rust::std::fs import metadata
from rust::std::primitive import i64 as RustI64
"#,
        )?;

        let paths = collect_rust_inspect_query_paths(&[module]);

        assert_eq!(
            paths,
            vec![
                "datafusion::execution::context::SessionContext".to_string(),
                "datafusion::prelude::CsvReadOptions".to_string(),
                "datafusion::prelude::read_csv".to_string(),
                "std::fs::metadata".to_string(),
            ]
        );

        let program_paths = collect_rust_inspect_query_paths_from_programs([&parsed_module_for_test(
            r#"
from rust::rustix::fs import flock
"#,
        )?
        .ast]);
        assert_eq!(program_paths, vec!["rustix::fs::flock".to_string()]);
        Ok(())
    }

    #[test]
    fn rust_inspect_derive_probes_include_only_used_direct_rust_imports() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
from rust::provider::prelude import Component as ForeignComponent, UnusedDerive

@derive(ForeignComponent, Clone)
model Velocity:
  x: f32

@rust.derive(ForeignComponent, "provider::derive::ExplicitComponent", "provider::r#async::Component", "provider::derive::Bad;Drop", Clone)
model ExplicitVelocity:
  x: f32
"#,
        )?;

        assert_eq!(
            collect_rust_inspect_derive_probe_paths(&[module]),
            vec![
                "provider::derive::ExplicitComponent".to_string(),
                "provider::prelude::Component".to_string(),
                "provider::r#async::Component".to_string(),
            ],
            "directly imported and explicit-path Rust derives that are actually invoked must become semantic probes"
        );
        Ok(())
    }

    #[test]
    fn rust_inspect_derive_probe_aliases_remain_module_scoped() -> Result<(), Box<dyn std::error::Error>> {
        let left = parsed_module_for_test(
            r#"
from rust::left_provider import Component as SharedDerive

@derive(SharedDerive)
model LeftValue:
  value: int
"#,
        )?;
        let right = parsed_module_for_test(
            r#"
from rust::right_provider import Component as SharedDerive

@rust.derive(SharedDerive)
model RightValue:
  value: int
"#,
        )?;

        assert_eq!(
            collect_rust_inspect_derive_probe_paths(&[left, right]),
            vec![
                "left_provider::Component".to_string(),
                "right_provider::Component".to_string(),
            ],
            "the same visible alias in separate modules must retain both module-local Rust identities"
        );
        Ok(())
    }


    #[test]
    fn rust_inspect_workspace_dir_is_namespaced_by_input_fingerprint() {
        let root = Path::new("/workspace");
        let first = super::rust_inspect_workspace_dir(root, "demo", "v1:aaaaaaaaaaaaaaaaaaaaaaaa");
        let second = super::rust_inspect_workspace_dir(root, "demo", "v1:bbbbbbbbbbbbbbbbbbbbbbbb");

        assert_ne!(first, second);
        assert!(first.ends_with(Path::new("target/incan_lock/rust_inspect/demo-aaaaaaaaaaaaaaaa")));
        assert!(second.ends_with(Path::new("target/incan_lock/rust_inspect/demo-bbbbbbbbbbbbbbbb")));
    }

}
