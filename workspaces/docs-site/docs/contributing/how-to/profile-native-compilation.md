# Profile native compilation and launch

Use this procedure to separate a changed Incan unit's preparation, native compilation and launch time. The native driver records compiler spans; Oven records the boundary immediately before executing the program or spawning a collected test. These observations diagnose latency. Measure acceptance timings separately with tracing disabled.

## Capture a changed unit

Run the program once to prepare its dependencies:

```sh
incan run src/main.incn
```

Edit its source, then capture the changed invocation in a fresh directory:

```sh
OVEN_OWNER=off INCAN_TRACE_DIR=/tmp/incan-native-trace INCAN_NATIVE_TIMING=1 OVEN_TRACE=- incan run src/main.incn
```

For a test unit, replace `run src/main.incn` with `test tests/test_value.incn`. `OVEN_OWNER=off` keeps the capture in this invocation's environment; a project owner can retain the tracing settings it inherited at startup. Change the source before capturing: an unchanged executable can be reused and has no new compiler phases to report. Keep successful test payloads distinct between measurement batches too; an operating system may recognize a binary it has already validated even when it was just rebuilt.

`INCAN_TRACE_DIR` produces the native driver's `PID-TIMESTAMP.jsonl` records. `INCAN_NATIVE_TIMING`, when set, additionally writes rustc phase timings as `time: {"pass":...,"time":...}` lines on standard error; `time` is seconds. It enables timing diagnostics within the pinned driver without granting unstable command-line options. Oven relays a resident compiler's diagnostics through the ordinary request console.

The requested diagnostic settings select the resident project owner as well as the compiler worker. Enabling or disabling them takes effect on the next command, including when this project already has a running owner. The compiler phase timings also apply to hand-written Rust units compiled through the driver.

## Attribute the intervals

Use the driver's `compile_unit`, `checked_modules`, `lower_program` and `native_compile` spans to locate compilation work. Match begin and end records by ID. The process record's Unix `epoch_ns` plus a record's relative `time_ns` places it on the same clock as Oven's launch timestamp.

Within native compilation, inspect `macro_expand_crate`, `type_check_crate`, `MIR_borrow_checking`, `codegen_crate`, `finish_ongoing_codegen`, `get_apple_sdk_root` and `run_linker`. The enclosing `link` includes SDK discovery and the linker. Timings nest or overlap; adding every reported phase double-counts work. Code signing performed inside the linker has no separate timing in this capture.

The driver also emits `native-time: {"boundary":...,"elapsed":...,"interval":...}`. Seconds are measured from entry to that invocation and from its previous boundary. These records separate plan validation, shared-library preparation, argument/search preparation, rustc options, session setup, Incan declaration injection, expansion, native signature verification, analysis, dynamic-input observation and the combined codegen/link/teardown interval. Compare that last interval with rustc's nested codegen and link phases to locate teardown. `unit_descriptor` uses a separate origin immediately before reading the unit JSON. These opt-in measurements include diagnostic writes; they are not acceptance timings.

Oven's `oven: prepare epoch=SECONDS boundary=NAME` records locate command entry, project and owner selection, owner receipt, current toolchain, prepared unit, source key, descriptor writing and worker selection/connection on the same Unix clock. Compare successive boundaries and the driver's first span to attribute work before compilation, including transport or scheduling gaps. Owner records are relayed with the reply, so use their timestamps rather than their display order.

Oven writes `oven: launch epoch=SECONDS program=PATH` immediately before a program's `exec` or a collected test's spawn. Compare that timestamp with the observed first output, or test completion, to separate preparation from execution. That interval includes operating-system launch checks, dynamic loading and program work before the observation; the timestamp alone cannot distinguish those causes. It also includes writing the trace line. Program output and test outcome rules are unchanged.

Repeat a newly linked executable directly to compare first and later launch costs. File faults and child CPU time can narrow a hypothesis, but identifying a system service requires an operating-system trace. Do not infer a specific service from elapsed time alone.

## Iterate on hand-written Rust

Debug builds of Rust facets, libraries, binaries and test harnesses use rustc incremental compilation. To use the same scratch reuse while iterating on an optimized compiler build, select it explicitly:

```sh
INCAN_RUST_INCREMENTAL=1 incan build loaves/toolchain/incan-rustc-driver --profile release
```

Set `INCAN_RUST_INCREMENTAL=0` to disable it, including for debug builds. An unset variable enables it only for debug; `1` enables it for release iteration as well. Ordinary release builds and `scripts/release-dev8.sh` qualification remain non-incremental. Use qualification outputs for packaging; iteration changes rustc's code generation policy and symbol namespace.

Scratch lives below the requesting build's `target/incan/rust/<profile>/`, separately for each library, binary and test role. It can be removed to recover disk space without invalidating the source or published artifacts. The scratch directory is never part of a unit identity, store record or bundle. Freshness still checks all source and linked inputs; a stable iteration symbol namespace lets rustc reuse unchanged work across edits. Incan-generated native units remain non-incremental because their pinned adapter does not yet track per-definition query inputs.

Measure an initial iteration build separately from an edit rebuild: the first creates rustc's scratch. For an edit/revert comparison, retain both sets of contract verdicts and compare them with the clean build. Record wall time, profile, revision, sample count and scratch disk size; a warm filesystem cache or shared host must be identified in the measurement.

## Measure ordinary latency

Debug `run` and `test` reuse native shared libraries. Warm the dependency identities before collecting an edit batch; their first preparation is a separate cold condition. `build` outputs and release executables retain their ordinary archive linkage. See [Native linkage during development](../explanation/development-linking.md).

Unset `INCAN_TRACE_DIR`, `INCAN_NATIVE_TIMING` and `OVEN_TRACE`. Retain every sample and report medians, ranges and the number below the target. Keep edited sources distinct, use a new directory for each new-project sample, and measure unchanged invocations separately. Keep dependency builds and unrelated experiments outside the measured interval.
