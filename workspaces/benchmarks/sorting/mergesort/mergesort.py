# Mergesort Benchmark
# Sort 1,000,000 random integers (with allocation)

import sys

sys.setrecursionlimit(50000)  # Increase for deep recursion

def mergesort(arr: list) -> list:
    n = len(arr)
    if n <= 1:
        return arr

    mid = n // 2
    left = mergesort(arr[:mid])
    right = mergesort(arr[mid:])

    return merge(left, right)

def merge(left: list, right: list) -> list:
    result = []
    i = 0
    j = 0

    while i < len(left) and j < len(right):
        if left[i] <= right[j]:
            result.append(left[i])
            i += 1
        else:
            result.append(right[j])
            j += 1

    # Append remaining elements
    result.extend(left[i:])
    result.extend(right[j:])

    return result

def generate_random_array(size: int, seed: int) -> list:
    arr = []
    state = seed
    for _ in range(size):
        state = (state * 1103515245 + 12345) % 2147483648
        arr.append(state % 1000000)
    return arr

def main():
    size = 1_000_000
    arr = generate_random_array(size, 42)
    sorted_arr = mergesort(arr)
    print(f"Sorted {size} elements")
    print(f"First 5: {sorted_arr[0]}, {sorted_arr[1]}, {sorted_arr[2]}, {sorted_arr[3]}, {sorted_arr[4]}")

if __name__ == "__main__":
    main()
