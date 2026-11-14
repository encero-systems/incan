# Benchmark results

These are controls on a packed and freshly reinstalled seed. The integrated native source rebuild was refused; these timings do not certify it. See [scope, refusals, and profiling evidence](analysis.md). Six programs matched both baselines exactly; N-body and mergesort remain refused by name below.

Measured: 2026-10-24T14:58:01Z

Commit: `e8f19f3cecdea4cfa83d07e390a80f77065a0841`

Toolchain: `0.6.0-dev.8`; driver SHA-256: `8c6c71701034a646c43c58042a022bbc07d8c6be2603e8e72c1c142d1b484e52`.

Rust: `rustc 1.98.0 (88d9e12ae 2026-08-18)`; Python: `Python 3.14.4`; host: `Darwin arm64`.

Raw evidence directory: `run.hKQmMP` under `BENCH_SCRATCH` (default `workspaces/benchmarks/target`).

Hyperfine: two warmups, at least ten measured runs, shell disabled. Times are means in milliseconds.
August values are the 0.5 measurements from 2026-08-21; they are historical comparisons on a different toolchain.

| Benchmark | dev.8 ms | August Incan ms | Rust ms | Python ms | Output | Performance flags |
| --- | ---: | ---: | ---: | ---: | --- | --- |
| fib | 4.65 | 3 | 4.13 | 45.01 | verified | >10% August |
| collatz | 142.49 | 94 | 98.09 | 4515.68 | verified | >10% August, >25% Rust |
| gcd | 1363.36 | 92 | 101.18 | 939.85 | verified | >10% August, >25% Rust |
| mandelbrot | 127.46 | 113 | 123.66 | 4964.24 | verified | >10% August |
| nbody | refused | 20 | 17.36 | 1590.42 | build refused | none |
| primes | 511.56 | 141 | 117.76 | 3340.69 | verified | >10% August, >25% Rust |
| quicksort | 64.8 | 56 | 51.88 | 892.04 | verified | >10% August |
| mergesort | refused | 83 | 116.33 | 1329.73 | build refused | none |
