# Fibonacci Benchmark
# Compute fib(N) iteratively, mod 10^9+7 to avoid overflow
# N = 1,000,000

def fib_mod(n: int, modulo: int) -> int:
    if n <= 1:
        return n

    prev = 0
    curr = 1

    for _ in range(2, n + 1):
        next_val = (prev + curr) % modulo
        prev = curr
        curr = next_val

    return curr

def main():
    n = 1_000_000
    modulo = 1_000_000_007
    result = fib_mod(n, modulo)
    print(f"fib({n}) mod {modulo} = {result}")

if __name__ == "__main__":
    main()
