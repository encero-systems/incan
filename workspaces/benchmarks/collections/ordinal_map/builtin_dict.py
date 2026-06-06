#!/usr/bin/env python3
"""Builtin dict baseline for the OrdinalMap benchmark scaffold."""

from __future__ import annotations

import argparse
import time


def make_keys(count: int) -> list[str]:
    return [f"field_{idx:08d}" for idx in range(count)]


def make_probe_indexes(key_count: int, probe_count: int) -> list[int]:
    return [(idx * 1103515245 + 12345) % key_count for idx in range(probe_count)]


def run(key_count: int, probe_count: int) -> tuple[int, float, float]:
    keys = make_keys(key_count)

    started = time.perf_counter()
    mapping = {key: ordinal for ordinal, key in enumerate(keys)}
    build_seconds = time.perf_counter() - started

    probe_indexes = make_probe_indexes(key_count, probe_count)
    started = time.perf_counter()
    checksum = 0
    for index in probe_indexes:
        checksum += mapping[keys[index]]
    lookup_seconds = time.perf_counter() - started

    return checksum, build_seconds, lookup_seconds


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--keys", type=int, default=100_000)
    parser.add_argument("--probes", type=int, default=1_000_000)
    args = parser.parse_args()

    checksum, build_seconds, lookup_seconds = run(args.keys, args.probes)
    print(f"implementation=python_dict")
    print(f"keys={args.keys}")
    print(f"probes={args.probes}")
    print(f"checksum={checksum}")
    print(f"build_seconds={build_seconds:.6f}")
    print(f"lookup_seconds={lookup_seconds:.6f}")


if __name__ == "__main__":
    main()
