//! Layering guardrails to prevent the compiler from depending on the runtime stdlib.
//!
//! The compiler — the root `incan` crate and every crate under `loaves/compiler` and `loaves/kernel` — may only use
//! a standard library facet (`incan_std_core` and the others) as a **dev-dependency** (for parity tests). This test
//! scans those manifests and fails if a facet appears in `[dependencies]`. The facets themselves are pinned two ways:
//! the registry's facet facts must agree with `sdk-components.toml` and the crates on disk, and the compiler ring must
//! spell no runtime crate the catalog does not know — the retired `incan_stdlib` included.

use std::path::{Path, PathBuf};

use incan_test_support as support;
use support::repo_root;

use incan_core::lang::stdlib::{self, facets};
use incan_frontend::provider::{SDK_SOURCE_CATALOG_FILE, SdkSourceCatalog};

/// The root manifest plus every crate manifest in the compiler, kernel and toolchain rings.
fn compiler_manifests() -> Vec<PathBuf> {
    let root = repo_root();
    let mut manifests = vec![root.join("Cargo.toml")];
    for ring in ["loaves/compiler", "loaves/kernel", "loaves/toolchain"] {
        let Ok(entries) = std::fs::read_dir(root.join(ring)) else {
            continue;
        };
        for entry in entries.flatten() {
            let manifest = entry.path().join("Cargo.toml");
            if manifest.is_file() {
                manifests.push(manifest);
            }
        }
    }
    manifests.sort();
    manifests
}

/// Return the `[dependencies]` lines of a manifest that name a standard library facet, comments stripped.
fn stdlib_runtime_dependencies(manifest: &str) -> Vec<String> {
    let mut in_dependencies = false;
    let mut offenders = Vec::new();
    for raw_line in manifest.lines() {
        let line = raw_line.trim();
        // Track when we enter/exit the `[dependencies]` table.
        if line.starts_with('[') {
            in_dependencies = line == "[dependencies]";
            continue;
        }
        if !in_dependencies || line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Strip inline comments for robustness.
        let line_no_comment = line.split('#').next().unwrap_or("").trim();
        if line_no_comment.starts_with("incan_std_") {
            offenders.push(line_no_comment.to_string());
        }
    }
    offenders
}

#[test]
fn compiler_does_not_depend_on_stdlib_in_main_dependencies() -> Result<(), Box<dyn std::error::Error>> {
    let manifests = compiler_manifests();
    assert!(
        manifests.len() > 1,
        "the compiler ring manifests were not found beside the root manifest"
    );
    for path in manifests {
        let manifest = std::fs::read_to_string(&path)?;
        let offenders = stdlib_runtime_dependencies(&manifest);
        assert!(
            offenders.is_empty(),
            "{}: a standard library facet must not appear in [dependencies]; use [dev-dependencies] instead: {offenders:?}",
            path.display()
        );
    }
    Ok(())
}

/// Every source file of the compiler, kernel and toolchain rings plus the root crate, for spelling scans.
///
/// A package's `tests/` directory is skipped: the roots there spell whatever they assert about, this guard included.
fn compiler_ring_sources() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name == "tests") {
                    continue;
                }
                walk(&path, out);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                out.push(path);
            }
        }
    }
    let root = repo_root();
    let mut sources = Vec::new();
    for ring in ["src", "loaves/compiler", "loaves/kernel", "loaves/toolchain"] {
        walk(&root.join(ring), &mut sources);
    }
    sources.sort();
    sources
}

