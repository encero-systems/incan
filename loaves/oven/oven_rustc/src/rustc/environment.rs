//! Constraining inherited process state and applying Oven compiler profiles.

use super::{Command, OVEN_COMPILER_TEST_PROFILE, OsStr, OsString, PathBuf, env};

/// Whether one inherited environment variable must be cleared before launching `rustc`.
///
/// Anything Cargo sets, plus the wrapper and flag variables, changes what the compiler does without appearing in
/// the recorded invocation. Leaving one set would make an identity describe a compilation that did not happen.
///
/// `RUSTC_BOOTSTRAP` belongs here too: the compiler-suite scheduler sets it for every libtest child so libtest
/// accepts `-Z unstable-options`, and a stable compiler that inherits it reports nightly-only facts. One such fact
/// is the bare `target_has_atomic` flag beside the valued `target_has_atomic="8"` entries, which the cfg snapshot
/// validator refuses as a key recorded both ways, so every publisher-side `--print cfg` probe under the suite
/// failed until the variable was cleared.
pub(super) fn direct_rustc_excludes_inherited_environment(name: &OsStr) -> bool {
    name == "CARGO"
        || name.to_string_lossy().starts_with("CARGO_")
        || matches!(
            name.to_str(),
            Some("RUSTC_WRAPPER" | "RUSTC_WORKSPACE_WRAPPER" | "RUSTFLAGS" | "RUSTC_BOOTSTRAP")
        )
}

/// Clear ambient Cargo state before direct consumer execution; the explicit compiler path remains authoritative.
///
/// Direct `rustc` launches must not inherit `CARGO_HOME`: even a cache-location variable is ambient Cargo state at
/// this boundary, and retaining it would make the compiler invocation depend on an input absent from its receipt.
pub fn clear_inherited_cargo_environment(command: &mut Command) {
    for (name, _) in env::vars_os().filter(|(name, _)| direct_rustc_excludes_inherited_environment(name)) {
        command.env_remove(name);
    }
    command.env_remove("CARGO_HOME");
}

/// Clear ambient compiler controls for a Cargo launch while retaining its resolved Cargo home.
///
/// Cargo needs its home to locate registry sources during locked offline publication. An explicit command value or
/// inherited `CARGO_HOME` wins; otherwise the default is resolved from `HOME` and written onto the child command so
/// the launch no longer relies on Cargo rediscovering that location after the rest of its ambient state is removed.
pub fn clear_inherited_cargo_environment_for_cargo(command: &mut Command) {
    let cargo_home = explicit_command_cargo_home(command)
        .or_else(|| env::var_os("CARGO_HOME").filter(|value| !value.is_empty()))
        .or_else(|| {
            env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".cargo").into_os_string())
        });
    for (name, _) in env::vars_os().filter(|(name, _)| direct_rustc_excludes_inherited_environment(name)) {
        command.env_remove(name);
    }
    if let Some(cargo_home) = cargo_home {
        command.env("CARGO_HOME", cargo_home);
    }
}

/// Return a Cargo home already set directly on a command before ambient resolution is applied.
pub(super) fn explicit_command_cargo_home(command: &Command) -> Option<OsString> {
    command
        .get_envs()
        .find(|(name, _)| *name == OsStr::new("CARGO_HOME"))
        .and_then(|(_, value)| value)
        .filter(|value| !value.is_empty())
        .map(OsString::from)
}

/// Apply the named Oven compiler-suite contract to a direct compiler invocation.
///
/// Cargo sees the matching manifest-declared profile only inside the explicit bootstrap publisher. Every normal
/// consumer uses this direct-rustc representation instead, so its flags are deliberate receipt semantics rather
/// than inherited Cargo environment state.
pub(super) fn apply_oven_profile(command: &mut Command, profile: &str) {
    for (name, value) in oven_profile_codegen_options(profile) {
        command.arg("-C").arg(format!("{name}={value}"));
    }
}

/// Return the complete typed profile projection shared by physical invocation and JEC identity.
pub(super) fn oven_profile_codegen_options(profile: &str) -> &'static [(&'static str, &'static str)] {
    if profile == OVEN_COMPILER_TEST_PROFILE {
        return &[
            ("debuginfo", "0"),
            ("strip", "debuginfo"),
            ("debug-assertions", "on"),
            ("overflow-checks", "on"),
        ];
    }

    // Optimization is part of what a profile *means*, and rustc optimizes nothing unless told to. Cargo used to
    // supply this implicitly from `[profile.release]`; once Oven replaced Cargo on the normal build path, nothing
    // did, so `incan build --release` emitted `opt-level=0` binaries and ran roughly six times slower than the
    // same sources at `-C opt-level=3`. These flags mirror Cargo's own release and dev profiles so a release
    // binary performs like the Rust it compiles to, and both profiles state their level explicitly rather than
    // inheriting a compiler default that has already gone wrong once.
    match profile {
        "release" => &[("opt-level", "3")],
        _ => &[("opt-level", "0")],
    }
}
