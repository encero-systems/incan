# Compilation scaling

Measure the release edit loop for the lowering library, std.data and Oven (the mid-size project). Each subject has a cold build, an unchanged rebuild and an equivalent one-function-body edit. The fixed patches add a redundant boolean conjunction; public signatures and dependencies stay the same. The source revision fixes the workloads' size and generic density.

## Run

Use a dedicated checkout on macOS, with an installed native toolchain and support generation prepared from that same checkout through the source-build scripts. Run preparation from the checkout root: rustc records the working directory in its metadata, so support and compiler producers must share it. The checkout must be reserved for this experiment: the harness temporarily applies each fixed patch, reverses it after success, and verifies the original SHA-256. It refuses uncommitted changes to the edited files. After a failed edited build, its patch remains in the experiment checkout for diagnosis. The supplied home is cloned with APFS copies; its immutable dependency artifacts remain referenced by exact path.

```sh
export SCALING_SOURCE=/path/to/reserved/incan
export INCAN=/path/to/installed/incan
export INCAN_HOME=/path/to/coherent/installed/home
export SCALING_SUPPORT=/path/to/source-built/toolchain-native
export SCALING_OUTPUT=/path/to/new/results
make -C workspaces/benchmarks/compile_scaling -j2 measure
```

`SCALING_SUBJECTS='lowering data'` narrows the set; `SCALING_COLD_ONLY=1` measures only cold builds for policy comparisons. `SCALING_DEBUG_INFO=0` or `SCALING_DEBUG_INFO=line-tables-only` adds an explicit native debug-information override, recorded in the effective command. Use a new output directory for each policy. The harness waits for one-minute load at most 3, at least 50% free memory and 60 GB free disk before every build. Keep other heavy jobs stopped throughout each set. A two-token inherited jobserver limits native work; no compiler or library behavior is replaced by the instrumentation.

Cold means withdrawing only the subject's retained build proof, with dependencies already built. It does not mean a cold OS page cache or a fresh toolchain. The lowering subject builds its compiler consumer too, so the command includes dependent driver relinking. Standard packages use the same complete library proof as project libraries; the unchanged trial measures whether that producer reuses its output. Oven uses its ordinary executable reuse path.

`times.tsv` records complete command time, including orchestration and diagnostic compression. `phases.tsv` and `invocations.json` record individual driver wall time, frontend/checking, Body IR/context, codegraph, Incan lowering, rustc setup/analysis, LLVM/ThinLTO and link. Structured invocation spans define the Incan intervals; pinned rustc JSON timers define native phases. Library graph encoding and later completion fall outside the named graph-collection span. Nested timers are not added to enclosing durations; phase columns do not exhaust wall time.

`postprocess_wall` measures trace compression after the driver has completed; `collector_wall` covers observer startup through that postprocessing, excluding the final receipt write and process teardown. Neither interval is compiler work. Subtract the union of recorded subprocess intervals when attributing command time; summing overlapping parent and child observations double-counts work. Resident worker lifetimes are retained separately in `daemon-invocations.json` and excluded from compiler phase rows. An idle daemon's wall lifetime does not describe compilation or CPU time.

Each driver invocation retains its exact arguments, unit description, driver digest, timestamped output, parsed source inventory and compressed traces. The checked-IR diagnostic serialization adds measurement overhead. Peak footprint is the kernel's lifetime-max physical footprint for each driver process, including a final zombie reading before reaping; it is not RSS or the sum of concurrent processes. Reused invocations have no compiler phase or footprint, rather than a fabricated zero-cost compiler run.

The experiment home's long socket path selects the compiler's existing single-process fallback, allowing a final per-invocation kernel reading. Oven's ordinary artifact reuse remains enabled. The installed support must pass Oven's source, feature, compiler and artifact proofs; the compiler project consumes that same validated `incan_lang` producer as its runtime and standard closure.

## Baseline

