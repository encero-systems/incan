# Checked C interop

A C binding declares the Incan-facing names, scalar categories, native symbol spellings, ownership and output contracts. Its source is the authority; a header verifies that declaration rather than inventing the application API.

A target probe checks supported signatures and declared enum or layout facts without executing the C library. A successful probe does not establish that a deployable application has been linked, packaged or signed.

The application should use an ordinary Incan facade around a private raw binding. An `unsafe:` call exposes the foreign boundary; the facade gives it domain meaning and keeps resource or output handling explicit.

A package states what each target requires in `[interop.c]`: headers, definitions, compatible toolchain and SDK capabilities, mobile platform facts, and the static archives, bundled runtime libraries and system capabilities it needs. These are requirements, never a record of an installation: a check verifies against the host, or against one declared target with `--interop-target`, and holds the compiler and SDK it selects to those requirements. An ordinary build links the declared static archives and compiles the project's own C (a `[c]` facet). Staging copies bundled runtime libraries into a fixed Android or iOS layout for a platform packager; it does not build, sign or assemble an application.

See [Work with bindings](../how-to/checked_c_bindings.md) for source organization and [`std.interop`](../reference/stdlib/interop.md) for the language contract.
