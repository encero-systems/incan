//! Carry the ordered runtime demands of a successful plan between records requests.

use std::path::Path;

use incan_lowering::RuntimeRequirement;

const PAGE: &str = "runtime-requirements";
const REPORT: &str = "runtime-requirements.report";
const FORMAT: &[u8] = b"incan.plan.runtime-requirements/1\n";

/// Prior demands are hints. Only the requirements selected by this request become its published evidence.
#[derive(Default)]
pub(super) struct RuntimePlanRecords {
    prior: Vec<Vec<RuntimeRequirement>>,
    selected: Vec<Vec<RuntimeRequirement>>,
}

impl RuntimePlanRecords {
    /// Read a selected page if its schema is understood. Ordinary planning covers missing or malformed hints.
    pub(super) fn read(products: Option<&Path>) -> Self {
        let prior = products
            .and_then(|directory| std::fs::read(directory.join(PAGE)).ok())
            .and_then(|bytes| postcard::from_bytes(bytes.strip_prefix(FORMAT)?).ok())
            .unwrap_or_default();
        Self {
            prior,
            selected: Vec::new(),
        }
    }

    /// Offer the next prior batch after this request has settled the preceding batches.
    pub(super) fn next(&self) -> Option<&[RuntimeRequirement]> {
        self.prior.get(self.selected.len()).map(Vec::as_slice)
    }

    /// Keep the current source spans, including when the previous batch selected the same paths at other locations.
    pub(super) fn record(&mut self, requirements: Vec<RuntimeRequirement>) {
        self.selected.push(requirements);
    }

    /// Report this request's demands, including an empty list when the edit removed all of them.
    pub(super) fn publish(self, products: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let mut bytes = FORMAT.to_vec();
        bytes.extend(postcard::to_allocvec(&self.selected)?);
        std::fs::write(products.join(REPORT), bytes)?;
        Ok(())
    }
}

/// Match exactly the demands and their ordering before a prepared batch may alter the live closure.
pub(super) fn same_requirements(saved: &[RuntimeRequirement], live: &[RuntimeRequirement]) -> bool {
    saved.len() == live.len()
        && saved
            .iter()
            .zip(live)
            .all(|(a, b)| a.path == b.path && a.canonical == b.canonical)
}
