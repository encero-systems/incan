# Development-loop measurements

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
