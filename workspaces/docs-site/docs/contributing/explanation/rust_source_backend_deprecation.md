# Rust-source backend deprecation

The Rust-source backend lowers checked Incan to IR and emits Rust source from it. This page explains the policy for changing that backend while semantic authority moves out of generated Rust, the backend behavior inventory that supports the move, and the interop bugs that shaped the policy. The migration-note fields, the semantic owners, the emission-tree freeze and the guardrails are stated in [Rust-source backend migration notes and freeze](../reference/rust_source_backend_deprecation.md); the classified behavior is in the [backend behavior inventory](../reference/backend_behavior_inventory.md); reviewing a change against both is described in [Auditing generated Rust](../how-to/auditing_generated_rust.md#review-a-backend-change).

## The policy

The policy was first written in the 0.5 line, for issue [#647](https://github.com/encero-systems/incan/issues/647). It does not remove the Rust-source backend. It draws the boundary for backend work so the old backend remains useful while semantic authority moves toward stable IDs, backend-neutral facts, `IncanType`, HIR, Body IR, ABI metadata, and diagnostics.

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

## Compatibility fixes

Compatibility fixes in the old backend are allowed when they keep current users or 0.4/0.5 proof lanes unblocked. When such a fix adds or preserves behavior that should move to the middle end, it carries a [migration note](../reference/rust_source_backend_deprecation.md#migration-note).

Do not use the migration note as bureaucracy. Use it to prevent backend-only fixes from becoming hidden architecture.

These are not allowed without explicit maintainer approval:

- Adding new source semantics only in an emitter branch.
- Duplicating typechecker decisions in codegen by matching method names, Rust strings, or generated token shapes.
- Adding `.clone()`, `.into()`, `.to_string()`, `.as_ref()`, or borrow rewrites as local emitter patches without routing the decision through ownership or Rust-boundary planning.
- Treating a generated-Rust snapshot as sufficient evidence for package, import, vocab, test-batch, or downstream behavior when those boundaries can observe the change.
- Expanding `__incan_std` source materialization as if it were the long-term stdlib packaging model.

## The compilation-session handoff

The [backend-foundation artifacts](../reference/backend_behavior_inventory.md#backend-foundation-artifacts) were the first deliverables of the 0.5 backend-foundation lane. They are compiler-facing, and they made frontend decisions inspectable without changing the Rust-source backend.

In the 0.5 line, `CompilationSession` came to own one checked analysis result for executable builds, generated-Rust inspection, and codegraph inspection. That result bundles the lowering inputs and source-backed stdlib metadata still required by the Rust-source backend with a `SemanticModuleSnapshot` per module. The build paths pass that analysis into `IrCodegen` rather than asking codegen to typecheck the same source again, and codegraph resolves checked call/reference targets from semantic facts rather than `TypeCheckInfo` directly.

The remaining internal `IrCodegen` typecheck fallback is deliberate and narrow: it serves direct backend API callers that do not supply a session analysis. It must not receive new semantic decisions.

A `CompilerNodeId` is semantic rather than Rust-shaped, but bridge code may derive its path from a span or a source path: an expression's identity is built from its module and source byte span (`CompilerNodeId::expression_span`). Consumers treat the rendered form as a compiler identity, not as an emitted Rust item path.

## The backend behavior inventory

The [backend behavior inventory](../reference/backend_behavior_inventory.md) was seeded for issue [#646](https://github.com/encero-systems/incan/issues/646) as the inventory of the 0.5 backend-foundation lane. It exists so backend replacement work can preserve supported behavior intentionally, retire accidental behavior deliberately, and stop treating generated Rust snapshots as the only source of truth. It is not a freeze of every pre-1.0 behavior.

Its hosted-runtime assumptions are recorded so that restricted or freestanding targets can consume them from the inventory and metadata rather than rediscover them through failed builds.

## Lessons from 0.5 interop bugs

| Issue | Backend policy lesson |
| --- | --- |
| [#803](https://github.com/encero-systems/incan/issues/803) | Rust type identity must not depend on emitted Rust formatting. The `usize` identity fix lives in the boundary coercion matrix, with generated-project verification as the parity check. |
| [#804](https://github.com/encero-systems/incan/issues/804) | `.into()` insertion is semantic call planning. It should be owned by Rust-boundary compatibility facts, not by a local emitter convenience. |
| [#805](https://github.com/encero-systems/incan/issues/805) | Callback adaptation needs explicit callable and borrowed-parameter facts. Accepting source callbacks by value and hoping Rust rejects them is not a diagnostic strategy. |
| [#806](https://github.com/encero-systems/incan/issues/806) | Receiver-side type arguments and method-level type arguments must be distinguished before emission. The emitter can realize the plan, but it should not invent it. A receiver-side generic constructor turbofish was accepted and emitted incorrectly until PR #807 fixed the emitted shape. |

## Relationship to the 0.6 cutover

The 0.6 backend cutover should consume 0.5 facts rather than rediscover behavior from generated Rust. The old backend should still be useful as a parity oracle, but parity means "same supported source behavior," not "same emitted tokens."

The emission tree was frozen at the close of v0.6 slice 6 ([#1561](https://github.com/encero-systems/incan/issues/1561)). The frozen path set, the policy states, the manifest schema and the checker's modes are described in the module docstring of `scripts/check_emitter_freeze.py` and recorded on [#1561](https://github.com/encero-systems/incan/issues/1561#issuecomment-5750178488).
