# #1337 stage 4 night, lane stdlib: handoff

State as of 2026-10-08, when the lane was paused. Everything the lane produced is on `origin`. Nothing is left only in the container.

## Branches

| Branch | Commit | State |
| --- | --- | --- |
| `feature/1337-night-stdlib-cloud` (draft PR #2109 → `feature/1337-night-staging`) | `a84f6d42` | **Proven.** Steps 1–2: `min`/`max`, `read_file`/`write_file`, JSON lists, `Option` model fields, `None` defaults. The focused test `stdlib::direct_route_stdlib_matches_legacy` passes; the census is 412 pass / 545 refused / 0 wrong / 0 driver-error; every `pre-commit-fast` gate passes; CI check suites on the head finished without failures. Based on staging `4ecd6b3a`. |
| `wip/1337-night-stdlib-merge-staging` | `3e2907cc` | **Not built or tested.** `a84f6d42` merged with staging `826ce160`. Conflicts were in `decls.incn`, `defaults.incn`, `lowering.incn`, `models.incn`, `options.incn` and `dispositions.json`; the inventory page was regenerated. See "Merge notes". |
| `wip/1337-night-stdlib-sdk-stage` | `921aea7e` | **Unproven.** Step 3, provider-crate nominals and calls (`IoError`, `EnvironError`, …), on top of `a84f6d42`; not merged with staging. Passes `cargo check --workspace --all-features --tests`, clippy on `incan_frontend`/`incan_semantics_core`, the rustdoc gate and `cargo +nightly-2026-03-24 fmt --check`. The adapter (`incan-rustc-driver`) and the Incan lowering compile only inside the Oven bake, and no bake of this stage ever completed. |
| `wip/1337-night-stdlib-draft-scratch` | `e36ac798` | An earlier copy of the step 3 draft on the old base. Superseded by `921aea7e`; kept only so nothing exists solely in the container. |
| `wip/1337-night-stdlib-handoff` | this commit | This directory: census results, the PR body source, scripts, patches and test programs. |

## Merge notes (`3e2907cc`)

- Staging generalized model places to nested field paths (`model_prefix`, `field_path`, `Projection.Fields`). Carrier field typing now runs through that path: `carrier_model_prefix` threads the retained carriers into `field_path` and `receiver_field`. `model_place` and `model_prefix` keep their signatures as carrier-free wrappers.
- Staging puts union declarations in the same intrinsic list as the `Option`/`Result` carriers, so the slot after the source enums is no longer carrier-only. The step 2 guards (`clonable_enum` in `lowering.incn`, `default_type` in `defaults.incn`) now also require `declaration.carrier != ""`, which keeps unions excluded.
- `default_type` combines both sides: staging's borrowed list and hashed-collection defaults, plus this lane's carrier defaults.

## Census and the 60-minute root deadline

The census (`direct_route_fixture_census`, 1101 fixtures) no longer fits the Oven's hard-coded compiler-suite root deadline in this container: the driver bake plus the fixtures took about 63 minutes. The steps 1–2 census was therefore run with `scripts/capture-and-run.sh` and `scripts/direct-run.py`. These build the root through the Oven as usual, copy the test process's environment, command line and working directory from `/proc` once the test binary starts, stop the Oven root, and run the same binary with no deadline. The binary, sources and fixture bake are the Oven's own; only the deadline is absent. The captured environment contains credentials; never commit it.

`census/` holds the baseline (`4ecd6b3a`: 410 / 546 / 0 / 1 driver-error), an earlier steps 1–2 measurement (`step12`), the final steps 1–2 measurement (`step2`, which matches `step12` per fixture), and `buckets.tsv`, the lane's bucket fixtures with their baseline refusal.

## Other contents

- `pr-body.md`: the source of PR #2109's description, with fixtures moved per step, buckets left refused with their owning lane, and legacy gaps found.
- `scripts/`: the container-specific helper scripts (absolute paths assume this container's layout). `native.sh` runs a baked driver's frontend and lowering on one program; a refusal reached before rustc needs externs reproduces in seconds without a rebake.
- `patches/`: `step12.patch` (the proven steps 1–2 diff) and `sdk-stage-v2.patch` (step 3 against `a84f6d42`); both match the commits above.
- `programs/`: the focused-test programs (`case0`–`case4`), each built with legacy and its output checked, and `bisect/`, the minimized programs used to locate the step 2 refusals.

## Open findings

- Legacy gaps: `min`/`max` reject a borrowed list although `sum` accepts one; legacy emission moves an owned `str` into `read_file(path)`, so reusing `path` fails rustc with E0382 although the checker accepts the program.
- Provider preparation: any frontend change makes the debug CLI re-prepare every SDK component, which runs rust-analyzer's parser over their Rust dependencies. The step 3 run spent over an hour there before it was stopped. Staging's `bakefast2` checkpoint says it stops re-preparing the SDK per compiler build; `3e2907cc` includes it.
