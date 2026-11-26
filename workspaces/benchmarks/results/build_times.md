# Incan build times

These are installed-seed controls with the same source-build limits as the [execution table](results.md). `/usr/bin/time -p` provides 10 ms resolution here; the development-loop table retains finer-grained samples.

Measured: 2026-10-24T14:58:01Z

Commit: `e8f19f3cecdea4cfa83d07e390a80f77065a0841`

Toolchain: `0.6.0-dev.8`; driver SHA-256: `8c6c71701034a646c43c58042a022bbc07d8c6be2603e8e72c1c142d1b484e52`.

Rust: `rustc 1.98.0 (88d9e12ae 2026-08-18)`; Python: `Python 3.14.4`; host: `Darwin arm64`.

Raw evidence directory: `run.hKQmMP` under `BENCH_SCRATCH` (default `workspaces/benchmarks/target`).

Cold means one release build in a fresh project with installed dependencies already available. Warm is an unchanged second build. Both include CLI startup; neither measures installing a toolchain or flushing OS caches.

| Benchmark | Cold ms | Warm ms | Status |
| --- | ---: | ---: | --- |
| fib | 1370 | 50 | verified |
| collatz | 1350 | 50 | verified |
| gcd | 1560 | 50 | verified |
| mandelbrot | 1430 | 50 | verified |
| nbody | refused | unmeasured | build refused |
| primes | 1360 | 50 | verified |
| quicksort | 1390 | 50 | verified |
| mergesort | refused | unmeasured | build refused |
