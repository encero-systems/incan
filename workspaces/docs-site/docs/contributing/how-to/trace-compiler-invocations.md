# Trace compiler invocations

Use invocation tracing to investigate repeated work in a compiler command. It is disabled by default. Ordinary logging still follows `RUST_LOG`; invocation diagnostics have a separate opt-in switch and do not appear in ordinary logs.

## Capture an unchanged command

Run the command once to establish its artifacts. Then run the same command with the same sources, compiler, dependency declarations and Store, saving diagnostics to a fresh directory:

```sh
INCAN_TRACE_DIR=/tmp/incan-invocations incan run
```

Remove `INCAN_TRACE_DIR` for normal runs and performance acceptance measurements. Enabled tracing writes records synchronously and adds overhead; its elapsed times explain the observed execution rather than establish normal performance. Disabled diagnostic fields are not constructed and no diagnostic sink is opened.

Each participating CLI process creates a distinct `PID-TIMESTAMP.jsonl` file without overwriting earlier captures. Retain all files from a capture together. A requested directory that cannot be opened fails before the command executes. A later write failure prints `invocation trace incomplete` once; treat that capture as incomplete.

## Read the records

The `incan.invocations/1` process header records the executable, process ID and clock origin. `begin` records identify an invocation, its parent, function boundary, instrumentation source location and input fields. Selected function boundaries also record their caller's source location. `end` records close the same ID. `event` records attach decisions and actual work counts to their enclosing invocation. IDs include the process ID so files can be combined.

Group invocations by function, caller and input to identify repeated traversal. Equal input labels establish repeated calls; they do not by themselves establish unnecessary compilation. Compare associated reuse/preparation decisions and the command's actual-work reports before drawing that conclusion.

Subtract the union of covered, nested, same-thread child intervals to calculate exclusive covered wall time. This still includes uninstrumented work and is not CPU time. Do not add inclusive intervals across nested functions: that double-counts their shared time. Missing ends, unknown parents and uninstrumented children must remain visible in any summary. Commands that terminate through `process::exit`, including some argument-only paths, may leave an open root span.

## Coverage

The initial lane instruments Store manifest traversal and verification, payload selection, project-output reuse, source-authority hashing, file-digest reuse, inspection-authority loading, public-provider lookup, native-loaf preparation and the bounded subprocess runner. Both compiler CLI entrypoints share the same collector.

The pinned native driver also accepts `INCAN_TRACE_DIR`. Replay its existing receipt-selected `--source` command with the same explicit externs, search paths and sysroot. Its trace separates source compilation, module collection/checking, analysis, Incan lowering and native compilation. The source-compilation span follows work into the compiler-stack thread. For manifest-backed sources with outputs inside the owning project's `target/`, the driver selects completed output through the ordinary Store, source authority and metadata reconciliation before entering these stages. A reuse trace contains `incan_driver::reuse_native_source_output` and no checking, analysis, lowering or native compilation. Standalone sources and outputs outside that generated tree still compile without this project-output optimization. Keep driver compilation and test-fixture preparation outside the workload measurement.

Native reuse binds the driver's bytes, project and resolution inputs, ordered externs and search paths, standard library, output projection and ordinary pinned linker closure. It uses a separate `native-mir` output intent within the existing completed-output Store; it does not select ordinary Rust-emission outputs. Native dependency search checks immediate file members without walking their archived sources. Missing unused externs and search directories participate as absent inputs, so their appearance invalidates reuse. On a cache miss, the driver applies the ordinary platform policy and launches under its explicit environment before starting embedded rustc. A conflicting ambient `SDKROOT` or deployment target does not replace that policy. This driver facility does not change normal commands' backend selection.

Supported bounded subprocess launches pass `INCAN_TRACE_PARENT` so an instrumented child CLI can link its root to the launch invocation. External tools, native test executables and applications do not automatically gain internal instrumentation. Launches outside this helper, children that clear the environment, provider implementation internals, and compiler functions without explicit spans are coverage gaps. The lane is a trace of named boundaries, not an exhaustive function-call profiler. Add a span at the relevant existing boundary when a capture identifies a gap that prevents diagnosis.

