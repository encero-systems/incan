# Oven Alpha benchmark protocol

Use this protocol to measure the DX-recovery lane: explicit release-envelope preparation into an empty developer
environment, then repeated normal Oven `build`, `run`, or `test` commands. It is separate from generated-program
runtime benchmarks. The measured normal commands and prepared compiler-suite replay run with Cargo guarded out; only
an explicit `incan oven bake` miss or the separately named compiler-suite publisher may use Cargo.

The harness is deliberately strict. It starts with an empty `INCAN_HOME`, records an explicit `incan oven bake` where
the workload needs preparation, records the first normal command, then records unchanged normal-command repeats.
The first command is not labelled warm. A required failing `cargo` executable is probed to confirm that it exits with
status 97, then prepended to `PATH`; a successful normal stage therefore proves that it did not launch Cargo.

## Reference-machine requirements

Run the same supported workload on one documented macOS machine and one documented Linux machine. Record the checkout revision, release archive/artifact identity, `incan --version`, OS/architecture, exact source fixture and digest, profile, storage limits, and whether the store started empty. Keep the generated `report.json` and per-phase logs with the release evidence. The harness requires an archive or CI-artifact identity rather than silently treating an arbitrary local binary as a comparable measurement.

The documented Alpha envelope is intentionally finite. The release archive ships one complete standard-library Loaf
family with two profile variants: debug and release. Each immutable variant contains the checked full
standard-library/provider closure, its direct-`rustc` plan, sealed registry-source authority, provenance, digests,
and byte accounting. An unsupported provider/dependency closure must fail explicitly or be prepared through the
public `incan oven bake` boundary; do not invoke its hidden publisher directly to make a benchmark pass.

## Run a guarded test workload

Extract the candidate release archive and use the matching Incan checkout for the harness and its fixtures; those benchmark assets are deliberately not shipped inside the toolchain archive. Then create a task-specific failing Cargo guard outside the repository. The guard proves that the tested normal command cannot accidentally invoke Cargo.

```bash
mkdir -p /tmp/incan-oven-cargo-guard
printf '#!/bin/sh\necho unexpected Cargo launch >&2\nexit 97\n' > /tmp/incan-oven-cargo-guard/cargo
chmod +x /tmp/incan-oven-cargo-guard/cargo

bash scripts/bench_oven_alpha.sh \
  --incan /path/to/extracted/incan/bin/incan \
  --release-identity 'incan-VERSION-TARGET.tar.gz sha256:...' \
  --rustc "$(rustup which --toolchain stable rustc)" \
  --checkout-revision "$(git rev-parse HEAD)" \
  --workload test \
  --source tests/fixtures/test_assert_canary.incn \
  --incan-home /tmp/incan-oven-test-home \
  --output /tmp/incan-oven-test-evidence \
  --cargo-guard-dir /tmp/incan-oven-cargo-guard \
  --repetitions 2
```

To prove that reuse is not tied to the original checkout, create a clean worktree at the same revision and pass its byte-identical fixture as `--clean-worktree-source`. The harness rejects a different source digest rather than calling two unrelated commands a reuse measurement:

```bash
git worktree add --detach /tmp/incan-oven-benchmark-clean HEAD
# Add --clean-worktree-source /tmp/incan-oven-benchmark-clean/tests/fixtures/test_assert_canary.incn
```

For build or run, set `--workload build` or `--workload run` and use a small project inside the documented release envelope; the checked sources that bake that envelope live in `src/oven/fixtures/`. A `std.testing` fixture is debug-only and is intentionally not a release-build benchmark. On Linux, use a task-specific directory below `/tmp`; the store must start empty so its first materialization is attributable. The default policy is the normal developer policy: 3 GiB aggregate physical allocation, 1 GiB physical allocation per compatibility domain, and 768 MiB logical artifact bytes per domain. Pass explicit byte overrides only when recording a different policy.

Pass the exact `rustc` compatible with the shipped Loafs. The harness passes it as `RUSTC` to every measured command and records both its path and `--version` identity. This prevents an ambient Rustup default from minting an incompatible receipt and turning a toolchain mismatch into a misleading benchmark failure.

## Read the report

`report.json` contains:

- the machine and toolchain identity;
- `first_materialization`, each `warm_repeat_N`, and (when requested) `clean_worktree_reuse` elapsed duration and exit status;
- the required Cargo-guard probe status and verdict that successful normal stages did not launch Cargo;
- storage junctions before materialization, after first materialization, after every warm repeat, and after the clean-worktree repeat. Each keeps its own bounded-store inspection (physical allocation separate from logical artifact bytes, reclaimable bytes, and lease-protected bytes) plus raw store/output disk totals; and
- one log file per phase, including verbose Oven timing for a test workload.

Use `first_materialization` for the supported compiler-shipped Loaf's initial user-machine cost and the warm repeats for the unchanged normal-command goal. If the first normal command fails, a warm command launches the guard, a plan changes unexpectedly, or store reporting loses the physical/logical distinction, treat the result as a failure rather than averaging it into a performance claim.

## Measure the complete compiler repository suite

The repository suite has one explicit preparation boundary and one Cargo-guarded consumer:

1. `make test-prewarm-oven-loafs` invokes `incan oven legacy-cargo bake-loafs --envelope compiler-suite --suite-store ...`. One baker call creates or exactly reuses both the typed Loaf envelope and the receipt-bound compiler-suite store.
2. `incan oven compiler-libtests` compiles and executes every discovered root from the prepared store with Cargo guarded out.

The Makefile owns only this command composition. Oven owns Loaf identity, contents, admission, storage policy, selection, root inventory, and reporting. `make test-one TEST_ROOT=tests/cli_integration.rs` is the fast failure-isolation path; `make test-oven` is the complete local gate. Both pin the explicit publisher to `nightly-2026-03-24`, while the consumer remains direct `rustc`.

```bash
cargo build --features lsp
INCAN_TEST_COMPILER_ALREADY_BUILT=1 make test-prewarm-oven-loafs
make test-one TEST_ROOT=tests/cli_integration.rs
make test-oven
```

For retained timing evidence, run the baker command shown by `make -n test-prewarm-oven-loafs`, then the replay command shown by `make -n test-oven`, using fresh task-specific evidence directories. Put a Cargo executable that records its arguments and exits 97 first on `PATH` only for the `compiler-libtests` command. Record these phases independently:

- cold Loaf and compiler-suite-store preparation;
- exact baker reuse, including confirmation that Cargo did not start;
- Cargo-free prepared replay;
- `incan oven store inspect --format json` before publication and after replay; and
- `du -sk` for the store, Loaf root, and caller-owned output at each junction.

The suite JSON reports total, passed, failed, and ignored cases plus reported, green, failed, and unreported roots. The baker and store reports keep logical artifact bytes, policy-accounted physical bytes, whole-root owned/raw disk allocation, reclaimable bytes, active-lease bytes, and the configured limits distinct. A repeat is valid only when the baker reports `reused`, its compiler-suite result reports receipt-compatible reuse, the Cargo guard remains empty, and the complete root aggregate is green. Virtualized Linux is a portability gate, not authoritative reference-machine timing.

GitHub Actions keeps these two responsibilities separate. The ordinary `CI` workflow runs the focused Oven/Loaf regressions and a Cargo-guarded release-envelope smoke on Linux stable, macOS stable, and Linux MSRV. The manual and release-branch **Oven Release Evidence** workflow owns the complete three-platform repository suite and retained normal-command benchmark artifacts. Do not add cold publication or complete-suite timing back to the per-commit workflow merely to refresh release evidence.
