//! Actual-executable controls for mandatory ordinary native source selection.

use std::fs;
use std::path::Path;

use incan_lang::lang::generated_support::SUPPORT_CRATES_EVERY_PROGRAM_LINKS;
use incan_lang::lang::standard_packages::standard_package_namespace_policy;
use oven_model::manifest::DependencySource;

use super::CompilerSupportSources;

type TestResult = Result<(), Box<dyn std::error::Error>>;
const CHILD: &str = "INCAN_DEV7_ORDINARY_SUPPORT_CHILD";
// Real authored identity and proc-macro declaration used by the pinned core dependency.
const DERIVE_DECLARATION: &str = "[project]\nname='incan_derive'\nversion='0.6.0-dev.6'\n[rust]\nname='incan_derive'\ntype='proc-macro'\nedition='2024'\n";

/// Exercise real executable geometry while all ambient source/catalog selectors point at a productive decoy.
fn child(mode: &str) -> TestResult {
    let root = tempfile::tempdir()?;
    let core = standard_package_namespace_policy("incan_stdlib_core").ok_or("core policy missing")?;
    write(&root.path().join("stdlib/core/loaf.toml"), core.declaration)?;
    write(
        &root.path().join("stdlib/derive/incan_derive/loaf.toml"),
        DERIVE_DECLARATION,
    )?;
    // Normal manifest admission classifies an authored local native edge using its actual Rust source entry.
    write(
        &root.path().join("stdlib/derive/incan_derive/src/lib.rs"),
        "extern crate proc_macro;",
    )?;
    if mode == "unpinned" {
        fs::write(
            root.path().join("stdlib/core/loaf.toml"),
            format!("{}\n# changed\n", core.declaration),
        )?;
    }
    if matches!(mode, "wrong-version" | "wrong-role") {
        fs::write(
            root.path().join("stdlib/derive/incan_derive/loaf.toml"),
            if mode == "wrong-version" {
                DERIVE_DECLARATION.replace("0.6.0-dev.6", "2.0.0")
            } else {
                DERIVE_DECLARATION.replace("proc-macro", "lib")
            },
        )?;
    }
    let executable = root.path().join("bin/incan");
    fs::create_dir_all(executable.parent().ok_or("executable parent missing")?)?;
    fs::copy(std::env::current_exe()?, &executable)?;
    let decoy = tempfile::tempdir()?;
    write(&decoy.path().join("core/loaf.toml"), core.declaration)?;
    write(&decoy.path().join("derive/incan_derive/loaf.toml"), DERIVE_DECLARATION)?;
    let output = std::process::Command::new(executable)
        .args([
            "--exact",
            "build::ordinary_support::tests::ordinary_support_actual_executable_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, mode)
        .env("INCAN_SOURCE_ROOT", decoy.path())
        .env("INCAN_STDLIB", decoy.path())
        .env("INCAN_SDK_INVENTORY", decoy.path().join("forged-sdk.json"))
        .current_dir(decoy.path())
        .output()?;
    assert!(
        output.status.success(),
        "{mode}: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    Ok(())
}

/// Actual pinned declarations supply all mandatory aliases despite hostile ambient selectors.
#[test]
fn ordinary_support_selects_actual_executable_declarations() -> TestResult {
    child("valid")
}

/// An authored byte change refuses even when the original modification time is restored.
#[test]
fn ordinary_support_refuses_preserved_time_change() -> TestResult {
    child("changed")
}

/// An equal-byte declaration replacement cannot substitute for the originally retained member.
#[test]
fn ordinary_support_refuses_equal_byte_replacement() -> TestResult {
    child("replaced")
}

/// A present source declaration differing from compiler policy cannot fall back to the ambient decoy.
#[test]
fn ordinary_support_refuses_unpinned_declaration() -> TestResult {
    child("unpinned")
}

/// A genuine executable-relative derive package must meet the pinned core's actual version requirement.
#[test]
fn ordinary_support_refuses_incompatible_derive_version() -> TestResult {
    child("wrong-version")
}

/// A library at the derive source coordinate cannot supply the required host proc-macro role.
#[test]
fn ordinary_support_refuses_wrong_derive_role() -> TestResult {
    child("wrong-role")
}

/// Select from the actual child executable and exercise original member ownership at each dependency handoff.
#[test]
fn ordinary_support_actual_executable_child() -> TestResult {
    let Some(mode) = std::env::var_os(CHILD) else {
        return Ok(());
    };
    if matches!(mode.to_str(), Some("unpinned" | "wrong-version" | "wrong-role")) {
        assert!(CompilerSupportSources::discover().is_err());
        return Ok(());
    }
    let selected = CompilerSupportSources::discover()?.ok_or("compiler support not discovered")?;
    let owner = selected.declaration_owner()?.to_path_buf();
    for _ in 0..2 {
        let dependencies = selected.dependencies()?;
        assert_eq!(
            dependencies
                .iter()
                .map(|dependency| dependency.crate_name.as_str())
                .collect::<Vec<_>>(),
            SUPPORT_CRATES_EVERY_PROGRAM_LINKS
        );
        for (dependency, package, directory) in [
            (&dependencies[0], "incan_stdlib_core", "core"),
            (&dependencies[1], "incan_derive", "derive/incan_derive"),
        ] {
            assert_eq!(dependency.package.as_deref().unwrap_or(&dependency.crate_name), package);
            assert!(!dependency.optional);
            let DependencySource::Path { path } = &dependency.source else {
                return Err("support dependency is not a genuine local source".into());
            };
            assert_eq!(path, &owner.join("stdlib").join(directory));
        }
    }
    if mode == "changed" {
        let path = owner.join("stdlib/core/loaf.toml");
        let modified = fs::metadata(&path)?.modified()?;
        let bytes = fs::read_to_string(&path)?;
        fs::write(&path, bytes.replace("Apache-2.0", "Apache-2.1"))?;
        fs::OpenOptions::new()
            .write(true)
            .open(path)?
            .set_times(fs::FileTimes::new().set_modified(modified))?;
        assert!(selected.dependencies().is_err());
    } else if mode == "replaced" {
        let path = owner.join("stdlib/derive/incan_derive/loaf.toml");
        let replacement = path.with_extension("replacement");
        fs::write(&replacement, fs::read(&path)?)?;
        fs::rename(replacement, path)?;
        assert!(selected.dependencies().is_err());
    } else {
        assert_eq!(mode, "valid");
        selected.verify()?;
    }
    Ok(())
}

/// Write the actual fixed-layout fixture members without altering production discovery.
fn write(path: &Path, contents: &str) -> TestResult {
    fs::create_dir_all(path.parent().ok_or("fixture parent missing")?)?;
    fs::write(path, contents)?;
    Ok(())
}
