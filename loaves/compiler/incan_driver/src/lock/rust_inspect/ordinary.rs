//! Rust metadata preparation from original ordinary native producers, independent of SDK inventories.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use oven_rustc::rustc::{OvenRustInspectionToolchain, prepare_rust_inspection_toolchain};

use oven_rustc::native_loaf::{NativeLoafInspectionInputs, NativeLoafRequestObservation, NativeLoafRoot};
use oven_store::OvenBuildIntent;
use rust_inspect::{RetainedInspectionProject, RustMetadataError, RustWorkspace};

use crate::error::{CliError, CliResult};
use crate::lock::OvenRustInspectSourceAuthorityRequest;

/// Original complete request and source owners, kept inside the inspector's cached database.
struct OrdinaryInspectionProject {
    observation: NativeLoafRequestObservation,
    toolchain: Arc<OvenRustInspectionToolchain>,
    inputs: NativeLoafInspectionInputs,
    roots: Vec<NativeLoafRoot>,
    rustc: PathBuf,
    intent: OvenBuildIntent,
    manifest_dir: PathBuf,
    /// Expected generated root from the admitted derive request, independent of mutable on-disk bytes.
    consumer_source: String,
    /// Exact macro coordinates derived from the retained native units, never from a caller's sidecar.
    native_macros: Vec<rust_inspect::OvenInspectionProcMacro>,
}

impl RetainedInspectionProject for OrdinaryInspectionProject {
    /// Recheck the complete request and every original source/fact before projecting the bound compiler's sysroot.
    fn verified_project(&self) -> Result<serde_json::Value, RustMetadataError> {
        self.observation
            .verify_intent(&self.rustc, &self.intent)
            .map_err(|error| invalid(&self.manifest_dir, error))?;
        let mut graph = self
            .inputs
            .inspection_project()
            .map_err(|error| invalid(&self.manifest_dir, error))?;
        // Bind the consumer's namespace to exact records, even when host or transitive crates share names.
        let consumer_source = self.manifest_dir.join("src/main.rs");
        if std::fs::read(&consumer_source).map_err(|error| invalid(&self.manifest_dir, error))?
            != self.consumer_source.as_bytes()
        {
            return Err(invalid(
                &self.manifest_dir,
                "ordinary inspection consumer source changed",
            ));
        }
        let dependencies = self
            .roots
            .iter()
            .map(|root| {
                let index = self
                    .inputs
                    .units()
                    .keys()
                    .position(|identity| identity == &root.record_identity)
                    .ok_or_else(|| {
                        invalid(
                            &self.manifest_dir,
                            "ordinary inspection root is outside its retained closure",
                        )
                    })?;
                Ok(serde_json::json!({"crate": index, "name": root.alias.replace('-', "_")}))
            })
            .collect::<Result<Vec<_>, RustMetadataError>>()?;
        let crates = graph
            .get_mut("crates")
            .and_then(serde_json::Value::as_array_mut)
            .ok_or_else(|| invalid(&self.manifest_dir, "ordinary inspection graph has no crate array"))?;
        // No display name: this generated root contributes dependency aliases, not a new public crate namespace.
        crates.push(serde_json::json!({
            "root_module": consumer_source, "edition": "2024", "deps": dependencies,
            "cfg": [], "env": {}, "is_workspace_member": true, "is_proc_macro": false,
        }));
        self.toolchain
            .verify_compiler(&self.rustc, &self.intent.target)
            .map_err(|error| invalid(&self.manifest_dir, error))?;
        RustWorkspace::inspection_project_with_sources(
            graph,
            self.toolchain.sysroot(),
            self.toolchain.source_root(),
            self.toolchain.cfg(),
        )
    }

    /// Supply the exact original native macro requests only after full producer and compiler revalidation.
    fn verified_native_macros(&self) -> Result<Vec<rust_inspect::OvenInspectionProcMacro>, RustMetadataError> {
        self.verified_project()?;
        Ok(self.native_macros.clone())
    }
}

