//! Producer-side vocab publication boundary for `incan build --lib`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::build::OvenDirectRustcPlanSelection;
use crate::cli::{CliError, CliResult};
use crate::library_manifest::{SoftKeywordActivation, VocabDesugarerArtifact, VocabExports};
use crate::manifest::ProjectManifest;
use crate::oven::compiler_suite_env::{
    OvenCompilerSuiteVocabCapability, OvenVocabSupportClosure, OvenVocabSupportFile,
};

/// A receipt-bound closure retained for the producer-owned vocabulary native-compilation carrier.
///
/// A normal Oven library build derives this from its selected plan and keeps that plan's lease through publication.
/// The carrier must consume these admitted files and compiler identity directly; consumers and build commands may not
/// reconstruct compilation arguments from a Cargo manifest.
pub(crate) struct OvenVocabDirectRustcContext<'a> {
    capability: OvenCompilerSuiteVocabCapability,
    /// Keep the actual selected store/Loaf owner alive through producer publication.
    _selected_plan: Option<&'a OvenDirectRustcPlanSelection>,
}

pub(crate) struct LibraryVocabExtraction {
    pub(crate) payload: VocabExports,
    pub(crate) compatibility_activations: Vec<SoftKeywordActivation>,
    pub(crate) pending_desugarer_artifact: Option<PendingDesugarerArtifact>,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingDesugarerArtifact {
    pub(crate) metadata: VocabDesugarerArtifact,
    pub(crate) source_path: PathBuf,
}

/// Collect producer-owned vocab metadata for packaging a library artifact.
///
/// The former implementation interpreted the companion's `Cargo.toml`, planned standalone `rustc` commands and kept
/// a cache private to this module. Those were competing build and identity authorities. Until the selected Oven plan
/// publishes the native-compilation result through the RFC 123 carrier, a vocab-bearing producer must fail closed.
pub(crate) fn collect_library_vocab_metadata(
    manifest: &ProjectManifest,
    _project_root: &Path,
    selected_context: Option<&OvenVocabDirectRustcContext<'_>>,
) -> CliResult<Option<LibraryVocabExtraction>> {
    if manifest.vocab().is_none() {
        return Ok(None);
    }
    let scheduler_context = if selected_context.is_none() {
        oven_compiler_suite_rustc_context()?
    } else {
        None
    };
    let selected_context = selected_context.or(scheduler_context.as_ref()).ok_or_else(|| {
        CliError::failure("vocabulary publication requires a selected Oven native-compilation context")
    })?;
    let _selected_input_identity = selected_context
        .capability
        .verified_cache_identity("producer-vocab-publication")
        .map_err(CliError::failure)?;
    Err(CliError::failure(
        "vocabulary publication requires producer-published native-compilation metadata; the selected Oven plan does not yet carry that result",
    ))
}

/// Load the admitted vocabulary capability exported by a compiler-suite parent.
fn oven_compiler_suite_rustc_context() -> CliResult<Option<OvenVocabDirectRustcContext<'static>>> {
    OvenCompilerSuiteVocabCapability::from_environment()
        .map_err(CliError::failure)
        .map(|capability| {
            capability.map(|capability| OvenVocabDirectRustcContext {
                capability,
                _selected_plan: None,
            })
        })
}