Measured October 28, 2026 on macOS arm64, with two native build tokens and release debug information at level 2. Each trial passed the load/memory/disk admission above. These are single observations, not medians. All nine commands succeeded and the three edited files were restored to their original SHA-256. The first edited Oven invocation was interrupted; its incomplete receipt is retained separately from the successful replacement trial.

| Workload | Cold command (s) | Unchanged (s) | One body edit (s) |
| --- | ---: | ---: | ---: |
| Lowering library and compiler consumer | 291.278 | 1.780 | 261.035 |
| std.data source producer | 29.884 | 26.866 | 27.137 |
| Oven | 129.381 | 0.260 | 123.744 |

Cold compiler preparation also rebuilds native dependencies in the measurement home; the individual lowering invocation is 243.498 s. After a body edit, the whole lowering invocation takes 244.581 s and its dependent driver relinks. std.data is compiled as a whole package even unchanged. Oven reuses its executable unchanged and recompiles its complete source closure after the body edit.

| Cold invocation | Checking (s) | Body IR/context (s) | Codegraph (s) | Lowering (s) | rustc analysis (s) | LLVM/ThinLTO (s) | Link (s) | Peak physical bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Lowering | 28.751 | 6.396 | 42.791 | 91.001 | 29.133 | 39.835 | 0.023 | 4,831,452,216 |
| std.data | 3.179 | 1.136 | 0.928 | 15.832 | 0.914 | 4.393 | 0.006 | 2,113,244,472 |
| Oven | 20.071 | 4.612 | — | 63.123 | 2.407 | 36.655 | 0.166 | 3,528,086,232 |

The phase columns use the definitions above and exclude residual orchestration, graph encoding and publication. The lowering and data cold peaks are below their 6 GiB and 2 GiB budgets. An incomplete invocation is listed in `incomplete-invocations.json` and never enters the phase tables. Store complete local receipts outside the repository; retain the workload patches and this baseline in version control.

## Release debug-information comparison

The same baseline compiler compiled each workload with full debug information, an explicit `-Cdebuginfo=0` override and an explicit `-Cdebuginfo=line-tables-only` override. Dependencies were prepared from the same source and native generation; workloads were unchanged. Every measured build passed the host admission guard. These are individual invocation observations, not medians or statistically established speedups.

| Subject | Debug information | Invocation (s) | LLVM/ThinLTO (s) | Link (s) | Peak physical bytes |
| --- | --- | ---: | ---: | ---: | ---: |
| Lowering | Full (`2`) | 243.498 | 39.835 | 0.023 | 4,831,452,216 |
| Lowering | None (`0`) | 236.448 | 27.920 | 0.006 | 4,038,268,256 |
| Lowering | Line tables only | 242.631 | 31.059 | 0.013 | 4,105,016,696 |
| std.data | Full (`2`) | 27.632 | 4.393 | 0.006 | 2,113,244,472 |
| std.data | None (`0`) | 26.004 | 3.051 | 0.004 | 1,931,381,952 |
| std.data | Line tables only | 27.318 | 3.406 | 0.006 | 1,946,373,360 |

Level `0` reduces observed lowering invocation time by 7.050 s (2.9%) and LLVM/ThinLTO by 11.915 s (29.9%); std.data falls by 1.628 s (5.9%) and 1.342 s (30.6%), respectively. Peak physical footprint falls by 793,183,960 bytes for lowering and 181,862,520 bytes for data. Other phases vary between trials, so phase savings cannot be added to wall-time savings.

Release selects level `0`; debug retains level `2`. Oven writes the selected level into `debug_info` in the unit description, which enters reuse identity and also supplies direct standard-package builds. Explicit compiler arguments remain overrides. A fixed assertion probe prints identical `AssertionError` stderr and exits `101` under both policies. This checks the observed panic output; release level `0` omits DWARF for native debugging.

### Installed defaults

The installed compiler, Oven and source-description helper were rebuilt, followed by the runtime and all eleven standard packages in both profiles. Cold builds then used the installed default without an override. Their unit descriptions explicitly contain `debug_info: 0`.

