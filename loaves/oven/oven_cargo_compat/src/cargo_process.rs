//! Spawning Cargo the way the compatibility path needs it: the executable it resolves to, the rustc it pins, the
//! target directory it writes, and the environment it must not inherit.
//!
//! These are the facts every Cargo invocation in Oven's compatibility mode shares — the generated-project fallback
//! build, the vocab extraction build, the rust-inspect workspace prewarm — spelled once so the process is
//! configured the same way wherever it starts.

use std::env;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Remove process-environment entries that Cargo and Rust build scripts cannot represent as Unicode.
///
/// Incan programs may intentionally inspect a non-Unicode environment value through `std.environ`. Cargo's build
/// script support, however, exposes the environment through Unicode-only APIs and can panic before compilation when
/// such a value is inherited. Generated binaries retain their original environment; only compiler-owned Cargo child
/// processes receive this sanitized view.
pub fn sanitize_cargo_environment(command: &mut Command) {
    for (key, value) in env::vars_os() {
        if key.to_str().is_none() || value.to_str().is_none() {
            command.env_remove(key);
        }
    }
}

/// Resolve the Cargo executable selected for compiler-owned generated-project commands.
pub fn cargo_executable() -> OsString {
    env::var_os("CARGO")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "cargo".into())
}

/// Resolve the configured Cargo program to a regular file for the bounded Oven compatibility baker.
///
/// Ordinary generated-project commands can pass a bare `cargo` name to [`Command`], which performs PATH lookup itself.
/// The baker verifies and records its executable before launching it, so it needs the same lookup as an explicit path
/// rather than treating a valid bare name as a missing file.
pub fn resolved_cargo_executable() -> io::Result<PathBuf> {
    // An explicit `CARGO` selection still wins. Otherwise prefer the Cargo belonging to Incan's own provisioned
    // toolchain: the compatibility baker's Cargo and the direct-Rustc compiler must come from one toolchain, and
    // `resolve_active_rustc` already prefers that same isolated installation.
    if env::var_os("CARGO").filter(|value| !value.is_empty()).is_none()
        && let Some(cargo) = oven_rustc::rustc::incan_owned_cargo()
    {
        return Ok(cargo);
    }
    resolve_cargo_executable_from_path(PathBuf::from(cargo_executable()), env::var_os("PATH"))
}

/// Resolve one configured Cargo selection without consulting process-global state.
///
/// A regular path is accepted directly. A bare executable name is resolved only against the supplied PATH, which
/// keeps the bounded publisher's executable proof testable and prevents a later ambient lookup from selecting a
/// different Cargo program.
fn resolve_cargo_executable_from_path(selected: PathBuf, search_path: Option<OsString>) -> io::Result<PathBuf> {
    if selected.is_file() {
        return Ok(selected);
    }
    if selected.components().count() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "configured Cargo executable is not a regular file: {}",
                selected.display()
            ),
        ));
    }
    let Some(search_path) = search_path else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Cargo executable `{}` is not on PATH", selected.display()),
        ));
    };
    let resolved = env::split_paths(&search_path)
        .map(|directory| directory.join(&selected))
        .find(|candidate| candidate.is_file());
    resolved.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("Cargo executable `{}` is not on PATH", selected.display()),
        )
    })
}

/// Create one compiler-owned Cargo command from the canonical executable selection.
///
/// An installed Incan resolves its compiler from its own provisioned toolchain, so Cargo has to come from that
/// same toolchain: the installer adds required targets (such as `wasm32-wasip1`) there and not to the user's
/// ambient Rustup, and a Cargo from a different toolchain would not see them. Development checkouts have no
/// provisioned toolchain and keep using the ambient selection.
pub fn cargo_command() -> Command {
    if env::var_os("CARGO").filter(|value| !value.is_empty()).is_none()
        && let Some(cargo) = oven_rustc::rustc::incan_owned_cargo()
    {
        let mut command = Command::new(cargo);
        pin_incan_owned_rustc(&mut command);
        return command;
    }
    Command::new(cargo_executable())
}

