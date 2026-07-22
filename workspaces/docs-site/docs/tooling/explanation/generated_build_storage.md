# Generated-build storage model

Incan compiles generated Rust through Cargo. Cargo dependency and intermediate artifacts are usually the largest part of
that pipeline, so Incan shares those rebuildable files across compatible projects while keeping source and durable
outputs project-local.

## Storage ownership

| Category | Default owner and location | Lifecycle |
| --- | --- | --- |
| Generated Rust source and manifests | Project `target/incan/`, `target/incan_tests/`, and `target/incan_lock/` | Recreated from Incan source; remains project-local |
| Generated Cargo dependency and intermediate output | `$INCAN_HOME/cache/generated-cargo/v1/<identity>/target/` | Shared by compatible commands; idle domains are LRU-pruned toward a 20 GiB soft limit, and an idle domain whose measured output exceeds its 20 GiB default safety bound has only its rebuildable Cargo target discarded |
| Published executable | The generated project below project `target/incan/` | Copied atomically from Cargo output after a successful build; never pruned by `incan cache` |
| Test harness and rust-inspect workspace source/metadata | Project `target/incan_tests/` and `target/incan_lock/rust_inspect/` | Project-local generated inputs; their Cargo output uses the applicable shared or explicit target |
| Compiled SDK providers | The selected SDK provider store below `INCAN_HOME`, or an explicit provider-store override | Content-addressed SDK lifecycle; not removed by generated-cache pruning |
| Vocab companion metadata | Beside the selected generated target, unless `INCAN_VOCAB_COMPANION_CACHE_DIR` overrides it | Input-fingerprinted companion cache |
| Durable Incan library | Project `target/lib/*.incnlib` | User-facing build artifact; never removed by generated-cache pruning |
| Incan compiler repository build | Repository `target/` | Ordinary Cargo development state, outside the installed compiler's generated-cache manager |
| Legacy Rust `CompilationPlan` executor output | Caller-selected plan output directory | Caller-owned compatibility API; contained below that output and not used by CLI build, run, test, lock, or library paths |

The cache reports recursive logical file lengths. These are useful for deterministic category comparisons but are not
the same as allocated or uniquely reclaimable filesystem blocks on APFS, sparse, compressed, cloned, or hardlinked
storage.

## Compatibility and bounded growth

A compatibility identity includes the Incan version, selected Rust backend command and verbose host/version output,
Rust/Cargo target, profile, and flag environment selectors, the Cargo executable and verbose version, Cargo profile, normalized dependency lock, Cargo feature selection,
and Cargo arguments that can affect compiled artifacts. Execution-only offline, lock-enforcement, timing, verbosity, and
color policy does not split otherwise compatible domains. Cargo fingerprints its remaining compiler and configuration inputs inside the domain.
Cargo passthrough cannot set `--target-dir` or load a file-valued `--config`, because either could move artifacts outside
the directory protected and reported by Incan; use the explicit generated-target override instead. Generated binary
target names use a path-independent digest of generated root source plus package, dependency, provider, lock, edition,
and feature inputs, so an unchanged logical fixture does not leave a new top-level binary and incremental tree merely
because a temporary project directory changed. A per-root exclusive lock spans Cargo execution and atomic project-local
publication; the executable path itself comes from Cargo's JSON artifact message, including target-triple layouts.

Incan takes a shared activity lease before Cargo can use a domain. Cleanup takes an exclusive lease and skips active domains. Automatic cleanup runs before acquiring the requested domain and prunes other idle domains first. The total limit is therefore soft while domains are active. When the final Cargo lease ends, Incan measures the domain; if it exceeds the per-domain safety bound, Incan discards that domain's rebuildable `target/` tree while preserving its identity metadata. The same recovery runs before reusing an interrupted idle domain, so repeated crashes cannot retain unmeasured growth indefinitely. Set `INCAN_GENERATED_CACHE_MAX_ENTRY_BYTES` to change the bound, or use an explicit generated-target override when an external system owns a larger target lifecycle. The lease ends after Cargo has published the project-local executable and before `incan run` starts user code.

If a build process exits before releasing its lease, the next acquisition of that idle identity treats its unmeasured size as unknown, measures the partial domain before taking the new shared lease, and discards an oversized rebuildable target. Acquiring a different identity also includes the interrupted idle domain in ordinary LRU pruning.

## Reproducing the audit

Use an isolated `INCAN_HOME` and the same compiler binary for every sample. For offline warm-cache evidence, seed Cargo's
registry first, then set `CARGO_NET_OFFLINE=true` for both runs.

1. Remove only the task-local project targets and task-local `INCAN_HOME`.
2. Run `incan build`, `incan run`, `incan test`, and `incan build --lib` once and record `/usr/bin/time -p`.
3. Record `incan cache inspect --format json` and `du -sk` for each project-local category in the table above.
4. Repeat the unchanged command, then repeat it from a compatible clean project or worktree.
5. Compare compatibility-identity count and category growth. Do not use one platform-specific absolute byte ceiling as a
   regression assertion.

### Initial v0.5 cache evidence

On 2026-07-20, a macOS APFS canary used two separate projects with the same `serde_json` dependency graph, an isolated
`INCAN_HOME`, and `CARGO_NET_OFFLINE=true`:

| Scenario | First project | Compatible second project | Result after both |
| --- | ---: | ---: | --- |
| `incan build` (release) | 67.74 s | 0.43 s | One 58,829,076-logical-byte release domain; both final binaries project-local |
| `incan run` (debug) | 6.94 s | 0.44 s | One additional 113,103,114-logical-byte debug domain, separated by profile |

After both scenarios, project-local generated source plus published output was 436 KiB per project; the managed cache
reported 171,932,190 logical bytes across the two intentional profile domains. The second runs compiled only their root
crate, demonstrating offline dependency reuse without an explicit target override.

The CI-focused audit also found that path-derived binary names made an unchanged 11-fixture rerun grow one shared target
from 241,280 KiB to 285,924 KiB (+44,644 KiB) while the provider store stayed at 6,140 KiB. The path-independent generated
source identity above fixes that demonstrated root-artifact growth mechanism. Regression checks assert stable identities
rather than those machine-specific byte totals.

This table is evidence for the managed-cache reuse and profile-isolation slice, not the complete #876 audit. Before #876
closes, the same harness must still record cold/warm `incan test` and `incan build --lib`, all four commands against a
representative dependency-heavy downstream package, and before/after category totals for rust-inspect, lock/preheat,
vocab, provider, and durable library output. Until those samples are recorded, release claims should describe the cache
feature and the demonstrated fixes without claiming the broader audit is complete.

## Remaining Cargo-owned cost

The shared domain still contains Cargo's dependency objects, build-script output, fingerprints, and incremental state.
Different profiles, compiler/toolchain contracts, target selectors, features, or dependency locks intentionally create
separate compatibility domains. `incan cache inspect` reports logical file lengths without creating a cache-management
lock; `incan cache prune --dry-run` previews the resulting logical usage, and exact idle identities can be removed with
`incan cache prune --identity <SHA256>`.
