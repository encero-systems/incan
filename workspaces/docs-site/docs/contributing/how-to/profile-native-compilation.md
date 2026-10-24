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

## Attribute the intervals

Use the driver's `compile_unit`, `checked_modules`, `lower_program` and `native_compile` spans to locate compilation work. Match begin and end records by ID. The process record's Unix `epoch_ns` plus a record's relative `time_ns` places it on the same clock as Oven's launch timestamp.

Within native compilation, inspect `macro_expand_crate`, `type_check_crate`, `MIR_borrow_checking`, `codegen_crate`, `finish_ongoing_codegen`, `get_apple_sdk_root` and `run_linker`. The enclosing `link` includes SDK discovery and the linker. Timings nest or overlap; adding every reported phase double-counts work. Code signing performed inside the linker has no separate timing in this capture.

Oven writes `oven: launch epoch=SECONDS program=PATH` immediately before a program's `exec` or a collected test's spawn. Compare that timestamp with the observed first output, or test completion, to separate preparation from execution. That interval includes operating-system launch checks, dynamic loading and program work before the observation; the timestamp alone cannot distinguish those causes. It also includes writing the trace line. Program output and test outcome rules are unchanged.

Repeat a newly linked executable directly to compare first and later launch costs. File faults and child CPU time can narrow a hypothesis, but identifying a system service requires an operating-system trace. Do not infer a specific service from elapsed time alone.

## Measure ordinary latency

Unset `INCAN_TRACE_DIR`, `INCAN_NATIVE_TIMING` and `OVEN_TRACE`. Retain every sample and report medians, ranges and the number below the target. Keep edited sources distinct, use a new directory for each new-project sample, and measure unchanged invocations separately. Keep dependency builds and unrelated experiments outside the measured interval.
