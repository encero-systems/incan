//! Explicit checked-output transition across the actual canonical lock publisher (#1337/#1698).
//!
//! Normal metadata identity continues to include lock bytes. Only the producer's retained exact writer proof can
//! authorize re-sealing unchanged checked outputs under a post-publication source recipe.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{MetadataPreparation, MetadataSourceSnapshot, current_source_snapshot, invalid};
use crate::build::library_metadata::{SelectedLibraryMetadata, publish_library_metadata_with_requirements};
use crate::build::native_sdk::NativeSdkPublicationContext;
use crate::build::source_authority::canonical_baked_project_lock_path;
use crate::error::CliResult;
use crate::lock::PublishedOvenProjectLock;
use crate::session::CompilationSession;
use oven_model::manifest::ProjectManifest;

/// Current producer inputs used by the same actual preparation observer before and after lock publication.
pub(crate) struct MetadataTransitionContext<'a> {
    pub project: &'a ProjectManifest,
    pub session: &'a CompilationSession,
    pub out_dir: &'a Path,
    pub manifest_path: &'a Path,
    pub native_sdk: Option<&'a NativeSdkPublicationContext<'a>>,
}

/// Complete original preparation/source/metadata leases retained over one exact canonical lock write.
pub(crate) struct MetadataLockTransition {
    preparation: MetadataPreparation,
    source: MetadataSourceSnapshot,
    lock: PathBuf,
    metadata: Arc<SelectedLibraryMetadata>,
}

impl MetadataLockTransition {
    /// Capture a source-current generation before the writer, refusing stale metadata or incomplete preparation.
    pub fn capture(
        preparation: MetadataPreparation,
        context: &MetadataTransitionContext<'_>,
        metadata: Arc<SelectedLibraryMetadata>,
    ) -> CliResult<Self> {
        preparation.revalidate(context.project, context.session, context.out_dir, context.native_sdk)?;
        verify_original_materialization(context, &metadata)?;
        if preparation.recipe != *metadata.recipe() {
            return Err(invalid(
                "lock transition original metadata differs from current complete preparation",
            ));
        }
        let lock = canonical_lock_coordinate(context.project.project_root())?;
        let features = context
            .session
            .package_feature_plan
            .as_ref()
            .ok_or_else(|| invalid("lock transition lacks original feature graph"))?;
        let source = current_source_snapshot(context.project.project_root(), features)?;
        if source.digest()? != preparation.recipe.source_digest {
            return Err(invalid("lock transition original source changed during capture"));
        }
        Ok(Self {
            preparation,
            source,
            lock,
            metadata,
        })
    }

