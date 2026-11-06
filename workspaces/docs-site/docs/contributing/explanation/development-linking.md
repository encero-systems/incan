# Native linkage during development

Debug `incan run` and `incan test` reuse shared native libraries for the installed runtime and reached Loaves. Oven requests this policy for those commands; the compiler derives each shared library from the exact existing native artifacts and retains it in the home's native store. An edit compiles the program's own code while reusing the shared dependencies. The first use of a new native input identity also prepares its shared library.

Checking still runs for every edited unit. The shared libraries retain the original native declaration and type identities, so reuse does not substitute for checking the current source or verifying native signatures. Each executable records the dynamic libraries it needs. Oven validates those libraries before reusing the executable; a missing or changed library requires preparation again.

These development executables depend on the installed home and pinned Rust toolchain. Use `incan build` for an output you intend to copy or deploy. Build outputs and all release executables retain ordinary archive linkage for their Rust and Loaf dependencies; platform system libraries follow the existing target policy.
