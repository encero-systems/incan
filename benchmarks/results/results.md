# Benchmark Results

Generated: Thu Apr  9 22:34:18 CEST 2026

| Benchmark         | Incan | Rust  | Python | Incan vs Python |
| ----------------- | ----- | ----- | ------ | --------------- |
| Fibonacci (1M)    | 4ms   | 3ms   | 42ms   | 10.5x faster    |
| Collatz (1M)      | 93ms  | 94ms  | 4568ms | 49.1x faster    |
| GCD (10M)         | 142ms | 99ms  | 853ms  | 6.0x faster     |
| Mandelbrot (2K)   | 118ms | 117ms | 5660ms | 47.9x faster    |
| N-Body (500K)     | 19ms  | 16ms  | 1820ms | 95.7x faster    |
| Prime Sieve (50M) | 125ms | 107ms | 3195ms | 25.5x faster    |
| Quicksort (1M)    | 57ms  | 48ms  | 840ms  | 14.7x faster    |
| Mergesort (1M)    | 76ms  | 119ms | 1245ms | 16.3x faster    |
