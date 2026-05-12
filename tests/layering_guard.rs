//! Layering guardrails to prevent the compiler crate from depending on the runtime stdlib.
//!
//! The compiler (`incan` crate) may only use `incan_stdlib` as a **dev-dependency** (for parity tests).
//! This test scans the root `Cargo.toml` and fails if `incan_stdlib` appears in `[dependencies]`.

use incan_core::lang::stdlib;

#[test]
fn compiler_does_not_depend_on_stdlib_in_main_dependencies() {
    let manifest = include_str!("../Cargo.toml");
    let mut in_dependencies = false;

    for raw_line in manifest.lines() {
        let line = raw_line.trim();
        // Track when we enter/exit the `[dependencies]` table.
        if line.starts_with('[') {
            if line == "[dependencies]" {
                in_dependencies = true;
                continue;
            }
            // Any new section after `[dependencies]` ends the scan window.
            if in_dependencies {
                break;
            }
        }

        if !in_dependencies || line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Strip inline comments for robustness.
        let line_no_comment = line.split('#').next().unwrap_or("").trim();
        if line_no_comment.starts_with("incan_stdlib") {
            panic!("`incan_stdlib` must not appear in [dependencies]; use [dev-dependencies] instead");
        }
    }
}

#[test]
fn std_collections_namespace_stays_source_stdlib_only() {
    let ns = stdlib::find_namespace("collections").expect("std.collections should be registered");

    assert_eq!(ns.feature, None, "std.collections must not activate a Cargo feature");
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
}

#[test]
fn std_collections_source_has_no_rust_backed_dispatch_markers_when_present() {
    let source_path = std::path::Path::new("crates/incan_stdlib/stdlib/collections.incn");
    let Ok(source) = std::fs::read_to_string(source_path) else {
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
fn std_encoding_source_stays_incan_authored_without_rust_externs() {
    let source_root = std::path::Path::new("crates/incan_stdlib/stdlib/encoding");
    let Ok(entries) = std::fs::read_dir(source_root) else {
        return;
    };

    for entry in entries {
        let entry = entry.expect("encoding stdlib directory entries should be readable");
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("incn") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("encoding stdlib source should be readable");
        for forbidden in ["rust.module", "@rust.extern", "from rust::"] {
            assert!(
                !source.contains(forbidden),
                "`{forbidden}` is not allowed in Incan-authored std.encoding source: {}",
                path.display()
            );
        }
    }
}
