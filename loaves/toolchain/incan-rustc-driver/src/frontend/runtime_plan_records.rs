//! Carry the ordered runtime demands of a successful plan between records requests.

use std::collections::BTreeMap;
use std::path::Path;

use incan_lowering::{
    RecordedRuntimeInputs, RuntimeRequirement, UnitContinuation, UnitPlanning, complete_recorded_runtime_plan,
};
use incan_semantics_core::CanonicalSymbolId;
use incan_semantics_core::body_ir::{LinkedStaticAccessors, NativeUnionDeclaration};

use super::checked_program::{PreparedRuntimeRequirements, apply_runtime_requirements, prepare_runtime_requirements};
use super::{CheckedProgram, RefusalReceipt};

const PAGE: &str = "runtime-requirements";
const REPORT: &str = "runtime-requirements.report";
const FORMAT: &[u8] = b"incan.plan.runtime-requirements/1\n";

/// Prior demands are hints. Only the requirements selected by this request become its published evidence.
#[derive(Default)]
pub(super) struct RuntimePlanRecords {
    prior: Vec<Vec<RuntimeRequirement>>,
    selected: Vec<Vec<RuntimeRequirement>>,
}

/// Unit options the recorded and ordinary completion routes share with the original request.
pub(super) struct RuntimeCompletion<'a> {
    pub kind: &'a str,
    pub tests: &'a [String],
    pub linked_static_exports: &'a BTreeMap<CanonicalSymbolId, LinkedStaticAccessors>,
    pub receipt: &'a RefusalReceipt,
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

    /// Complete the first already authenticated match and any subsequent exact match in the same checked context.
    /// A new or changed live batch returns to ordinary continuation before a saved provider path is read.
    pub(super) fn complete_matches(
        &mut self,
        program: &mut CheckedProgram,
        mut preserved: UnitContinuation,
        mut requirements: Vec<RuntimeRequirement>,
        mut prepared: PreparedRuntimeRequirements,
        options: &RuntimeCompletion<'_>,
        published: &mut Vec<NativeUnionDeclaration>,
    ) -> Result<UnitPlanning, Box<dyn std::error::Error>> {
        loop {
            let selected = apply_runtime_requirements(program, &requirements, prepared)?;
            self.record(requirements);
            let inputs = RecordedRuntimeInputs {
                modules: &program.modules,
                sources: &program.sources,
                files: &program.files,
                selected: &selected,
                entry: program.entry,
                kind: options.kind,
                tests: options.tests,
                package_versions: &program.dependencies.package_versions,
                linked_static_exports: options.linked_static_exports,
            };
            let planning = complete_recorded_runtime_plan(preserved, &inputs, published)
                .inspect_err(|_| options.receipt.deterministic())?;
            let UnitPlanning::NeedsRuntime(next, live) = planning else {
                return Ok(planning);
            };
            // These are already live demands, so use the current index without a speculative copy.
            let saved = self.next();
            let matched = saved.is_some_and(|saved| same_requirements(saved, &live));
            tracing::trace!(target: "incan::invocation", phase = "recorded_runtime_batch", batch = self.selected.len() + 1,
                offered = saved.is_some(), matched, demands = live.len());
            if !matched {
                return Ok(UnitPlanning::NeedsRuntime(next, live));
            }
            prepared = prepare_runtime_requirements(program, &program.dependencies.loaves, &live)?;
            program.dependencies.loaves.validate_demands()?;
            preserved = next;
            requirements = live;
        }
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
