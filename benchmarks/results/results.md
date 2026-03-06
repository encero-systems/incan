# Benchmark Results

Generated: Fri Mar  6 23:40:06 CET 2026

| Benchmark         | Incan | Rust  | Python  | Incan vs Python |
| ----------------- | ----: | ----: | ------: | --------------: |
| Fibonacci (1M)    | 13ms  | 13ms  | 354ms   | 27.2x faster    |
| Collatz (1M)      | 154ms | 152ms | 9113ms  | 59.1x faster    |
| GCD (10M)         | 296ms | 299ms | 1990ms  | 6.7x faster     |
| Mandelbrot (2K)   | 254ms | 245ms | 12217ms | 48.0x faster    |
| N-Body (500K)     | 37ms  | 36ms  | 4880ms  | 131.8x faster   |
| Prime Sieve (50M) | 184ms | 129ms | 9496ms  | 51.6x faster    |
| Quicksort (1M)    | 91ms  | 79ms  | 2317ms  | 25.4x faster    |
| Mergesort (1M)    | 128ms | 187ms | 3617ms  | 28.2x faster    |
