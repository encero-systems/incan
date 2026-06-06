# Quicksort Benchmark
# Sort 1,000,000 random integers in-place

import sys

sys.setrecursionlimit(10000)  # Increase for deep recursion

def quicksort(arr: list, low: int, high: int):
    if low < high:
        pivot_idx = partition(arr, low, high)
        quicksort(arr, low, pivot_idx - 1)
        quicksort(arr, pivot_idx + 1, high)

def partition(arr: list, low: int, high: int) -> int:
    pivot = arr[high]
    i = low - 1

    for j in range(low, high):
        if arr[j] <= pivot:
            i += 1
            arr[i], arr[j] = arr[j], arr[i]

    arr[i + 1], arr[high] = arr[high], arr[i + 1]
    return i + 1

def generate_random_array(size: int, seed: int) -> list:
    # Simple LCG random number generator for reproducibility
    arr = []
    state = seed
    for _ in range(size):
        state = (state * 1103515245 + 12345) % 2147483648
        arr.append(state % 1000000)
    return arr

def main():
    size = 1_000_000
    arr = generate_random_array(size, 42)

    quicksort(arr, 0, len(arr) - 1)
    print(f"Sorted {size} elements")
    print(f"First 5: {arr[0]}, {arr[1]}, {arr[2]}, {arr[3]}, {arr[4]}")

if __name__ == "__main__":
    main()