/// Prepare a source-only inspection database from one complete native request and exact declared query roots.
/// Macro queries and derive expansion consume only original host macro outputs from those roots' actual forward native
/// closure. The existing macro loader independently verifies their source, receipt, bytes, host and compiler before
/// use; a query spelling grants no execution authority.
#[allow(
    clippy::too_many_arguments,
    reason = "Retains explicit original native, compiler and source-owner inputs at one semantic boundary"
)]
pub(super) fn prepare(
    manifest_dir: &Path,
    target_dir: &Path,
    project_root: &Path,
    observation: &NativeLoafRequestObservation,
    rustc: &Path,
    authority: &OvenRustInspectSourceAuthorityRequest<'_>,
    queries: &[String],
    derives: &[String],
    retained_toolchain: Option<&Arc<OvenRustInspectionToolchain>>,
) -> CliResult<Arc<OvenRustInspectionToolchain>> {
    let intent = OvenBuildIntent {
        target: authority.target.to_string(),
        toolchain: authority.toolchain.to_string(),
        profile: authority.profile.to_string(),
        features: authority.features.to_vec(),
    };
    let digest = observation.verify_intent(rustc, &intent).map_err(failure)?.to_string();
    let roots = observation
        .graph()
        .select_dependency_roots(authority.registry_dependencies, project_root, "target")
        .map_err(failure)?;
    // The original request remains complete and retained below; the query database follows only actual declared
    // roots and their exact forward edges, so unrelated versions cannot compete for a caller's crate spelling.
    let inputs = observation
        .graph()
        .select(&roots)
        .map_err(failure)?
        .inspection_inputs()
        .map_err(failure)?;
    for derive in derives {
        let name = derive.split("::").next().unwrap_or_default();
        let root = roots
            .iter()
            .find(|root| root.alias.replace('-', "_") == name)
            .ok_or_else(|| failure(format!("ordinary derive `{derive}` has no declared native root")))?;
        let forward = observation
            .graph()
            .select(std::slice::from_ref(root))
            .map_err(failure)?;
        if !forward
            .inspection_inputs()
            .map_err(failure)?
            .units()
            .values()
            .any(|unit| unit.is_proc_macro())
        {
            return Err(failure(format!(
                "ordinary derive `{derive}` has no original host macro output"
            )));
        }
    }
    for query in queries {
        let name = query.split("::").next().unwrap_or_default();
        if matches!(name, "std" | "core" | "alloc") {
            continue;
        }
        let root = roots
            .iter()
            .find(|root| root.alias.replace('-', "_") == name)
            .ok_or_else(|| {
                failure(format!(
                    "ordinary inspection query `{query}` has no declared native root"
                ))
            })?;
        if !inputs.units().contains_key(&root.record_identity) {
            return Err(failure(format!(
                "ordinary inspection query `{query}` has no retained declared root"
            )));
        }
    }
    let toolchain = match retained_toolchain {
        Some(toolchain) => Arc::clone(toolchain),
        None => Arc::new(
            prepare_rust_inspection_toolchain(&crate::oven_store::open_default_oven_store()?, rustc, authority.target)
                .map_err(failure)?,
        ),
    };
    toolchain.verify_compiler(rustc, authority.target).map_err(failure)?;
    let mut macros = inputs
        .units()
        .values()
        .filter(|unit| unit.is_proc_macro())
        .map(|unit| {
            let selected = unit.selected();
            let record = selected.record();
            if record.source.domain != "host" {
                return Err(failure("ordinary native macro belongs to a non-host domain"));
            }
            let (owner, _) = selected.plan_members("macro").map_err(failure)?;
            Ok(rust_inspect::OvenInspectionProcMacro {
                root_module: unit.root_module().to_path_buf(),
                store: owner.store,
                identity: record.native.identity.clone(),
                receipt_identity: record.native.receipt_identity.clone(),
                relative_path: record.native.relative_path.clone(),
                digest: record.native.digest.clone(),
            })
        })
        .collect::<CliResult<Vec<_>>>()?;
    macros.sort_by(|left, right| left.root_module.cmp(&right.root_module));
    // This sidecar is diagnostic only for retained loading; the original capability supplies execution requests.
    rust_inspect::write_oven_inspection_proc_macro_authority(manifest_dir, macros.clone()).map_err(failure)?;
    let project = OrdinaryInspectionProject {
        toolchain: Arc::clone(&toolchain),
        observation: observation.clone(),
        inputs,
        roots,
        rustc: rustc.to_path_buf(),
        intent,
        manifest_dir: manifest_dir.to_path_buf(),
        consumer_source: crate::rust_inspect_workspace::direct_rust_inspect_probe_source(derives),
        native_macros: macros,
    };
    let graph = project.verified_project().map_err(failure)?;
    let sources = project
        .inputs
        .units()
        .values()
        .map(|unit| {
            let selected = unit.selected();
            let record = selected.record();
            let source_root = unit.source_root().map_err(failure)?;
            Ok(rust_inspect::OvenInspectionRegistrySource {
                package: record
                    .source
                    .loaf
                    .strip_prefix("crates-io/")
                    .unwrap_or(&record.source.loaf)
                    .to_string(),
                version: record.source.version.clone(),
                registry: "loaf+native".to_string(),
                checksum: record.source.archive_digest.clone(),
                features: record.source.features.clone(),
                source_digest: oven_store::digest_source_tree(&source_root).map_err(failure)?,
                source_root,
            })
        })
        .collect::<CliResult<Vec<_>>>()?;
    rust_inspect::write_sealed_oven_inspection_source_authority(manifest_dir, sources).map_err(failure)?;
    std::fs::write(
        manifest_dir.join(rust_inspect::OVEN_DIRECT_LOAF_PROJECT_FILE),
        serde_json::to_vec(&graph).map_err(failure)?,
    )
    .map_err(failure)?;
    std::fs::write(
        manifest_dir.join(rust_inspect::OVEN_LOAF_ONLY_INSPECTION_MARKER),
        b"1\n",
    )
    .map_err(failure)?;
    std::fs::write(manifest_dir.join(rust_inspect::OVEN_RETAINED_INSPECTION_MARKER), digest).map_err(failure)?;
    // Only the retained loader can load this root; its original producer and macro coordinates remain verified.
    rust_inspect::RustMetadataCache::new()
        .prepare_retained_project(manifest_dir, target_dir, Box::new(project), &|_| {})
        .map(|_| toolchain)
        .map_err(failure)
}

/// Preserve the generated projection as the diagnostic coordinate for a producer refusal.
fn invalid(path: &Path, error: impl std::fmt::Display) -> RustMetadataError {
    RustMetadataError::LoadWorkspace {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}

/// Preserve native producer and inspector errors in the compiler command's error domain.
fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}

