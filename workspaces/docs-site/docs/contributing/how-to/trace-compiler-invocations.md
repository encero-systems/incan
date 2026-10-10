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

Supported bounded subprocess launches pass `INCAN_TRACE_PARENT` so an instrumented child CLI can link its root to the launch invocation. External tools, native test executables and applications do not automatically gain internal instrumentation. Launches outside this helper, children that clear the environment, provider implementation internals, and compiler functions without explicit spans are coverage gaps. The lane is a trace of named boundaries, not an exhaustive function-call profiler. Add a span at the relevant existing boundary when a capture identifies a gap that prevents diagnosis.

## Check native SDK traversal

On an unchanged command, inspect file-observation paths beneath the native compiler installation. Cached native archives are selected by the pinned source and fact receipt and their stored bytes are checked under lease. Reusing them needs no installed native compiler and must not read its SDK tree. Check that native preparation and compilation counts are zero as well as checking file paths; a short elapsed time alone does not establish reuse.

An actual native archive miss selects a virtual toolchain loaf. This descriptor binds the exact compiler bytes, required owner-relative locations and small installation version files. It reads directly named `incan-toolchain.json`, `SDKSettings.json`, `source.properties`, `package.xml` or `SDKManifest.xml` at the installation root or declared directories, without enumerating descendants. A directory installation with none of these files must supply a nonempty `incan-toolchain.json` containing its version/build identity. Version files are limited to 128 KiB and cannot be symlinks. Compiler and explicit regular-file arguments retain byte identities; SDK directory contents are trusted as part of the system installation. Changing a header without changing installation version evidence deliberately does not change this descriptor.

The producer rechecks the descriptor at invocation and persists it in its execution receipt. The cache key retains the original pinned native fact, so updating a host SDK does not invalidate already admitted outputs produced for that fact. Selecting a different compiler/fact or source changes the requested product. Generic byte-owned publisher closures retain recursive validation, and the Rust linker still verifies its explicitly selected files; distinguish these paths from an unwanted whole-SDK walk.

The descriptor logic is independent of host OS. Filename support does not prove compatibility with an installed vendor SDK. Compare equivalent traces on each host and record the actual compiler, SDK evidence and selected paths. Publisher execution also requires platform filesystem confinement; Linux and Windows currently refuse that execution boundary because their confinement implementation is unavailable.
