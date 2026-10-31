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

A span carries run/request/work identities, its parent, producer and operation, a key or redacted locator, input snapshot, monotonic start/end nanoseconds, process and thread, outcome, optional CPU time, and byte/record/call counters. CPU time is null when unavailable. Span identities are scoped to a process and its lifetime; callers must supply the run identity when combining process trees.

The current `rust::std::time::Instant` clock is labeled `process-monotonic`: it cannot expose the host epoch through its public API. Cross-process clock alignment, request-scoped paged trace retention and the compiler writer remain tracked boundaries of #2481 and DD-0013 §11.1; this module does not claim those proof rows. The existing Rust invocation writer is the bootstrap boundary until its host is Incan. No wall timestamp is used to reconstruct elapsed durations.

Oven enables the canonical file channel with `INCAN_ENGINE_TRACE=<existing directory>`, writing `engine-oven-<pid>.jsonl`. `INCAN_ENGINE_RUN` and `INCAN_ENGINE_REQUEST` provide caller correlation across processes; absent overrides, the run is scoped to a process's clock lifetime and the request to the invocation process. Each span separately names that clock lifetime in `clock_id`. A wall-clock reading distinguishes lifetimes only; it never enters durations. `INCAN_JEC_TRACE` retains its existing `oven-<pid>.jsonl` export and `OVEN_TRACE` retains its text export (including the explicitly requested `-` stderr destination). These are format adapters over the platform observations and file writer. A relayed owner text line is not recorded twice as canonical work.
