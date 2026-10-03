//! Admission of complete publisher tool observations into typed work records.

use std::path::Path;

use oven_model::manifest::{
    RustFactArtifactKind, RustFactOutput, RustFactWorkObservation, RustFactWorkRecord, is_sha256_identity,
};

use super::model::{HarvestAdmissionRefusal, HarvestObservedProduct, HarvestToolObservation};
use super::output::safe_relative;

/// Convert one raw tool observation after proving executable, invocation, and product byte identities.
pub(super) fn tool_record_from_observation(
    observation: &HarvestToolObservation,
    target: &str,
) -> Result<oven_model::manifest::RustFactTool, HarvestAdmissionRefusal> {
    if !is_sha256_identity(&observation.probe_digest)
        || !is_sha256_identity(&observation.executable_identity)
        || !observed_products_are_bound(&observation.output_tree_digest, &observation.products)
        || !tool_products_match_outputs(&observation.products, &observation.outputs)
        || observation
            .executable
            .as_ref()
            .is_none_or(|executable| executable.digest != observation.executable_identity)
    {
        return Err(HarvestAdmissionRefusal::UnresolvedToolObservations);
    }
    let work = RustFactWorkObservation {
        role: oven_model::manifest::RustFactProducerRole::Tool,
        name: observation.name.clone().unwrap_or_default(),
        target: target.to_string(),
        executable: observation.executable.clone(),
        objects: Vec::new(),
        arguments: observation.arguments.clone(),
        environment: observation.environment.clone(),
        inputs: observation.inputs.clone(),
        outputs: observation.outputs.clone(),
        library: None,
    };
    match RustFactWorkRecord::try_from_observation(work) {
        Ok(RustFactWorkRecord::Tool(tool)) => Ok(tool),
        Ok(RustFactWorkRecord::Link(_)) | Err(_) => Err(HarvestAdmissionRefusal::UnresolvedToolObservations),
    }
}

/// Whether the asset-side product inventory exactly satisfies the logical tool output contract.
fn tool_products_match_outputs(products: &[HarvestObservedProduct], outputs: &[RustFactOutput]) -> bool {
    outputs.iter().all(|output| {
        products.iter().any(|product| match output.kind {
            RustFactArtifactKind::File => product.owner_relative_path == output.path,
            RustFactArtifactKind::Tree => Path::new(&product.owner_relative_path).starts_with(&output.path),
        })
    }) && products.iter().all(|product| {
        outputs.iter().any(|output| match output.kind {
            RustFactArtifactKind::File => product.owner_relative_path == output.path,
            RustFactArtifactKind::Tree => Path::new(&product.owner_relative_path).starts_with(&output.path),
        })
    })
}

/// Whether every observed product and its complete tree carry canonical byte identities.
pub(super) fn observed_products_are_bound(tree_digest: &str, products: &[HarvestObservedProduct]) -> bool {
    is_sha256_identity(tree_digest)
        && products
            .iter()
            .all(|product| safe_relative(&product.owner_relative_path).is_some() && is_sha256_identity(&product.digest))
}
