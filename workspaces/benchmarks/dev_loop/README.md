# Development-loop benchmarks

Measure #1675 on an installed dev.8 toolchain freshly installed from a pack of the source being evaluated. Select `INCAN`, `INCAN_HOME`, and matching `INCAN_STDLIB`, then run `zsh workspaces/benchmarks/dev_loop/run.sh`. Set `BENCH_SCRATCH` to a local scratch directory and optionally `DEV_LOOP_REPORT` to a report path. Zsh, jq, standard macOS command-line tools, and the installed toolchain are required. `SAMPLES` defaults to 15 and cannot be lower.

The harness creates three subjects: one application source file, two application modules, and an application with a path-dependency library. Each has one test exercising the editable value function. Every edit changes that function's returned generation; run samples verify the exact printed generation. Tests must execute and report a passing outcome. Unchanged operations are conditioned immediately before measurement.

Each subject measures first, unchanged, and edited runs to first output; unchanged and edited checks and tests; debug and release builds in fresh projects and unchanged projects; formatting checks; and unchanged codegraph queries. Fresh builds have warm installed dependencies and OS caches. Every command retains stdout, stderr, status, and `uptime`. Subjects rotate within each sample round to interleave machine load. Run on a quiet machine; interpret elevated-load samples using the retained load readings.

The report gives median, minimum-to-maximum spread, successful sample count, and the number meeting each applicable target. Refusals are retained as failed samples. Direct first and repeated executable launches measure the first-launch system cost independently; their difference includes assessment and cache effects, so it is not subtracted as a fixed 140 ms allowance. The historical dev.7 figures are contextual measurements, not matched baselines for these subjects.

To render retained samples again, run `zsh workspaces/benchmarks/dev_loop/report.sh SAMPLES_DIR OUTPUT.md OVEN INCAN_HOME [TIMES.tsv]`. The optional table argument selects an independently retained correction; preserve the original samples and document any correction in the report.

`START_ROUND` defaults to 1. After an input interruption, it can select the first round to collect again in a new output directory. Replace whole affected rounds, preserve the interrupted table, and document the input verification and correction. A subset report shows its actual sample count and does not certify 15 samples by itself.
