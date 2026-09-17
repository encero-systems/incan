//! The generated project's Cargo.lock: the lock-payload test that needs the project generator.

use std::fs;

use crate::backend::ProjectGenerator;

#[test]
fn cargo_lock_payload_materializes_in_project() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let project_dir = temp_dir.path().join("test_lock_project");

    let mut generator = ProjectGenerator::new(&project_dir, "test_lock", true);
    generator.set_cargo_lock_payload(Some("[[package]]\nname = \"hello\"\nversion = \"0.1.0\"\n".to_string()));

    generator.generate("fn main() {}")?;

    let cargo_lock_path = project_dir.join("Cargo.lock");
    assert!(cargo_lock_path.exists(), "Cargo.lock should be written to project dir");
    let content = fs::read_to_string(&cargo_lock_path)?;
    assert!(
        content.contains("hello"),
        "Cargo.lock should contain the payload, got:\n{content}"
    );
    Ok(())
}
