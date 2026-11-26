# Development-loop measurements

## Resident compilation and indexed search directories (#2256)

These rows come from the harness with 15 samples per row on the integration lane's source-built toolchain, not a packed seed: the lane home with its relinked release driver and standard packages, and the native Oven built from the measured checkout. Before is `4bfc6a496` (driver `57adf7231869`, Oven `9dcf79dc`). After is `f984397e2` (driver `a17fe76401c8`, Oven `796aa335`), with the debug standard packages republished by that driver, so their native closures list only the directories beyond the toolchain's search index. Both runs use this checkout's harness, which conditions the unchanged check like every other unchanged row. Each side's load is the median one-minute load average of its samples' `uptime`. The reports were generated on 2026-10-25 at 01:12 and 02:52 UTC on Darwin arm64; the raw samples are `dev-loop.ewk6b9` (before) and `dev-loop.p4kPp5` (after) under the lane's `BENCH_SCRATCH`. All 1,350 samples of the two runs succeeded.

| Subject | Metric | Target ms | Before median ms | Before spread | Before load | After median ms | After spread | After load | After met target | After successful |
| --- | --- | ---: | ---: | --- | ---: | ---: | --- | ---: | --- | --- |
| single | check_unchanged | - | 290.9 | 286.1-292.9 | 2.57 | 16.5 | 16.1-18.6 | 2.69 | n/a | 15/15 |
| single | check_edit | - | 291.3 | 285.1-295.3 | 2.57 | 28.6 | 28.1-29.4 | 2.69 | n/a | 15/15 |
| single | fmt_check | - | 41.5 | 35.6-46.2 | 2.57 | 15.3 | 12.4-22.4 | 2.68 | n/a | 15/15 |
| single | run_unchanged | 50 | 260.5 | 257.1-517.7 | 2.57 | 14.2 | 9.8-30.9 | 2.69 | 15/15 | 15/15 |
| single | run_edit | 250 | 829.5 | 805.1-1041.9 | 2.57 | 207.8 | 195.0-335.5 | 2.69 | 12/15 | 15/15 |
| single | test_unchanged | 100 | 264.6 | 257.0-270.5 | 2.57 | 17.4 | 12.9-19.6 | 2.69 | 15/15 | 15/15 |
| single | test_edit | 400 | 836.7 | 814.7-1071.3 | 2.57 | 219.2 | 208.3-291.3 | 2.69 | 15/15 | 15/15 |
| single | direct_first_launch | - | 156.6 | 144.3-222.3 | 2.44 | 146.4 | 143.5-212.8 | 2.58 | n/a | 15/15 |
| single | direct_repeat_launch | - | 3.6 | 3.0-4.8 | 2.44 | 3.5 | 2.4-4.2 | 2.58 | n/a | 15/15 |
| modules | check_unchanged | - | 292.3 | 284.2-294.2 | 2.65 | 16.3 | 15.3-16.9 | 2.58 | n/a | 15/15 |
| modules | check_edit | - | 292.3 | 284.2-294.7 | 2.65 | 28.6 | 27.9-29.1 | 2.58 | n/a | 15/15 |
| modules | fmt_check | - | 42.1 | 40.1-47.8 | 2.71 | 15.2 | 12.5-17.3 | 2.58 | n/a | 15/15 |
| modules | run_unchanged | 50 | 260.2 | 248.6-514.6 | 2.65 | 13.7 | 10.6-33.0 | 2.58 | 15/15 | 15/15 |
| modules | run_edit | 250 | 824.3 | 807.6-964.3 | 2.65 | 206.3 | 203.5-244.1 | 2.58 | 15/15 | 15/15 |
| modules | test_unchanged | 100 | 261.6 | 258.1-268.2 | 2.65 | 16.7 | 15.2-20.3 | 2.58 | 15/15 | 15/15 |
| modules | test_edit | 400 | 841.6 | 807.2-1028.1 | 2.71 | 224.2 | 212.7-266.6 | 2.58 | 15/15 | 15/15 |
| modules | direct_first_launch | - | 167.3 | 146.7-312.0 | 2.59 | 147.5 | 142.9-204.2 | 2.64 | n/a | 15/15 |
| modules | direct_repeat_launch | - | 3.7 | 3.4-4.8 | 2.59 | 3.6 | 2.7-4.1 | 2.64 | n/a | 15/15 |
| library | check_unchanged | - | 849.5 | 829.9-857.2 | 2.57 | 28.9 | 28.1-30.4 | 2.64 | n/a | 15/15 |
| library | check_edit | - | 1265.9 | 1240.7-1278.9 | 2.57 | 68.5 | 67.3-72.5 | 2.64 | n/a | 15/15 |
| library | fmt_check | - | 45.2 | 41.4-46.3 | 2.60 | 18.4 | 13.9-19.8 | 2.68 | n/a | 15/15 |
| library | run_unchanged | 50 | 1084.8 | 1070.3-1876.3 | 2.71 | 44.3 | 40.2-72.2 | 2.62 | 14/15 | 15/15 |
| library | run_edit | 250 | 2286.5 | 2257.7-2461.5 | 2.71 | 465.3 | 441.1-546.0 | 2.62 | 0/15 | 15/15 |
| library | test_unchanged | 100 | 1088.5 | 1068.7-1133.7 | 2.68 | 48.0 | 43.7-50.4 | 2.68 | 15/15 | 15/15 |
| library | test_edit | 400 | 2333.4 | 2255.7-2528.1 | 2.68 | 467.1 | 450.5-524.0 | 2.68 | 0/15 | 15/15 |
| library | direct_first_launch | - | 166.9 | 144.5-1106.1 | 2.69 | 145.3 | 142.7-186.5 | 2.68 | n/a | 15/15 |
| library | direct_repeat_launch | - | 3.8 | 3.1-4.9 | 2.66 | 3.5 | 3.2-4.0 | 2.68 | n/a | 15/15 |

