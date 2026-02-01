# Benchmark Results

Generated: Thu Feb  5 19:54:27 CET 2026

| Benchmark | Incan | Rust | Python | Incan vs Python |
|-----------|-------|------|--------|-----------------|
| Fibonacci (1M) | 16ms | 12ms | 359ms | 22.4x faster |
| Collatz (1M) | 163ms | 191ms | 10057ms | 61.6x faster |
| GCD (10M) | 314ms | 315ms | 2002ms | 6.3x faster |
| Mandelbrot (2K) | 253ms | 257ms | 12967ms | 51.2x faster |
| N-Body (500K) | 45ms | 36ms | 5124ms | 113.8x faster |
| Prime Sieve (50M) | 217ms | 135ms | 11093ms | 51.1x faster |
| Quicksort (1M) | 152ms | 167ms | 2597ms | 17.0x faster |
| Mergesort (1M) | 148ms | 467ms | 4445ms | 30.0x faster |
