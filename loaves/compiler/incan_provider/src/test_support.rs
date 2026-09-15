//! Fixtures shared by the provider's and the driver's test modules: on under `cfg(test)` and the `test_support`
//! feature, which the crates above turn on in their dev-dependencies.

use std::path::{Path, PathBuf};

use incan_frontend::library_manifest::LibraryManifest;
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::{lexer, parser};
/// Lex and parse one source string as a `main` module for tests that need a checked module without a file.
pub fn parsed_module_for_test(source: &str) -> Result<ParsedModule, Box<dyn std::error::Error>> {
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

/// Write the smallest published library artifact a dependency index accepts: a Cargo package, an empty `lib.rs`,
/// and the given manifest under `deps/<key>/target/lib`.
pub fn write_minimal_library_artifact(
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
