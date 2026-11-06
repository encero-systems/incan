//! Explicit ordinary source capabilities; a serialized graph cannot replace their original retained owners.

use std::path::Path;

use super::{RustMetadataError, RustWorkspace, native_macros};

/// Generated-root marker requiring an explicitly admitted original producer capability for database loading.
pub const OVEN_RETAINED_INSPECTION_MARKER: &str = ".incan_retained_oven_inspection";

/// A compiler-owned complete producer request and its original source/output leases.
///
/// Implementations must revalidate the complete original request, source inventories, facts, physical edges and
/// compiler intent before returning its exact inspection graph. They must bind sysroot sources to that compiler. A
/// selected-root hint or deserialized path catalog cannot construct this authority. Macro execution is separate:
/// implementations must explicitly retain and verify their actual original host macro outputs.
pub trait RetainedInspectionProject: Send {
    /// Return the current exact source graph only after revalidating the original complete producer request.
    fn verified_project(&self) -> Result<serde_json::Value, RustMetadataError>;

    /// Return only original admitted host macro requests after verifying their complete producer inputs.
    /// Source-only capabilities supply no executable permission, even when a serialized macro sidecar is present.
    fn verified_native_macros(&self) -> Result<Vec<super::OvenInspectionProcMacro>, RustMetadataError> {
        Ok(Vec::new())
    }
}

impl RustWorkspace {
    /// Whether this database still owns the original producer capability required by its generated root.
    pub(crate) fn has_retained_project(&self) -> bool {
        self.retained_project.is_some()
    }

    /// Reuse this database only when its original complete producer still verifies to the exact incoming graph.
    pub(crate) fn matches_retained_project(
        &self,
        graph: &serde_json::Value,
        macros: &[super::OvenInspectionProcMacro],
    ) -> Result<bool, RustMetadataError> {
        match &self.retained_project {
            Some(project) => Ok(project.verified_project()? == *graph && project.verified_native_macros()? == macros),
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
        let mut graph = project.verified_project()?;
        if native_macros::contains_unowned_macro_path(&graph) {
            return Err(RustMetadataError::LoadWorkspace {
                path: manifest_dir.to_path_buf(),
                message: "ordinary source inspection does not grant native macro execution".to_string(),
            });
        }
        let macros = project.verified_native_macros()?;
        let owners = native_macros::select_retained_native_macros(manifest_dir, &mut graph, macros)?;
        Self::load_oven_graph(manifest_dir, target_dir, graph, owners, Some(project), progress)
    }

    /// Project compiler-bound source coordinates only after the caller verifies their original retained owner.
    ///
    /// This helper performs no compiler discovery. The retained producer capability remains responsible for source
    /// and target evidence; these paths alone never authorize loading an ordinary database.
    pub fn inspection_project_with_sources(
        mut graph: serde_json::Value,
        sysroot: &Path,
        source: &Path,
        cfg: &[String],
    ) -> Result<serde_json::Value, RustMetadataError> {
        if !sysroot.is_absolute() || !source.is_absolute() || !source.join("core/src/lib.rs").is_file() {
            return Err(RustMetadataError::LoadWorkspace {
                path: source.to_path_buf(),
                message: "ordinary inspection requires retained absolute compiler source coordinates".to_string(),
            });
        }
        let mut sysroot_project = super::sysroot_project_graph(source);
        if let Some(crates) = sysroot_project["crates"].as_array_mut() {
            for krate in crates {
                krate["cfg"] = serde_json::json!(cfg);
            }
        }
        graph["sysroot"] = serde_json::json!(sysroot);
        graph["sysroot_src"] = serde_json::json!(source);
        graph["sysroot_project"] = sysroot_project;
        Ok(graph)
    }
}
