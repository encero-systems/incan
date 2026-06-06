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
