# Backend selection & execution receipts

Every `incan build` records which compiler backend produced its output. `incan_emit::selection` (#986) is the compiler-owned boundary that makes that record: a build declares the backend it intends to use before code generation starts, and records the backend that actually ran once it has produced output.

Incan has one execution route. Today that route emits Rust (`IrCodegen` in `loaves/compiler/incan_emit/`) and hands it to Oven, and the records below name it `legacy`. The direct route of #1337, which lowers Body IR inside the pinned `rustc` without generating Rust (DD-0004), replaces it as that one route rather than running beside it. The Body IR interpreter that an earlier dev slice ran as a second, partial backend has been removed, together with its `--backend`, `--backend-fallback` and `--shadow` build flags; the programs its tests ran are behavior fixtures now.

This is a different axis from Oven's own receipt, described in [Oven Alpha](oven_alpha.md). Oven's receipt selects how an already generated artifact is compiled, its legacy-Cargo-versus-direct-`rustc` build boundary, and never decides which backend produced that artifact.

## The two records

**`BackendSelection`** is a versioned, content-identified record of what was decided *before* execution: the selected backend, its implementation revision, and a content identity of the source being compiled. It is built by `select_backend` before code generation starts.

**`BackendExecutionReceipt`** is a versioned, content-identified record of what happened *after* execution: the backend that ran, the diagnostic-contract version in force, and a content identity of the produced output. It embeds the `BackendSelection` it is bound to, and the backend that ran is always the one selected: there is no other route to fall back to.

Both are plain data with no I/O, and both carry a content-derived `sha256:` identity checked by `verify_identity()`, the same pattern Oven's receipt uses: a later stage that holds only a serialized copy does not have to re-derive trust from the fields.

The schema is version 3. Version 2 also recorded a selection reason, a compatibility profile, a fallback policy and outcome and a shadow-comparison state; those existed only while a second, partial backend could be requested, and they left with it.

## Why every build records a selection

A build never asks for a backend, yet it still writes the record rather than leaving the choice implicit. A completed-output reuse is eligible only when its immutable Loaf carries a receipt that verifies under the running compiler: the current schema, a consistent identity and the backend revision this compiler runs. The build then republishes that receipt after materialization instead of inventing a new execution record. An output whose receipt does not verify, such as one sealed under schema 2, is rebuilt. A reader of `.incan/backend/receipt.json` can therefore always tell which backend produced the output beside it, including after a cache hit.

## Reading a receipt

A successful build publishes its receipt to `.incan/backend/receipt.json` in the project root, beside Oven's `.incan/oven/receipt.json`, and embeds it as the `backend` field of `incan build --report json`. Inspect a persisted receipt with:

```bash
incan inspect backend-selection --receipt .incan/backend/receipt.json
incan inspect backend-selection --receipt .incan/backend/receipt.json --format json
```

`inspect backend-selection` calls `verify_identity()` on the receipt and, transitively, on the selection it embeds, and refuses to render a receipt whose recorded identity does not match its content: the same tamper and staleness detection Oven's `inspect oven` performs on its own receipt.

`incan inspect representation` reads a published library's [executable representation](../reference/package_executable_representation.md) coverage from the representation's declared index, never from its content, so a declaration covered with an empty body stays distinct from one the publisher refused. It reports two states rather than refusing them: a package that links without publishing a representation reports `none published`, and a representation whose version this build cannot interpret still reports that version.

## Provenance for Oven and other clients

Oven and other clients can key provenance on the pre-execution `BackendSelection.identity` and attach the post-execution `BackendExecutionReceipt` to their own outputs without reading private HIR or Body IR: the receipt is a public, versioned projection of which backend produced an output. `diagnostic_contract_version` ties it to the diagnostics schema in force when it was produced (`incan_syntax::diagnostics::stable::DIAGNOSTIC_SCHEMA_VERSION`, in `loaves/kernel/incan_syntax/src/diagnostics/stable.rs`), so a consumer can tell whether the receipt's diagnostics are interpretable under its own contract version.
