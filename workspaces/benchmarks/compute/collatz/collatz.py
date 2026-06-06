# Collatz Conjecture Benchmark
# Count total steps for all numbers 1 to N

def collatz_steps(n: int) -> int:
    count = 0
    num = n
    while num != 1:
        if num % 2 == 0:
            num = num // 2
        else:
            num = 3 * num + 1
        count += 1
    return count

def main():
    limit = 1_000_000
    total_steps = 0

    for n in range(1, limit + 1):
        total_steps += collatz_steps(n)

    print(f"Total Collatz steps for 1..{limit}: {total_steps}")

if __name__ == "__main__":
    main()
