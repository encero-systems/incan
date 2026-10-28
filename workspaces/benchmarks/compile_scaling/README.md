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

Cold means withdrawing only the subject's retained build proof, with dependencies already built. It does not mean a cold OS page cache or a fresh toolchain. The lowering subject builds its compiler consumer too, so the command includes dependent driver relinking. The supported standard-package producer compiles std.data even unchanged; that repeated work is intentionally measured. Oven uses its ordinary executable reuse path.

`times.tsv` records complete command time, including orchestration and diagnostic compression. `phases.tsv` and `invocations.json` record individual driver wall time, frontend/checking, Body IR/context, codegraph, Incan lowering, rustc setup/analysis, LLVM/ThinLTO and link. Structured invocation spans define the Incan intervals; pinned rustc JSON timers define native phases. Library graph encoding and later completion fall outside the named graph-collection span. Nested timers are not added to enclosing durations; phase columns do not exhaust wall time.

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
