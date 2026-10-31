# Platform

`incan_platform` is the OS-facing layer below `std`. It is the one home of the OS primitives that the build engine, the compiler's worker host and `std` share: the store substrate, memory mapping, durable I/O, IPC with peer identity, process control, resource probes and tracing. `std` wraps the platform for users, and the engine builds on it directly.

**Versioning:** Release-train cadence; the toolchain bundle pins the generation its tools were built with.

| Module | Holds |
| --- | --- |
| `trace` | Versioned engine spans, monotonic clocks, aggregate counters and bounded file channels. |
| `ipc` | Bounded, length-framed transport over Unix sockets, with every frame length checked before its buffer is allocated; per-direction data credits; OS peer identity; listeners, liveness probes, fresh session identities, and a connection's descriptor lent to native code that only observes it. |
| `durability` | Ordinary filesystem synchronization through `sync_ordered(path: str) -> Result[None, str]`, for a regular file or directory. Publishers flush the ordered graph before selecting it and the selection before acknowledging it; ordinary synchronization alone is not a drive-cache flush. |

## Contract

- **It is not compiler work.** The platform never imports compiler or language-kernel Loaves, and never imports `std` packages. `std` wraps the platform, never the other way round.
- **It wraps; it never reimplements.** Each primitive reaches its OS or Rust facility thinly, through interop (`rust::std`). Low level means precise and minimal, not homegrown.
- **It carries no protocol content.** IPC carries bytes and peer identity only. Encoding a payload and knowing a message family belong to the protocol that sends it, so the owner and engine protocols live with Oven, and the compiler worker's side of `incan.compiler-worker/1` lives with its host, `incan_worker_host`.
- **It bounds requests, not stores.** A bound applies to one frame or one read, never to what a connection or store holds over its lifetime.

A consumer declares a path dependency and imports a module by name:

```toml
[dependencies]
incan_platform = { path = "../../loaves/platform" }
```

```incan
from pub::incan_platform.ipc import Connection, dial, listen
```

## Observations

`trace` owns the `incan.engine.trace/1` JSONL schema, monotonic operation clocks, aggregated byte/record/call counters and retained file channels. `begin(false)` returns before touching clock state; `record("", span)` returns before encoding. A record has at most 65,536 UTF-8 bytes, and a text field at most 4,096. Invalid durations, counters and failed channels return `TraceError`; no diagnostic is written to a program stream. JSON string escaping delegates to the existing native `serde_json` serializer, including control characters. Consumers own retention and correlation identities; traces do not grant build or cache authority.

`open_writer(path)` returns a typed `Writer` with a 65,536-byte native `BufWriter` and a separate fixed encoding slice of the same size. `Writer.record(span)` validates the span and serializes it through native serde into that slice; overflow refuses the record without writing a prefix. `Writer.append(text)` rejects oversized text before buffering it. Writes that fill the output buffer synchronously drain to the OS, providing back-pressure. `Writer.flush()` drains that channel. Copies share the writer, encoding storage and locks. The `record` and `append` convenience functions retain at most eight named channels per process, opening each once; `flush()` drains every retained channel and reports any failure after attempting them all. These are bounds on producer resources, never on file size. Callers must flush before normal process return, explicit exit, or process replacement; abrupt termination can leave the last buffer unwritten. Oven flushes at all three lifecycle boundaries. An explicitly owned writer also has the native buffer's best-effort drop behavior, but that is not a substitute for checking `flush()`.

`Emitter.open(path, producer, run, request)` retains the channel and validates its invariant text fields once. `Emitter.emit(clock, operation, key, bytes)` takes a fixed `Operation` enum, checks dynamic bounds and timing, and serializes the wire record directly without an intermediate public span or JSON value. It emits one completed call with `calls=1`; byte counts describe the logical input once. The general `Writer.record` API still validates caller-built spans. The emitter uses the calling thread identity for each event and participates in the same process-wide flush boundary.

A span carries run/request/work identities, its parent, producer and operation, a key or redacted locator, input snapshot, monotonic start/end nanoseconds, process and thread, outcome, optional CPU time, and byte/record/call counters. CPU time is null when unavailable. Span identities are scoped to a process and its lifetime; callers must supply the run identity when combining process trees.

Trace records label their clock `host-monotonic-raw`: nanoseconds on the host's raw monotonic clock, shared by processes on the same boot. macOS uses `mach_absolute_time` and its timebase conversion; Linux uses `clock_gettime(CLOCK_MONOTONIC_RAW)`. Both exclude suspend time and wall-clock adjustments. The safe `rust::incan_platform_clock::raw_uptime_ns() -> Result[u64, Error]` binding lives in this Loaf's `rust/` facet. It is the platform's only unsafe Rust, sanctioned because RFC 005 refuses unsafe Rust calls from Incan. OS calls and conversion stay behind that safe signature; tracing, lifecycle policy and encoding remain Incan. Unsupported targets, failed clock reads and values outside the schema's signed nanosecond range refuse observations rather than substituting another clock. Raw nanoseconds compare only within one host boot, never across hosts or reboots. Request-scoped paged trace retention remains a tracked boundary of #2481 and DD-0013 §11.1; no wall timestamp reconstructs an elapsed duration.

Oven enables the canonical file channel with `INCAN_ENGINE_TRACE=<existing directory>`, writing `engine-oven-<pid>.jsonl`. `INCAN_ENGINE_RUN` and `INCAN_ENGINE_REQUEST` provide caller correlation across processes; absent overrides, the run is scoped to a process's clock lifetime and the request to the invocation process. Each span separately names its process lifetime in `clock_id`, which scopes span identities rather than changing the shared timestamp epoch. A wall-clock reading distinguishes process lifetimes only; it never enters durations. The compiler worker host writes `engine-worker-<pid>.jsonl` through the same platform emitter and flushes its activation before replying, so the engine request encloses its worker interval. `INCAN_JEC_TRACE` retains its existing `oven-<pid>.jsonl` export and `OVEN_TRACE` retains its text export (including the explicitly requested `-` stderr destination). These are format adapters over the platform observations and file writer. A relayed owner text line is not recorded twice as canonical work.

Oven retains separate file-read, JSON-decode, parse and hash spans: the phases measure different work even when their input locator is the same. Repeated reads remain visible because the underlying work actually repeats. Fixed operation names are checked at call sites rather than accepted as arbitrary text; reducing the emitter cost preserves those timing facts.

When canonical and JEC exports are requested together, Oven completes the operation once and projects that same span into both formats, preserving identical end timestamps and byte counts. The canonical-only configuration uses the retained fixed-key emitter.
