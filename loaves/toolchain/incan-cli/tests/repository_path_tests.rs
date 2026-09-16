//! Checkout and subprocess anchors remain explicit when the integration-test runner has another working directory.

use incan_test_support as support;

/// Source discovery always names the checkout that built this test root.
#[test]
fn checkout_sources_are_absolute_and_present() {
    let root = support::repo_root();
    assert!(root.is_absolute());
    assert!(root.join("examples/intermediate/collections.incn").is_file());
    assert!(root.join("loaves/compiler/incan_emit/tests/codegen_snapshots").is_dir());
}

/// A default CLI subprocess uses the checkout, independently of the parent process directory.
#[test]
fn compiler_command_defaults_to_checkout() {
    let command = support::repo_command();
    assert_eq!(command.get_current_dir(), Some(support::repo_root().as_path()));
    let configured = support::incan_command();
    assert_eq!(configured.get_current_dir(), Some(support::repo_root().as_path()));
}

/// Temporary projects retain control of their command's working directory.
#[test]
fn temporary_project_overrides_checkout() -> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let mut command = support::repo_command();
    command.current_dir(project.path());
    assert_eq!(command.get_current_dir(), Some(project.path()));
    Ok(())
}

/// Binary overrides are resolved before the command changes its working directory.
#[test]
fn compiler_binary_override_stays_absolute() {
    assert!(std::path::Path::new(support::repo_command().get_program()).is_absolute());
}

/// Relative harness overrides retain their caller-relative meaning when the CLI uses the checkout directory.
#[test]
fn relative_binary_overrides_survive_foreign_cwd() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = tempfile::tempdir()?;
    std::fs::create_dir(fixture.path().join("debug"))?;
    std::fs::write(fixture.path().join("debug/incan"), "binary location probe")?;
    for direct_binary in [true, false] {
        let mut child = std::process::Command::new(std::env::current_exe()?);
        child
            .current_dir(fixture.path())
            .args(["--exact", "compiler_binary_override_stays_absolute"])
            .env("CARGO_TARGET_DIR", ".");
        if direct_binary {
            child.env("CARGO_BIN_EXE_incan", "debug/incan");
        } else {
            child.env_remove("CARGO_BIN_EXE_incan");
        }
        let output = child.output()?;
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    }
    Ok(())
}
