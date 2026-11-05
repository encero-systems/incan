# Vocabulary definition contracts

This ambient standard Loaf owns the bootstrap-safe descriptor, scoped-surface, syntax-model, version and transport declarations used to define Incan vocabularies. It publishes no vocabulary entry and activates no syntax.

Authors use the identity-preserving `std.primitives.vocab` and `std.vocab` re-exports. Oven presents this lower Loaf as an ambient toolchain-provided facade under its package identity, so no manifest declares it and it owns no third `std` namespace. Keeping the definitions below `incan_stdlib_primitives` lets the primitives vocabulary entry import its descriptor contracts without importing its own not-yet-admitted surface.