/// Borrow the selected plan's owner and bind its declared helper files to the already materialized auxiliary paths.
///
/// A missing helper is a missing selection, not permission to open another base Loaf. The borrowed plan retains the
/// actual store lease or generation lock until the producer publishes its vocabulary metadata.
pub(crate) fn oven_vocab_direct_rustc_context_from_plan<'a>(
    rustc: &Path,
    selection: &'a OvenDirectRustcPlanSelection,
) -> CliResult<OvenVocabDirectRustcContext<'a>> {
    let artifacts = selection.artifacts();
    let artifact_root = selection
        .vocab_artifact_root()
        .ok_or_else(|| CliError::failure("selected vocabulary helper is missing or spans unsupported artifact roots"))?
        .canonicalize()
        .map_err(|error| CliError::failure(error.to_string()))?;
    let mut auxiliary_targets = BTreeMap::new();
    for auxiliary in &artifacts.vocab_auxiliary_targets {
        let plan = artifacts
            .materialize_trusted_vocab_auxiliary_target(&artifact_root, &auxiliary.target)
            .map_err(|error| CliError::failure(error.to_string()))?
            .ok_or_else(|| {
                CliError::failure(format!(
                    "selected vocabulary helper lacks target `{}`",
                    auxiliary.target
                ))
            })?;
        let files = artifacts
            .composition_artifacts()
            .map_err(|error| CliError::failure(error.to_string()))?
            .into_iter()
            .map(|file| OvenVocabSupportFile {
                label: file.relative_path.clone(),
                path: artifact_root.join(&file.relative_path),
                digest: file.digest,
            });
        let closure = OvenVocabSupportClosure::from_selected_files(
            plan.dependency_search_paths,
            plan.externs.into_iter().collect(),
            files,
        )
        .map_err(CliError::failure)?;
        if auxiliary_targets.insert(auxiliary.target.clone(), closure).is_some() {
            return Err(CliError::failure(format!(
                "selected vocabulary helper repeats target `{}`",
                auxiliary.target
            )));
        }
    }
    let host = auxiliary_targets.remove(&artifacts.intent.target).ok_or_else(|| {
        CliError::failure(format!(
            "selected plan lacks vocabulary helper target `{}`",
            artifacts.intent.target
        ))
    })?;
    let capability = OvenCompilerSuiteVocabCapability::new(
        OvenVocabSupportFile::selected_compiler(rustc.to_path_buf()).map_err(CliError::failure)?,
        artifacts.intent.clone(),
        host,
        auxiliary_targets,
    );
    Ok(OvenVocabDirectRustcContext {
        capability,
        _selected_plan: Some(selection),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn selected_vocab_context_fixture(
        root: &Path,
    ) -> Result<OvenVocabDirectRustcContext<'static>, Box<dyn std::error::Error>> {
        let directory = root.join("selected-support");
        fs::create_dir_all(&directory)?;
        let mut files = Vec::new();
        let mut externs = BTreeMap::new();
        for name in ["incan_vocab", "serde_json"] {
            let path = directory.join(format!("lib{name}.rlib"));
            fs::write(&path, name)?;
            files.push(OvenVocabSupportFile {
                label: name.to_string(),
                path: path.clone(),
                digest: crate::oven::digest_bytes(name.as_bytes()),
            });
            externs.insert(name.to_string(), path);
        }
        let rustc = crate::oven::rustc::resolve_active_rustc()?;
        let intent = crate::oven::OvenBuildIntent {
            target: crate::oven::rustc::rustc_host_target(&rustc)?,
            toolchain: crate::oven::rustc::rustc_identity(&rustc)?,
            profile: "debug".to_string(),
            features: Vec::new(),
        };
        Ok(OvenVocabDirectRustcContext {
            capability: OvenCompilerSuiteVocabCapability::new(
                OvenVocabSupportFile::selected_compiler(rustc)?,
                intent,
                OvenVocabSupportClosure {
                    dependency_search_paths: vec![directory],
                    externs,
                    files,
                },
                BTreeMap::new(),
            ),
            _selected_plan: None,
        })
    }

    #[test]
    fn vocab_free_library_needs_no_native_compilation_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let manifest = ProjectManifest::from_str(
            "[project]\nname = \"plain_library\"\n",
            &project.path().join("loaf.toml"),
        )?;

        assert!(collect_library_vocab_metadata(&manifest, project.path(), None)?.is_none());
        Ok(())
    }

    #[test]
    fn vocab_publication_refuses_without_a_producer_native_compilation_context()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let manifest = ProjectManifest::from_str(
            "[project]\nname = \"vocab_library\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
            &project.path().join("loaf.toml"),
        )?;

        let error = collect_library_vocab_metadata(&manifest, project.path(), None)
            .err()
            .ok_or("vocab publication unexpectedly succeeded")?;
        assert!(error.message.contains("selected Oven native-compilation context"));
        Ok(())
    }

    #[test]
    fn vocabulary_context_keeps_the_selected_store_owner_until_publication_finishes()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::oven::rustc::{
            OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenRustcArtifactExtern, OvenRustcArtifactManifest,
            OvenRustcAuxiliaryTarget,
        };
        use crate::oven::store::{
            OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, OvenStoreLimits,
        };

        let root = tempfile::tempdir()?;
        let fixture = selected_vocab_context_fixture(root.path())?;
        let generated = root.path().join("generated.rs");
        fs::write(&generated, "pub fn fixture() {}")?;
        let receipt = crate::oven::receipt_generated_project(
            &crate::oven::OvenGeneratedProjectRequest::new(
                root.path(),
                "fixture",
                "0.1.0",
                fixture.capability.intent.target.clone(),
                fixture.capability.intent.toolchain.clone(),
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated),
        )?;
        let helper_externs = fixture
            .capability
            .host
            .files
            .iter()
            .map(|file| {
                Ok(OvenRustcArtifactExtern {
                    crate_name: file.label.clone(),
                    relative_path: format!(
                        "helper/{}",
                        file.path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .ok_or("filename absent")?
                    ),
                    digest: file.digest.clone(),
                })
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        let materialized_files = fixture
            .capability
            .host
            .files
            .iter()
            .zip(&helper_externs)
            .map(|(file, external)| OvenArtifactMaterializedFile {
                source_path: file.path.clone(),
                relative_path: external.relative_path.clone(),
            })
            .collect();
        let artifacts = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: BTreeMap::new(),
            supporting_artifacts: Vec::new(),
            vocab_auxiliary_targets: vec![OvenRustcAuxiliaryTarget {
                target: receipt.intent.target.clone(),
                dependency_search_paths: vec!["helper".to_string()],
                externs: helper_externs,
            }],
        };
        let store = OvenStore::new(
            root.path().join("store"),
            OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000),
        );
        store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "vocab-fixture".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&artifacts)?,
            materialized_files,
        })?;
        let selection = OvenDirectRustcPlanSelection::Stored(Box::new(
            super::super::build::select_receipt_direct_rustc_execution_plan(&store, &receipt)?
                .ok_or("selected plan absent")?,
        ));
        let context = oven_vocab_direct_rustc_context_from_plan(&fixture.capability.rustc.path, &selection)?;
        let pruner = OvenStore::new(root.path().join("store"), OvenStoreLimits::new(1, 1, 1));
        pruner.prune()?;
        assert!(store.inspect()?.active_lease_physical_bytes > 0);
        context.capability.rustc.verify()?;
        for file in &context.capability.host.files {
            file.verify()?;
        }
        drop(context);
        drop(selection);
        assert_eq!(store.inspect()?.active_lease_physical_bytes, 0);
        pruner.prune()?;
        assert!(store.manifests_for_selection()?.is_empty());
        Ok(())
    }
}
