# Platform

`incan_platform` is the OS-facing layer below `std`. It is the one home of the OS primitives that the build engine, the compiler's worker host and `std` share: the store substrate, memory mapping, durable I/O, IPC with peer identity, process control, resource probes and tracing. `std` wraps the platform for users, and the engine builds on it directly.

**Versioning:** Release-train cadence; the toolchain bundle pins the generation its tools were built with.

| Module | Holds |
| --- | --- |
| `ipc` | Bounded, length-framed transport over Unix sockets, with every frame length checked before its buffer is allocated; per-direction data credits; OS peer identity; listeners, liveness probes, fresh session identities, and a connection's descriptor lent to native code that only observes it. |

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
