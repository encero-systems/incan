# How standard packages reach a program

Standard packages in dev.8 are ordinary Loaves, compiled by the native compiler. The package source declares its public API; the library build publishes checked metadata, retained bodies and native artifacts together.

Oven discovers the installed standard-package source manifests from `INCAN_STDLIB` and selects their profile-specific artifacts in the active toolchain home. The unit description names those Loaves. The checker resolves imports against their checked contracts, and native compilation links the implementation the selected artifacts provide.

There is no SDK profile or component-selection layer. A missing compiled package is an installation failure. A `release` build needs `release` artifacts even when `debug` artifacts are present. Standard imports remain `std.*`.

Local application libraries use the same public package boundary. Their source projects are reached through explicit path dependencies; Oven builds them before their consumers. See [Build and consume a library](../tutorials/build_and_consume_library.md).
