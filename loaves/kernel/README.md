# Kernel ring

Stable contracts only: deterministic, dependency-light, no runtime side effects. Depends on nothing in this repository.

**Versioning:** Strict semver, rarely bumped. Everything else expresses a requirement on it.

| Directory | Purpose |
| --- | --- |
| `incan_lang/` | Language vocabulary, keyword and builtin tables, the stdlib registry, and shared semantic helpers. |
| `incan_syntax/` | Lexer, parser, AST, and the diagnostics catalog. |
| `incan_semantics/` | Surface-semantics contracts and registry interfaces. |
| `incan_vocab/` | Vocabulary registration contract for companion crates, including the WASM desugar ABI constants. |
| `incan_codegraph/` | Stable codegraph fact schema for tooling and agent context. |

See [`LAYOUT.md`](../LAYOUT.md) for the ring rules and the migration order.
