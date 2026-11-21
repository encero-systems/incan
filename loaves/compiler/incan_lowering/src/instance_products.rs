//! Generic instance requests and closed generic instances in keyed plan products.

use std::collections::BTreeMap;

use incan_semantics_core::CompilerNodeId;

use crate::plan_product_inputs::producer_generation;

/// The keyed-product transport of one records request, shared by every closed instance its unit lowers.
#[derive(Clone, Debug)]
pub struct InstanceProducts {
    pub directory: String,
    pub generation: Option<String>,
    pub admitted: BTreeMap<CompilerNodeId, String>,
    pub report: Option<String>,
    pub overflow_checks: bool,
}

/// Share one records request's keyed-product transport with its closed instances; other requests keep none.
pub fn instance_transport(
    directory: Option<String>,
    admitted: &BTreeMap<CompilerNodeId, String>,
    report: Option<String>,
    overflow_checks: bool,
) -> Option<InstanceProducts> {
    let root = directory?;
    let generation = producer_generation(Some(&root));
    Some(InstanceProducts {
        directory: root,
        admitted: admitted.clone(),
        report,
        overflow_checks,
        generation,
    })
}
