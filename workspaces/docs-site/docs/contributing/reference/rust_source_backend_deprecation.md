# Rust-source backend deprecation policy

This policy seeds issue [#647](https://github.com/encero-systems/incan/issues/647). It does not remove the Rust-source backend. It draws the boundary for 0.5 work so the old backend remains useful while semantic authority moves toward stable IDs, backend-neutral facts, `IncanType`, HIR, Body IR, ABI metadata, and diagnostics.

## Policy

The Rust-source backend is a compatibility and reference backend. It may keep current users unblocked and may remain inspectable, but new language semantics should not be implemented only in Rust-source lowering or emission.

Generated Rust can still answer useful questions:

- what the current backend emits;
- whether a generated project compiles and runs;
- whether public tooling reports useful artifacts;
- whether compatibility behavior still works during migration.

Generated Rust must not be the only answer to semantic questions such as:

- what source declaration, expression, local, type, or call an operation means;
- which overload, trait dispatch, callable surface, or generic binding was selected;
- which ownership, borrow, coercion, runtime-helper, or target requirement exists;
- which package/import/reexport identity a downstream consumer should see.

## Allowed old-backend compatibility fixes

Compatibility fixes in the old backend are allowed when they keep current users or 0.4/0.5 proof lanes unblocked. They must include a migration note when they add or preserve behavior that should move to the middle end.

Use this template in the code comment, test name, issue note, or PR text:

| Field | Required content |
| --- | --- |
| Compatibility issue | Link the bug or release issue that needs the old backend fix. |
| Behavior evidence | Name the test/snapshot/downstream lane proving the behavior. |
| Semantic owner | Name the future owner: stable IDs, semantic facts, `IncanType`, HIR, Body IR, ABI metadata, runtime-service facts, diagnostics, or package metadata. |
| Retirement condition | State what will let this compatibility path disappear or become a thin adapter. |

Do not use the template as bureaucracy. Use it to prevent backend-only fixes from becoming hidden architecture.

## Current v0.5 adoption

`CompilationSession` now owns one checked analysis result for executable builds, generated-Rust inspection, and
codegraph inspection. That result bundles the lowering inputs and source-backed stdlib metadata still required by the
current backend with a `SemanticModuleSnapshot` per module. The build paths pass that analysis into `IrCodegen` rather
than asking codegen to typecheck the same source again, and codegraph resolves checked call/reference targets from
semantic facts rather than `TypeCheckInfo` directly.

The remaining internal `IrCodegen` typecheck fallback is deliberate and narrow: it serves direct backend API callers
that do not yet supply a session analysis. Its owner is [#225](https://github.com/encero-systems/incan/issues/225);
remove it when those callers supply session analysis and Body IR owns the lowering-specific queries that facts do not
yet model. It must not receive new semantic decisions.

## Not allowed without explicit maintainer approval

- Adding new source semantics only in an emitter branch.
- Duplicating typechecker decisions in codegen by matching method names, Rust strings, or generated token shapes.
- Adding `.clone()`, `.into()`, `.to_string()`, `.as_ref()`, or borrow rewrites as local emitter patches without routing the decision through ownership or Rust-boundary planning.
- Treating a generated-Rust snapshot as sufficient evidence for package, import, vocab, test-batch, or downstream behavior when those boundaries can observe the change.
- Expanding `__incan_std` source materialization as if it were the long-term stdlib packaging model.

## Semantic destinations

| If a change needs to know... | Put the authority in... |
| --- | --- |
| Declaration, expression, statement, local, or type identity | Stable compiler IDs and semantic facts. |
| Source-level type meaning independent of Rust spelling | `IncanType` or the backend-neutral semantic type model. |
| Normalized typed program shape | HIR v0 and semantic module snapshots. |
| Ownership, borrow, move, clone, drop, or call argument use | Duckborrower facts and Body IR. |
| Runtime helper, target, allocator, panic, or service requirement | ABI v0 hooks and runtime-service metadata. |
| User-facing expected/actual facts | Diagnostics metadata and schema. |
| Generated project layout, Cargo manifest shape, or artifact reports | Backend preparation and artifact plan. |
| Public import, reexport, package, or checked API identity | Package metadata and checked API facts. |

## Existing guardrails to reuse

Before adding a new broad regression lane, check whether the repo already has a compact guardrail for the boundary:

| Boundary | Existing guardrail |
| --- | --- |
| Stringly semantic checks in compiler code | `tests/vocab_guardrails.rs` and `tests/fixtures/vocab_guardrails/semantic_string_audit.json`. |
| Import/package/facade identity | `tests/fixtures/boundary_parity/README.md` and its fixture families. |
| Generated Rust public library artifacts | `tests/generated_rust_artifact_tests.rs`, `tests/generated_rust_callability_artifact_tests.rs`, and `tests/generated_rust_native_consumer_tests.rs`. |
| Stdlib generated-Rust coverage | `workspaces/docs-site/docs/contributing/reference/generated_rust_stdlib_coverage.md` and `tests/stdlib_generated_rust_snapshot_tests.rs`. |
| Rust interop call/coercion behavior | Focused `tests/codegen_snapshots/rfc041_*`, `tests/codegen_snapshots/rfc043_*`, and `tests/codegen_snapshots/rust_interop_*` fixtures. |

## Review checklist

Use this checklist when reviewing compiler/backend changes during 0.5:

- Does the patch change source behavior or only generated artifact shape?
- If it changes source behavior, is the behavior recorded before backend emission?
- If it changes generated Rust, is the generated Rust a consumer of semantic facts or the source of the decision?
- Does the test cover the boundary that can observe the behavior: direct, import, facade/reexport, package consumer, test batch, vocab, generated project, or downstream lane?
- If it is a compatibility fix, is there a migration note and retirement condition?
- Does it preserve the current Rust-source backend without making replacement harder?

## Examples from current 0.5 bugs

| Issue | Backend policy lesson |
| --- | --- |
| [#803](https://github.com/encero-systems/incan/issues/803) | Rust type identity must not depend on emitted Rust formatting. The `usize` identity fix lives in the boundary coercion matrix, with generated-project verification as the parity check. |
| [#804](https://github.com/encero-systems/incan/issues/804) | `.into()` insertion is semantic call planning. It should be owned by Rust-boundary compatibility facts, not by a local emitter convenience. |
| [#805](https://github.com/encero-systems/incan/issues/805) | Callback adaptation needs explicit callable and borrowed-parameter facts. Accepting source callbacks by value and hoping Rust rejects them is not a diagnostic strategy. |
| [#806](https://github.com/encero-systems/incan/issues/806) | Receiver-side type arguments and method-level type arguments must be distinguished before emission. The emitter can realize the plan, but it should not invent it. |

## Relationship to 0.6 cutover

The 0.6 backend cutover should consume 0.5 facts rather than rediscover behavior from generated Rust. The old backend should still be useful as a parity oracle, but parity means "same supported source behavior," not "same emitted tokens."
