//! Explicit vocabulary-production handoff for compiler-owned standard readiness (#1337, #1698).
//!
//! This removable bootstrap transport uses the existing SDK-native auxiliary producer until dev.8 removes that
//! producer. It grants no standard namespace or ordinary-command fallback: the ordinary publisher retains its own
//! source/dependency/native authorities, and this capability covers only the declared vocabulary's genuine output.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use incan_frontend::provider::source_policy::TrustedStandardSourcePublication;
use incan_provider::vocab_extraction::{LibraryVocabExtraction, collect_native_sdk_vocab_metadata};
use incan_provider::{ProviderPlan, SDK_SOURCE_CATALOG_FILE, SdkSourceCatalog};
use oven_model::manifest::ProjectManifest;
use oven_model::observed_inputs::InputObservations;
use oven_rustc::sdk_closure::SdkCompiledClosure;
use oven_store::{digest_bytes, project_source_tree_evidence};

use crate::build::native_loaf_plan::ConfiguredNativeLoafInputs;
use crate::error::{CliError, CliResult};

/// Genuine vocabulary output, original native/auxiliary leases and source/compiler observations for one publisher.
/// Construction is private to explicit standard readiness; a matching manifest or copied SDK inventory is insufficient.
pub(crate) struct PreparedStandardVocabulary {
    source: Arc<TrustedStandardSourcePublication>,
    declaration: serde_json::Value,
    standard_root: PathBuf,
    kernel_root: PathBuf,
    source_identity: String,
    observations: InputObservations,
    producer: String,
    rustc: PathBuf,
    toolchain: String,
    closure: SdkCompiledClosure,
    extraction: LibraryVocabExtraction,
    identity: String,
    _scratch: tempfile::TempDir,
}

impl PreparedStandardVocabulary {
    /// Compile the exact declared companion and auxiliary module through the existing native SDK producer.
    /// Current native graph selection is supplied explicitly by readiness; no ambient graph or namespace can replace
    /// it.
    pub(super) fn prepare(
        source: Arc<TrustedStandardSourcePublication>,
        manifest: &ProjectManifest,
        standard_root: &Path,
        graph: &Path,
        inputs: &ConfiguredNativeLoafInputs,
        rustc: &Path,
        output: &Path,
    ) -> CliResult<Self> {
        // ---- Original compiler-owned source and declaration ----
        let root = source.verified_package_root().map_err(failure)?;
        if root != manifest.project_root().canonicalize().map_err(failure)? {
            return Err(failure("standard vocabulary source differs from the admitted project"));
        }
        let declaration = pinned_vocabulary_declaration(&source, manifest)?;
        let standard_root = standard_root.canonicalize().map_err(failure)?;
        if root.parent() != Some(standard_root.as_path()) {
            return Err(failure(
                "standard vocabulary source is outside the retained standard tree",
            ));
        }
        let kernel_root = standard_root
            .parent()
            .ok_or_else(|| failure("standard tree has no Loaf owner"))?
            .join("kernel");
        let mut observations = InputObservations::default();
        let companion = root
            .join(
                manifest
                    .vocab()
                    .and_then(|vocab| vocab.crate_path.as_deref())
                    .ok_or_else(|| failure("vocabulary companion source is absent"))?,
            )
            .canonicalize()
            .map_err(failure)?;
        if !companion.starts_with(root) {
            return Err(failure("vocabulary companion escapes its compiler-owned package"));
        }
        for tree in [&companion, &kernel_root, &standard_root.join("derive")] {
            oven_store::project_source_tree_evidence_observed(tree, &mut |path| observations.observe(path))
                .map_err(failure)?;
        }
        observations.observe(&standard_root.join(SDK_SOURCE_CATALOG_FILE));
        observations.observe(&standard_root.join(incan_provider::sdk_native::VOCABULARY_DESUGARER_LOCK));
        let source_identity = source_identity(&standard_root, &kernel_root)?;
        let producer = crate::build::source_authority::current_compiler_identity_digest()?;
        let rustc = rustc.canonicalize().map_err(failure)?;
        observations.observe(&rustc);
        observations.observe(
            &std::env::current_exe()
                .map_err(failure)?
                .canonicalize()
                .map_err(failure)?,
        );
        let toolchain = oven_rustc::rustc::rustc_identity(&rustc).map_err(failure)?;
        require_original_observations(&observations)?;

        // ---- Genuine host and auxiliary producers, retaining their original leases ----
        let catalog =
            SdkSourceCatalog::read_from_path(&standard_root.join(SDK_SOURCE_CATALOG_FILE)).map_err(failure)?;
        let closure = incan_provider::sdk_native::prepare_sdk_native_closure(
            &standard_root,
            &catalog,
            &incan_provider::sdk_native::SdkNativeInputs {
                blobs: inputs.blobs.clone(),
                index: inputs.index.clone(),
                output: output.join("vocabulary-native"),
                rustc: rustc.clone(),
                compiler_graph: Some(graph.to_path_buf()),
            },
        )
        .map_err(failure)?;
        closure.verify_retained_outputs().map_err(failure)?;
        std::fs::create_dir_all(output).map_err(failure)?;
        let scratch = tempfile::Builder::new()
            .prefix("standard-vocabulary-")
            .tempdir_in(output)
            .map_err(failure)?;
        let extraction = collect_native_sdk_vocab_metadata(manifest, root, &closure, &rustc, scratch.path())
            .map_err(failure)?
            .ok_or_else(|| failure("declared standard vocabulary produced no metadata"))?;

        // ---- Bind genuine outputs before they may enter ordinary publication ----
        let mut prepared = Self {
            source,
            declaration,
            standard_root,
            kernel_root,
            source_identity,
            observations,
            producer,
            rustc,
            toolchain,
            closure,
            extraction,
            identity: String::new(),
            _scratch: scratch,
        };
        prepared.identity = prepared.current_identity()?;
        prepared.verify_for(manifest)?;
        Ok(prepared)
    }

