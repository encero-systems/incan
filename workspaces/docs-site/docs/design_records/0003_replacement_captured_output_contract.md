---
id: DD-0003
title: Capture replacement output as receipt-bound execution evidence
status: Accepted
type: design-decision
date: 2026-08-30
review_target: 0.6 Slice 2
sources:
  - https://github.com/encero-systems/incan/issues/1249
  - https://github.com/encero-systems/incan/issues/1249#issuecomment-5469488595
  - https://github.com/encero-systems/incan/pull/1248
---

# DD-0003: Capture replacement output as receipt-bound execution evidence

## Context

Issue [#1249](https://github.com/encero-systems/incan/issues/1249) was opened when the replacement executor refused `println` as an ordinary named call. That diagnosis became stale when #1246 landed through [PR #1248](https://github.com/encero-systems/incan/pull/1248): the compiler now resolves the `println` alias to compiler-owned `BuiltinFnId::Print`, and the direct executor records output instead of writing to the host. The maintainer confirmed that this is delivered behavior in the [2026-08-30 issue comment](https://github.com/encero-systems/incan/issues/1249#issuecomment-5469488595).

The existing behavior needs a durable boundary because it is tempting to describe it as either a general effect system or a completed shadow-comparison feature. Neither description is true. `examples/simple/hello.incn` now executes through the replacement backend and records one line of output, but current replacement execution exposes an ordered `Vec<String>` of lines, not a typed general effect trace. Likewise, the shadow profile notices output but deliberately makes that comparison unavailable rather than comparing it.

## Decision

The accepted v0.6 direct-execution contract is the following.

- **Operation identity is compiler-owned.** `NamedCallableTarget.builtin` carries `BuiltinFnId` only when the typechecker proved the call does not bind a source declaration. Consumers must dispatch from that identity, not from `println`, `print`, or a method spelling. A same-module declaration retains `direct_call_id` and therefore keeps its own meaning. `Callee::Helper` remains a distinct explicit operation category for compiler-owned helper operations; this record does not make helpers I/O effects.
- **Replacement execution captures output; it does not write it.** `print` and `println` append rendered lines to `ReplacementExecution::emitted_output()` in source-emission order. The executor performs no host stdout I/O for those lines. This is a no-host-I/O boundary for output, although the private executor accumulates its returned output in a mutable vector.
- **The current trace is deliberately narrow.** `emitted_output` is `Vec<String>`, not an enum or a typed general effect trace. This record accepts that representation for output lines only. It neither introduces a general effect vocabulary nor promises that future file, provider, process, or helper operations will use the same carrier.
- **Only a successful direct execution becomes CLI output.** The replacement CLI executes the validated plan, finalizes and persists the backend execution receipt using the execution's output identity, and only then relays the captured lines. A refusal, runtime failure, or receipt-persistence failure returns before that relay.
- **Receipts commit to the captured output.** The replacement output identity includes a length-prefixed summary of the output lines alongside the consumed Body IR, observed result, ownership evidence, runtime requirements, task lifecycle, and provider-execution summary. The `BackendExecutionReceipt` then binds that output identity and its explicit shadow-comparison state.
- **Current shadow comparison is explicitly non-green for printed output.** Its source observable is the returned value. If the replacement route has any captured output, the comparator returns `Unavailable` because the legacy route currently recovers the value from a stdout result frame that program output would break. It must not claim a return-value match while ignoring output.

## Consequences

- `examples/simple/hello.incn` is a valid direct replacement execution case: its `println` line is captured, receipt-bound, and materialized by the CLI after successful receipt persistence.
- A caller can inspect and test program output without letting the Body IR executor write to the host. Machine-readable build reports retain `emitted_output` and the same output identity that the backend receipt uses.
- An output-bearing shadow comparison is honest but unavailable, not silently green. A later profile that compares output must define how both routes transport and compare ordered lines before it can report `Matched`.
- New builtin execution remains deliberate. The executor admits only its proven subset; it refuses a builtin whose direct answer could diverge from the Rust-emission backend rather than inferring a behavior from spelling.

### Remaining committed-example builtin calls

The fixed corpus walks every committed `.incn` file under `examples/` (excluding generated `target/` trees). The inventory below excludes prose/docstring examples and records every non-`Print`/`Range` builtin spelling used in those sources at this record's base commit. It is a source and profile inventory, not a claim that each enclosing example reaches the listed call: the corpus reports an aggregate `unsupported direct replacement profile` bucket, while model construction, imports, methods, and other earlier constructs can stop an individual example first.

| Builtin identity | Committed example call sites | Current direct-profile boundary |
| --- | --- | --- |
| `Len` | [`custom_traits.incn` lines 14, 16, 18, 20](https://github.com/encero-systems/incan/blob/0212a9f218fb477e07fc51296e11d7440b33c75f/examples/advanced/custom_traits.incn#L14-L20), [`collections.incn` lines 35, 46](https://github.com/encero-systems/incan/blob/0212a9f218fb477e07fc51296e11d7440b33c75f/examples/intermediate/collections.incn#L35-L46), [`comprehensions.incn` line 46](https://github.com/encero-systems/incan/blob/0212a9f218fb477e07fc51296e11d7440b33c75f/examples/intermediate/comprehensions.incn#L43-L49), [`supertraits.incn` line 22](https://github.com/encero-systems/incan/blob/0212a9f218fb477e07fc51296e11d7440b33c75f/examples/intermediate/supertraits.incn#L21-L23), [`typed_data_processor` source line 33](https://github.com/encero-systems/incan/blob/0212a9f218fb477e07fc51296e11d7440b33c75f/examples/advanced/typed_data_processor/src/main.incn#L31-L35), [`typed_data_processor` test line 25](https://github.com/encero-systems/incan/blob/0212a9f218fb477e07fc51296e11d7440b33c75f/examples/advanced/typed_data_processor/tests/test_transform.incn#L23-L27), [`codegraph_importer` source line 124](https://github.com/encero-systems/incan/blob/0212a9f218fb477e07fc51296e11d7440b33c75f/examples/pro/codegraph_importer/src/importer.incn#L122-L126), and [`codegraph_importer` test line 45](https://github.com/encero-systems/incan/blob/0212a9f218fb477e07fc51296e11d7440b33c75f/examples/pro/codegraph_importer/tests/test_importer.incn#L43-L47). | Admitted only for replacement lists, collected generators, and tuples. A string call such as `len(name)` has the explicit byte-versus-character refusal; other value kinds also refuse. |
| `Int`, `Str`, `Float` | [`type_conversions.incn` lines 20, 24, 28, 32, 35, 36](https://github.com/encero-systems/incan/blob/0212a9f218fb477e07fc51296e11d7440b33c75f/examples/advanced/type_conversions.incn#L18-L38), [`pricing.incn` line 15](https://github.com/encero-systems/incan/blob/0212a9f218fb477e07fc51296e11d7440b33c75f/examples/advanced/library_package/producer/src/pricing.incn#L13-L17), [`models.incn` line 16](https://github.com/encero-systems/incan/blob/0212a9f218fb477e07fc51296e11d7440b33c75f/examples/advanced/multifile/models.incn#L14-L18), and [`transform.incn` line 9](https://github.com/encero-systems/incan/blob/0212a9f218fb477e07fc51296e11d7440b33c75f/examples/advanced/typed_data_processor/src/transform.incn#L7-L11). | Resolved to builtin identities but outside `EXECUTABLE_BUILTINS`; the profile validator refuses their calls. |
| `Enumerate`, `Zip` | [`iterators.incn` lines 26, 33](https://github.com/encero-systems/incan/blob/0212a9f218fb477e07fc51296e11d7440b33c75f/examples/advanced/iterators.incn#L19-L36). | Resolved identities, but outside the admitted direct-builtin subset; the profile validator refuses their calls, and no replacement iterator implementation is implied by the existing Rust-emission support. |
| `ReadFile`, `WriteFile` | [`file_io.incn` lines 18, 23, 31](https://github.com/encero-systems/incan/blob/0212a9f218fb477e07fc51296e11d7440b33c75f/examples/advanced/file_io.incn#L16-L34). | Resolved identities, but outside the direct profile, so validation refuses their calls. This record does not turn captured `Print` output into authority for filesystem effects. |
| `JsonStringify` | [`derives_and_json.incn` line 45](https://github.com/encero-systems/incan/blob/0212a9f218fb477e07fc51296e11d7440b33c75f/examples/advanced/derives_and_json.incn#L43-L47) and [`codegraph_importer/main.incn` line 16](https://github.com/encero-systems/incan/blob/0212a9f218fb477e07fc51296e11d7440b33c75f/examples/pro/codegraph_importer/src/main.incn#L14-L18). | Resolved identity, but outside the admitted direct-builtin subset, so validation refuses its calls. |

The remaining registry identities (`Abs`, `Sum`, `Min`, `Max`, `Bool`, `Sorted`, and `IsInstance`) have no non-docstring call site in the committed `examples/` corpus at this base. `Abs`, `Sum`, `Min`, and `Max` are already members of the executor's admitted subset, but that is not corpus evidence for the other identities.

## Non-goals

- Implementing any remaining builtin, import, public-boundary, method, filesystem, provider, or process semantics.
- Replacing `Vec<String>` with a typed effect trace, or deciding a general effect system.
- Making output-bearing shadow comparisons green, changing the source-observable profile, or comparing only a returned value while discarding output.
- Changing the replacement session work in #1254 / PR #1259, the release version, or the selected backend profile.
- Closing #1249. It remains open for the documented builtin execution work.

## Revisit condition

Revisit this record when one of the following is proposed:

- another externally observable builtin needs direct execution;
- a shadow profile can transport and compare both returned values and ordered output without relying on an ambiguous stdout frame; or
- a caller needs a broader effect vocabulary than output lines.

That work must state its own operation identity, authority, receipt, replay, and comparison contract. It must not treat this narrow captured-output carrier as a general-purpose effect mechanism by implication.

## Provenance

This record derives from #1249, the maintainer's accepted clarification, and #1246's landing through PR #1248. It records the current replacement-output contract and does not supersede the issue, the backend-selection receipt contract, or future builtin-execution planning.

## References

- [Issue #1249: decide and represent how the replacement executor calls builtins like `println`](https://github.com/encero-systems/incan/issues/1249)
- [Maintainer clarification on #1249](https://github.com/encero-systems/incan/issues/1249#issuecomment-5469488595)
- [PR #1248: lower collection operations, execute them, and fix assignment scoping](https://github.com/encero-systems/incan/pull/1248)
- [Direct replacement output-capture and shadow boundary test](https://github.com/encero-systems/incan/blob/0212a9f218fb477e07fc51296e11d7440b33c75f/tests/replacement_backend_execution_tests.rs#L4004-L4097)
- [Backend selection and execution receipt](https://github.com/encero-systems/incan/blob/0212a9f218fb477e07fc51296e11d7440b33c75f/src/backend/selection.rs#L224-L245)