| Subject | Invocation (s) | Checking (s) | Body IR/context (s) | Codegraph (s) | Lowering (s) | rustc analysis (s) | LLVM/ThinLTO (s) | Link (s) | Peak physical bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Lowering | 256.359 | 29.188 | 6.440 | 43.349 | 109.739 | 33.735 | 28.360 | 0.009 | 4,177,827,264 |
| std.data | 28.314 | 3.862 | 1.172 | 0.991 | 16.602 | 1.057 | 3.283 | 0.004 | 1,950,747,840 |

Both physical-footprint budgets pass. Compared with the initial baseline, LLVM time and footprint are lower, while total invocation time is higher by 12.861 s for lowering and 0.683 s for data. The parsed source sizes and Body IR body/statement/local counts are identical. The compiler and native dependency closure also changed between those measurements; the source of the additional non-LLVM time remains unresolved. The option comparison above supports reduced LLVM work and footprint; these observations establish no overall cold-build improvement.

A full-debug control then used the same final installed compiler and standard generation, with an explicit level `2` override. Local native compiler dependencies were rebuilt under that selected option, as in the default run.

| Subject | Full-debug invocation (s) | LLVM/ThinLTO (s) | Link (s) | Peak physical bytes |
| --- | ---: | ---: | ---: | ---: |
| Lowering | 248.391 | 41.917 | 0.021 | 4,682,636,272 |
| std.data | 29.157 | 4.550 | 0.007 | 2,026,360,096 |

Relative to this final control, level `0` saves 13.557 s (32.3%) in lowering LLVM/ThinLTO and 1.267 s (27.8%) for data. Physical peaks fall by 504,809,008 and 75,612,256 bytes. Whole invocation time is 7.969 s higher for lowering and 0.843 s lower for data. Lowering's Incan phase varies from 91.730 s in the control to 109.739 s in the default sample despite identical source sizes and Body IR counts. These single trials support the backend and memory benefit; total cold time remains variable.

## Standard-package profile reuse

The standard source producer can retain checking, Body IR, source provenance and library interfaces across equivalent release and debug builds. Oven supplies an explicit private artifact and equivalence key; native lowering and codegen still run separately. Requested codegraphs have their own producer and are not inputs to checking. The key covers authored inputs, declared and active features, vocabulary, every admitted Loaf's semantic artifacts and native closure, Rust facts, compiler generation, target and SDK. Only native profile policy and authenticated locations of semantically equivalent Loaf artifacts are normalized. Executable vocabulary and C headers refuse reuse until their additional observations can be witnessed, so interop builds independently in both profiles.

The following comparison records commit `af909718a`, before Incan graphs moved to on-demand production. Its graph timings describe that earlier policy.

The experiment builds core, testing, async, system, codecs, compression, web, data, observability, vocab and interop in that order, release first and then debug. Dependencies are already built and remain fixed. Each observed driver invocation uses the same two-token jobserver, diagnostic observer and host admission as above. Descriptions are freshly prepared by the source-built `toolchain-native` helper, which attaches the private semantic key. For a separate uncached control, prepare the same description, remove `checked_artifact` and `checked_key`, and compile again with the same driver, working directory and output paths. Compare every `.rlib`, `.incnlib`, `.incnbody`, `.incngraph`, `.native.json`, and any Rust ABI or vocabulary worker output by SHA-256. Different working directories or output paths can change native metadata and are unsuitable byte controls. A byte-only validation pass precedes the timed comparison. Retain cached and control originals separately so no prior artifact can mask a missing output.

The preceding compiler's 22 invocations took 115.270 s, or 120.558 s including the observed commands and receipt processing. Its descriptions were prepared before timing. Debug alone repeated 10.495 s of frontend/checking, 1.667 s of Body IR/context and 2.004 s of codegraph collection. These aggregate intervals include interop, which remains uncached. They exclude graph encoding and other residual work, and cannot be added to predict the final saving.

