# Benchmark Results

Generated: Fri Apr 17 19:02:33 CEST 2026

| Benchmark         | Incan | Rust  | Python | Incan vs Python |
| ----------------- | ----- | ----- | ------ | --------------- |
| Fibonacci (1M)    | 3ms   | 4ms   | 45ms   | 15.0x faster    |
| Collatz (1M)      | 101ms | 100ms | 4719ms | 46.7x faster    |
| GCD (10M)         | 101ms | 98ms  | 882ms  | 8.7x faster     |
| Mandelbrot (2K)   | 122ms | 123ms | 5777ms | 47.3x faster    |
| N-Body (500K)     | 19ms  | 17ms  | 1892ms | 99.5x faster    |
| Prime Sieve (50M) | 139ms | 118ms | 3409ms | 24.5x faster    |
| Quicksort (1M)    | 58ms  | 51ms  | 912ms  | 15.7x faster    |
| Mergesort (1M)    | 80ms  | 127ms | 1340ms | 16.7x faster    |
