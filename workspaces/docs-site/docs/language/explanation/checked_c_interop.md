# Checked C interop

A C binding declares the Incan-facing names, scalar categories, native symbol spellings, ownership and output contracts. Its source is the authority; a header verifies that declaration rather than inventing the application API.

A target probe checks supported signatures and declared enum or layout facts without executing the C library. A successful probe does not establish that a deployable application has been linked, packaged or signed.

The application should use an ordinary Incan facade around a private raw binding. An `unsafe:` call exposes the foreign boundary; the facade gives it domain meaning and keeps resource or output handling explicit.

Dev.8 accepts declared header paths through `[interop.c]` and checks source through the native compiler. It has no interop deployment-plan, bake or stage CLI. Platform packaging and native dependency provisioning require their own verified inputs.

See [Work with bindings](../how-to/checked_c_bindings.md) for source organization and [`std.interop`](../reference/stdlib/interop.md) for the language contract.
