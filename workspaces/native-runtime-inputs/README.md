# Native runtime identity

This private Incan workspace projects build-unit inputs from an explicit canonical native receipt catalog, compiler identity, provider records, selected stdlib facets and portable dependency declarations. It reads no Cargo lock or manifest. Required runtime unit names come from the existing toolchain registry through the transport; this workspace does not maintain a second runtime registry.

The file exchange supports runtime projection and dependency-root projection. Both validate the catalog and bind the response to the exact request. Runtime projection additionally requires all declared runtime units. Dependency declarations retain aliases, package renames, version requirements, features, default-feature activation and optionality. Path declarations carry a source kind rather than a machine-local directory. Existing native selection must subsequently verify the corresponding admitted source, version, features and domain; an identity digest alone grants no compilation authority.

The compiler transport is removable debt for the minimized API gap tracked by [#1698](https://github.com/encero-systems/incan/issues/1698). The Incan bootstrap prepares this executable internally and binds its bytes in the source compiler's receipt before installing it beside that compiler. Compiler calls verify the compiler output/receipt binding and the engine bytes. A native projection refusal cannot authorize a Cargo fallback.

Six focused tests exercise binding and refusal. They do not prove compiler integration, installed-toolchain packaging, Cargo metadata independence of every route, or Just Enough Compilation across compiler edits. Those remain separate recovery gates.
