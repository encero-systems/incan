# Kernel ring

Stable contracts only: deterministic, dependency-light, no runtime side effects. Depends on no other ring; within the ring, `incan_syntax` and `incan_semantics_core` build on `incan_lang` (and `incan_syntax` on `incan_vocab` and `incan_semantics_core`).

**Versioning:** Strict semver, rarely bumped. Everything else expresses a requirement on it.

| Directory | Purpose |
| --- | --- |
| `incan_lang/` | Language vocabulary, keyword and builtin tables, the stdlib registry, and shared semantic helpers. |
| `incan_syntax/` | Lexer, parser, AST, and the diagnostics catalog. |
| `incan_semantics_core/` | Surface-semantics contracts and registry interfaces. |
| `incan_vocab/` | Vocabulary descriptors, native desugarer metadata, and shared registration and syntax contracts. |
| `incan_codegraph/` | Stable codegraph fact schema for tooling and agent context. |

The ring rules live in *Repository layout* in `workspaces/docs-site/docs/contributing/explanation/architecture.md`; the migration is #1478's history.
