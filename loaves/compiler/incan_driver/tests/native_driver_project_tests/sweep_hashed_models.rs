//! Hashed nominal storage does not fabricate native source-comparison implementations.

use super::{census, corpus, driver_fixture, success, support};
use std::{fs, process::Command};

/// The complete original advances from missing storage to its separately owned source Eq implementation.
#[test]
fn comparison_original_reaches_source_equality() -> Result<(), Box<dyn std::error::Error>> {
    census::require_named_refusal(
        "lowering_declarations",
        "comparison_trait_adopters_compare_and_sort",
        "unsupported Body IR model comparison without derived PartialEq",
    )
}

/// Real native bounds reject a missing Eq implementation before MIR; the same empty set executes in legacy.
#[test]
fn source_hashed_model_reaches_native_bounds() -> Result<(), Box<dyn std::error::Error>> {
    let graph = driver_fixture()?;
    let root = graph.scratch("sweep-source-hash-bounds")?;
    let source = root.join("main.incn");
    fs::write(
        &source,
        "from std.derives.comparison import Eq\n\n@derive(Hash)\nmodel UserKey with Eq:\n    id: int\n\n    def __eq__(self, other: UserKey) -> bool:\n        return self.id == other.id\n\ndef main() -> None:\n    keys: Set[UserKey] = Set()\n    println(len(keys))\n",
    )?;
    let closure = corpus::runtime_closure(&graph.formatting)?;
    let result = corpus::source_command(
        &graph.driver_binary("debug"),
        &source,
        &root.join("native"),
        &graph.sysroot,
        &closure,
    )
    .output()?;
    assert!(!result.status.success());
    let diagnostic = String::from_utf8_lossy(&result.stderr);
    assert!(diagnostic.contains("unsupported"), "{diagnostic}");
    assert!(
        diagnostic.contains("no native impl for the callee's bounds"),
        "{diagnostic}"
    );
    assert!(!diagnostic.contains("collection leaf"), "{diagnostic}");
    let legacy = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .env("INCAN_HOME", support::oven_fixture_home()?)
            .arg("build")
            .arg(&source)
            .arg(&legacy)
            .output()?,
        "source Eq set legacy compilation",
    );
    let output = Command::new(legacy.join("oven/release/main")).output()?;
    success(&output, "source Eq empty set legacy execution");
    assert_eq!(output.stdout, b"0\n");
    assert!(output.stderr.is_empty());
    Ok(())
}

/// Existing derived Eq and Hash retain exact insertion, membership and deduplication behavior.
#[test]
fn derived_hashed_models_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let graph = driver_fixture()?;
    corpus::check_builtin_source(
        &graph.driver_binary("debug"),
        &graph.scratch("sweep-derived-hash")?,
        &graph.sysroot,
        &graph.formatting,
        r#"@derive(Eq, Hash)
model Key:
    id: int

def main() -> None:
    values = {Key(id=1), Key(id=1), Key(id=2)}
    println(len(values))
    println(Key(id=2) in values)
"#,
    )
}
