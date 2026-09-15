# Kernel ring

Stable contracts only: deterministic, dependency-light, no runtime side effects. Depends on nothing in this repository.

**Versioning:** Strict semver, rarely bumped. Everything else expresses a requirement on it.

| Directory | Purpose |
| --- | --- |
| `incan_lang/` | Language vocabulary, keyword and builtin tables, the stdlib registry, and shared semantic helpers. The crate is here as `incan_core/` since step 4b and takes this name in step 5. |
| `incan_syntax/` | Lexer, parser, AST, and the diagnostics catalog. |
| `incan_semantics_core/` | Surface-semantics contracts and registry interfaces; `incan_semantics` once the crates are renamed (step 5). |
| `incan_vocab/` | Vocabulary registration contract for companion crates, including the WASM desugar ABI constants. |
| `incan_codegraph/` | Stable codegraph fact schema for tooling and agent context. |

See [`LAYOUT.md`](../LAYOUT.md) for the ring rules and the migration order.
