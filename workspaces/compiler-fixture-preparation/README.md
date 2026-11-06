# Native fixture runtime preparation

This private Incan workspace prepares the formatting runtime for the native compiler test kernel. It materializes the current authored Incan runtime and its math dependency declaration, then invokes the selected compiler to publish the runtime library itself. It returns the root only after the owning debug receipt and native library exist. The consumer still selects and validates that receipt before linking.

The prior Rust fixture helper baked a generated Rust caller and returned the runtime source root. Caller preparation published the dependency beneath the caller's output tree and restored the runtime root's previous receipt state, so a new fixture had neither the receipt nor native library where the consumer expected them. A diagnostic pre-bake made the numeric comparison pass, proving that handoff mismatch without establishing ordinary-command correctness. The failed and seeded runs are retained in the dev.7 recovery evidence directory.

[#1698](https://github.com/encero-systems/incan/issues/1698) tracks this compiler development boundary. The existing native libtest kernel has no Incan fixture hook; its minimal transport invokes this workspace through the exact selected compiler and consumes the typed result. That transport is removable when fixture orchestration is hosted directly in Incan. The workspace contains no authored Rust caller and requires no manual preparation by the user.

Compiler execution, unchanged reuse and behavior remain separate acceptance gates. File existence alone does not authorize reuse or an unverified library.
