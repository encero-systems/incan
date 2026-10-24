# Build artifact storage

Dev.8 keeps native build artifacts in the project or installed toolchain that owns them. It does not materialize a generated Cargo project or manage a generated-Cargo cache.

| Artifact | Location |
| --- | --- |
| Program executable | `<project>/target/incan/<profile>/bin/<crate>` |
| Path dependency library and sidecars | `<consumer>/target/incan/loaves/<package>/<profile>/` |
| Standard package library and sidecars | `<home>/toolchains/0.6.0-dev.8/loaves/<package>/<profile>/` |
| Unit input observations | `<project>/target/incan/observations/` |
| Graph inspection cache | `<project>/target/incan/codegraph/` |
| Resident owner endpoints | `<home>/owners/` |

A library's native output and checked sidecars form one artifact set. Copying only an rlib or only an `.incnlib` does not preserve the package. Toolchain packing collects the installed closure and rewrites its path-bearing documents for relocation.

Executable reuse depends on observed input and output identities. Replacing a file can invalidate reuse even when its length and modification time match. Owner reuse avoids repeated preparation; it does not skip fresh program or test execution.

Stop commands using a generated tree before removing it. Keep source, manifests and installed standard packages separate from disposable project build output. The CLI has no cache-inspection or cache-pruning command.
