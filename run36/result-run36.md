# Release-speed result

## Outcome

The release rehearsal passed from source commit `b36c27f005c7ba1828aab34e05ce99231e5e22e2` in **1,397.38 seconds (23 minutes 17.38 seconds)**. The bundle retained baseline parity: 1,491 contract passes, zero failures, zero pending cases, 1,131 baseline keys with zero lost passes, all 20 Rust facet suites, and all 171 Oven tests.

The requested **15-minute target was not met**. The final run missed it by 497.38 seconds. Relative to the brief's 62-68 minute starting range, the result saves 38 minutes 43 seconds to 44 minutes 43 seconds, or approximately 62.4-65.8%.

The final bundle hashes were:

| Artifact | SHA-256 |
| --- | --- |
| `incan` | `2a9a31dfa5f846906bf4971b81381956628f33a286d294fec7160c455946e518` |
| `incan-lsp` | `b31cc9932c48d48e6ec257c311d6b341079d8229528b6b7127681b9a739f29` |
| `toolchain.tar.zst` | `71b0dfb6e7d900101837e39c635c0c5b6b7127681b9a739f29aff22e304c5bec` |

## Installed verification baseline

The clean installed-toolchain baseline took 1,894.27 wall seconds. Aggregate CPU time is `user + sys`; average cores is aggregate CPU divided by wall time. Memory-contract CPU is an underestimate because its persistent owner outlived the timed client command.

| Gate | Wall (s) | Aggregate CPU (s) | Average cores |
| --- | ---: | ---: | ---: |
| Whole installed verification | 1,894.27 | 2,426.77 | 1.28 |
| Oven tests | 874.83 | 717.93 | 0.82 |
| Contract census | 400.50 | 861.40 | 2.15 |
| Rust facets, 20 suites run serially | 276.12 | 673.92 | 2.44 |
| Memory contracts | 145.75 | 30.32 | 0.21 |
| LSP contracts | 122.72 | 117.45 | 0.96 |
| Driver helper build | 41.93 | 0.82 | 0.02 |
| Driver identity | 11.17 | 8.81 | 0.79 |
| Runner build | 5.08 | 0.26 | 0.05 |
| Driver runtime | 2.30 | 1.03 | 0.45 |

### Rust facet baseline by suite

| Suite | Wall (s) | Aggregate CPU (s) | Average cores |
| --- | ---: | ---: | ---: |
| `incan_driver` | 11.60 | 13.38 | 1.15 |
| `incan_format` | 5.31 | 10.05 | 1.89 |
| `incan_frontend` | 74.93 | 335.85 | 4.48 |
| `incan_semantics_stdlib` | 1.15 | 1.03 | 0.90 |
| `incan_codegraph` | 5.65 | 8.74 | 1.55 |
| `incan_lang` | 4.11 | 7.69 | 1.87 |
| `incan_loaf_names` | 0.44 | 0.32 | 0.73 |
| `incan_semantics_core` | 8.28 | 38.98 | 4.71 |
| `incan_syntax` | 9.78 | 23.08 | 2.36 |
| `incan_vocab` | 2.95 | 5.22 | 1.77 |
| `incan_platform` | 0.39 | 0.27 | 0.69 |
| `stdlib-async` | 0.95 | 1.74 | 1.83 |
| `stdlib-data` | 1.14 | 1.93 | 1.69 |
| `incan_derive` | 0.48 | 0.35 | 0.73 |
| `incan_web_macros` | 2.32 | 4.89 | 2.11 |
| `stdlib-primitives` | 3.19 | 6.52 | 2.04 |
| `stdlib-testing` | 0.74 | 0.71 | 0.96 |
| `stdlib-web` | 0.59 | 0.47 | 0.80 |
| `incan-lsp` | 28.57 | 79.21 | 2.77 |
| `incan-rustc-driver` | 113.55 | 133.49 | 1.18 |
| **Total** | **276.12** | **673.92** | **2.44** |

## Changes and measured effects

Each root cause has its own commit.

| Commit | Root cause and change | Before | After |
| --- | --- | ---: | ---: |
| `12cac5774` | Oven source tests shared one project and the Rust facets were serial. Give each Oven source a private project and schedule the 20 facets concurrently. | Oven 874.83 s; facets 276.12 s | Oven 351.50 s; facets 146.57 s |
| `d49629a4d` | Five independent post-Oven gates ran serially. Run facets, driver integration, LSP, census, and memory gates concurrently. | Installed verification 1,187.23 s | 764.99 s; concurrent group 422.02 s |
| `d6354d5eb` | The seed produced debug standard packages that no later release stage consumed. Build only the seed release profile while retaining release and debug candidate/fixed-point validation. | Seed debug standards 45.63 s | 0 s in the final release |
| `004d6c105` | The immutable seed reference census was repeated despite identical compiler, LSP, manifest, and contract-tree content. Cache the complete parsed census by those identities. | Reference census approximately 253 s in the brief | Cache hit 0.016 s |
| `6c01939b4` | Oven verification still serialized ahead of the other independent gates. Run all six installed gates concurrently under declared memory reservations. | Installed verification 764.99 s | 505.14 s; concurrent group 489.32 s |
| `b36c27f00` | The Rust relocation cache escaped the assigned `INCAN_HOME`, which blocked the first full rehearsal in the sandbox. Keep both release caches under the assigned home. | Full rehearsal refused at relocation-cache creation | Full rehearsal passed |