This is whole-package semantic reuse. An authored body edit still checks and lowers the affected package as a whole. It does not provide function-level incremental checking or reuse native plans between profiles.

### Observed results

Measured October 28, 2026 on macOS arm64. Each timing set contains 22 successful invocations admitted individually under the load, memory and disk limits above. Release starts without a retained checked-source snapshot; debug can reuse the first profile's result. These are single observations, not medians. The current compiler was also measured with both proof fields removed, providing a matched-driver control.

| Set | Release driver (s) | Debug driver (s) | All 22 driver invocations (s) | Observed commands (s) |
| --- | ---: | ---: | ---: | ---: |
| Preceding compiler, no reuse | 56.160 | 59.111 | 115.270 | 120.558 |
| Current compiler, reuse disabled | 46.344 | 50.871 | 97.215 | 110.192 |
| Current compiler, reuse enabled | 47.871 | 45.459 | 93.330 | 106.417 |

Against the matched-driver control, reuse reduces observed driver time by 3.885 s (4.0%) and command time by 3.774 s (3.4%). Cold release is 1.527 s slower; the second profile is 5.412 s faster. Commands include description preparation, vocabulary-worker staging and receipt processing; the preceding compiler's descriptions were prepared beforehand. Its uncached checking also took longer despite byte-identical standard Rust ABI caches. The larger parent-to-current difference therefore does not isolate the effect of this cache.

Ten debug invocations reuse checked source; interop checks independently in both profiles. Their Body IR body, statement and local counts match the uncached controls and preceding compiler. Across the ten eligible debug builds, the matched control spends 7.168 s in checking, Body IR/context and graph collection. Reuse replaces those calls with 0.632 s of snapshot reads. The first profile spends 1.186 s writing and structurally validating snapshots. Native lowering, rustc analysis, LLVM and linking still run; their variability contributes to the observed wall-time difference.

| Debug phase, all eleven packages | Reuse disabled (s) | Reuse enabled (s) |
| --- | ---: | ---: |
| Frontend/checking | 3.666 | 0.008 |
| Body IR/context | 1.599 | 0.019 |
| Codegraph collection | 1.958 | 0.027 |
| Incan lowering | 23.275 | 24.442 |
| rustc analysis | 2.547 | 2.681 |
| LLVM/ThinLTO | 13.994 | 14.496 |
| Link | 0.039 | 0.030 |

The remaining semantic intervals in the enabled column belong to interop. No checking or graph-collection span occurs on a hit. Snapshot publication streams to a private file and validates one module and Rust item at a time, avoiding a second complete program or an in-memory payload buffer.

| std.data | Reuse disabled invocation (s) | Reuse enabled invocation (s) | Disabled peak physical bytes | Enabled peak physical bytes |
| --- | ---: | ---: | ---: | ---: |
| Release, cold semantic result | 22.816 | 23.317 | 2,049,100,296 | 2,068,695,608 |
| Debug, second profile | 24.482 | 22.359 | 1,987,561,992 | 1,720,043,992 |

Cold publication raises the observed release peak by 19,595,312 bytes against the matched control. Both release observations remain below 2 GiB; the enabled sample has 78,788,040 bytes of headroom. The debug hit lowers the observed peak by 267,518,000 bytes. These per-process observations are not a maximum-memory guarantee.

All 22 native/public output inventories match separate uncached builds byte for byte, with cached and control originals retained separately. An `i8` overflow probe shares the same checked-source key while release prints `-128` and debug exits `101`. A corrupt private snapshot falls back to checking and reproduces the original artifacts. Source, feature, interface and native-closure changes invalidate equivalence; directory sidecars and equivalent search aliases do not. Full validation passes with unchanged verdicts: 1,214 contracts pass, zero fail and one remains pending; all 69 Oven tests, 18 Rust facets and 23 source builds pass.
