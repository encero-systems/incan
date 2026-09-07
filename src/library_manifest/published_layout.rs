//! Where a published library keeps its executable representation on disk.
//!
//! This is the one place producer and consumer agree on that layout. A library build writes surfaces through it and
//! an importing compilation reads them through it, so the naming cannot drift between the two halves of a contract
//! whose whole point is that both sides mean the same thing.
//!
//! Two details RFC 123 leaves open are settled here, and both are chosen to survive RFC 034's archive arriving:
//!
//! - The directory is named for the `semantic/` slot a `.incanpkg` reserves, so publication becomes a move rather than
//!   a redesign. No packaging exists in this repository yet, so today the directory sits beside the manifest.
//! - A module's file is named by its module path, so a consumer reaches it from the path its canonical identity already
//!   carries rather than from a listing it would have to load and trust first.

use std::path::{Path, PathBuf};

/// Directory holding a library's executable representation, named for RFC 034's reserved archive slot.
pub const EXECUTABLE_SURFACE_DIRECTORY: &str = "semantic";

/// Extension identifying one module's executable representation.
pub const EXECUTABLE_SURFACE_EXTENSION: &str = "incnsem";

/// File name a module's executable representation is published under.
///
/// The root module has no path segments and would otherwise produce an empty name, so it is spelled explicitly.
/// Segments join with `.` rather than nesting directories, so one flat listing shows a library's whole published
/// surface and no consumer has to walk a tree to find out what exists.
pub fn executable_surface_file_name(module_path: &[String]) -> String {
    let stem = if module_path.is_empty() {
        "root".to_string()
    } else {
        module_path.join(".")
    };
    format!("{stem}.{EXECUTABLE_SURFACE_EXTENSION}")
}

/// Path of one module's executable representation, relative to the library manifest that publishes it.
///
/// Returns `None` only when the manifest path has no parent directory, which is not a layout a published library
/// can have.
pub fn executable_surface_path(manifest_path: &Path, module_path: &[String]) -> Option<PathBuf> {
    Some(
        manifest_path
            .parent()?
            .join(EXECUTABLE_SURFACE_DIRECTORY)
            .join(executable_surface_file_name(module_path)),
    )
}

#[cfg(test)]
mod tests {
    use super::{executable_surface_file_name, executable_surface_path};
    use std::path::Path;

    #[test]
    fn a_module_path_names_its_own_file() {
        assert_eq!(
            executable_surface_file_name(&["dataset".to_string(), "ops".to_string()]),
            "dataset.ops.incnsem"
        );
    }

    #[test]
    fn the_root_module_is_named_rather_than_left_empty() {
        // Joining no segments would produce ".incnsem", which is a hidden file on Unix and names nothing.
        assert_eq!(executable_surface_file_name(&[]), "root.incnsem");
    }

    #[test]
    fn a_surface_sits_in_the_slot_beside_its_manifest() -> Result<(), Box<dyn std::error::Error>> {
        let path = executable_surface_path(Path::new("/pkg/target/lib/thing.incnlib"), &["lib".to_string()])
            .ok_or("a manifest path with a parent must yield a surface path")?;

        assert_eq!(path, Path::new("/pkg/target/lib/semantic/lib.incnsem"));
        Ok(())
    }
}
