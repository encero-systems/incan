# Benchmark Results

Generated: Wed Apr 15 17:24:17 CEST 2026

| Benchmark         | Incan | Rust  | Python | Incan vs Python |
| ----------------- | ----- | ----- | ------ | --------------- |
| Fibonacci (1M)    | 5ms   | 4ms   | 50ms   | 10.0x faster    |
| Collatz (1M)      | 106ms | 104ms | 5141ms | 48.5x faster    |
| GCD (10M)         | 144ms | 106ms | 971ms  | 6.7x faster     |
| Mandelbrot (2K)   | 127ms | 126ms | 6252ms | 49.2x faster    |
| N-Body (500K)     | 20ms  | 18ms  | 2025ms | 101.2x faster   |
| Prime Sieve (50M) | 157ms | 138ms | 3680ms | 23.4x faster    |
| Quicksort (1M)    | 61ms  | 52ms  | 1021ms | 16.7x faster    |
| Mergesort (1M)    | 83ms  | 129ms | 1504ms | 18.1x faster    |