## Attribute test preparation separately from execution

Prepare the tests once, then capture an unchanged repeat:

```sh
incan test tests
INCAN_TRACE_DIR=/tmp/incan-test-invocations incan test tests
```

The `incan_cli::test` span covers the command. Its `incan_cli::test_stage` children identify discovery, session construction, candidate selection, lock policy, collection, test selection, dependency admission, scheduling and reporting. Each scheduled `incan_cli::test_batch` has stages for reading sources, preparing modules, dependency requirements, native inputs, analysis, code generation, receipt construction, plan selection, runtime libraries, native output preparation, test execution and outcome mapping. Parallel workers retain the scheduling span as their parent.

Compare `session`, `test_dependency_requirements`, `admit_test_native_inputs` and `test_plan_selection` with `test_execution`. A reused native executable does not imply that frontend or dependency preparation was skipped. `--list` ends after selection and must have no test batch, code generation or execution spans. A keyword-filtered run records the selected batch size; reconcile it with the command's actual test results. Preserve failure captures too: spans close on normal error returns.

Use tracing-disabled commands for performance measurements. These stages attribute the existing test implementation; they do not enable test-result caching or change which tests execute.

On Unix, `incan test --list` uses the same automatically started project owner as ordinary builds. Capture two list requests with the same tracing directory to distinguish first session construction from retained session reuse:

```sh
INCAN_TRACE_DIR=/tmp/incan-test-list-invocations incan test tests --list
INCAN_TRACE_DIR=/tmp/incan-test-list-invocations incan test tests --list
```

The owner retains the canonical parser-only session under the existing project, lock, compiler, feature/profile and provider-input checks. Test and conftest discovery, parsing and selection still run against current source on every request. The unchanged repeat should have no `incan_driver::discover_session`, provider reconstruction, Store manifest reads or native test batches. Collection sessions cannot authorize execution: artifact preparation uses a distinct retained identity. Other platforms invoke the same collection operation in process. Ordinary test execution uses that same owner with artifact-purpose preparation and the existing scheduler. Capture two execution requests with the same tracing directory to inspect retained session reuse separately from dependency admission, generated harness preparation and fresh test execution. The `incan_driver::shared_native_admission` span distinguishes original admission from reuse of the same owners by builds and tests. Changed selected native inputs force admission again. Harness input projection, plan selection, frontend and code generation still run on an unchanged execution request; session and admission retention alone do not eliminate those stages. List timings do not describe execution performance.

## Check native SDK traversal

On an unchanged command, inspect file-observation paths beneath the native compiler installation. Cached native archives are selected by the pinned source and fact receipt and their stored bytes are checked under lease. Reusing them needs no installed native compiler and must not read its SDK tree. Check that native preparation and compilation counts are zero as well as checking file paths; a short elapsed time alone does not establish reuse.

An actual native archive miss selects a virtual toolchain loaf. This descriptor binds the exact compiler bytes, required owner-relative locations and small installation version files. It reads directly named `incan-toolchain.json`, `SDKSettings.json`, `source.properties`, `package.xml` or `SDKManifest.xml` at the installation root or declared directories, without enumerating descendants. A directory installation with none of these files must supply a nonempty `incan-toolchain.json` containing its version/build identity. Version files are limited to 128 KiB and cannot be symlinks. Compiler and explicit regular-file arguments retain byte identities; SDK directory contents are trusted as part of the system installation. Changing a header without changing installation version evidence deliberately does not change this descriptor.

The producer rechecks the descriptor at invocation and persists it in its execution receipt. The cache key retains the original pinned native fact, so updating a host SDK does not invalidate already admitted outputs produced for that fact. Selecting a different compiler/fact or source changes the requested product. Generic byte-owned publisher closures retain recursive validation, and the Rust linker still verifies its explicitly selected files; distinguish these paths from an unwanted whole-SDK walk.

The descriptor logic is independent of host OS. Filename support does not prove compatibility with an installed vendor SDK. Compare equivalent traces on each host and record the actual compiler, SDK evidence and selected paths. Publisher execution also requires platform filesystem confinement; Linux and Windows currently refuse that execution boundary because their confinement implementation is unavailable.
