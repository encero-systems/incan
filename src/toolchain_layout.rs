//! Helpers for resolving files relative to the running Incan toolchain.
//!
//! Installers expose `incan` through symlinks or wrapper scripts, while the real toolchain lives under a versioned
//! directory containing `bin/`, `stdlib/`, and bundled support crates. Runtime lookup must therefore consider both the
//! executable path reported by the OS and the canonical target of that path.

use std::fs;
use std::path::{Path, PathBuf};

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

/// Return candidate base directories around `exe_path`.
pub(crate) fn executable_search_bases_for(exe_path: &Path) -> Vec<PathBuf> {
    let mut bases = Vec::new();
    push_executable_bases(&mut bases, exe_path);
    if let Ok(canonical_exe_path) = fs::canonicalize(exe_path) {
        push_executable_bases(&mut bases, &canonical_exe_path);
    }
    bases
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

    use super::executable_search_bases_for;

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

    #[cfg(unix)]
    fn symlink_file(original: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(original, link)
    }

    #[cfg(windows)]
    fn symlink_file(original: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(original, link)
    }
}
