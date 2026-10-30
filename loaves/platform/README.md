# Platform

`incan_platform` is the OS-facing layer below `std`. It is the one home of the OS primitives that the build engine, the compiler's worker host and `std` share: the store substrate, memory mapping, durable I/O, IPC with peer identity, process control, resource probes and tracing. `std` wraps the platform for users, and the engine builds on it directly.

**Versioning:** Release-train cadence; the toolchain bundle pins the generation its tools were built with.

| Module | Holds |
| --- | --- |
| `trace` | Versioned engine spans, monotonic clocks, aggregate counters and bounded file channels. |
| `ipc` | Bounded, length-framed transport over Unix sockets, with every frame length checked before its buffer is allocated; per-direction data credits; OS peer identity; listeners, liveness probes and fresh session identities. |

## Contract

- **It is not compiler work.** The platform never imports compiler or language-kernel Loaves, and never imports `std` packages. `std` wraps the platform, never the other way round.
- **It wraps; it never reimplements.** Each primitive reaches its OS or Rust facility thinly, through interop (`rust::std`). Low level means precise and minimal, not homegrown.
- **It carries no protocol content.** IPC carries bytes and peer identity only. Encoding a payload and knowing a message family belong to the protocol that sends it, so the owner, compiler-worker and engine protocols live with Oven.
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

`trace` owns the `incan.engine.trace/1` JSONL schema, monotonic operation clocks, aggregated byte/record/call counters and bounded synchronous file writes. `begin(false)` returns before touching clock state; `record("", span)` returns before encoding. A record has at most 65,536 UTF-8 bytes, and a text field at most 4,096. Invalid durations, counters, unsupported control characters and failed channels return `TraceError`; no diagnostic is written to a program stream. Synchronous writes supply back-pressure and retain no event queue. Consumers own retention and correlation identities; traces do not grant build or cache authority.

A span carries run/request/work identities, its parent, producer and operation, a key or redacted locator, input snapshot, monotonic start/end nanoseconds, process and thread, outcome, optional CPU time, and byte/record/call counters. CPU time is null when unavailable. Span identities are scoped to a process and its lifetime; callers must supply the run identity when combining process trees.

The current `rust::std::time::Instant` clock is labeled `process-monotonic`: it cannot expose the host epoch through its public API. Cross-process clock alignment, request-scoped paged trace retention and the compiler writer remain tracked boundaries of #2481 and DD-0013 §11.1; this module does not claim those proof rows. The existing Rust invocation writer is the bootstrap boundary until its host is Incan. No wall timestamp is used to reconstruct elapsed durations.
