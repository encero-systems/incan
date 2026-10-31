# Compiler worker host

`incan_worker_host` is the host of one compiler worker process: the server side of the `incan.compiler-worker/1` protocol and the worker's lifecycle. The compiler driver's `--resident SOCKET GENERATION READY` entry links it, asks it for one request at a time, runs the request through the entry point its short-lived command uses, and gives it the outcome. The host consumes the compiler through that entry; it is not part of the compiler's query interfaces.

**Versioning:** Release-train cadence, with the driver that links it.

| Module | Holds |
| --- | --- |
| `protocol` | The shared request, reply and readiness documents: request construction and admission, reply encoding and completion validation, and readiness encoding and validation. Oven imports this module as the client. |
| `lifecycle` | The retirement policy, apart from any socket: idle period, footprint margin, outcomes. |
| `endpoint` | The worker's socket file, its withdrawal, the idle watchdog, and what a departed caller ends. |
| `host` | The listener, peer identity, one activation at a time, replies and retirement. |

## Contract

- **Entry.** `start(arguments, footprint)` binds the private socket, tells the starting process the worker is ready and starts the idle watchdog. `next_request(host)` returns the next admitted `CompileRequest`, or `None` once the worker has retired and the process should exit. `complete(host, outcome, footprint)` answers it. `caller_watch(host)` names what the entry observes while the compiler runs, and `abandon(watch)` ends a worker whose caller left.
- **Messages.** `CompileRequest`, `WorkerCommand`, `ActivationOutcome` and `CallerWatch` are Incan models and enums of strings, integers and lists; no Rust type crosses the entry.
- **Peers.** Only the user that owns the socket is served. A connection from another user is closed unanswered.
- **Frames.** Framing is `incan_platform.ipc`. A frame that is not a request of this protocol is refused with a reason, and the worker goes on serving. The idle watchdog wakes the worker with an empty frame and waits for its refusal, so at most one wake-up is ever queued, however long an activation runs; it stops once the worker's socket file is gone.
- **One activation at a time.** An entry that asks for another request before completing the open one leaves the process in an unknown state; the host answers the open request and ends the worker.
- **Retirement.** The worker retires after 10 s without a request; when its physical footprint stays more than 16 MiB above its post-start baseline for two activations in a row; at once when its physical footprint after an activation is above 1 GiB, because only exiting returns a heavy compile's peak heap; when a request of another protocol or compiler generation reaches it; and when the process could not be restored after an activation. A retiring worker withdraws its socket before its last reply, marks that reply `"retired": true`, and answers the requests already connected unless the process is unsafe. `INCAN_WORKER_FOOTPRINT_BOUND` lowers, and never raises, the 1 GiB ceiling, so a test can retire a worker after every request.
- **Faults.** A compiler panic the entry contained is answered as a compiler fault, with Rust's panic status 101, and the worker serves the next request.
- **Departed callers.** A caller that leaves while its activation compiles ends the worker with status 101, after the socket is withdrawn, as the short-lived command would have ended with it.

The host's cases are in `loaves/compiler/incan_worker_host/tests/test_worker_host.incn`; `incan test` runs them from this directory, and CI runs them with the Oven tests.
