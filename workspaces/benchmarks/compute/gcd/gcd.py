# GCD Benchmark (Optimized)
# Compute GCD for many number pairs using Euclidean algorithm
# Note: we are giving python a fair chance by using the built-in gcd function which is implemented in C.

from math import gcd


def main():
    iterations = 10_000_000

    # Local binding avoids repeated module lookup
    _gcd = gcd

    # Deterministic pairs based on loop index
    total = sum(
        _gcd((i * 17) % 10000 + 1, (i * 31) % 10000 + 1)
        for i in range(1, iterations + 1)
    )

    print(f"Sum of {iterations} GCDs: {total}")


if __name__ == "__main__":
    main()
