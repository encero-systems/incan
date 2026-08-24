# Backend selection & execution receipts

The v0.6 replacement-backend cutover ([#652](https://github.com/encero-systems/incan/issues/652)) introduces a second compiler backend: the Body IR replacement backend tracked by [#653](https://github.com/encero-systems/incan/issues/653), alongside the current Rust-source-emission backend (`IrCodegen`, `src/backend/ir/`) that this document calls the legacy backend. #988 provides the first deliberately partial direct-execution profile. Every request still declares the intended backend, records the backend that actually ran, and refuses unsupported source visibly rather than quietly falling back to legacy. `src/backend/selection.rs` (#986) is the compiler-owned boundary that makes that possible.

This is a different axis from Oven's own receipt, described in [Oven Alpha](oven_alpha.md): Oven's receipt selects how an already-generated artifact is compiled (its legacy-Cargo-vs-direct-`rustc` build boundary), and never influences which compiler backend produced that artifact in the first place. Legacy builds hand generated source to Oven; the #988 direct replacement profile does not create generated source or an Oven plan.

## The two records

**`BackendSelection`** is a versioned, content-identified record of what was decided *before* execution: the requested backend, its implementation revision, the compatibility profile it declares for this compilation, a content identity of the source being compiled, why that backend was selected (the compiler-owned default, or an explicit `--backend` request), and the declared fallback policy. It is built by `select_backend` before any codegen starts.

**`BackendExecutionReceipt`** is a versioned, content-identified record of what actually happened *after* execution: the backend that actually ran, whether a declared fallback occurred, the shadow-comparison outcome (always present and explicit, even when the comparison could not run), the diagnostic-contract version in force, and a content identity of the produced output. It embeds the `BackendSelection` it is bound to.

Both types are plain, I/O-free data — building and executing them does not touch `IrCodegen` or any other execution machinery directly. Both carry a content-derived `sha256:` identity, verified with `verify_identity()`, the same pattern Oven's own receipt uses: a later stage that only holds a serialized copy does not need to re-derive trust from the fields themselves.

## No silent fallback

Every successful `incan build` exposes a selection and a receipt, including the default path: selecting the legacy backend with no flags produces an explicit `selection_reason: "default"` record rather than an implicit, unrecorded choice. A source-current completed-output reuse is eligible only when its immutable Loaf already carries and verifies that same implicit-default receipt; it republishes the verified receipt after materialization rather than inventing a new execution record.

The first #988 replacement profile produces a visible outcome, never a silent legacy execution:

- `incan build --backend replacement --backend-fallback refuse` typechecks one source-only module containing a selected zero-argument `main`, lowers it to Body IR, and executes that body directly. An admitted `main` may call an admitted sibling Body-IR function; the runtime never reconstructs that call from source or routes it through generated Rust. Its receipt records `executed_backend: "replacement"`, `fallback_outcome: "not_needed"`, and an output identity over the actual Body-IR execution result; this path does not construct generated Rust or an Oven plan.
- The supported profile is deliberately partial: scalar arithmetic, scalar local bindings with no repeated user-binding name, returns, compiler-owned string concatenation, normalized range/while branches and loops, assertions, and source-local recursive tuple/list values. Body IR retains an exact declaration-span identity for each same-module named call, so direct dispatch selects the typechecker-chosen sibling body rather than scanning by name; imports and other targets without that identity refuse. It also admits a source-local, undecorated, non-generic plain `model` with no traits, methods, method aliases, partials, properties, or field aliases when every field is explicitly supplied with an already-admitted recursive structural value: execution verifies its declaration identity and canonical field layout before materializing a value, then permits one read-only named-field projection. Floats, callables, generator values, and nested nominal values are not structural model fields in this profile. Omitted model defaults, field writes, nested nominal values or projections, classes, generic or decorated models, traits, methods, aliases, enums, and `Result` values remain visible refusals. One numeric tuple-field projection, one integer list-index projection, and one list-index assignment are admitted; nested projection, slices, named projections outside that plain-model exception, dicts, sets, and a selected-entrypoint aggregate output remain visible refusals. Builtin iteration remains narrower: it accepts only a source-local `list[tuple[scalar, scalar]]`, including typed empty values, and may use a plain binding or the one-level `for a, b in pairs` destructuring lowering. The compiler records the recognized `range` builtin as an explicit Body-IR target fact, while a same-module declaration named `range` has its own declaration identity and dispatches as that declaration; an imported or unresolved `range` never borrows the builtin rule. The profile also executes the admitted callable vocabulary retained in Body IR: captured local closures, partial presets, source-evaluable declaration defaults, identity-selected sibling-function calls, generator expressions and generator functions consumed by `.collect()`, plus their lazy `.map()` and `.filter()` adapters when their callbacks are admitted local callables. Generator frames resume from retained Body-IR state; adapters do not poll their source until collection consumes them. The compiler currently records adapter calls as positional method calls, which the runtime accepts only for the two-operand `map`/`filter` profile; it does not infer a general stdlib signature. Packages, imports, Rust interop, unsupported callable/default forms, general destructuring, and any repeated user-binding spelling (lexical shadowing or reassignment) are refused visibly with the original Incan source span. A refusal emits no new receipt; it does not remove a receipt written by an earlier successful build. Direct execution and its selection/execution receipts leave the #987 structural-value and nominal-value rows non-green: #1154 must produce paired source-observable evidence through #1146's completed route before any parity claim can turn green.
- The #988 CLI surface accepts only `--backend-fallback refuse`. It has no receipt-bound legacy execution path for an unavailable source profile, so users must choose `--backend legacy` explicitly when the source is outside the profile.

A `--shadow` request on a build remains `{"unavailable": {"reason": "..."}}`, never `"matched"`. The reason names the boundary: a build executes the module's `main`, and a program entrypoint's return value is not observable from the produced legacy process, so there is nothing for the two routes to agree or disagree about. It is deliberately non-green — generated-Rust token shape is not a semantic comparison. The comparison that *does* run is described in [Source-observable shadow comparison](#source-observable-shadow-comparison) below.

Any explicit backend request, fallback policy, or shadow request bypasses completed-output reuse and takes the source-based path appropriate to the declared backend. This prevents a cached default result from being presented as the outcome of a different declared selection.

## Source-observable shadow comparison

[#1146](https://github.com/encero-systems/incan/issues/1146) adds the first shadow comparison that actually runs, for one deliberately narrow profile (`incan.shadow_comparison.direct_scalar_free_function.v0`, `src/backend/shadow/`): one source-only module holding exactly one free function, whose name is not `main`, called with concrete `int`, `bool`, or `str` arguments.

Both routes observe that same source, independently:

- The **replacement route** typechecks the module, lowers it to Body IR, and executes the named function directly. It generates nothing and spawns nothing.
- The **legacy route** runs the module plus a generated Incan entrypoint that calls the same function with the same arguments and prints the result. That program goes through Oven, the adopted build and execution authority: `IrCodegen` emits Rust, an Oven receipt authorizes those exact bytes, an immutable direct-`rustc` plan selected from the bounded Oven store compiles them with no Cargo process, and the produced binary runs as a separate process.

What is compared is what the two routes *did*, never the Rust one of them was built from.

### The result is transported losslessly

The legacy route reads its answer out of a real process's standard output, so the transport has to be exact. Trimming whitespace would silently equate `"x"` with `"x\n"`, and `""` with `"\n"`, turning a genuine divergence into a match. The generated entrypoint therefore frames the result between two marker lines, and the payload is recovered by anchoring on the exact leading and trailing byte sequences — never by searching, trimming, or lossy UTF-8 conversion. Output that does not carry the frame exactly is unavailable, because a result that cannot be read back byte-for-byte has not been observed.

### What agreement is allowed to mean

The compared observable is small on purpose: either the function returned normally — and both routes must agree on the exact spelling of that scalar — or it stopped on a runtime failure the profile recognizes identically on both sides. The recognized failures are a failed `assert`, an integer division or modulo by zero, and an arithmetic overflow; those stay distinct classes, because an overflow is not a division by zero and collapsing them would let a real divergence report as agreement. A failure neither route can classify is unavailable, not agreement: "both stopped somehow" does not show they stopped the same way.

Every comparison records exactly one of three outcomes, and only the first is green:

- `matched` — both routes ran and produced the same observable. It carries the compared observable itself, so a reader can see what agreement was claimed over, plus two profile facts described below.
- `diverged` — both routes ran and produced different observables, with both sides named. This is a regression signal on the backend-selection axis, never a reason to prefer one route's answer.
- `unavailable` — no comparison was made, with the concrete boundary that stopped it. A legacy program that fails to build lands here rather than in `diverged`: a comparison that could not run has proven nothing, so build success is never allowed to stand in for a semantic verdict.

Both ran states record two profile facts, and both are needed. `profile_kind` is the stable kind of comparison that ran — `incan.shadow_comparison.direct_scalar_free_function.v0` today — so a registry keyed on comparison capability can link against it without decoding a hash, and the link survives every change to the compared source. `profile_identity` is the content identity of the exact instance: this source, this observed function, these arguments. Recording only the hash would leave a consumer unable to say what kind of comparison it is looking at; recording only the kind would let two different comparisons claim the same evidence.

### Receipts, and what survives an unavailable comparison

Each route that executed is selected and finalized through the same `select_backend` / `resolve_execution` / `finalize_receipt` sequence a build uses, and both are declared `FallbackPolicy::Refuse`, so neither route can quietly become the other. Both receipts carry the same `source_identity` and the same comparison outcome; they differ, correctly, in `selected_backend`, `executed_backend`, and `output_identity`, because each route's output identity covers what that route actually produced. Treating the two receipts as interchangeable would erase the independence the comparison depends on.

The legacy route's output identity additionally covers the Oven authority that permitted the run — the project receipt identity, the reusable build-unit identity, the direct-`rustc` plan identity, and the produced output digest — so "which authority produced this legacy answer" is recorded rather than assumed.

An unavailable comparison does not discard work that really happened. When the replacement route executed and only the legacy route could not, the replacement route keeps its own receipt and Body-IR evidence alongside the unavailable state. Throwing that away would lose a real execution and make a staging gap look identical to a source the replacement backend cannot run at all. A `main`-observing profile is the clearest case: the replacement backend executes a zero-argument `main` perfectly well, and only the legacy route has no way to observe an entrypoint's return value.

The same rule covers receipt finalization. If a route executed but its receipt could not be finalized, the comparison is no longer verifiable and the verdict is withdrawn to `unavailable` naming the failure — but the route's observation is still reported, because an execution that really happened must not disappear because its record could not be written.

Receipts recording a comparison use selection/receipt `schema_version: 2`. Version 1 could only ever record `not_requested` or `unavailable`, so no version-1 receipt carries a comparison payload.

### Staging the legacy route

An Oven receipt's reusable build unit comes from build intent and build-unit inputs, never from generated source, so a comparison adopts the intent and build-unit inputs of a project whose direct-`rustc` plan was already published by an explicit `incan oven bake`, and replaces only the generated-source evidence with its own program. Where no such capability is staged, the comparison is `unavailable` with that reason — it is never approximated by invoking a compiler directly, because an unauthorized build produces a result no receipt can account for.

Because an unstaged run is honestly non-green everywhere, a default test run cannot tell a comparison that was never staged from one that was never implemented. `make shadow-comparison-evidence` closes that gap: it bakes a throwaway project to publish a plan, then runs the comparison and parity-corpus suites with `INCAN_SHADOW_REQUIRE_LEGACY_ROUTE=1`, so a missing or failing comparison is a hard failure rather than a reported skip. CI runs it as the `Shadow comparison evidence (#1146)` job and uploads the resulting parity summary.

## CLI surface

`incan build` accepts three flags:

- `--backend <legacy|replacement>` — declare the backend for this build. Defaults to `legacy`.
- `--backend-fallback <refuse>` — declare that an unavailable backend must stop visibly. Omitting this flag means refuse.
- `--shadow` — request a comparison against the replacement backend alongside normal execution.

A successful build publishes its receipt to `.incan/backend/receipt.json` in the project root (parallel to Oven's own `.incan/oven/receipt.json`), and embeds it as the `backend` field of `incan build --report json` output. The #988 direct path has the explicit `incan.replacement_execution.v0` report schema: alongside `status`, `mode`, and `entrypoint`, its `replacement_execution` payload projects the Body-IR result, receipt-bound output identity, snapshot, canonical ownership reads, and runtime requirements. It intentionally has no generated-artifact or Oven fields. An eligible completed-output reuse republishes its verified sealed receipt at the same path. Inspect a persisted receipt directly with:

```bash
incan inspect backend-selection --receipt .incan/backend/receipt.json
incan inspect backend-selection --receipt .incan/backend/receipt.json --format json
```

`inspect backend-selection` reads the receipt, calls `verify_identity()` on it (and, transitively, on the selection it embeds), and refuses to render a receipt whose recorded identity does not match its own content — the same tamper/staleness detection Oven's `inspect oven` performs on its receipt.

## Provenance for Oven and other clients

Oven and other clients can key provenance on the pre-execution `BackendSelection.identity` and attach the post-execution `BackendExecutionReceipt` to their own outputs, without reading private HIR or Body IR structures: the receipt is a public, versioned projection of "what backend produced this," independent of how either backend represents a program internally. `diagnostic_contract_version` on the receipt ties it to the diagnostics schema (`crates/incan_syntax/src/diagnostics/stable.rs::DIAGNOSTIC_SCHEMA_VERSION`) in force when it was produced, so a consumer can tell whether a receipt's diagnostics are still interpretable under its own contract version.
