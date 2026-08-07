//! Helpers for resolving files relative to the running Incan toolchain.
//!
//! Installers expose `incan` through symlinks or wrapper scripts, while the real toolchain lives under a versioned
//! directory containing `bin/`, `stdlib/`, and bundled support crates. Runtime lookup must therefore consider both the
//! executable path reported by the OS and the canonical target of that path.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Internal scheduler handoff for compiler-owned immutable data when a direct-rustc child is baked outside the
/// installed toolchain layout.
const INTERNAL_TOOLCHAIN_DATA_ROOT_ENV: &str = "INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT";
/// Internal scheduler handoff for children that must select the immutable Loaf closure.
///
/// This is deliberately narrower than a general environment override. A scheduler-owned child can inherit test
/// harness source-path overrides, but its receipt must continue to identify the closure baked by that scheduler.
const INTERNAL_OVEN_LOAF_EXECUTION_ENV: &str = "INCAN_INTERNAL_OVEN_LOAF_EXECUTION";
/// Internal scheduler handoff for the sealed compiler runtime source closure.
///
/// A fixture may deliberately clear `INCAN_SDK_INVENTORY` to exercise cold provider discovery. Its nested normal
/// Oven command must still derive runtime identity from the suite's receipt-authorized closure rather than from an
/// ambient checkout. This value is accepted only alongside the scheduler-native execution capability above.
const INTERNAL_OVEN_RUNTIME_ROOT_ENV: &str = "INCAN_INTERNAL_OVEN_RUNTIME_ROOT";
/// Explicit SDK inventory selected by an Oven publisher or direct-rustc child.
///
/// When that inventory seals a compact compiler runtime closure, generated publisher manifests must use the same
/// source roots as the provider manifests in the inventory. Otherwise Cargo sees two path packages with the same
/// name and version: one beneath the immutable inventory and one beneath the compiler checkout.
const SDK_INVENTORY_OVERRIDE_ENV: &str = "INCAN_SDK_INVENTORY";
const OVEN_LOAF_ROOT: &str = "share/incan/oven/loafs";

/// Inputs that determine which built-in stdlib source tree belongs to the active toolchain.
struct StdlibSearchPaths {
    override_roots: Vec<PathBuf>,
    development_root: PathBuf,
    current_dir: Option<PathBuf>,
    executable_bases: Vec<PathBuf>,
    installed_roots: Vec<PathBuf>,
}

/// Inputs that select compiler-owned support crates for generated Cargo projects and semantic lock identity.
struct ToolchainPathSearchPaths {
    crates_override: Option<PathBuf>,
    sealed_sdk_runtime_crates: Option<PathBuf>,
    development_root: PathBuf,
    executable_bases: Vec<PathBuf>,
}

/// Return candidate base directories around the current executable.
///
/// The list includes the executable directory, its parent, and its grandparent for both the raw executable path and its
/// canonical path. This covers development builds, installed toolchains, and user-facing symlinks such as
/// `~/.local/bin/incan -> ~/.incan/toolchains/<version>/bin/incan`.
pub(crate) fn current_executable_search_bases() -> Vec<PathBuf> {
    let Ok(exe_path) = std::env::current_exe() else {
        return Vec::new();
    };
    executable_search_bases_for(&exe_path)
}

/// Resolve one compiler-owned support crate through release staging, an installed SDK, or the development checkout.
pub(crate) fn resolve_toolchain_crate_path(crate_name: &str) -> PathBuf {
    resolve_toolchain_relative_path(&Path::new("crates").join(crate_name))
}