#[test]
fn the_registry_facets_are_the_catalog_components_with_a_rust_crate() -> Result<(), Box<dyn std::error::Error>> {
    let stdlib_root = repo_root().join("loaves/stdlib");
    let catalog = SdkSourceCatalog::read_from_path(&stdlib_root.join(SDK_SOURCE_CATALOG_FILE))?;

    // ---- A component has a facet exactly when a `rust/` crate sits in its directory, named by LAYOUT's rule ----
    for component in catalog.components.values() {
        let manifest = component.project_root.join("rust/Cargo.toml");
        match facets::for_component(&component.id) {
            Some(facet) => {
                let manifest_text = std::fs::read_to_string(&manifest)
                    .map_err(|error| format!("{}: facet `{facet}` has no crate: {error}", component.id))?;
                assert!(
                    manifest_text.contains(&format!("name = \"{facet}\"")),
                    "{}: {} must be the crate `{facet}`",
                    component.id,
                    manifest.display()
                );
            }
            None => assert!(
                !manifest.exists(),
                "{}: {} exists but the registry names no facet for the component",
                component.id,
                manifest.display()
            ),
        }
    }

    // ---- A namespace's facet is the facet of the component the catalog says owns it ----
    for namespace in stdlib::STDLIB_NAMESPACES {
        let owner = catalog
            .components
            .values()
            .find(|component| component.namespace_roots.contains(namespace.name));
        let Some(owner) = owner else {
            assert!(
                namespace.facet.is_none() || namespace.typechecker_only,
                "std.{} names facet {:?} but no catalog component owns it",
                namespace.name,
                namespace.facet
            );
            continue;
        };
        if let Some(facet) = namespace.facet {
            assert_eq!(
                Some(facet),
                facets::for_component(&owner.id),
                "std.{} links `{facet}` but its owner `{}` has a different facet",
                namespace.name,
                owner.id
            );
        }
    }
    Ok(())
}

