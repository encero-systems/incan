# Benchmark Results

Generated: Tue Feb 10 14:07:02 CET 2026

|     Benchmark     | Incan | Rust  | Python  | Incan vs Python |
| ----------------- | ----- | ----- | ------- | --------------- |
| Fibonacci (1M)    | 16ms  | 12ms  | 281ms   | 17.5x faster    |
| Collatz (1M)      | 156ms | 163ms | 9383ms  | 60.1x faster    |
| GCD (10M)         | 291ms | 347ms | 2198ms  | 7.5x faster     |
| Mandelbrot (2K)   | 262ms | 260ms | 10445ms | 39.8x faster    |
| N-Body (500K)     | 40ms  | 37ms  | 3452ms  | 86.3x faster    |
| Prime Sieve (50M) | 189ms | 128ms | 9108ms  | 48.1x faster    |
| Quicksort (1M)    | 96ms  | 77ms  | 2000ms  | 20.8x faster    |
| Mergesort (1M)    | 128ms | 196ms | 2876ms  | 22.4x faster    |