All comparable installed-verification runs retained the same summary: 1,491 passes, zero failures, zero pending cases, 1,131 baseline keys, zero lost passes, and 20 Rust facet suites.

| Installed-verification configuration | Wall (s) | Aggregate CPU (s) | Average cores | Change from baseline |
| --- | ---: | ---: | ---: | ---: |
| Clean baseline | 1,894.27 | 2,426.77 | 1.28 | - |
| Private Oven projects and parallel facets | 1,187.23 | 2,404.58 | 2.03 | -707.04 s |
| Five post-Oven gates concurrent | 764.99 | 2,778.90 | 3.63 | -1,129.28 s |
| All six gates concurrent | 505.14 | 3,093.48 | 6.12 | -1,389.13 s |

The final full rehearsal's installed verification took 603.24 seconds rather than 505.14 seconds because the release run performed its clean source and relocation work and ran the six gates under broader full-release contention. It still completed every gate.

## Final full-release timing

| Phase | Wall (s) |
| --- | ---: |
| Installed verification | 603.24 |
| Driver build | 250.70 |
| LSP build | 114.76 |
| Oven build | 82.08 |
| Candidate source interfaces | 12.11 |
| Source interfaces | 11.48 |
| Installed relocated Rust | 9.37 |
| Support build | 7.89 |
| Pack | 13.25 |
| Installed toolchain setup | 20.26 |
| Seed reference-cache hit | 0.016 |
| **Whole release** | **1,397.38** |

Within installed verification, the concurrent group took 522.47 seconds: census 521.19 seconds, Oven 371.71 seconds, facets 342.36 seconds, LSP 317.42 seconds, memory 262.84 seconds, and driver integration 139.22 seconds. The remaining critical path is therefore not another skipped check: it is the full clean census under contention, followed by the 250.70-second candidate driver build and the 114.76-second LSP build. Reaching 15 minutes requires eliminating or materially reducing at least 497.38 more seconds from those real build and verification paths.

## Acceptance and resource checks

- Release receipt: outcome `passed`, phase `complete`, exit code `0`, baseline parity `true`.
- Source: exact committed SHA `b36c27f005c7ba1828aab34e05ce99231e5e22e2`.
- Contracts: 1,491 pass, zero fail, zero pending.
- Oven: 171 expected, 171 passed, zero failed.
- Installed verification: every original gate retained; independent gates overlap rather than being removed.
- Environment: all release invocations cleared `INCAN_STDLIB` and `INCAN_SOURCE_ROOT` and used the one assigned `INCAN_HOME`.
- Scratch-home cleanup: the one literal release scratch home was deleted by the release cleanup path; the receipt records zero residual build, source, verification, and relocated-Rust bytes.
- Disk: peak observed physical lane use was 21 GiB, below the 25 GiB cap. Post-cleanup use was 1.5 GiB for the lane, 1.3 GiB for the final release output, and 4.1 GiB for the assigned home.

## Verification notes

Focused checks passed:

- `incan test scripts/test_oven_tests.incn`: 15 tests.
- `incan test scripts/test_release_support.incn`: 11 tests.
- `incan check scripts/release.incn`.
- `incan check scripts/verify_installation.incn`.
- `incan check scripts/verify_fixed_point.incn`.
- Full release rehearsal from the committed source SHA.

The qualified `make pre-commit` run passed formatting, US English, changed-rustdoc, agent-documentation, contributor-path, and reference-contract checks, then stopped on two pre-existing missing documentation inputs: `build_engine_producer_map.svg` referenced by `build_engine_architecture.md`, and `workspaces/oven/tests/verify_toolchain_relocation.sh` referenced by `toolchain_release_design.md`.

During implementation, cross-module imported `Enum(str)` `.value()` use through an indexed list comprehension exposed an existing compiler lowering defect. The minimized reproduction and observed Body IR error were added to existing issue `#2995`; the release code uses an explicit match helper and retains the intended profile mapping.

The committed measurement artifacts are under `run36/evidence-baseline`, `run36/evidence-after-parallel`, `run36/evidence-after-concurrent`, and `run36/evidence-after-all-concurrent`. Failed exploratory output is retained separately under `run36/evidence-after-failed` and is not counted as acceptance evidence.
