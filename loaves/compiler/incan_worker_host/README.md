# Compiler worker host, Incan binding

`incan_worker_host` gives Incan callers the compiler worker's protocols: the client side of the `incan.compiler-worker/1` compile-command protocol, the `incan.compiler-worker/2` record-activation protocol, the typed Crumb packets a `cw3-` worker generation speaks, compiler snapshot descriptors with each home's store binding, and bounded compiler directory reads. Every item is the Rust worker host's, `loaves/compiler/incan_worker_host_native`, re-exported under the reference module paths; the rustc driver's resident worker runs that crate as its host.

**Versioning:** Release-train cadence, with the driver whose worker host it binds.

| Module | Holds |
| --- | --- |
| `protocol` | The version-one compile request, reply and readiness frames, as Oven's compile client writes and reads them. |
| `worker_packets` | The `cw3-` prefix that selects the typed packet generation. |
| `record_protocol` | The `Prepare`, `Prepared`, `NeedPrerequisite`, `Evaluate`, `Produced`, `SemanticRefusal` and `CompilerFault` messages and their closed request and reply sets. |
| `record_snapshot` | Snapshot descriptors, their resolution under a home's binding, and the portable and typed store bindings. |
| `record_directory` | Directory member keys and bounded directory reads. |

The host's behavior, lifecycle and wire bytes are tested in the Rust crate (`incan test` in `loaves/compiler/incan_worker_host_native`); Oven's tests exercise the client side through this binding.