    /// Recheck source, compiler, original host/auxiliary outputs and the produced desugarer before each handoff.
    pub(crate) fn verify(&self) -> CliResult<()> {
        require_original_observations(&self.observations)?;
        self.source.verify().map_err(failure)?;
        self.closure.verify_retained_outputs().map_err(failure)?;
        if source_identity(&self.standard_root, &self.kernel_root)? != self.source_identity
            || crate::build::source_authority::current_compiler_identity_digest()? != self.producer
            || oven_rustc::rustc::rustc_identity(&self.rustc).map_err(failure)? != self.toolchain
            || self.current_identity()? != self.identity
        {
            return Err(failure("prepared standard vocabulary authority changed"));
        }
        Ok(())
    }

    /// Refuse reuse for another package, source root, declaration or companion even if its public name matches.
    pub(super) fn verify_for(&self, manifest: &ProjectManifest) -> CliResult<()> {
        self.verify()?;
        if self.source.verified_package_root().map_err(failure)?
            != manifest.project_root().canonicalize().map_err(failure)?
            || pinned_vocabulary_declaration(&self.source, manifest)? != self.declaration
        {
            return Err(failure("prepared standard vocabulary does not cover this publication"));
        }
        Ok(())
    }

    /// Bind checked demand facts to the independently admitted own-source publisher, never an imported namespace.
    pub(crate) fn verified_declaration_for_plan(&self, plan: &ProviderPlan) -> CliResult<&serde_json::Value> {
        self.verify()?;
        let source = plan
            .standard_source_publication()
            .ok_or_else(|| failure("prepared vocabulary has no own-source publisher"))?;
        if source.verified_package_root().map_err(failure)? != self.source.verified_package_root().map_err(failure)?
            || source.verified_declaration_bytes().map_err(failure)?
                != self.source.verified_declaration_bytes().map_err(failure)?
        {
            return Err(failure("prepared vocabulary belongs to a different source publisher"));
        }
        Ok(&self.declaration)
    }

    /// Include the exact produced vocabulary and auxiliary closure in the ordinary metadata recipe and replay check.
    pub(super) fn semantic_identity(&self) -> CliResult<&str> {
        self.verify()?;
        Ok(&self.identity)
    }

    /// Borrow production results by value while this capability retains their original source and native leases.
    pub(super) fn extraction_for(&self, manifest: &ProjectManifest) -> CliResult<LibraryVocabExtraction> {
        self.verify_for(manifest)?;
        Ok(LibraryVocabExtraction {
            payload: self.extraction.payload.clone(),
            compatibility_activations: self.extraction.compatibility_activations.clone(),
            pending_desugarer_artifact: self.extraction.pending_desugarer_artifact.clone(),
        })
    }