An unchanged check replays its report in 16.5 ms for one file and for two modules, and in 28.9 ms with a path dependency, whose preparation describes and keys two units. `fmt --check` takes 15.2 to 18.4 ms through the resident worker. Edited runs, tests and checks compile in the resident worker: an edited check takes 28.6 ms, or 68.5 ms when the edit is in the path dependency, which is compiled again.

An edited run or test ends with the system's first launch of a new executable, about 145 ms against 3.5 ms for a repeated launch, and a library subject's run also loads a newly compiled shared library. Incan's own share is the time from the command's start to the program launch Oven traces. Measured separately with 15 edited samples per row (`OVEN_TRACE`), before is the parent of the resident-compilation change (driver `32e389f6e6ca`, Oven `7ad35c8a`):

| Subject | Metric | Before to launch ms | Before completed ms | After to launch ms | After completed ms |
| --- | --- | ---: | ---: | ---: | ---: |
| single | run_edit | 212.6 | 357.3 | 71.5 | 219.9 |
| single | test_edit | 211.6 | 373.4 | 71.4 | 218.6 |
| modules | run_edit | 208.2 | 357.4 | 66.7 | 212.0 |
| modules | test_edit | 212.3 | 360.5 | 70.8 | 214.4 |
| library | run_edit | 577.7 | 873.0 | 184.7 | 484.9 |
| library | test_edit | 586.4 | 877.9 | 194.1 | 482.9 |

## Packed seed controls (2026-10-24)

These are controls on the packed seed identified in [analysis.md](analysis.md), with 675 successful command samples: 45 rows, 15 samples each. They do not certify the refused integrated source rebuild or its resident compiler.

The configured ABI file disappeared during rounds 14–15. Its cause is unestablished. The restored archive copy and a new extraction by the selected driver have identical SHA-256 `967c1162700d00313efbcd027b9d1b96c318c4f64f422ed64016d957e2db05d8`. Whole rounds 14–15 were recollected in `dev-loop.yGDgGu`, and the original interrupted table remains intact. `final-times.tsv` combines the first 13 rounds with those replacements, without selecting samples by elapsed time.

Load capture is outside the timer. Completion timings include immediate outcome validation for tests and graph queries; first-output timings stop before output comparison.

Report generated: 2026-10-24T15:32:07Z. Harness checkout: 11663123403ec4065591521dcff0d8145aad17fe. Host: Darwin arm64.

Driver SHA-256: 8c6c71701034a646c43c58042a022bbc07d8c6be2603e8e72c1c142d1b484e52. Oven SHA-256: 532169638e946130a1f7fc2d2841b828783f2d717dfd037bdba32ec384685351.

Raw samples, load readings and command output: dev-loop.Fu0zni under BENCH_SCRATCH. Table: final-times.tsv.

Run metrics measure time to the first verified output line. Other metrics measure completion. Each command starts a new CLI process; project owners may be reused. Subjects rotate within each round to interleave load. Spread is minimum to maximum. Failed commands have no passing timing.

Direct first/repeat launches isolate a newly compiled executable from its repeated execution. Their difference includes system assessment and cache effects; no fixed OS-cost subtraction is applied.

The dev.7 handoff measured small unchanged commands at 11–15 ms and one check at 12.76 ms. Other rows have no matched historical baseline.

