//! Fixtures shared by the driver and provider test modules that the split of `commands/common.rs` distributed.

use std::path::{Path, PathBuf};

use crate::frontend::parsed_module::ParsedModule;
use crate::frontend::{lexer, parser};
use crate::library_manifest::LibraryManifest;
pub(crate) fn parsed_module_for_test(source: &str) -> Result<ParsedModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;
    Ok(ParsedModule {
        name: "main".to_string(),
        path_segments: vec!["main".to_string()],
        file_path: PathBuf::from("main.incn"),
        source: source.to_string(),
        ast,
    })
}

pub(crate) fn write_minimal_library_artifact(
    root: &Path,
    dependency_key: &str,
    manifest_name: &str,
    manifest: &LibraryManifest,
) -> Result<(), Box<dyn std::error::Error>> {
    let artifact_root = root.join("deps").join(dependency_key).join("target").join("lib");
    std::fs::create_dir_all(artifact_root.join("src"))?;
    std::fs::write(
        artifact_root.join("Cargo.toml"),
        format!("[package]\nname = \"{manifest_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
    )?;
    std::fs::write(artifact_root.join("src/lib.rs"), "")?;
    manifest.write_to_path(&artifact_root.join(format!("{manifest_name}.incnlib")))?;
    Ok(())
}
