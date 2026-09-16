# Kernel ring

Stable contracts only: deterministic, dependency-light, no runtime side effects. Depends on nothing in this repository.

**Versioning:** Strict semver, rarely bumped. Everything else expresses a requirement on it.

| Directory | Purpose |
| --- | --- |
| `incan_lang/` | Language vocabulary, keyword and builtin tables, the stdlib registry, and shared semantic helpers. The crate arrived as `incan_core/` in step 4b and took this name in step 5. |
| `incan_syntax/` | Lexer, parser, AST, and the diagnostics catalog. |
| `incan_semantics_core/` | Surface-semantics contracts and registry interfaces; `incan_semantics` once the crates are renamed (step 5). |
| `incan_vocab/` | Vocabulary registration contract for companion crates, including the WASM desugar ABI constants. |
| `incan_codegraph/` | Stable codegraph fact schema for tooling and agent context. |

The ring rules live in *Repository layout* in `workspaces/docs-site/docs/contributing/explanation/architecture.md`; the migration is #1478's history.
