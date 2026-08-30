# Design records

Design records capture lightweight, dated project decisions that need durable public context but do not require a full RFC. They record a decision and its provenance; they do not replace the RFC process or silently redefine an RFC, issue, or release plan.

<div class="inc-route-grid">
  <a class="inc-route-card" href="../RFCs/"><span class="inc-eyebrow">Govern a material change</span><strong>Browse RFCs</strong><span>Use the RFC process for language, compiler, tooling, and architecture proposals.</span></a>
  <a class="inc-route-card" href="TEMPLATE/"><span class="inc-eyebrow">Preserve context</span><strong>Start a design record</strong><span>Record a scoped decision, constraint, or deferral with explicit provenance.</span></a>
  <a class="inc-route-card" href="../project/"><span class="inc-eyebrow">Choose an artifact</span><strong>Project records overview</strong><span>See how RFCs, design records, whitepapers, roadmaps, and release notes differ.</span></a>
</div>

## Status vocabulary

- `Draft`: The decision is under discussion.
- `Accepted`: The decision is approved. An accepted decision may defer or exclude implementation.
- `Rejected`: The proposal was considered and not adopted.
- `Superseded`: A later design record or RFC replaced the decision.
- `Withdrawn`: The record was removed before acceptance.

## Authority and provenance

- Every design record must cite the RFCs, issues, pull requests, discussions, or release-planning notes from which it derives.
- A design record does not supersede one of those authorities unless it says so explicitly.
- Release targets in `review_target` are review points, not delivery commitments.
- Once a record is accepted, change the decision through a new or superseding record rather than rewriting its history.

## ID sequence

IDs are stable and sequential: `DD-0001`, `DD-0002`, and so on. File names use `NNNN_short_slug.md`; the `id` field in the record metadata is canonical.

Start from the [design-record template](TEMPLATE.md) when proposing a new decision.

## Records

| ID | Decision | Status | Review point |
| --- | --- | --- | --- |
| [DD-0001](0001_gpu_target_capability_deferred.md) | Defer GPU target capability and graphics contracts | Accepted | v0.8 planning |
| [DD-0002](0002_single_pinned_rust_version.md) | Pin one Rust version and one generated-project edition | Accepted | v0.7 planning |
| [DD-0003](0003_replacement_captured_output_contract.md) | Capture replacement output as receipt-bound execution evidence | Accepted | 0.6 Slice 2 |
