//! Manifest target collisions fail before any compiler or provider work begins.

use std::error::Error;
use std::fs;
use std::process::Command;

mod support;

/// Every command that consumes script targets reports the same authored manifest error.
#[test]
fn script_library_collision_fails_before_building() -> Result<(), Box<dyn Error>> {
    let project = tempfile::tempdir()?;
    fs::create_dir(project.path().join("src"))?;
    fs::write(
        project.path().join("src/lib.incn"),
        "pub def hello() -> str:\n    return \"hi\"\n",
    )?;
    fs::write(
        project.path().join("loaf.toml"),
        "[project]\nname = \"lib_only_repro\"\nversion = \"0.1.0\"\n\n[project.scripts]\nmain = \"src/lib.incn\"\n",
    )?;
    for args in [
        vec!["oven", "bake", "--project", "."],
        vec!["lock"],
        vec!["run"],
        vec!["build", "--lib"],
    ] {
        let output = Command::new(support::incan_binary())
            .current_dir(project.path())
            .args(&args)
            .env("INCAN_HOME", project.path().join("home"))
            .env_remove("INCAN_INTERNAL_PROJECT_ROOT")
            .env_remove("INCAN_INTERNAL_MANIFEST_OVERRIDE")
            .output()?;
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{args:?} unexpectedly succeeded");
        assert!(diagnostic.contains("loaf.toml:6:"), "{args:?}: {diagnostic}");
        assert!(diagnostic.contains("main"), "{args:?}: {diagnostic}");
        assert!(diagnostic.contains("src/lib.incn"), "{args:?}: {diagnostic}");
        assert!(!diagnostic.contains("E0601"), "{args:?}: {diagnostic}");
        assert!(!project.path().join("target").exists());
    }
    Ok(())
}
