//! Carry the ordered runtime demands of a successful plan between records requests.

use std::collections::BTreeMap;
use std::path::Path;

use incan_lowering::{
    RecordedRuntimeInputs, RuntimeRequirement, UnitContinuation, UnitPlanning, complete_recorded_runtime_plan,
};
use incan_mir_plan_native::source_span;
use incan_semantics_core::body_ir::{LinkedStaticAccessors, NativeUnionDeclaration};
use incan_semantics_core::{CanonicalSymbolId, decode_incan_symbol_identity, encode_incan_symbol_identity};
use internal_models_native::PlanSourceSpanRecord;
use internal_models_native::runtime_requirement_rows::{
    RuntimeRequirementRow, decode_runtime_requirement_batches, encode_runtime_requirement_batches,
};

use super::checked_program::{PreparedRuntimeRequirements, apply_runtime_requirements, prepare_runtime_requirements};
use super::{CheckedProgram, RefusalReceipt};

const PAGE: &str = "runtime-requirements.crumb2";
const REPORT: &str = "runtime-requirements.crumb2.report";

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
            .and_then(|bytes| decode_runtime_requirement_batches(&bytes).ok())
            .and_then(|batches| {
                batches
                    .into_iter()
                    .map(|batch| batch.into_iter().map(planning_requirement).collect())
                    .collect::<Result<Vec<_>, _>>()
                    .ok()
            })
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
        let batches = self
            .selected
            .into_iter()
            .map(|batch| batch.into_iter().map(graph_requirement).collect())
            .collect();
        let bytes = encode_runtime_requirement_batches(batches)?;
        std::fs::write(products.join(REPORT), bytes)?;
        Ok(())
    }
}

/// Restore complete canonical declarations and shared source locations at the driver boundary.
fn planning_requirement(row: RuntimeRequirementRow) -> Result<RuntimeRequirement, String> {
    let canonical = row
        .canonical
        .as_deref()
        .map(|identity| {
            decode_incan_symbol_identity(identity)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "runtime requirement has no canonical symbol identity".to_owned())
        })
        .transpose()?;
    Ok(RuntimeRequirement {
        path: row.path,
        canonical,
        span: source_span(&row.span.file, row.span.line, row.span.column),
    })
}

/// Encode one chosen demand without replacing its exact path or its live diagnostic coordinates.
fn graph_requirement(requirement: RuntimeRequirement) -> RuntimeRequirementRow {
    RuntimeRequirementRow {
        path: requirement.path,
        canonical: requirement.canonical.as_ref().map(encode_incan_symbol_identity),
        span: PlanSourceSpanRecord {
            file: requirement.span.file.as_str().to_owned(),
            line: requirement.span.line,
            column: requirement.span.column,
        },
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