/// Resolve one toolchain-relative path through the same layout policy used by generated Cargo and lock semantics.
pub(crate) fn resolve_toolchain_relative_path(relative_path: &Path) -> PathBuf {
    let sealed_sdk_runtime_root = sealed_sdk_runtime_root();
    resolve_toolchain_relative_path_in(
        relative_path,
        &ToolchainPathSearchPaths {
            crates_override: external_toolchain_crates_override(
                env::var_os("INCAN_TOOLCHAIN_CRATES_DIR")
                    .filter(|path| !path.is_empty())
                    .map(PathBuf::from),
                scheduler_loaf_execution(),
                sealed_sdk_runtime_root.is_some(),
            ),
            sealed_sdk_runtime_crates: sealed_sdk_runtime_root.map(|root| root.join("crates")),
            development_root: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            executable_bases: current_executable_search_bases(),
        },
    )
}

/// Return the external support-crate override only when no sealed runtime closure is authoritative.
///
/// In ordinary commands `INCAN_TOOLCHAIN_CRATES_DIR` remains an intentional developer/test override. A valid
/// explicit SDK inventory, like a scheduler-owned compiler-suite child, seals the runtime source closure used for
/// generated manifests and Loaf identities. Honoring a parent checkout override in that situation would make
/// a Loaf compatible with neither the inventory nor the compiler suite that consumes it.
fn external_toolchain_crates_override(
    override_path: Option<PathBuf>,
    scheduler_native_execution: bool,
    sealed_sdk_runtime_available: bool,
) -> Option<PathBuf> {
    if scheduler_native_execution || sealed_sdk_runtime_available {
        None
    } else {
        override_path
    }
}

/// Return whether this process is an explicitly scheduler-owned direct-rustc child.
fn scheduler_loaf_execution() -> bool {
    env::var_os(INTERNAL_OVEN_LOAF_EXECUTION_ENV).is_some_and(|value| value == "1")
}

/// Resolve compiler-owned immutable data through the active installed toolchain or development checkout.
///
/// Unlike support crates, immutable Oven Loafs are ordinary data directories and do not contain a `Cargo.toml`.
/// This resolver deliberately accepts only compiler-relative paths; normal commands never accept an artifact-root path
/// from a project or from a generated Cargo target.
pub(crate) fn resolve_toolchain_data_path(relative_path: &Path) -> PathBuf {
    resolve_toolchain_data_path_in(
        relative_path,
        scheduler_toolchain_data_root(
            env::var_os(INTERNAL_TOOLCHAIN_DATA_ROOT_ENV)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
            scheduler_loaf_execution(),
        ),
        current_executable_search_bases(),
    )
}

/// Return the active compiler-owned data root only when it contains immutable Oven Loaf data.
///
/// The compiler-suite publisher and scheduler use the same internal handoff when an uninstalled development binary
/// needs to carry its already-selected data into caller-owned direct-rustc output. The override is accepted only
/// through the internal scheduler environment and must name a complete compiler-owned Loaf layout; no public
/// command accepts a data-root argument from a project.
pub(crate) fn compiler_owned_oven_data_root() -> Option<PathBuf> {
    compiler_owned_oven_data_root_in(
        scheduler_toolchain_data_root(
            env::var_os(INTERNAL_TOOLCHAIN_DATA_ROOT_ENV)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
            scheduler_loaf_execution(),
        ),
        current_executable_search_bases(),
    )
}

/// Return the compiler-data handoff only for an explicitly scheduler-owned direct-rustc child.
///
/// A bare environment variable is not authority for an ordinary command: otherwise a caller could select a
/// compiler-suite-only Loaf root without the scheduler's receipt and retained lease.
fn scheduler_toolchain_data_root(
    configured_root: Option<PathBuf>,
    scheduler_native_execution: bool,
) -> Option<PathBuf> {
    scheduler_native_execution.then_some(configured_root).flatten()
}

/// Resolve one immutable compiler-data path from the scheduler handoff or executable-relative layout.
fn resolve_toolchain_data_path_in(
    relative_path: &Path,
    scheduler_data_root: Option<PathBuf>,
    executable_bases: Vec<PathBuf>,
) -> PathBuf {
    if let Some(root) = scheduler_data_root.filter(|root| root.is_absolute()) {
        let candidate = root.join(relative_path);
        if candidate.exists() && root.join(OVEN_LOAF_ROOT).is_dir() {
            return canonical_toolchain_path(candidate);
        }
    }
    for base in executable_bases {
        let candidate = base.join(relative_path);
        if candidate.exists() {
            return canonical_toolchain_path(candidate);
        }
    }
    canonical_toolchain_path(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative_path))
}