    /// Reobserve full current authority and re-seal original checked outputs only for a proven exact lock transition.
    ///
    /// The candidate must come from the normal MetadataPreparation::observe with current session/native inputs.
    /// An unchanged recipe retains the original owner; this never reparses or rechecks the authored Incan source.
    pub fn finalize(
        self,
        candidate: MetadataPreparation,
        context: &MetadataTransitionContext<'_>,
        published: &PublishedOvenProjectLock,
    ) -> CliResult<Arc<SelectedLibraryMetadata>> {
        published.verify_published_file()?;
        if published.canonical_lock_path() != self.lock
            || canonical_lock_coordinate(context.project.project_root())? != self.lock
        {
            return Err(invalid(
                "lock transition publication changed canonical lock coordinates",
            ));
        }
        candidate.revalidate(context.project, context.session, context.out_dir, context.native_sdk)?;
        verify_original_materialization(context, &self.metadata)?;
        self.preparation.verify_native_authority()?;
        for dependency in &self.preparation.dependency_owners {
            dependency.verify()?;
        }
        let mut original = self.preparation.recipe.clone();
        original.source_digest = candidate.recipe.source_digest.clone();
        if original != candidate.recipe || self.preparation.delivery_coordinates != candidate.delivery_coordinates {
            let before = serde_json::to_value(&original).map_err(|error| invalid(error.to_string()))?;
            let after = serde_json::to_value(&candidate.recipe).map_err(|error| invalid(error.to_string()))?;
            let changed = before
                .as_object()
                .ok_or_else(|| invalid("lock transition recipe is not an object"))?
                .iter()
                .filter(|(key, value)| after.get(*key) != Some(*value))
                .map(|(key, value)| {
                    (
                        key.clone(),
                        serde_json::json!({"before": value, "after": after.get(key)}),
                    )
                })
                .collect::<std::collections::BTreeMap<_, _>>();
            return Err(invalid(format!(
                "lock transition changed non-source compiler/dependency/native/provider authority: {}",
                serde_json::json!({
                    "recipe_changes": changed,
                    "delivery_changed": self.preparation.delivery_coordinates != candidate.delivery_coordinates,
                })
            )));
        }
        let features = context
            .session
            .package_feature_plan
            .as_ref()
            .ok_or_else(|| invalid("lock transition lacks current feature graph"))?;
        let source = current_source_snapshot(context.project.project_root(), features)?;
        if source.digest()? != candidate.recipe.source_digest {
            return Err(invalid("lock transition source changed during final observation"));
        }
        validate_source_transition(&self.source, &source, &self.lock, published)?;
        if self.preparation.recipe == candidate.recipe {
            published.verify_published_file()?;
            return Ok(self.metadata);
        }
        let requirements = self
            .metadata
            .checked_requirements()
            .ok_or_else(|| invalid("lock transition cannot fabricate missing checked requirements"))?
            .clone();
        let selected = publish_library_metadata_with_requirements(
            &candidate.store,
            &candidate.recipe,
            &candidate.receipt,
            context.out_dir,
            context.manifest_path,
            requirements.rust_abi_queries.clone(),
            Some(requirements),
        )?;
        self.metadata.verify_same_checked_output(&selected)?;
        // Keep every original checked dependency lease, rather than adopting candidate substitutes or old plans.
        let selected = selected.retaining_dependencies(&self.preparation.dependency_owners)?;
        published.verify_published_file()?;
        candidate.revalidate(context.project, context.session, context.out_dir, context.native_sdk)?;
        Ok(selected)
    }
}

/// Validate the actual writer's source change through complete canonical snapshots, without a name-only exception.
fn validate_source_transition(
    original: &MetadataSourceSnapshot,
    current: &MetadataSourceSnapshot,
    lock: &Path,
    published: &PublishedOvenProjectLock,
) -> CliResult<()> {
    published.verify_published_file()?;
    if published.canonical_lock_path() != lock
        || !original.unchanged_except_published_lock(current, lock, published.published_content_digest())?
    {
        return Err(invalid(
            "lock transition changed inputs beyond the exact published canonical lock",
        ));
    }
    Ok(())
}

/// Bind the caller's exact publication path to the original full checked closure, including declared sidecars.
fn verify_original_materialization(
    context: &MetadataTransitionContext<'_>,
    metadata: &SelectedLibraryMetadata,
) -> CliResult<()> {
    metadata.verify_materialization(context.out_dir)?;
    let files = crate::build::output_paths::packaged_library_metadata_files(
        context.manifest_path,
        metadata.manifest(),
        context.out_dir,
    )?;
    if files != metadata.checked_files() {
        return Err(invalid(
            "lock transition selected a different checked manifest or output closure",
        ));
    }
    Ok(())
}

/// Normalize the actual project/workspace canonical lock without following a replacement file symlink.
pub(super) fn canonical_lock_coordinate(root: &Path) -> CliResult<PathBuf> {
    let requested = canonical_baked_project_lock_path(root)?;
    let parent = requested
        .parent()
        .ok_or_else(|| invalid("canonical lock has no parent"))?;
    let name = requested
        .file_name()
        .ok_or_else(|| invalid("canonical lock has no filename"))?;
    let lock = std::fs::canonicalize(parent)
        .map_err(|error| invalid(error.to_string()))?
        .join(name);
    match std::fs::symlink_metadata(&lock) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            Err(invalid("canonical metadata lock is not a regular file"))
        }
        Ok(_) => Ok(lock),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(lock),
        Err(error) => Err(invalid(error.to_string())),
    }
}

#[cfg(test)]
mod tests;
