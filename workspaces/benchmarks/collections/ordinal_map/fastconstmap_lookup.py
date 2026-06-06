#!/usr/bin/env python3
"""fastconstmap comparison for the OrdinalMap benchmark."""

from __future__ import annotations

import argparse
import importlib.util
import sys
import time


def make_keys(count: int) -> list[str]:
    return [f"field_{idx:08d}" for idx in range(count)]


def make_probe_indexes(key_count: int, probe_count: int) -> list[int]:
    return [(idx * 1103515245 + 12345) % key_count for idx in range(probe_count)]


def run_map(map_type: type, name: str, key_count: int, probe_count: int) -> None:
    keys = make_keys(key_count)

    started = time.perf_counter()
    pairs = {key: ordinal for ordinal, key in enumerate(keys)}
    mapping = map_type(pairs)
    build_seconds = time.perf_counter() - started

    probe_indexes = make_probe_indexes(key_count, probe_count)
    started = time.perf_counter()
    checksum = 0
    for index in probe_indexes:
        checksum += mapping[keys[index]]
    lookup_seconds = time.perf_counter() - started

    probe_keys = [keys[index] for index in probe_indexes]
    started = time.perf_counter()
    batch_values = mapping.get_many(probe_keys)
    batch_seconds = time.perf_counter() - started
    batch_checksum = sum(batch_values)

    print(f"implementation={name}")
    print(f"keys={key_count}")
    print(f"probes={probe_count}")
    print(f"checksum={checksum}")
    print(f"batch_checksum={batch_checksum}")
    print(f"build_seconds={build_seconds:.6f}")
    print(f"lookup_seconds={lookup_seconds:.6f}")
    print(f"batch_seconds={batch_seconds:.6f}")
    print(f"nbytes={mapping.nbytes()}")
    print(f"serialized_size={mapping.serialized_size()}")
    print("")


def main() -> int:
    if importlib.util.find_spec("fastconstmap") is None:
        print("SKIP: Python package 'fastconstmap' is not installed.")
        return 77

    from fastconstmap import ConstMap, VerifiedConstMap

    parser = argparse.ArgumentParser()
    parser.add_argument("--keys", type=int, default=100_000)
    parser.add_argument("--probes", type=int, default=1_000_000)
    args = parser.parse_args()

    run_map(ConstMap, "fastconstmap_const", args.keys, args.probes)
    run_map(VerifiedConstMap, "fastconstmap_verified", args.keys, args.probes)
    return 0


if __name__ == "__main__":
    sys.exit(main())
