# Local dev.8 compiler-authoring experiment

## Goal and scope
Use the existing compiled dev.8 SDK as the seed for an isolated compiler build that can compile a standalone Incan rustc consumer. No runtime or standard-library rebuild, no consumer dependency on Incan compiler internals, no authored Rust consumer bridge, no public capability contract, and no publication of this experimental branch. The existing dedicated toolchain remains authoritative for Rust identity.

## Pattern intake and tracked limitations
Active areas: toolchain adapter and Incan native lowering. Precedents: `native_shared.rs` for scoped compiler-owned feature activation; `frontend.rs` for explicit per-invocation options; `layout_types.incn` for shared/exclusive foreign references; `values.rs` and `validation.rs` for checked unsizing. DD-0004 currently restricts internal Rust APIs to driver units. This explicitly opted-in local experiment investigates a replacement boundary; it does not silently amend that production policy.

Backlog CA-1: compiler-internal library linkage is not exposed to Incan units. Exact rustc artifacts pass metadata checking, but linking refuses duplicate std/core formats. Existing rustc callbacks provide the only current primitive for setting compiler session features; a minimal adapter change is required and remains experimental implementation debt until the public authoring boundary is settled.

Backlog CA-2: source class as mutable foreign trait object passes checking then fails lowering at NativeTraitObject. The minimized repro is the standalone experiment's 02_rustc_callback. An imported implementation fails checking as well. Native callback execution is unproven. Fix actual reference/coercion boundaries; do not claim success from typechecking alone.

## Execution
1. Preserve the original SDK and use an independent copied installation. Reuse compiled runtime/std providers.
2. Red: reproduce control success and compiler import/callback failures from standalone sources.
3. Add explicit experimental per-invocation compiler-internal linkage in the local adapter, preserving pinned identity verification and rejection of ambient RUSTC_BOOTSTRAP. Ordinary invocations retain their behavior.
4. Rebuild the affected compiler dependency closure through installed Oven. Do not rebuild the full SDK.
5. Reduce the consumer unit's supplied providers to the verified required set and execute the rustc enum probe.
6. Fix the callback boundary, then execute an Incan-authored callback with observable state. Retain smaller passing milestones and exact failures if a subsequent boundary is absent.

## Acceptance contract
The consumer remains a separate repository and contains no Rust source. Successful linkage, callback invocation, and typed-body injection are separate proof levels. This iteration targets callback invocation. Check and build must agree; an ungranted ordinary invocation must not inherit experimental permissions. Rust library identity and bootstrap refusal must remain enforced. Re-run the control after patches. IncQL, public RFC lifecycle, and full distribution release proof are out of scope for this local experiment.

## Documentation and code docs
Record exact source, commands, SDK identities, results and remaining gaps in the standalone findings. Document all changed adapter methods. No stable public API or release claim. Update the remotely accessible findings artifact after validation.

## Gate
| Step | Command |
| --- | --- |
| Compiler build | `DEV8_LANE=/Volumes/Builds/incan-compiler-authoring-dev8 scripts/relink-driver.sh --lowering` |
| Rust formatting | `make fmt-check` |
| Patch hygiene | `git diff --check` |
| Baseline failures | standalone `sh scripts/verify.sh` using the original SDK |
| Candidate | explicit unit invocation with isolated driver; enum output and observable callback assertion |
| Regression | same candidate without opt-in; identity check; unchanged ordinary control |

- [ ] Minimal compiler-only rebuild succeeds.
- [ ] Reduced consumer inputs execute rustc enum probe.
- [ ] Explicit opt-in is required and compiler identity checks remain intact.
- [ ] Incan-authored callback executes.
- [ ] Evidence clearly separates achieved and unproven boundaries.