/// Prefer a scheduler-leased compiler data root, then the installed executable layout, when locating Loafs.
///
/// A relative scheduler value is deliberately ignored: it would make a suite child’s sealed compiler closure depend
/// on its current working directory rather than the explicit receipt-selected toolchain data root.
fn compiler_owned_oven_data_root_in(
    scheduler_data_root: Option<PathBuf>,
    executable_bases: Vec<PathBuf>,
) -> Option<PathBuf> {
    if let Some(root) = scheduler_data_root.filter(|root| root.is_absolute())
        && root.join(OVEN_LOAF_ROOT).is_dir()
    {
        return Some(canonical_toolchain_path(root));
    }
    executable_bases
        .into_iter()
        .find(|base| base.join(OVEN_LOAF_ROOT).is_dir())
        .map(canonical_toolchain_path)
}

/// Resolve the lockfile that belongs to the active compiler runtime closure.
///
/// Release archives keep this below `crates/Cargo.lock`, alongside the checked support-crate sources. Development
/// checkouts retain the workspace lock at the repository root. Loaf identity must use the installed
/// representation when one exists, rather than the checkout from which a compiler binary happened to be built.
pub(crate) fn resolve_toolchain_runtime_lockfile() -> PathBuf {
    if let Some(runtime_root) = sealed_sdk_runtime_root() {
        let lockfile = runtime_root.join("Cargo.lock");
        if lockfile.is_file() {
            return canonical_toolchain_path(lockfile);
        }
    }
    for base in current_executable_search_bases() {
        let candidate = base.join("crates/Cargo.lock");
        if candidate.is_file() {
            return canonical_toolchain_path(candidate);
        }
    }
    canonical_toolchain_path(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock"))
}

/// Apply the canonical support-path search order to injected, testable layout inputs.
fn resolve_toolchain_relative_path_in(relative_path: &Path, paths: &ToolchainPathSearchPaths) -> PathBuf {
    let crate_relative = relative_path.strip_prefix("crates").ok();
    if let (Some(crates_dir), Some(crate_relative)) = (paths.crates_override.as_deref(), crate_relative) {
        let candidate = crates_dir.join(crate_relative);
        if toolchain_relative_path_exists(&candidate, crate_relative) {
            return canonical_toolchain_path(candidate);
        }
    }
    if let (Some(crates_dir), Some(crate_relative)) = (paths.sealed_sdk_runtime_crates.as_deref(), crate_relative) {
        let candidate = crates_dir.join(crate_relative);
        if toolchain_relative_path_exists(&candidate, crate_relative) {
            return canonical_toolchain_path(candidate);
        }
    }
    for base in &paths.executable_bases {
        let candidate = base.join(relative_path);
        if toolchain_relative_path_exists(&candidate, crate_relative.unwrap_or(relative_path)) {
            return canonical_toolchain_path(candidate);
        }
    }
    canonical_toolchain_path(paths.development_root.join(relative_path))
}

/// Return the compact compiler-runtime closure sealed beside an explicitly selected SDK inventory.
///
/// This is intentionally narrower than general SDK discovery: it is only a compiler-owned runtime source closure
/// with all crates generated publisher manifests can name. A malformed or ordinary inventory simply does not alter
/// toolchain layout selection; higher-level SDK discovery remains responsible for validating the inventory itself.
fn sealed_sdk_runtime_root() -> Option<PathBuf> {
    if scheduler_loaf_execution()
        && let Some(runtime_root) = env::var_os(INTERNAL_OVEN_RUNTIME_ROOT_ENV)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .and_then(validated_sdk_runtime_root)
    {
        return Some(runtime_root);
    }
    let inventory_path = env::var_os(SDK_INVENTORY_OVERRIDE_ENV).filter(|path| !path.is_empty())?;
    let inventory_path = fs::canonicalize(inventory_path).ok()?;
    inventory_path
        .parent()
        .map(|parent| parent.join("runtime"))
        .and_then(validated_sdk_runtime_root)
}

/// Confirm that a scheduler-provided or inventory-adjacent directory is a complete compiler runtime closure.
fn validated_sdk_runtime_root(runtime_root: PathBuf) -> Option<PathBuf> {
    let runtime_root = fs::canonicalize(runtime_root).ok()?;
    if !runtime_root.join("Cargo.lock").is_file() {
        return None;
    }
    let crates_root = runtime_root.join("crates");
    for crate_name in ["incan_core", "incan_derive", "incan_stdlib", "incan_web_macros"] {
        if !crates_root.join(crate_name).join("Cargo.toml").is_file() {
            return None;
        }
    }
    Some(runtime_root)
}

/// Return a canonical path whenever the selected compiler-owned source exists.
///
/// Generated Cargo manifests can combine paths from an SDK artifact with paths from the active compiler. On macOS,
/// `/tmp` is a symlink to `/private/tmp`; preserving both spellings makes Cargo treat one checkout as two distinct path
/// packages. Missing fallback paths remain unchanged so lookup errors retain their useful requested spelling.
fn canonical_toolchain_path(path: PathBuf) -> PathBuf {
    fs::canonicalize(&path).unwrap_or(path)
}

/// Require the owning crate manifest while allowing the requested path to point below that crate root.
fn toolchain_relative_path_exists(candidate: &Path, crate_relative: &Path) -> bool {
    if crate_relative.components().next().is_none() {
        return false;
    }
    let tail_len = crate_relative.components().count().saturating_sub(1);
    let mut crate_root = candidate.to_path_buf();
    for _ in 0..tail_len {
        if !crate_root.pop() {
            return false;
        }
    }
    crate_root.join("Cargo.toml").is_file()
}

/// Return candidate base directories around `exe_path`.
pub(crate) fn executable_search_bases_for(exe_path: &Path) -> Vec<PathBuf> {
    let mut bases = Vec::new();
    push_executable_bases(&mut bases, exe_path);
    if let Ok(canonical_exe_path) = fs::canonicalize(exe_path) {
        push_executable_bases(&mut bases, &canonical_exe_path);
    }
    bases
}

/// Return the built-in stdlib source directory selected for the active toolchain.
///
/// `INCAN_STDLIB` and `INCAN_STDLIB_DIR` are explicit overrides and therefore take precedence over every
/// auto-detected development or installed layout. Keeping this policy here ensures parsing, typechecking, testing
/// metadata, and compiled-provider publication cannot silently select different stdlib source trees.
pub(crate) fn find_stdlib_source_dir() -> Option<PathBuf> {
    find_stdlib_source_dir_in(StdlibSearchPaths {
        override_roots: [env::var_os("INCAN_STDLIB"), env::var_os("INCAN_STDLIB_DIR")]
            .into_iter()
            .flatten()
            .filter(|root| !root.is_empty())
            .map(PathBuf::from)
            .collect(),
        development_root: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        current_dir: env::current_dir().ok(),
        executable_bases: current_executable_search_bases(),
        installed_roots: [env::var_os("INCAN_STDLIB_PATH")]
            .into_iter()
            .flatten()
            .filter(|root| !root.is_empty())
            .map(PathBuf::from)
            .collect(),
    })
}

/// Resolve one `stdlib/...` source path through the active toolchain's canonical stdlib root.
pub(crate) fn find_stdlib_source_file(relative_path: &str) -> Option<PathBuf> {
    stdlib_source_file_from_dir(&find_stdlib_source_dir()?, relative_path)
}

/// Resolve a source path relative to an already selected stdlib directory.
fn stdlib_source_file_from_dir(stdlib_dir: &Path, relative_path: &str) -> Option<PathBuf> {
    let relative_path = Path::new(relative_path)
        .strip_prefix("stdlib")
        .unwrap_or_else(|_| Path::new(relative_path));
    let path = stdlib_dir.join(relative_path);
    path.is_file().then_some(path)
}

/// Apply the canonical stdlib source search order to injected, testable path inputs.
fn find_stdlib_source_dir_in(paths: StdlibSearchPaths) -> Option<PathBuf> {
    for root in paths.override_roots {
        if let Some(stdlib) = stdlib_source_dir_from_root(&root) {
            return Some(stdlib);
        }
    }

    // `incan build --lib` is valid from the built-in stdlib root itself. Recognize that layout before the compiler's
    // build workspace so source imports resolve inside the stdlib being built.
    if let Some(current_dir) = paths.current_dir.as_deref()
        && is_builtin_stdlib_source_dir(current_dir)
    {
        return Some(current_dir.to_path_buf());
    }

    if let Some(stdlib) = stdlib_source_dir_from_development_root(&paths.development_root) {
        return Some(stdlib);
    }

    if let Some(current_dir) = paths.current_dir.as_deref()
        && let Some(stdlib) = stdlib_source_dir_from_development_root(current_dir)
    {
        return Some(stdlib);
    }

    for base in paths.executable_bases {
        if let Some(stdlib) = stdlib_source_dir_from_development_root(&base) {
            return Some(stdlib);
        }
    }

    for root in paths.installed_roots {
        if let Some(stdlib) = stdlib_source_dir_from_root(&root) {
            return Some(stdlib);
        }
    }

    None
}

/// Resolve the stdlib beneath a repository, crate, or installed toolchain root.
fn stdlib_source_dir_from_development_root(root: &Path) -> Option<PathBuf> {
    [root.join("crates/incan_stdlib/stdlib"), root.join("stdlib")]
        .into_iter()
        .find(|candidate| candidate.is_dir())
}

/// Resolve either a direct stdlib directory or a toolchain/crate root containing `stdlib/`.
fn stdlib_source_dir_from_root(root: &Path) -> Option<PathBuf> {
    if !root.is_dir() {
        return None;
    }
    let nested = root.join("stdlib");
    if nested.is_dir() {
        return Some(nested);
    }
    Some(root.to_path_buf())
}

/// Return whether `path` is the Incan built-in stdlib source root itself.
fn is_builtin_stdlib_source_dir(path: &Path) -> bool {
    path.is_dir() && path.join("incan.toml").is_file() && path.join("prelude.incn").is_file()
}

/// Append `exe_path`'s directory, parent, and grandparent to `bases`.
fn push_executable_bases(bases: &mut Vec<PathBuf>, exe_path: &Path) {
    let Some(exe_dir) = exe_path.parent() else {
        return;
    };
    push_unique(bases, exe_dir.to_path_buf());
    if let Some(parent) = exe_dir.parent() {
        push_unique(bases, parent.to_path_buf());
        if let Some(grandparent) = parent.parent() {
            push_unique(bases, grandparent.to_path_buf());
        }
    }
}

/// Push `path` only if it has not already been recorded.
fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{
        StdlibSearchPaths, ToolchainPathSearchPaths, compiler_owned_oven_data_root_in, executable_search_bases_for,
        external_toolchain_crates_override, find_stdlib_source_dir_in, resolve_toolchain_data_path_in,
        resolve_toolchain_relative_path_in, scheduler_toolchain_data_root, stdlib_source_file_from_dir,
        validated_sdk_runtime_root,
    };

    #[test]
    fn executable_search_bases_include_symlink_target_ancestors() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let toolchain_bin = tmp.path().join("home/toolchains/0.4.0-test/bin");
        let launcher_bin = tmp.path().join("launcher/bin");
        fs::create_dir_all(&toolchain_bin)?;
        fs::create_dir_all(&launcher_bin)?;
        let real_exe = toolchain_bin.join("incan");
        fs::write(&real_exe, "")?;
        let launcher = launcher_bin.join("incan");
        symlink_file(&real_exe, &launcher)?;

        let bases = executable_search_bases_for(&launcher);

        let canonical_toolchain_bin = fs::canonicalize(tmp.path().join("home/toolchains/0.4.0-test/bin"))?;
        let canonical_toolchain_root = fs::canonicalize(tmp.path().join("home/toolchains/0.4.0-test"))?;

        assert!(bases.contains(&launcher_bin));
        assert!(bases.contains(&canonical_toolchain_bin));
        assert!(bases.contains(&canonical_toolchain_root));
        Ok(())
    }

    #[test]
    fn explicit_stdlib_override_wins_over_development_and_executable_layouts() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let explicit = tmp.path().join("explicit-stdlib");
        let development_root = tmp.path().join("development");
        let executable_root = tmp.path().join("old-toolchain");
        for path in [
            explicit.clone(),
            development_root.join("crates/incan_stdlib/stdlib"),
            executable_root.join("stdlib"),
        ] {
            fs::create_dir_all(path)?;
        }

        let found = find_stdlib_source_dir_in(StdlibSearchPaths {
            override_roots: vec![explicit.clone()],
            development_root,
            current_dir: None,
            executable_bases: vec![executable_root],
            installed_roots: Vec::new(),
        })
        .ok_or("expected an explicit stdlib source override")?;

        assert_eq!(found, explicit);
        Ok(())
    }

    #[test]
    fn stdlib_source_build_uses_the_current_stdlib_root() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let current_stdlib = tmp.path().join("checked-out-stdlib");
        fs::create_dir_all(&current_stdlib)?;
        fs::write(
            current_stdlib.join("incan.toml"),
            "[project]\nname = \"incan_builtin_stdlib\"\n",
        )?;
        fs::write(current_stdlib.join("prelude.incn"), "")?;
        let development_root = tmp.path().join("compiler-source");
        fs::create_dir_all(development_root.join("crates/incan_stdlib/stdlib"))?;

        let found = find_stdlib_source_dir_in(StdlibSearchPaths {
            override_roots: Vec::new(),
            development_root,
            current_dir: Some(current_stdlib.clone()),
            executable_bases: Vec::new(),
            installed_roots: Vec::new(),
        })
        .ok_or("expected the current built-in stdlib source root")?;

        assert_eq!(found, current_stdlib);
        Ok(())
    }

    #[test]
    fn installed_support_crates_resolve_independently_without_web_macros() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let installed_root = tmp.path().join("toolchain");
        for crate_name in ["incan_stdlib", "incan_derive"] {
            let crate_root = installed_root.join("crates").join(crate_name);
            fs::create_dir_all(&crate_root)?;
            fs::write(
                crate_root.join("Cargo.toml"),
                format!("[package]\nname = \"{crate_name}\"\nversion = \"0.5.0\"\n"),
            )?;
        }
        let search_paths = ToolchainPathSearchPaths {
            crates_override: None,
            sealed_sdk_runtime_crates: None,
            development_root: tmp.path().join("absent-checkout"),
            executable_bases: vec![installed_root.clone()],
        };
        let installed_crates = installed_root.join("crates").canonicalize()?;

        assert_eq!(
            resolve_toolchain_relative_path_in(Path::new("crates/incan_stdlib"), &search_paths),
            installed_crates.join("incan_stdlib")
        );
        assert_eq!(
            resolve_toolchain_relative_path_in(Path::new("crates/incan_derive"), &search_paths),
            installed_crates.join("incan_derive")
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn toolchain_override_canonicalizes_symlinked_crate_paths() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let real_crates = tmp.path().join("real/crates");
        let real_core = real_crates.join("incan_core");
        fs::create_dir_all(&real_core)?;
        fs::write(
            real_core.join("Cargo.toml"),
            "[package]\nname = \"incan_core\"\nversion = \"0.5.0\"\n",
        )?;
        let alias_crates = tmp.path().join("alias-crates");
        symlink_file(&real_crates, &alias_crates)?;
        let search_paths = ToolchainPathSearchPaths {
            crates_override: Some(alias_crates),
            sealed_sdk_runtime_crates: None,
            development_root: tmp.path().join("absent-checkout"),
            executable_bases: Vec::new(),
        };

        let resolved = resolve_toolchain_relative_path_in(Path::new("crates/incan_core"), &search_paths);
        assert_eq!(resolved, fs::canonicalize(real_core)?);
        assert_ne!(resolved, PathBuf::from(tmp.path()).join("alias-crates/incan_core"));
        Ok(())
    }

    #[test]
    fn sealed_runtime_execution_ignores_an_external_toolchain_crates_override() {
        let override_path = PathBuf::from("/test-only/checkout/crates");

        assert_eq!(
            external_toolchain_crates_override(Some(override_path.clone()), false, false),
            Some(override_path)
        );
        assert_eq!(
            external_toolchain_crates_override(Some(PathBuf::from("/test-only/checkout/crates")), true, false),
            None
        );
        assert_eq!(
            external_toolchain_crates_override(Some(PathBuf::from("/test-only/checkout/crates")), false, true),
            None
        );
    }

    #[test]
    fn sealed_runtime_root_requires_the_complete_compiler_source_closure() -> Result<(), Box<dyn std::error::Error>> {
        let runtime = tempfile::tempdir()?;
        fs::write(runtime.path().join("Cargo.lock"), "version = 4\n")?;
        for crate_name in ["incan_core", "incan_derive", "incan_stdlib", "incan_web_macros"] {
            let crate_root = runtime.path().join("crates").join(crate_name);
            fs::create_dir_all(&crate_root)?;
            fs::write(
                crate_root.join("Cargo.toml"),
                format!("[package]\nname = \"{crate_name}\"\n"),
            )?;
        }

        assert_eq!(
            validated_sdk_runtime_root(runtime.path().to_path_buf()),
            Some(runtime.path().canonicalize()?)
        );
        fs::remove_file(runtime.path().join("crates/incan_web_macros/Cargo.toml"))?;
        assert!(validated_sdk_runtime_root(runtime.path().to_path_buf()).is_none());
        Ok(())
    }

    #[test]
    fn sealed_sdk_runtime_crates_precede_executable_layout_but_not_explicit_override()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let executable_root = tmp.path().join("installed-toolchain");
        let sealed_crates = tmp.path().join("sealed-sdk/runtime/crates");
        let override_crates = tmp.path().join("explicit-override");
        for root in [&executable_root.join("crates"), &sealed_crates, &override_crates] {
            let crate_root = root.join("incan_stdlib");
            fs::create_dir_all(&crate_root)?;
            fs::write(
                crate_root.join("Cargo.toml"),
                "[package]\nname = \"incan_stdlib\"\nversion = \"0.5.0\"\n",
            )?;
        }

        let mut search_paths = ToolchainPathSearchPaths {
            crates_override: None,
            sealed_sdk_runtime_crates: Some(sealed_crates.clone()),
            development_root: tmp.path().join("absent-checkout"),
            executable_bases: vec![executable_root],
        };
        assert_eq!(
            resolve_toolchain_relative_path_in(Path::new("crates/incan_stdlib"), &search_paths),
            fs::canonicalize(sealed_crates.join("incan_stdlib"))?
        );

        search_paths.crates_override = Some(override_crates.clone());
        assert_eq!(
            resolve_toolchain_relative_path_in(Path::new("crates/incan_stdlib"), &search_paths),
            fs::canonicalize(override_crates.join("incan_stdlib"))?
        );
        Ok(())
    }

    #[test]
    fn installed_root_and_stdlib_relative_file_resolve_to_one_source_tree() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let installed_root = tmp.path().join("installed-toolchain");
        let installed_stdlib = installed_root.join("stdlib");
        fs::create_dir_all(&installed_stdlib)?;
        fs::write(installed_stdlib.join("testing.incn"), "")?;

        let found = find_stdlib_source_dir_in(StdlibSearchPaths {
            override_roots: Vec::new(),
            development_root: tmp.path().join("absent-development-root"),
            current_dir: None,
            executable_bases: Vec::new(),
            installed_roots: vec![installed_root],
        })
        .ok_or("expected an installed stdlib source root")?;
        let source = stdlib_source_file_from_dir(&found, "stdlib/testing.incn")
            .ok_or("expected stdlib-relative source lookup")?;

        assert_eq!(found, installed_stdlib);
        assert_eq!(source, installed_stdlib.join("testing.incn"));
        Ok(())
    }

    #[test]
    fn direct_rustc_child_uses_only_a_scheduler_derived_toolchain_data_root() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let installed_root = tmp.path().join("installed-toolchain");
        let loaf = installed_root.join("share/incan/oven/loafs/unit.loaf/loaf.json");
        fs::create_dir_all(loaf.parent().ok_or("Loaf parent missing")?)?;
        fs::write(&loaf, "sealed Loaf")?;
        let relative_loaf = Path::new("share/incan/oven/loafs/unit.loaf/loaf.json");

        let resolved = resolve_toolchain_data_path_in(
            relative_loaf,
            Some(installed_root.clone()),
            vec![tmp.path().join("absent-executable-root")],
        );

        assert_eq!(resolved, fs::canonicalize(loaf)?);
        Ok(())
    }

    #[test]
    fn ordinary_commands_ignore_the_scheduler_data_root_environment_value() {
        let configured = Some(PathBuf::from("/compiler-suite-only-data"));
        assert_eq!(scheduler_toolchain_data_root(configured.clone(), false), None);
        assert_eq!(scheduler_toolchain_data_root(configured.clone(), true), configured);
        assert_eq!(scheduler_toolchain_data_root(None, true), None);
    }

    #[test]
    fn installed_parent_data_root_requires_the_loaf_layout() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let unrelated_root = tmp.path().join("unrelated");
        let installed_root = tmp.path().join("installed-toolchain");
        fs::create_dir_all(installed_root.join("share/incan/oven/loafs"))?;

        let resolved = compiler_owned_oven_data_root_in(None, vec![unrelated_root, installed_root.clone()])
            .ok_or("expected installed parent data root")?;

        assert_eq!(resolved, fs::canonicalize(installed_root)?);
        Ok(())
    }

    #[test]
    fn internal_data_root_precedes_the_scheduler_executable_layout() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let scheduler_root = tmp.path().join("scheduler-toolchain");
        let executable_root = tmp.path().join("executable-toolchain");
        for root in [&scheduler_root, &executable_root] {
            fs::create_dir_all(root.join("share/incan/oven/loafs"))?;
        }

        let resolved = compiler_owned_oven_data_root_in(Some(scheduler_root.clone()), vec![executable_root])
            .ok_or("expected scheduler data root")?;

        assert_eq!(resolved, fs::canonicalize(scheduler_root)?);
        Ok(())
    }

    #[test]
    fn relative_scheduler_data_root_is_ignored() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let relative_root = PathBuf::from("relative-toolchain");
        let fallback_root = tmp.path().join("fallback");
        let loaf = fallback_root.join("share/incan/oven/loafs/unit.loaf/loaf.json");
        fs::create_dir_all(loaf.parent().ok_or("Loaf parent missing")?)?;
        fs::write(&loaf, "sealed Loaf")?;
        let relative_loaf = Path::new("share/incan/oven/loafs/unit.loaf/loaf.json");

        let resolved = resolve_toolchain_data_path_in(relative_loaf, Some(relative_root), vec![fallback_root]);

        assert_eq!(resolved, fs::canonicalize(loaf)?);
        Ok(())
    }

    #[cfg(unix)]
    fn symlink_file(original: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(original, link)
    }

    #[cfg(windows)]
    fn symlink_file(original: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(original, link)
    }
}