/// Pin the compiler belonging to the same provisioned toolchain as the Cargo about to run.
///
/// Selecting Incan's Cargo is not enough to select Incan's compiler. Cargo takes `rustc` from `RUSTC` or from
/// `PATH`, and on any machine with Rustup installed `PATH` reaches the Rustup shim, which resolves to the user's
/// default toolchain rather than the one Incan provisioned. The build then mixes two rustc versions in one
/// dependency graph and fails late with "found crate `x` compiled by an incompatible version of rustc", naming a
/// dependency rather than the toolchain split that caused it.
///
/// An explicit `RUSTC` still wins, matching how `CARGO` is honored above.
fn pin_incan_owned_rustc(command: &mut Command) {
    if let Some(rustc) = rustc_pin_for_incan_owned_cargo(env::var_os("RUSTC"), oven_rustc::rustc::incan_owned_rustc()) {
        command.env("RUSTC", rustc);
    }
}

/// Decide which compiler to pin, given any explicit `RUSTC` and the provisioned toolchain's own.
///
/// Split out from the environment so the policy is testable: an explicit selection wins, an empty value is treated
/// as absent exactly as `CARGO` is, and a checkout with no provisioned toolchain pins nothing and keeps the ambient
/// compiler.
fn rustc_pin_for_incan_owned_cargo(explicit: Option<OsString>, owned: Option<PathBuf>) -> Option<PathBuf> {
    if explicit.is_some_and(|value| !value.is_empty()) {
        return None;
    }
    owned
}

