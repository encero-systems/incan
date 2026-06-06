# Release Surface Gates

Release-surface gates are small checks that keep public release intent, documentation, generated references, and representative tests connected. They are not a replacement for behavioral tests or downstream proof lanes. They are meant to catch the recurring failure mode where a feature lands but the release notes, feature inventory, CLI reference, or named regression coverage does not move with it.

For the 0.4 line, run:

```bash
make release-0-4-surface-gate
```

The gate checks the staged 0.4 release surfaces from the roadmap:

- boundary parity and symbol identity;
- preheat and test-runner observability;
- stable diagnostics and `incan explain`;
- build reports and generated Rust inspection;
- codegraph inspection;
- SDK installer and zero-clone starter flow;
- release direction and scope guard.

Each row in `scripts/check_release_surface.py` names the files and snippets that prove the surface remains wired together. Prefer adding one precise requirement to that matrix over writing another long test that exercises the same compiler behavior. If a public 0.4 command, feature-inventory entry, or release-note bullet is renamed intentionally, update the matrix in the same PR.

The final release hardening loop should still run the real gates:

```bash
make fmt
make pre-commit
make smoke-test
make docs-build
make release-0-4-surface-gate
```

Use downstream acceptance as a proof lane, not as the only regression source. InQL, Pallay, and other downstream packages are useful because they exercise real surfaces, but synthetic Incan fixtures should remain the first line of defense for import, package, vocab, diagnostics, generated-Rust, and starter-flow regressions.

## Downstream Timing Evidence

The Stage 2 preheat/observability lane should keep at least one downstream timing observation attached to the release branch while 0.4 is under active development. On 2026-06-06, an isolated InQL `origin/main` worktree at `de58b8e733d80ced70d85499e74660d277f6e132` was run with `incan 0.4.0-dev.7` from `chore/223-release-integration-hardening`: a cold `make build INCAN=...` reported `rust-inspect prewarm start: 216 item(s)` and completed metadata prewarm with `warmed=89 skipped=127 elapsed_ms=61533`; after the cache was warm, the same lane reported all 216 item names and completed metadata prewarm in `elapsed_ms=44`. The same proof run also verified that generated-library dependency preheat targets the generated library Cargo project and real shared target directory (`target/lib` with `target/.cargo-target`) rather than the rust-inspect lock workspace. The final downstream Cargo build then failed on unrelated generated Rust type errors in InQL, so this note is timing and observability evidence, not a downstream acceptance pass.
