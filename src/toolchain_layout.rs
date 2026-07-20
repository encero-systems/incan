//! Helpers for resolving files relative to the running Incan toolchain.
//!
//! Installers expose `incan` through symlinks or wrapper scripts, while the real toolchain lives under a versioned
//! directory containing `bin/`, `stdlib/`, and bundled support crates. Runtime lookup must therefore consider both the
//! executable path reported by the OS and the canonical target of that path.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

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
    resolve_toolchain_relative_path_in(
        relative_path,
        &ToolchainPathSearchPaths {
            crates_override: env::var_os("INCAN_TOOLCHAIN_CRATES_DIR")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
            development_root: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            executable_bases: current_executable_search_bases(),
        },
    )
}

/// Apply the canonical support-path search order to injected, testable layout inputs.
fn resolve_toolchain_relative_path_in(relative_path: &Path, paths: &ToolchainPathSearchPaths) -> PathBuf {
    let crate_relative = relative_path.strip_prefix("crates").ok();
    if let (Some(crates_dir), Some(crate_relative)) = (paths.crates_override.as_deref(), crate_relative) {
        let candidate = crates_dir.join(crate_relative);
        if toolchain_relative_path_exists(&candidate, crate_relative) {
            return candidate;
        }
    }
    for base in &paths.executable_bases {
        let candidate = base.join(relative_path);
        if toolchain_relative_path_exists(&candidate, crate_relative.unwrap_or(relative_path)) {
            return candidate;
        }
    }
    paths.development_root.join(relative_path)
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
    use std::path::Path;

    use super::{
        StdlibSearchPaths, ToolchainPathSearchPaths, executable_search_bases_for, find_stdlib_source_dir_in,
        resolve_toolchain_relative_path_in, stdlib_source_file_from_dir,
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
            development_root: tmp.path().join("absent-checkout"),
            executable_bases: vec![installed_root.clone()],
        };

        assert_eq!(
            resolve_toolchain_relative_path_in(Path::new("crates/incan_stdlib"), &search_paths),
            installed_root.join("crates/incan_stdlib")
        );
        assert_eq!(
            resolve_toolchain_relative_path_in(Path::new("crates/incan_derive"), &search_paths),
            installed_root.join("crates/incan_derive")
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

    #[cfg(unix)]
    fn symlink_file(original: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(original, link)
    }

    #[cfg(windows)]
    fn symlink_file(original: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(original, link)
    }
}
