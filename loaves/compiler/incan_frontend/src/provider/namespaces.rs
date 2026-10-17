//! Namespace capabilities retained from an already validated immutable provider selection.
//!
//! Ordinary checked package admission may consume these grants, but package names and self-declared module claims
//! never mint reserved namespace authority. Source publication retains genuine compiler-pinned ownership; public
//! inventory descriptors are not issuer evidence. Source-free installed authentication remains a caller gate
//! (#1337/#1698).

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use super::source_policy::TrustedStandardSourcePublication;
use crate::library_manifest::digest_provider_artifact;
use crate::library_manifest_index::{
    LibraryManifestIndexEntry, dependency_project_root, load_provider_dependency_artifact,
};

use super::plan::{
    NamespaceAuthority, ProviderIdentity, ProviderPlan, ProviderProvenance, ProviderRecord, active_provider_claims,
    implementation_facets,
};

/// Exact immutable reserved namespace selection, including its original provider and artifact coordinates.
#[derive(Debug, Clone)]
pub struct SelectedProviderNamespace {
    issuer: Arc<ProviderNamespaceIssuer>,
}

/// Private publication issuer retains its original compiler-owned source alongside the immutable package record.
#[derive(Debug)]
pub(super) struct ProviderNamespaceIssuer {
    record: ProviderRecord,
    source: Arc<TrustedStandardSourcePublication>,
}

impl SelectedProviderNamespace {
    /// Retain one enabled, materialized namespace grant from an existing validated provider plan.
    ///
    /// The caller must subsequently bind its manifest, artifact and exact package owner through ordinary admission.
    /// This transfer does not issue authority. Public records and inventories cannot create its private issuer;
    /// the retained source-publication issuer supplies that capability without an SDK inventory.
    pub fn from_plan(plan: &ProviderPlan, provider_identity: &str) -> Result<Self, String> {
        let issuer = plan
            .namespace_issuer(provider_identity)
            .ok_or_else(|| "selected provider namespace has no retained producer issuer".to_string())?;
        let selected = Self {
            issuer: Arc::clone(issuer),
        };
        selected.verify()?;
        Ok(selected)
    }

    /// Bind an ordinary publication to its genuine compiler-owned source and exact published artifact.
    ///
    /// Source policy grants only owned standard namespace roots. The admission caller must independently retain
    /// the published checked metadata and generation owners; a manifest or this namespace grant cannot replace them.
    pub fn from_source_publication(
        source: Arc<TrustedStandardSourcePublication>,
        artifact_root: &Path,
    ) -> Result<Self, String> {
        let root = std::fs::canonicalize(artifact_root).map_err(|error| error.to_string())?;
        let source_root = source.verified_package_root().map_err(|error| error.to_string())?;
        if dependency_project_root(&root).as_deref() != Some(source_root) {
            return Err("standard namespace publication is outside its original compiler-owned source".into());
        }
        let (package_name, _) = source.package_identity();
        let (manifest, artifact) = match load_provider_dependency_artifact(package_name, &root) {
            LibraryManifestIndexEntry::Loaded { manifest, metadata } => (manifest, metadata),
            LibraryManifestIndexEntry::Failed(failure) => return Err(failure.message),
        };
        let active_features = manifest.contract_metadata.provider.active_features.clone();
        let namespace_claims = active_provider_claims(&manifest, &active_features)
            .into_iter()
            .map(|claim| std::iter::once("std".to_string()).chain(claim).collect())
            .collect();
        let record = ProviderRecord {
            identity: ProviderIdentity {
                name: manifest.name.clone(),
                version: manifest.version.clone(),
                digest: digest_provider_artifact(&root).map_err(|error| error.to_string())?,
                feature_projection: active_features.clone(),
            },
            provenance: ProviderProvenance::StandardPublication {
                package_name: manifest.name.clone(),
                manifest_path: source_root.join("loaf.toml"),
            },
            authority: NamespaceAuthority::SdkReserved,
            namespace_claims,
            available: true,
            enabled: true,
            implementation_facets: implementation_facets(&manifest, &active_features),
            manifest: Some(Arc::from(manifest)),
            artifact: Some(artifact),
        };
        let selected = Self {
            issuer: Arc::new(ProviderNamespaceIssuer { record, source }),
        };
        selected.verify()?;
        Ok(selected)
    }

    /// Revalidate original publication source and immutable claims; checked package owners remain admission's job.
    pub fn verify(&self) -> Result<(), String> {
        let record = self.record();
        if record.authority != NamespaceAuthority::SdkReserved
            || !record.available
            || !record.enabled
            || record.namespace_claims.is_empty()
        {
            return Err("selected provider namespace has no enabled materialized grant".into());
        }
        let manifest = record
            .manifest
            .as_deref()
            .ok_or_else(|| "selected namespace manifest is absent".to_string())?;
        let artifact = record
            .artifact
            .as_ref()
            .ok_or_else(|| "selected namespace artifact is absent".to_string())?;
        let expected_claims: BTreeSet<Vec<String>> =
            active_provider_claims(manifest, &record.identity.feature_projection)
                .into_iter()
                .map(|claim| std::iter::once("std".to_string()).chain(claim).collect())
                .collect();
        if record.namespace_claims != expected_claims
            || record.identity.name != manifest.name
            || record.identity.version != manifest.version
            || record.identity.feature_projection != manifest.contract_metadata.provider.active_features
        {
            return Err("selected namespace differs from its checked package claims or identity".into());
        }
        let roots = expected_claims
            .iter()
            .filter_map(|claim| claim.get(1).cloned())
            .collect();
        self.issuer
            .source
            .validate_namespace_roots(&manifest.name, &manifest.version, &roots)
            .map_err(|error| error.to_string())?;
        if dependency_project_root(&artifact.crate_root).as_deref()
            != Some(
                self.issuer
                    .source
                    .verified_package_root()
                    .map_err(|error| error.to_string())?,
            )
        {
            return Err("selected namespace lost its original compiler-owned publication coordinate".into());
        }
        Ok(())
    }

    /// Borrow the complete original selected record; consumers cannot rewrite its identity, claims or provenance.
    pub fn record(&self) -> &ProviderRecord {
        &self.issuer.record
    }

    /// Retain an original dependency source while compiling another genuine standard source publisher.
    ///
    /// Standard trait defaults are expanded into adopters by lowering. This internal transfer requires both the
    /// publisher's original source publication and this namespace's retained issuer.
    pub(crate) fn source_for_standard_publisher(
        &self,
        publisher: &TrustedStandardSourcePublication,
    ) -> Result<Arc<TrustedStandardSourcePublication>, String> {
        publisher.verify().map_err(|error| error.to_string())?;
        self.verify()?;
        Ok(Arc::clone(&self.issuer.source))
    }

    /// Retain an original dependency source while compiling an ordinary admitted consumer.
    ///
    /// Lowering expands the dependency's trait defaults and constructor hooks into the consumer's adopters exactly
    /// as into a publisher's, so the consumer reads the same retained issuer source and nothing ambient. The grant
    /// is still verified on every transfer; a consumer has no own publication to cross-check.
    pub(crate) fn source_for_admitted_consumer(&self) -> Result<Arc<TrustedStandardSourcePublication>, String> {
        self.verify()?;
        Ok(Arc::clone(&self.issuer.source))
    }

    /// Retain the original private issuer capability when constructing another immutable admitted plan.
    pub(super) fn issuer(&self) -> &Arc<ProviderNamespaceIssuer> {
        &self.issuer
    }
}
