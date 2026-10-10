//! Producer-owned physical native edges captured from actual preparation, never from a publication catalog.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use oven_store::OvenReceipt;
use oven_store::store::{OvenArtifactKind, OvenStoreExecutionPayload};
use serde::Serialize;

use super::{Error, SdkCompiledUnit, SdkLockedUnit, SdkNativeArtifact};

/// One exact dependency selected by authoritative native preparation (#1337, #1698).
///
/// Fields are immutable outside this producer. Serialization supports a future sealed graph, but arbitrary
/// descriptors cannot be deserialized into producer authority. Output equality never supplies destination identity.
#[derive(Clone, Debug, Serialize)]
pub struct SdkPhysicalNativeEdge {
    /// Normalized Rust extern alias actually passed to the compiler.
    alias: String,
    /// Original Store containing this destination's retained execution owner.
    store: PathBuf,
    /// Complete source binding and receipt-bound native output coordinates.
    destination: SdkNativeArtifact,
}

impl SdkPhysicalNativeEdge {
    /// Borrow the compiler's exact extern spelling, including dependency renames.
    pub fn alias(&self) -> &str {
        &self.alias
    }

    /// Borrow the destination's original Store coordinate without acquiring another lease.
    pub fn store(&self) -> &Path {
        &self.store
    }

    /// Borrow the complete destination selected by preparation, not inferred from native bytes.
    pub fn destination(&self) -> &SdkNativeArtifact {
        &self.destination
    }
}

/// Capture actual destinations and check the complete extern set against the reproduced parent recipe.
pub(super) fn capture(
    owner: &OvenStoreExecutionPayload,
    receipt: &OvenReceipt,
    selected: &[(&str, &SdkCompiledUnit)],
) -> Result<Vec<SdkPhysicalNativeEdge>, Error> {
    let edges = selected
        .iter()
        .map(|(alias, unit)| record(alias, unit))
        .collect::<Result<Vec<_>, _>>()?;
    verify_recipe(owner, receipt, &edges)?;
    Ok(edges)
}

/// Compare records with the exact preparation nodes before checking the reproduced alias-to-bytes recipe.
pub(super) fn verify(
    owner: &OvenStoreExecutionPayload,
    receipt: &OvenReceipt,
    edges: &[SdkPhysicalNativeEdge],
    selected: &[(&str, &SdkCompiledUnit)],
) -> Result<(), Error> {
    let expected = selected
        .iter()
        .map(|(alias, unit)| record(alias, unit))
        .collect::<Result<Vec<_>, _>>()?;
    if serde_json::to_value(edges)? != serde_json::to_value(expected)? {
        return Err("physical native edge differs from its authoritative preparation destination".into());
    }
    verify_recipe(owner, receipt, edges)
}

/// Require canonical reproduced recipe identity and complete agreement with the selected Engine owner.
fn verify_reproduced_receipt(owner: &OvenStoreExecutionPayload, receipt: &OvenReceipt) -> Result<(), Error> {
    owner.verify_proven_native_payload()?;
    receipt.verify_identity()?;
    if owner.manifest.kind != OvenArtifactKind::Engine
        || owner.manifest.receipt_identity != receipt.identity
        || owner.manifest.build_unit_identity != receipt.build_unit_identity
        || serde_json::to_value(&owner.manifest.intent)? != serde_json::to_value(&receipt.intent)?
    {
        return Err("physical native recipe differs from its selected Engine owner".into());
    }
    Ok(())
}

/// Require the complete alias set and native bytes to match the source-current reproduced recipe.
fn verify_recipe(
    owner: &OvenStoreExecutionPayload,
    receipt: &OvenReceipt,
    edges: &[SdkPhysicalNativeEdge],
) -> Result<(), Error> {
    verify_reproduced_receipt(owner, receipt)?;
    let mut actual = BTreeMap::new();
    for edge in edges {
        if actual
            .insert(format!("extern:{}", edge.alias), edge.destination.digest.as_str())
            .is_some()
        {
            return Err("physical native edges contain a duplicate extern alias".into());
        }
    }
    let witnessed = receipt
        .sources
        .build_unit_inputs
        .iter()
        .filter(|(key, _)| key.starts_with("extern:"))
        .map(|(key, digest)| (key.clone(), digest.as_str()))
        .collect::<BTreeMap<_, _>>();
    if actual != witnessed {
        return Err("physical native edges disagree with the reproduced extern recipe".into());
    }
    Ok(())
}

/// Read one exact selected node under its original lease and authenticate its full source/output binding.
pub(super) fn record(alias: &str, unit: &SdkCompiledUnit) -> Result<SdkPhysicalNativeEdge, Error> {
    if alias.is_empty() {
        return Err("physical native edge has an invalid Rust extern alias".into());
    }
    verify_reproduced_receipt(&unit.owner, &unit.reproduced_receipt)?;
    let admitted: SdkLockedUnit = serde_json::from_slice(&unit.owner.payload)?;
    let mut destination = unit.native_artifact()?;
    destination.binding = destination.binding.identity_binding();
    let receipt = &unit.reproduced_receipt;
    if serde_json::to_value(admitted.identity_binding())? != serde_json::to_value(&destination.binding)?
        || unit.owner.manifest.domain != format!("sdk-source-unit-{}", destination.binding.domain)
        || destination.relative_path.contains(['/', '\\'])
        || receipt.identity != destination.receipt_identity
        || receipt.sources.build_unit_inputs.get("sdk-source-archive") != Some(&destination.binding.archive_digest)
        || receipt.sources.build_unit_inputs.get("domain") != Some(&destination.binding.domain)
        || receipt.intent.features.iter().collect::<BTreeSet<_>>()
            != destination.binding.features.iter().collect::<BTreeSet<_>>()
    {
        return Err("physical native destination differs from its admitted source binding".into());
    }
    let store = unit
        .owner
        .artifact_root
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or("physical native destination has no original Store root")?
        .to_path_buf();
    Ok(SdkPhysicalNativeEdge {
        alias: alias.to_string(),
        store,
        destination,
    })
}

#[cfg(test)]
mod tests;
