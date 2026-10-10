//! Explicit ordinary source capabilities; a serialized graph cannot replace their original retained owners.

use std::path::Path;

use super::{RustMetadataError, RustWorkspace, native_macros};

/// Generated-root marker requiring an explicitly admitted original producer capability for database loading.
pub const OVEN_RETAINED_INSPECTION_MARKER: &str = ".incan_retained_oven_inspection";

/// A compiler-owned complete producer request and its original source/output leases.
///
/// Implementations must revalidate the complete original request, source inventories, facts, physical edges and
/// compiler intent before returning its exact inspection graph. They must bind sysroot sources to that compiler. A
/// selected-root hint or deserialized path catalog cannot construct this authority. Macro execution is separate: this
/// initial loader admits source graphs without executable macro paths or a macro server.
pub trait RetainedInspectionProject: Send {
    /// Return the current exact source graph only after revalidating the original complete producer request.
    fn verified_project(&self) -> Result<serde_json::Value, RustMetadataError>;
}

impl RustWorkspace {
    /// Whether this database still owns the original producer capability required by its generated root.
    pub(crate) fn has_retained_project(&self) -> bool {
        self.retained_project.is_some()
    }

    /// Reuse this database only when its original complete producer still verifies to the exact incoming graph.
    pub(crate) fn matches_retained_project(&self, graph: &serde_json::Value) -> Result<bool, RustMetadataError> {
        match &self.retained_project {
            Some(project) => Ok(project.verified_project()? == *graph),
            None => Ok(false),
        }
    }

    /// Load an ordinary source graph and keep its original producer alive through every database query.
    ///
    /// The graph is checked again after rust-analyzer loads its source snapshot. Serialized SDK/source/macro authority
    /// files are never read by this entry, and executable macro paths are refused at every graph level.
    pub fn load_retained_oven_project(
        manifest_dir: &Path,
        target_dir: &Path,
        project: Box<dyn RetainedInspectionProject>,
        progress: &(dyn Fn(String) + Sync),
    ) -> Result<Self, RustMetadataError> {
        let graph = project.verified_project()?;
        if native_macros::contains_unowned_macro_path(&graph) {
            return Err(RustMetadataError::LoadWorkspace {
                path: manifest_dir.to_path_buf(),
                message: "ordinary source inspection does not grant native macro execution".to_string(),
            });
        }
        Self::load_oven_graph(manifest_dir, target_dir, graph, Vec::new(), Some(project), progress)
    }

    /// Add the explicit compiler's sysroot graph without ambient compiler discovery or Cargo metadata.
    pub fn inspection_project_with_compiler(
        mut graph: serde_json::Value,
        rustc: &Path,
    ) -> Result<serde_json::Value, RustMetadataError> {
        let output = std::process::Command::new(rustc)
            .args(["--print", "sysroot"])
            .output()?;
        if !output.status.success() {
            return Err(RustMetadataError::LoadWorkspace {
                path: rustc.to_path_buf(),
                message: "ordinary inspection compiler could not report its sysroot".to_string(),
            });
        }
        let text = std::str::from_utf8(&output.stdout).map_err(|error| RustMetadataError::LoadWorkspace {
            path: rustc.to_path_buf(),
            message: error.to_string(),
        })?;
        let reported = Path::new(text.trim());
        if !reported.is_absolute() {
            return Err(RustMetadataError::LoadWorkspace {
                path: rustc.to_path_buf(),
                message: "ordinary inspection compiler reported a non-absolute sysroot".to_string(),
            });
        }
        let sysroot = reported.canonicalize()?;
        let source = sysroot.join("lib/rustlib/src/rust/library");
        if !source.join("core/src/lib.rs").is_file() {
            return Err(RustMetadataError::LoadWorkspace {
                path: source,
                message: "ordinary inspection requires the explicit compiler's Rust library sources".to_string(),
            });
        }
        graph["sysroot"] = serde_json::json!(sysroot);
        graph["sysroot_src"] = serde_json::json!(source);
        graph["sysroot_project"] = super::sysroot_project_graph(&source);
        Ok(graph)
    }
}
