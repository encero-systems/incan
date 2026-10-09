//! Namespace capabilities retained from an already validated immutable provider selection.
//!
//! Ordinary checked package admission may consume these grants, but package names and self-declared module claims
//! never mint reserved namespace authority. An ordinary installed-set producer remains the final replacement for
//! the existing inventory-backed grant producer (#1337/#1698).

use std::collections::BTreeSet;

use super::plan::{NamespaceAuthority, ProviderPlan, ProviderRecord, active_provider_claims};

/// Exact immutable reserved namespace selection, including its original provider and artifact coordinates.
#[derive(Debug, Clone)]
pub struct SelectedProviderNamespace {
    record: ProviderRecord,
}

impl SelectedProviderNamespace {
    /// Retain one enabled, materialized namespace grant from an existing validated provider plan.
    ///
    /// The caller must subsequently bind its manifest, artifact and exact package owner through ordinary admission.
    /// This compatibility producer does not authorize a new namespace or transplant a grant onto another artifact.
    pub fn from_plan(plan: &ProviderPlan, provider_identity: &str) -> Result<Self, String> {
        let record = plan
            .records()
            .find(|record| record.identity.stable_key() == provider_identity)
            .ok_or_else(|| "selected provider namespace identity is absent".to_string())?;
        if record.authority != NamespaceAuthority::SdkReserved
            || !record.available
            || !record.enabled
            || record.manifest.is_none()
            || record.artifact.is_none()
            || record.namespace_claims.is_empty()
        {
            return Err("selected provider namespace has no enabled materialized grant".into());
        }
        let manifest = record
            .manifest
            .as_deref()
            .ok_or_else(|| "selected namespace manifest is absent".to_string())?;
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
        Ok(Self { record: record.clone() })
    }

    /// Borrow the complete original selected record; consumers cannot rewrite its identity, claims or provenance.
    pub fn record(&self) -> &ProviderRecord {
        &self.record
    }
}