#[test]
fn the_compiler_ring_spells_no_runtime_crate_the_catalog_does_not_know() -> Result<(), Box<dyn std::error::Error>> {
    let mut offenders = Vec::new();
    for path in compiler_ring_sources() {
        let source = std::fs::read_to_string(&path)?;
        for (line_no, line) in source.lines().enumerate() {
            let mut rest = line;
            while let Some(start) = rest.find("incan_std") {
                let candidate = &rest[start..];
                let end = candidate
                    .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
                    .unwrap_or(candidate.len());
                let name = &candidate[..end];
                let names_a_crate = candidate[end..].starts_with("::");
                // `crates/incan_stdlib/stdlib/…` is the v0.5 source layout a compatibility constant records, not a
                // crate.
                let historical_source_path = candidate[end..].starts_with("/stdlib");
                if (name == "incan_stdlib" && !historical_source_path)
                    || (names_a_crate && name.starts_with("incan_std_") && !facets::is_facet(name))
                {
                    offenders.push(format!("{}:{}: {name}", path.display(), line_no + 1));
                }
                rest = &candidate[end..];
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "the compiler ring must spell runtime crates as the catalog's facets:\n{}",
        offenders.join("\n")
    );
    Ok(())
}

#[test]
fn std_collections_namespace_links_the_data_facet_without_extra_crates() -> Result<(), Box<dyn std::error::Error>> {
    let ns = stdlib::find_namespace("collections").ok_or("std.collections should be registered")?;

    assert_eq!(
        ns.facet,
        Some(facets::DATA),
        "std.collections needs the ordinal-key helpers in the data facet used by its Incan source"
    );
    assert!(
        ns.extra_crate_deps.is_empty(),
        "std.collections must not add Rust crate dependencies"
    );
    assert!(
        ns.submodules.is_empty(),
        "std.collections should resolve as a leaf stdlib source module"
    );
    assert!(
        !ns.typechecker_only,
        "std.collections must load through the ordinary stdlib source path"
    );
    Ok(())
}

#[test]
fn std_collections_source_has_no_rust_backed_dispatch_markers_when_present() {
    let source_path = repo_root().join("loaves/stdlib/data/src/collections.incn");
    let Ok(source) = std::fs::read_to_string(&source_path) else {
        // The stdlib-source worker owns this file. This guard starts checking it once their slice is integrated.
        return;
    };

    for forbidden in ["rust.module", "@rust.extern"] {
        assert!(
            !source.contains(forbidden),
            "`{forbidden}` is not allowed in pure-Incan std.collections"
        );
    }
}

#[test]
fn std_encoding_source_stays_incan_authored_without_rust_externs() -> Result<(), Box<dyn std::error::Error>> {
    let source_root = repo_root().join("loaves/stdlib/codecs/src/encoding");
    let Ok(entries) = std::fs::read_dir(source_root) else {
        return Ok(());
    };

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("incn") {
            continue;
        }
        let source = std::fs::read_to_string(&path)?;
        for forbidden in ["rust.module", "@rust.extern", "from rust::"] {
            assert!(
                !source.contains(forbidden),
                "`{forbidden}` is not allowed in Incan-authored std.encoding source: {}",
                path.display()
            );
        }
    }
    Ok(())
}

#[test]
fn std_uuid_namespace_stays_source_stdlib_only() -> Result<(), Box<dyn std::error::Error>> {
    let Some(ns) = stdlib::find_namespace("uuid") else {
        panic!("std.uuid should be registered");
    };

    assert_eq!(ns.facet, None, "std.uuid must not link a facet beyond the core runtime");
    assert_eq!(
        ns.extra_crate_deps.iter().map(|dep| dep.crate_name).collect::<Vec<_>>(),
        vec!["rand"],
        "std.uuid crate dependencies should stay limited to source-visible Rust imports"
    );

    let source_path = repo_root().join("loaves/stdlib/data/src/uuid.incn");
    let source = std::fs::read_to_string(&source_path)?;
    for dep in ns.extra_crate_deps {
        let import_prefix = format!("from rust::{}", dep.crate_name);
        assert!(
            source.contains(&import_prefix),
            "`{}` must be visible as an inline std.uuid source import",
            dep.crate_name
        );
    }
    assert!(
        source.contains("from std.hash import md5 as hash_md5, sha1 as hash_sha1"),
        "std.uuid v3/v5 hashing should dogfood std.hash instead of direct digest crates"
    );

    assert!(
        ns.submodules.is_empty(),
        "std.uuid should resolve as a leaf stdlib source module"
    );
    assert!(
        !ns.typechecker_only,
        "std.uuid must load through the ordinary stdlib source path"
    );
    Ok(())
}

#[test]
fn std_uuid_source_has_no_rust_backed_type_markers() {
    let source_path = repo_root().join("loaves/stdlib/data/src/uuid.incn");
    let Ok(source) = std::fs::read_to_string(&source_path) else {
        panic!("std.uuid source should exist");
    };

    for forbidden in ["rust.module", "@rust.extern", "rusttype"] {
        assert!(
            !source.contains(forbidden),
            "`{forbidden}` is not allowed in source-defined std.uuid"
        );
    }
}

#[test]
fn std_regex_keeps_behavior_in_incan_source() -> Result<(), Box<dyn std::error::Error>> {
    let source_paths = [
        "loaves/stdlib/data/src/regex/prelude.incn",
        "loaves/stdlib/data/src/regex/_core.incn",
        "loaves/stdlib/data/src/regex/types.incn",
        "loaves/stdlib/data/src/regex/_replacement.incn",
    ];
    let mut source = String::new();
    for source_path in source_paths {
        source.push_str(&std::fs::read_to_string(repo_root().join(source_path))?);
        source.push('\n');
    }
    assert!(
        source.contains("from rust::regex import Regex as RustRegex, RegexBuilder"),
        "std.regex should dogfood direct regex crate interop for engine construction"
    );
    assert!(
        !source.contains("from rust::incan_std_core::regex"),
        "std.regex should not call a Rust snapshot-helper module"
    );
    assert!(
        source.contains("def _replacen_string") && source.contains("def _expand_replacement"),
        "std.regex replacement behavior should stay in Incan source"
    );

    let rust_path = repo_root().join("loaves/stdlib/data/rust/src/regex.rs");
    assert!(
        !rust_path.exists(),
        "std.regex should not keep a Rust runtime-helper module for source-level behavior"
    );
    Ok(())
}