| Subject | Metric | Target ms | dev.7 ms | dev.8 median ms | Spread ms | Met target | Successful |
| --- | --- | ---: | --- | ---: | --- | --- | --- |
| library | build_debug_cold | - | unmeasured | 2670.502 | 2573.313-2786.926 | n/a | 15/15 |
| library | build_debug_warm | - | unmeasured | 282.979 | 274.154-309.259 | n/a | 15/15 |
| library | build_release_cold | - | unmeasured | 2623.311 | 2571.846-2773.629 | n/a | 15/15 |
| library | build_release_warm | - | unmeasured | 414.450 | 292.509-448.497 | n/a | 15/15 |
| library | check_edit | - | unmeasured | 2463.387 | 2386.945-2747.509 | n/a | 15/15 |
| library | check_unchanged | - | 12.76 (one workload) | 1346.432 | 1313.689-1535.321 | n/a | 15/15 |
| library | direct_first_launch | - | unmeasured | 157.369 | 146.046-200.467 | n/a | 15/15 |
| library | direct_repeat_launch | - | unmeasured | 2.487 | 2.230-3.176 | n/a | 15/15 |
| library | fmt_check | - | unmeasured | 1012.143 | 992.389-1240.547 | n/a | 15/15 |
| library | graph_unchanged | - | unmeasured | 3759.849 | 3697.870-4027.591 | n/a | 15/15 |
| library | run_edit | 250 | unmeasured | 2853.574 | 2716.772-3081.921 | 0/15 | 15/15 |
| library | run_first | 1000 | unmeasured | 2800.654 | 2734.134-3010.183 | 0/15 | 15/15 |
| library | run_unchanged | 50 | 11-15 (small workloads) | 289.235 | 274.283-447.950 | 0/15 | 15/15 |
| library | test_edit | 400 | unmeasured | 2808.352 | 2756.773-3368.027 | 0/15 | 15/15 |
| library | test_unchanged | 100 | unmeasured | 285.893 | 280.406-311.369 | 0/15 | 15/15 |
| modules | build_debug_cold | - | unmeasured | 1332.039 | 1298.369-1467.874 | n/a | 15/15 |
| modules | build_debug_warm | - | unmeasured | 5.307 | 5.040-5.589 | n/a | 15/15 |
| modules | build_release_cold | - | unmeasured | 1343.950 | 1302.960-1393.352 | n/a | 15/15 |
| modules | build_release_warm | - | unmeasured | 5.402 | 5.078-6.178 | n/a | 15/15 |
| modules | check_edit | - | unmeasured | 1143.534 | 1113.251-1194.784 | n/a | 15/15 |
| modules | check_unchanged | - | 12.76 (one workload) | 1133.079 | 1112.282-1215.935 | n/a | 15/15 |
| modules | direct_first_launch | - | unmeasured | 154.961 | 142.056-210.356 | n/a | 15/15 |
| modules | direct_repeat_launch | - | unmeasured | 2.425 | 2.306-2.891 | n/a | 15/15 |
| modules | fmt_check | - | unmeasured | 999.161 | 987.392-1064.365 | n/a | 15/15 |
| modules | graph_unchanged | - | unmeasured | 3730.495 | 3676.668-4083.536 | n/a | 15/15 |
| modules | run_edit | 250 | unmeasured | 1456.907 | 1383.734-1578.476 | 0/15 | 15/15 |
| modules | run_first | 1000 | unmeasured | 1494.206 | 1449.137-2687.210 | 0/15 | 15/15 |
| modules | run_unchanged | 50 | 11-15 (small workloads) | 7.265 | 6.403-58.388 | 13/15 | 15/15 |
| modules | test_edit | 400 | unmeasured | 1345.619 | 1285.156-1448.095 | 0/15 | 15/15 |
| modules | test_unchanged | 100 | unmeasured | 10.031 | 8.952-10.694 | 15/15 | 15/15 |
| single | build_debug_cold | - | unmeasured | 1341.035 | 1297.819-1463.507 | n/a | 15/15 |
| single | build_debug_warm | - | unmeasured | 5.469 | 5.032-6.789 | n/a | 15/15 |
| single | build_release_cold | - | unmeasured | 1329.544 | 1295.078-1420.593 | n/a | 15/15 |
| single | build_release_warm | - | unmeasured | 5.478 | 4.832-6.538 | n/a | 15/15 |
| single | check_edit | - | unmeasured | 1147.226 | 1111.029-1257.455 | n/a | 15/15 |
| single | check_unchanged | - | 12.76 (one workload) | 1149.130 | 1111.250-1260.162 | n/a | 15/15 |
| single | direct_first_launch | - | unmeasured | 152.914 | 141.592-485.197 | n/a | 15/15 |
| single | direct_repeat_launch | - | unmeasured | 2.480 | 2.275-2.798 | n/a | 15/15 |
| single | fmt_check | - | unmeasured | 1018.553 | 985.684-1140.451 | n/a | 15/15 |
| single | graph_unchanged | - | unmeasured | 3764.012 | 3670.234-4100.692 | n/a | 15/15 |
| single | run_edit | 250 | unmeasured | 1450.698 | 1378.236-1531.849 | 0/15 | 15/15 |
| single | run_first | 1000 | unmeasured | 1501.257 | 1438.868-1594.771 | 0/15 | 15/15 |
| single | run_unchanged | 50 | 11-15 (small workloads) | 7.021 | 6.284-63.017 | 13/15 | 15/15 |
| single | test_edit | 400 | unmeasured | 1342.983 | 1287.083-1452.538 | 0/15 | 15/15 |
| single | test_unchanged | 100 | unmeasured | 9.912 | 8.943-11.385 | 15/15 | 15/15 |
