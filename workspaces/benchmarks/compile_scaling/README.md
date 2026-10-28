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