    /// Bind all actual unit recipes and physical outputs, including separate auxiliary target/profile identities.
    fn current_identity(&self) -> CliResult<String> {
        let pending = self
            .extraction
            .pending_desugarer_artifact
            .as_ref()
            .map(|artifact| {
                let digest = digest_bytes(&std::fs::read(&artifact.source_path).map_err(failure)?);
                if digest.strip_prefix("sha256:") != Some(artifact.metadata.sha256.as_str()) {
                    return Err(failure(
                        "prepared vocabulary desugarer differs from its declared digest",
                    ));
                }
                Ok::<_, CliError>(serde_json::json!({ "metadata": artifact.metadata, "bytes": digest }))
            })
            .transpose()?;
        Ok(digest_bytes(
            &serde_json::to_vec(&serde_json::json!({
                "contract": "explicit-standard-vocabulary-production-v1",
                "source": self.source_identity, "declaration": self.declaration,
                "compiler": self.producer, "toolchain": self.toolchain,
                "closure": closure_identity(&self.closure)?,
                "vocabulary": self.extraction.payload.to_json_string().map_err(failure)?, "activations": self.extraction.compatibility_activations,
                "desugarer": pending,
            }))
            .map_err(failure)?,
        ))
    }
}

/// Admit only the retained compiler-pinned declaration before executing any companion code.
fn pinned_vocabulary_declaration(
    source: &TrustedStandardSourcePublication,
    manifest: &ProjectManifest,
) -> CliResult<serde_json::Value> {
    let root = source.verified_package_root().map_err(failure)?;
    let pinned = ProjectManifest::from_str(
        std::str::from_utf8(source.verified_declaration_bytes().map_err(failure)?).map_err(failure)?,
        &root.join("loaf.toml"),
    )
    .map_err(failure)?;
    let expected = serde_json::to_value(
        pinned
            .vocab()
            .ok_or_else(|| failure("pinned standard source declares no vocabulary"))?,
    )
    .map_err(failure)?;
    let (name, version) = source.package_identity();
    if manifest.project.as_ref().and_then(|project| project.name.as_deref()) != Some(name)
        || manifest.project.as_ref().and_then(|project| project.version.as_deref()) != Some(version)
        || serde_json::to_value(manifest.vocab()).map_err(failure)? != expected
        || manifest.project_root().canonicalize().map_err(failure)? != root
    {
        return Err(failure(
            "standard vocabulary request differs from its pinned declaration",
        ));
    }
    Ok(expected)
}

/// Reject a changed original input generation, including edit-and-restoration, before accepting produced output.
/// Unsupported platforms cannot promise this retained bootstrap handoff and therefore refuse it explicitly.
fn require_original_observations(observations: &InputObservations) -> CliResult<()> {
    if !observations.supports_retention() || !observations.current() {
        return Err(failure(
            "original vocabulary input observations changed or cannot be retained",
        ));
    }
    Ok(())
}

/// Bind the same registered source ownership projection as ordinary metadata plus the complete kernel tree.
/// Selected package recipes and original producer observations independently retain their canonical lock authority.
fn source_identity(standard_root: &Path, kernel_root: &Path) -> CliResult<String> {
    let standard = super::metadata_replay::ordinary_native::shared_standard_source_digest(standard_root)?;
    let kernel = project_source_tree_evidence(kernel_root)
        .map_err(failure)?
        .digest()
        .map_err(failure)?;
    Ok(digest_bytes(&serde_json::to_vec(&(standard, kernel)).map_err(failure)?))
}

/// Project original compiled identities recursively; a parent target cannot stand in for its auxiliary target.
fn closure_identity(closure: &SdkCompiledClosure) -> CliResult<serde_json::Value> {
    let units = closure
        .units()
        .iter()
        .map(|unit| unit.native_artifact())
        .collect::<Result<Vec<_>, _>>()
        .map_err(failure)?;
    let auxiliary = closure
        .auxiliary_targets()
        .iter()
        .map(|(target, closure)| Ok((target.clone(), closure_identity(closure)?)))
        .collect::<CliResult<std::collections::BTreeMap<_, _>>>()?;
    Ok(serde_json::json!({ "units": units, "auxiliary": auxiliary }))
}

/// Keep existing typed CLI failure handling at the explicit bootstrap transport boundary.
fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}

#[cfg(test)]
mod tests;