/// Keep Cargo's target and unstable build-directory outputs inside the lifecycle-owned target root.
pub fn configure_cargo_target(command: &mut Command, target_dir: &Path) {
    command.env("CARGO_TARGET_DIR", target_dir);
    command.env("CARGO_BUILD_BUILD_DIR", target_dir);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use super::*;

    const OFFLINE_CARGO_HOME_FIXTURE: &str = "INCAN_OFFLINE_CARGO_HOME_FIXTURE";

    /// Write a version-only dependency whose only source is the supplied Cargo home's directory registry.
    fn write_offline_cargo_home_fixture(root: &Path) -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error>> {
        let cargo_home = root.join("cargo-home");
        let vendored_package = cargo_home.join("vendor/publisher-cache-fixture");
        let project = root.join("publisher-project");
        fs::create_dir_all(vendored_package.join("src"))?;
        fs::create_dir_all(project.join("src"))?;
        fs::write(
            cargo_home.join("config.toml"),
            format!(
                "[source.crates-io]\nreplace-with = \"fixture-cache\"\n\n[source.fixture-cache]\ndirectory = {:?}\n",
                cargo_home.join("vendor")
            ),
        )?;
        fs::write(
            vendored_package.join("Cargo.toml"),
            "[package]\nname = \"publisher-cache-fixture\"\nversion = \"1.0.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(
            vendored_package.join("src/lib.rs"),
            "pub fn cached() -> bool { true }\n",
        )?;
        fs::write(
            vendored_package.join(".cargo-checksum.json"),
            "{\"files\":{},\"package\":null}\n",
        )?;
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"publisher-project\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\npublisher-cache-fixture = \"=1.0.0\"\n",
        )?;
        fs::write(
            project.join("src/main.rs"),
            "fn main() { assert!(publisher_cache_fixture::cached()); }\n",
        )?;
        Ok((cargo_home, project))
    }

    /// Resolve and build the fixture through Cargo commands cleaned by the publisher environment contract.
    fn run_offline_publisher_fixture(project: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let cargo = resolved_cargo_executable()?;
        let mut lock_command = Command::new(&cargo);
        lock_command
            .current_dir(project)
            .args(["generate-lockfile", "--offline"]);
        oven_rustc::rustc::clear_inherited_cargo_environment_for_cargo(&mut lock_command);
        let lock_output = lock_command.output()?;
        if !lock_output.status.success() {
            return Err(format!(
                "offline publisher lock resolution did not use the custom Cargo home:\n{}",
                String::from_utf8_lossy(&lock_output.stderr)
            )
            .into());
        }

        let mut command = Command::new(cargo);
        command.current_dir(project).args(["build", "--offline", "--locked"]);
        oven_rustc::rustc::clear_inherited_cargo_environment_for_cargo(&mut command);
        command.env("CARGO_TARGET_DIR", project.join("target"));
        let output = command.output()?;
        if !output.status.success() {
            return Err(format!(
                "offline publisher Cargo launch did not use the custom Cargo home:\n{}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn resolved_cargo_executable_finds_a_bare_name_on_the_supplied_path() -> Result<(), Box<dyn std::error::Error>> {
        let tools = tempfile::tempdir()?;
        let cargo = tools.path().join("cargo");
        fs::write(&cargo, "cargo fixture")?;

        let resolved =
            resolve_cargo_executable_from_path(PathBuf::from("cargo"), Some(env::join_paths([tools.path()])?))?;
        assert_eq!(resolved, cargo);
        Ok(())
    }

    #[test]
    fn generated_cargo_target_contains_inherited_build_directory() {
        let target = Path::new("/managed/generated-target");
        let mut command = Command::new("cargo");
        command.env("CARGO_BUILD_BUILD_DIR", "/outside");
        configure_cargo_target(&mut command, target);

        let environment = command
            .get_envs()
            .filter_map(|(name, value)| value.map(|value| (name, value)))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            environment.get(std::ffi::OsStr::new("CARGO_TARGET_DIR")),
            Some(&target.as_os_str())
        );
        assert_eq!(
            environment.get(std::ffi::OsStr::new("CARGO_BUILD_BUILD_DIR")),
            Some(&target.as_os_str())
        );
    }

    #[test]
    fn offline_publisher_cargo_launch_keeps_its_custom_cargo_home() -> Result<(), Box<dyn std::error::Error>> {
        if let Some(project) = env::var_os(OFFLINE_CARGO_HOME_FIXTURE) {
            return run_offline_publisher_fixture(Path::new(&project));
        }

        let fixture = tempfile::tempdir()?;
        let (cargo_home, project) = write_offline_cargo_home_fixture(fixture.path())?;
        let output = Command::new(env::current_exe()?)
            .args([
                "--exact",
                "cargo_process::tests::offline_publisher_cargo_launch_keeps_its_custom_cargo_home",
                "--nocapture",
            ])
            .env(OFFLINE_CARGO_HOME_FIXTURE, &project)
            .env("CARGO_HOME", cargo_home)
            .env("HOME", fixture.path().join("empty-default-home"))
            .output()?;
        assert!(
            output.status.success(),
            "isolated offline publisher fixture failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    /// Selecting Incan's Cargo must also select Incan's compiler.
    ///
    /// Cargo takes `rustc` from `RUSTC` or `PATH`, and on a machine with Rustup installed `PATH` reaches the shim,
    /// which resolves to the user's default toolchain rather than the provisioned one. Leaving it unpinned mixes two
    /// rustc versions in one dependency graph, which surfaces much later as "found crate `x` compiled by an
    /// incompatible version of rustc" against a dependency name that says nothing about the toolchain split.
    #[test]
    fn incan_owned_cargo_pins_its_own_compiler_unless_one_is_chosen_explicitly() {
        let owned = PathBuf::from("/incan/rust/toolchains/1.98.0/bin/rustc");

        assert_eq!(
            super::rustc_pin_for_incan_owned_cargo(None, Some(owned.clone())),
            Some(owned.clone()),
            "a provisioned toolchain must pin its own compiler",
        );
        assert_eq!(
            super::rustc_pin_for_incan_owned_cargo(Some(OsString::from("")), Some(owned.clone())),
            Some(owned.clone()),
            "an empty RUSTC is absent, exactly as an empty CARGO is",
        );
        assert_eq!(
            super::rustc_pin_for_incan_owned_cargo(Some(OsString::from("/usr/local/bin/rustc")), Some(owned)),
            None,
            "an explicit RUSTC wins",
        );
        assert_eq!(
            super::rustc_pin_for_incan_owned_cargo(None, None),
            None,
            "a checkout with no provisioned toolchain keeps the ambient compiler",
        );
    }
}
