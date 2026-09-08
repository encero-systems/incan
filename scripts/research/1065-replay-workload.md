# Compiler-suite replay workload: issue 1065

Research snapshot: `9259c07fd` (`dev.4`), 2026-09-07. Scope: static workload audit, selected-case subprocess measurements, measurement protocol and partition design. This note does not change execution, scheduling, thread limits or CI, and does not close [#1065](https://github.com/encero-systems/incan/issues/1065).

## Decision supported by the current evidence

Keep the existing thread cap until both heavy roots have measured descendant-process data. In particular, **`integration_tests.rs` is not a low-subprocess root**. Its `incan_command()` helper constructs an Incan process at line 1162; the root contains 217 textual invocations of that helper, including helper definitions' bodies. `run_incan_source()` also calls it. Counting only `Command::new` hides the work.

Prioritize reading the four reports from #1425 before changing the scheduler. Compare root execution, inventory and direct-rustc bake separately. Measure root packing against those root costs before deciding whether splitting individual cases is worth its additional process/bake costs. Neither dividing total partition wall time by four nor assigning source bytes proves the attainable wall time.

At the initial static inspection, [#1425 CI run 34160698412](https://github.com/encero-systems/incan/actions/runs/34160698412) was still in progress; its artifacts API returned no artifacts. That initial inspection supplied no observed CI root times, measured speedup or actual peak process concurrency; the later focused local experiment below provides its own separate evidence. Historical issue-comment wall times are hypotheses to investigate, not measurements from this snapshot.

## Reproducible static inventory

Run from the repository root:

```sh
python3 - <<'PY'
from pathlib import Path
import re
for root in ('cli_integration', 'integration_tests'):
    text = Path(f'tests/{root}.rs').read_text()
    print(root, 'bytes', len(text.encode()), 'test markers', text.count('#[test]'))
    for name in ('run_incan', 'run_explicit_oven_bake', 'run_explicit_oven_bake_with_home',
                 'incan_command', 'run_incan_source', 'build_incan_source_binary',
                 'assert_runtime_error_cli', 'compile_source', 'compile_file', 'rustc_compile_ok'):
        calls = len(re.findall(r'(?<![\w])' + name + r'\s*\(', text))
        definitions = len(re.findall(r'fn\s+' + name + r'\s*\(', text))
        if calls > definitions:
            print(name, calls - definitions)
    print('Command::new', text.count('Command::new('))
PY
```

| Root | Bytes | `#[test]` markers | Principal textual call sites |
| --- | ---: | ---: | --- |
| `tests/cli_integration.rs` | 407,045 | 134 | `run_incan`: 192; `run_explicit_oven_bake`: 37; `run_explicit_oven_bake_with_home`: 2; `Command::new`: 8 |
| `tests/integration_tests.rs` | 744,656 | 281 | `incan_command`: 217; `run_incan_source`: 5; `build_incan_source_binary`: 2; `assert_runtime_error_cli`: 2; `Command::new`: 12 |

These are lexical counts, not a Rust AST inventory, executable case counts, spawn totals or concurrency measurements. Configuration gates, helper nesting, loops, early returns and source strings can change their meaning. Do not add helper counts together: that double-counts delegated calls. The original issue's 126/266 case and 219 invocation figures are not the current source inventory.

## Actual subprocess paths

- CLI ordinary calls: `run_incan` → `run_incan_with_env` → `run_incan_with_env_and_removed` → `configured_incan_command` → synchronous `.output()` (lines 36–217). Explicit bake and Cargo-guard helpers also synchronously wait. Most individual call sites thus run sequentially within one case; different libtest cases still overlap.
- CLI intentional overlap: `concurrent_normal_checks_reuse_sealed_sdk_inventory_without_mutable_publication` launches two children before either wait (lines 911–936). `workspace_lock_concurrent_publishers_leave_one_parseable_root_lock` does the same via its `spawn_lock` closure (lines 2569–2643). Two libtest threads therefore do not imply a two-child cap.
- CLI timing blind spots: the OS-environment helper (line 318), raw fixture closure near line 2121 and concurrent publisher closures bypass ordinary command-duration reporting. Existing `INCAN_TEST_COMMAND_TIMINGS` emits completion duration and test-thread name but no process ID or common start/end clock (`tests/support/mod.rs:10–31`), so it cannot reconstruct overlap.
- Integration root: `incan_command` → `Command::new(incan_debug_binary())` → caller `.output()` or timeout supervisor. `codegen_tests::run_incan_source` invokes `incan run -c`; `build_incan_source_binary` invokes `incan build`; `rustc_compile_ok` invokes rustc directly (lines 3527–3585). A root described as frontend tests also contains substantial native end-to-end work.
- Integration intentional overlap: the filesystem lock test builds two programs, starts a lock holder and runs the probe while it remains alive (lines 4441–4498). Those are executable descendants even when no Incan process remains.
- Below Incan, direct rustc, linker and generated programs matter. A count of only immediate Incan children cannot establish CPU/memory headroom. Capture descendant lifetimes and resource use, including wait-for-lock time versus CPU time.

## Redundant-work audit

| Candidate / existing consolidation | Evidence and coverage boundary | Recommendation |
| --- | --- | --- |
| Runtime diagnostics already share compilation | `runtime_error_canonicalization_cases` at integration line 2677 runs selectors against one compiled program after one CLI journey. | Retain; do not count each selector as an avoidable compiler invocation. |
| Logging cases already share a generated run | `std_logging_runtime_surfaces_share_one_generated_run` at integration line 525 covers direct and imported logging in one source project. | Retain independent assertions; use this shape for future runtime-only siblings. |
| Environment accessors already grouped | `run_std_environ_passing_accessors_share_one_program_issues557_rfc089` at CLI line 2842. | Avoid proposing already-landed consolidation as new savings. |
| Rooted workspace cold publication already grouped | CLI lines 2155 onward explicitly combine #908/#909/#931 and library/executable profiles in one cold journey. | Keep cold-store semantics; a globally warm fixture would remove the test's contract. |
| JSON public-library journey | `compiled_json_trait_owner_crosses_library_boundaries_issue946`, CLI lines 1445–1533: provider bake, generated trait-owner assertion, consumer bake, consumer run, consumer test. | Four calls exercise distinct admission/generated-code/run/test boundaries. Removing the consumer test or bake is coverage loss. Measure phase costs first; reuse immutable provider bytes only if source/SDK/receipt identity and mutation isolation remain exact. |
| Repeated provider/consumer scaffolding | Similar explicit provider/consumer pairs near CLI 1214/1256, 1310/1354, 7270/7292, 7366/7390 and 7484/7517. | Source-writing helper reuse is plausible maintainability work; it does not save a bake. Before caching a provider, compare its source closure, feature/profile, admission and relocation assertions. No duplicated immutable input identity has been proven here. |
| Native runtime fixture rebuilds | `incan_command` is used throughout integration tests, even where only runtime output differs. | Rank measured cases and compare generated inputs before consolidating. Group same-source runtime selectors; preserve separate compilation for import, generic inference, source diagnostics and emitted-code regressions. No quantified saving claimed. |

The audit finds substantial existing consolidation and hidden subprocess work, but does not establish a safe further redundant bake removal. The next useful artifact is a ranked command/phase cost table, not an arbitrary reduction in case count.

## Measurement protocol before any thread-cap patch

1. Use the pinned Linux CI toolchain and an otherwise idle runner. Record commit, runner CPU count/memory, suite/root identities, SDK/store identity, root-worker count, libtest threads, cache condition, exit status and instrumentation overhead. Keep all generated targets, fixtures and traces within the allocated Ralph storage domain; do not run two benchmark configurations concurrently.
2. Obtain each compiled root's exact libtest inventory and preserve it beside the report. Start with current two threads and warm receipt-compatible providers. Measure cold provider preparation separately so startup is not mislabelled as test execution.
3. Instrument command lifetimes in a temporary research patch at the actual spawn/wait boundaries, including explicit bake, guard, OS-env, timeout and raw concurrent closures. Record an invocation ID, case name, root identity, parent/child PID, argv category (avoid source contents/secrets), monotonic start/end, status and timeout. Flush a start record before waiting; write one file per invocation or synchronize JSONL writes. A duration-only completion record is insufficient. A wrapper selected by `CARGO_BIN_EXE_incan` is an exploratory alternative, but alters process ancestry and must not stand in for process-containment acceptance.
4. Independently trace descendant exec/exit events on Linux, using available process tracing, and match PID lifetimes to invocations. Account for child rustc/linkers and generated executables, PID reuse and signalled/orphaned children. If polling is used instead, label observed maxima as lower bounds and disclose sampling interval; short-lived children can disappear between samples. Resource totals need CPU and maximum resident memory, not summed command wall durations alone.
5. Report exact nested-Incan overlap from recorded intervals, descendant overlap, p50/p95 command duration by command and case, root elapsed time, CPU, peak memory, lock waits and failures. Preserve incomplete invocations when a run is killed. Shared-store serialization can yield high overlap with low CPU use; it is not evidence of spare memory.
6. Only after baseline data, compare thread budgets 1, 2 and 4 on each root separately, at least three interleaved warm repetitions. Keep the existing cap as control; do not assume the integration root is exempt. Record a failed/slower trial rather than dropping it. Choose a cap only if median wall time improves without resource exhaustion, flaky failures or a worse tail; document the observed resource margin rather than inventing a universal threshold.
7. Repeat the chosen policy in the four-partition replay under its actual root-worker mix and verify all cases once. A solo-root win can regress the multi-root shard. Publish before/after critical path with bake and execution split and no claim of full issue closure until its remaining acceptance bullets are evidenced.

## Per-case partition design

Current authority boundary: `OvenCompilerTestSuiteShardReference` in `src/oven/legacy_cargo.rs:455` carries immutable shard identity, complete target key and source-byte footprint. Suite schema is 15; shard schema is 2. `compiler_suite_selected_shard_references` in `src/cli/commands/oven.rs:3978` deterministically sorts by source bytes, source path and identity, then places whole roots in the lightest partition. `run_native_test_batch_all_in_directory_with_options` in `src/oven/native_test.rs:541` inventories a binary and launches all cases; existing multi-exact execution launches a fresh OS process for every name.

Proposed follow-up design, conditional on measured need:

- Keep artifact authority separate from timing estimates. Timing history may choose placement but may never authorize another root, source or executable. Bind workload identity to complete target key, immutable shard identity and inventory digest. Key history additionally by toolchain/target/profile and measurement configuration; ignore stale or incompatible records. Missing timing must still schedule a case, never silently omit it.
- Extend the versioned suite index with the exact normalized per-root case inventory (names plus ignored/test-kind metadata and an inventory digest) generated from the receipt-built binary. Inventory is currently obtained during replay; publishing it earlier requires building/listing the root at publication time or a separately validated inventory handoff. Measure that additional preparation cost and never substitute source-text test extraction for executable inventory. Bump the suite schema for the new contract; change shard schema only if shard payload fields change. Either explicitly reject old suite indexes with a republish diagnostic, or retain an explicitly tested whole-root legacy mode. Do not interpret an absent case list as an empty suite.
- At replay, inventory the exact authorized executable again and compare to the receipt-bound expected inventory before selecting. Reject unknown names, duplicates, mismatched inventory and conflicting root/partition selection. Validate the global partition plan as a disjoint union of all required cases; separately represent ignored and excluded cases. Every partition must use the same history snapshot/digest and tie-break rules, or independent placement can duplicate/omit cases.
- Add a subset mode to the bulk runner that executes a selected set in one process per root per partition. First probe the pinned libtest binary's support for multiple positional filters together with `--exact`; assert `a` never matches `a::b`, unknown names fail before execution, and selected cases run once. If supported, verified exact names form an OR selection without substring leakage. If not, evaluate exact complement `--skip` semantics before use; otherwise a dedicated test-list mechanism is a separate runtime design. Do not assume libtest version behavior or fall back to hundreds of processes silently. Guard command-line size limits; deterministic chunks must preserve aggregate timeout and report that they create additional processes.
- Define empty selection as explicit no-work with a valid partition coverage record, never an unfiltered command. Preserve ignored semantics and report requested, executed, ignored, failed, timed-out and unfinished cases. Reconcile libtest events against the selected inventory; nested child JSON cannot become outer completion evidence. Carry selection/inventory/partition digests in replay results so partial runs cannot masquerade as whole-root coverage.
- Schedule with both fixed root startup/bake cost and measured case cost. Splitting one root across four partitions may cause four materializations/inventories/bakes and execute different cache-locality paths. Sum of overlapping case durations is not root elapsed time. Evaluate a model against recorded schedules, then validate it on actual replay; do not use the ideal arithmetic share as a speedup claim.
- Acceptance tests for implementation: deterministic placement under permuted inputs; complete/disjoint coverage including missing history; source identity collision across resolved units; digest mismatch refusal; zero/unknown/duplicate selections; prefix names; ignored-only selection; partial failure and timeout; nested event contamination; output-before-exit; aggregate deadline; process-group cleanup; command length fallback; old-schema compatibility/refusal. Performance acceptance includes report overhead and duplicated bake costs.

This design and the static audit satisfy the corresponding research deliverables. Full native-workload fan-out data, a measured thread policy and replay wall-clock improvement remain open. #1425 owns timing/reporting changes; coordinate after it lands before implementing any runner, scheduler or CI part of this proposal.

## Executable measurement tool

`trace_replay_processes.py` supplies opt-in executable wrappers without editing the test roots or production runner. `install` creates wrapper paths, and `run` selects them through `CARGO_BIN_EXE_incan`, `RUSTC` and `PATH`. Each wrapper writes an atomic record before launch, after launch, and after waiting for exit. Records carry host-monotonic timestamps, invocation ancestry, PIDs, a command category, optional root/case labels and child resource usage. Source-code arguments are not recorded.

Use a fresh directory for every run. Start with Incan-only interception for receipt-bound execution, preserving the real `RUSTC`. Installing the optional `--rustc "$PINNED_RUSTC"` wrapper changes its executable path and may invalidate receipt expectations or build fingerprints; validate that boundary separately before using those results. Absence of rustc records in an Incan-only run means rustc was not instrumented, not that no rustc process ran. Set `--case` only when the root command selects exactly one known case; leave it unset for groups. A wrapper cannot discover the spawning libtest thread's name. Set all normal SDK/toolchain and generated-output environment variables exactly as the uninstrumented lane does before invoking the tool.

```sh
python3 scripts/research/trace_replay_processes.py install \
  --output "$TRACE_ROOT/one-case" --incan "$INCAN_BIN"
python3 scripts/research/trace_replay_processes.py run \
  --output "$TRACE_ROOT/one-case" --root tests/cli_integration.rs \
  --case run_synchronous_result_main_issue843 --timeout 120 \
  -- "$CLI_TEST_BINARY" --exact run_synchronous_result_main_issue843 --nocapture --test-threads=1
```

`TRACE_ROOT` must lie under the allocated task cache, not the repository's primary checkout. The command saves `summary.json` and individual `events/*.json`. A killed wrapper can leave a `running` record: the summary reports it as incomplete, never as success. Re-read retained events after interruption using the `summary` command.

The overlap metric counts **completed intercepted command intervals**, including overlapping parent/descendant commands. It is not a core utilization count or a complete OS process census. `start_before_ns`/`start_after_ns` bracket `Popen`; `end_observed_ns` is after `wait`, so scheduling and observation overhead affect the boundaries. CPU/max-RSS records cover each wrapper's reaped children, can include descendants, and must not be summed across ancestry as independent totals. Absolute compiler paths that bypass the wrappers, generated executables and non-compiler descendants may be absent. Python startup adds overhead. Compare instrumented and ordinary runs before using durations for policy; the wrapper changes ancestry and is not process-containment acceptance evidence. It forwards TERM/INT to its child; process-group delivery can repeat a signal, so programs with custom signal handlers require separate transparency validation.

Controlled validation (2026-09-07, macOS, no compiler build): `python3 scripts/research/test_trace_replay_processes.py` passed five tests. A synthetic parent launched two overlapping wrapped children: three records and interval peak three, with correct ancestry and root/case labels. Exit code 7, signal termination and stdout were preserved; a sleeping child exceeded its 0.3-second budget and was reported as timed out with root exit 124. This validates instrumentation behavior, not Incan workload performance. Interrupting the tracer also terminated and reaped its isolated workload, verified by the fixture PID disappearing. Focused-root measurements are recorded in the next section.

## Focused runtime evidence

Actual CLI-root measurements are now retained in [1065-focused-evidence.json](1065-focused-evidence.json), including sanitized invocation intervals and command CPU observations. The runtime snapshot is `d0e89b195b54266fe55bdb942dc4b2817270711b` (compiler `0.6.0-dev.3`, Rust 1.98.0, `aarch64-apple-darwin`), after the static-audit snapshot above. The compiled root lists 134 tests. It uses the warm dev.3 SDK inventory identity `77352b644836e2cb17db90b58c2a0858226d1c8aad7d132051e22cf4313e6eb2`; native Loaf families were not retained. Fixtures are fresh per run. No other compiler build ran during the measurements, but small independent metadata probes were permitted; these are not idle Linux CI benchmarks.

| Selected case(s) | Libtest threads | Result | Intercepted Incan commands | Observed interval peak | Root elapsed |
| --- | ---: | --- | ---: | ---: | ---: |
| `scheduler_nested_build_and_run_fail_closed_when_the_immutable_native_plan_is_absent` | 1 | Pass; both command failures expected | 2 | 1 | 1.956s |
| `concurrent_normal_checks_reuse_sealed_sdk_inventory_without_mutable_publication` | 1 | Pass | 2 | 2 | 1.047s |
| Both cases together, first run | 2 | Pass | 4 | 3 | 2.021s |
| Both cases together, repeat | 2 | Pass | 4 | 3 | 2.110s |
| Both cases together, wrappers disabled | 2 | Pass | Uninstrumented | Unmeasured | 1.642s |
| `run_synchronous_result_main_issue843` | 1 | Native-plan admission refusal | 1 | 1 | 1.110s |
| Same core case, wrappers disabled | 1 | Same admission refusal | Uninstrumented | Unmeasured | 0.870s |
| `compiled_sdk_providers_replace_consumer_fs_source_closure` | 1 | Native-plan admission refusal | 1 | 1 | 1.182s |

The selected concurrent-check case **actually overlaps two Incan command intervals with only one libtest thread**. Combining it with the sequential refusal case produces an observed peak of three Incan command intervals with two libtest threads, reproduced twice. This demonstrates the nested-process multiplier in real CLI tests; source call counts alone could not establish it. The two check commands each took approximately 552ms in the single-case run. Group invocation intervals and resource observations are in the JSON evidence; group records deliberately leave case attribution unset.

Instrumentation is materially visible: the group took 2.021s and 2.110s with wrappers versus one 1.642s uninstrumented control. That is insufficient repetition or isolation to estimate a stable overhead percentage, and it is not an improvement measurement. Do not feed these wrapper-inflated root durations into production scheduling weights.

The two native cases stop at the intended authority boundary: “This project's dependencies have not been compiled yet.” The core case produces the same refusal and build-record identity with wrappers disabled. A warm SDK inventory does not supply a missing immutable native execution plan. The filesystem test does not perform an explicit bake itself, so it stops before its native output and codegraph assertions. No refusal was suppressed or changed. These failures are environmental admission evidence, not a compiler regression finding or a measurement of rustc work.

This closes the bounded runtime instrumentation experiment, not #1065. The next measurement lane must retain compatible native Loafs and run both heavy roots on the pinned Linux CI environment, capturing native descendants as well as Incan invocations. Keep the current thread policy until that workload/resource evidence exists. No per-case scheduler, thread cap or CI change accompanies this research.

## Second heavy-root observation

The same experiment also exercised existing cases from `tests/integration_tests.rs`. To avoid rebuilding unrelated Cargo binaries, a research-only libtest executable was built directly with pinned rustc from source revision `15f5c085d4411f3213f5ea4baa4364a2225304d9`. Its selected source and helper files are unchanged from `d0e89b195`. Runtime Incan remains the stable dev.3 compiler from that earlier snapshot. The direct build listed 280 executable tests, compared with the static source's 281 test attributes, and produced a 185,953,136-byte executable within the 500MiB allowance.

The exact command, feature cfgs, compile-time environment and 54 external-crate paths are preserved with portable root placeholders under `second_root_build` in the evidence JSON. External artifacts were selected by matching the already-built CLI root's Cargo dependency fingerprints, not by choosing arbitrary newest rlibs. The build used edition 2024, `--test`, debug information disabled, optimization level zero, the seven `cli/default/lsp/rust_inspect/std_async/std_decorators/std_testing` feature cfgs, and source-root/dev.3 Cargo environment values. This is a direct-rustc research executable, not a receipt-bound compiler-suite replay or evidence of full Cargo/suite equivalence.

| Selected case(s) | Threads | Result | Incan commands | Observed interval peak | Root elapsed |
| --- | ---: | --- | ---: | ---: | ---: |
| `test_cli_fmt_wraps_long_parenthesized_logical_expression_chain` | 1 | Pass; formatter followed by check | 2 | 1 | 1.234s |
| `build_rejects_unbound_type_annotation_before_generated_rust_issue902` | 1 | Pass; two expected frontend refusals | 2 | 1 | 1.982s |
| Both cases together | 2 | Pass | 4 | 2 | 2.002s |
| Both cases, wrappers disabled | 2 | Pass | Uninstrumented | Unmeasured | 1.649s |

The parent held its compiler build until these runs finished; small independent metadata probes remained permitted. No production runner, thread cap, tests or authority checks were changed. The four additional runs bring the retained evidence to twelve runs, with every recorded peak reproducible from the sanitized intervals. Formatter and legacy `--check` commands appear under the tracer's conservative `other` command category; their exact role is identified by the selected source case, not inferred from an absent argument log.

Both heavy roots therefore have a real, bounded observation of nested CLI work and overlap. This strengthens the correction to the issue's low-subprocess premise, while leaving native build fan-out, full-root resource costs and Linux CI policy acceptance unmeasured. These lightweight cases cannot establish a safe higher cap for the entire integration root.
