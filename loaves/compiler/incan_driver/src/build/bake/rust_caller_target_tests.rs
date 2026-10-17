//! Rust caller target discovery shares manifest authority with ordinary native projects.

use super::{ProjectRustUnit, discover_project_rust_units};
use oven_model::manifest::ProjectManifest;
use std::fs;

/// Explicit source roots and Rust editions reach caller library discovery unchanged.
#[test]
fn library_caller_uses_declared_root_and_edition() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    fs::create_dir_all(root.join("native/src"))?;
    fs::write(root.join("native/src/entry.rs"), "pub fn value() {}")?;
    fs::write(
        root.join("loaf.toml"),
        "[project]\nname = \"caller\"\n[rust.source]\nroot = \"native\"\n[rust]\nname = \"library_caller\"\ntype = \"lib\"\npath = \"native/src/entry.rs\"\nedition = \"2021\"\n",
    )?;
    let manifest = ProjectManifest::discover(root)?.ok_or("missing fixture manifest")?;
    let units = discover_project_rust_units(&manifest)?;
    assert_eq!(units.len(), 1);
    assert!(units[0].library);
    assert_eq!(units[0].name, "library_caller");
    assert_eq!(units[0].source, root.join("native/src/entry.rs"));
    assert_eq!(units[0].edition, "2021");
    Ok(())
}

/// A same-name library and binary cannot overwrite each other's outputs, receipts or caller projections.
#[test]
fn same_name_library_and_binary_keep_distinct_coordinates() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    fs::create_dir_all(root.join("src"))?;
    fs::write(root.join("src/lib.rs"), "pub fn value() {}")?;
    fs::write(root.join("src/main.rs"), "fn main() {}")?;
    fs::write(
        root.join("loaf.toml"),
        "[project]\nname = \"caller\"\n[rust]\nname = \"caller\"\ntype = \"lib\"\n",
    )?;
    let manifest = ProjectManifest::discover(root)?.ok_or("missing fixture manifest")?;
    let units = discover_project_rust_units(&manifest)?;
    assert_eq!(units.len(), 2);
    assert_ne!(units[0].report_target(), units[1].report_target());
    assert_ne!(units[0].caller_key(), units[1].caller_key());
    let binary = units.iter().find(|unit| !unit.library).ok_or("missing binary")?;
    let library = units.iter().find(|unit| unit.library).ok_or("missing library")?;
    assert_eq!(
        binary.output_paths(root, "debug")?.0,
        root.join("target/rust/debug/caller")
    );
    assert_eq!(
        library.output_paths(root, "debug")?.0,
        root.join("target/rust/library/debug/libcaller.rlib")
    );
    assert_ne!(
        binary.output_paths(root, "debug")?.1,
        library.output_paths(root, "debug")?.1
    );
    Ok(())
}

/// Output path validation cannot be bypassed by declaring a caller name with directory traversal.
#[test]
fn caller_output_coordinates_refuse_traversal() {
    let unit = ProjectRustUnit::binary("../escape".into(), "src/main.rs".into());
    assert!(unit.output_paths(std::path::Path::new("project"), "debug").is_err());
}
