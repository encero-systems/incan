# OrdinalMap benchmark

This directory contains the RFC 101 lookup benchmark. It reports measured values only; it does not encode expected speedups.

The comparison is:

- builtin Python `dict` mapping string keys to integer ordinals
- Python `fastconstmap.ConstMap` and `fastconstmap.VerifiedConstMap`, when the optional package is installed
- Incan `std.collections.OrdinalMap[str]`
- the prior Rust spike baseline recorded in `results.md`

The default corpus has 100,000 string keys and 1,000,000 deterministic present-key probes. The runner also accepts `--keys` and `--probes`; for non-default values it generates a temporary `.incn` source with those constants before compiling the Incan benchmark.

## Run

The runner builds the repository's release compiler before running Incan unless `INCAN=/path/to/incan` is set.

The Python `dict` baseline has no extra dependency. To include `fastconstmap`, install it in a Python environment and pass that interpreter through `PYTHON`:

```bash
python3 -m venv /private/tmp/incan-fastconstmap-venv
/private/tmp/incan-fastconstmap-venv/bin/python -m pip install fastconstmap
PYTHON=/private/tmp/incan-fastconstmap-venv/bin/python bash workspaces/benchmarks/collections/ordinal_map/run.sh
```

Without `fastconstmap`, `run.sh` still runs Python `dict` and Incan and reports the optional dependency as skipped.

Non-default `--keys` or `--probes` values run all available implementations, including Incan.

## Files

- `builtin_dict.py` runs the Python builtin-dict baseline.
- `fastconstmap_lookup.py` runs both fastconstmap map variants when the optional package is installed, otherwise exits with a skip status.
- `ordinal_map.incn` builds and runs the Incan `OrdinalMap[str]` benchmark.
- `run.sh` runs the available implementations and skips only unavailable optional dependencies.
- `results.md` records the latest local run and the prior spike baseline used when RFC 101 was accepted.

## Interpreting results

The benchmark separates safe and unchecked lookup paths:

- Python `dict` and Incan safe lookup are exact for missing keys.
- `fastconstmap.ConstMap` and Incan `get_unchecked` are unchecked paths.
- `fastconstmap.VerifiedConstMap` and Incan `get`/`require` include exact missing-key detection.

`payload bytes/key` is not process heap. For Incan, it is `storage_bytes() / keys`: compact payload sections only. It excludes ordinary object/header overhead and runtime lookup caches. For `fastconstmap`, the benchmark reports the package's `nbytes()` value.

The latest 1,000,000-key local run in `results.md` is a single directional run, not a median over repeated samples. In that run, `OrdinalMap[str]` exact single-key lookup was lower than Python plus `fastconstmap.VerifiedConstMap`, and unchecked single-key lookup was lower than `fastconstmap.ConstMap`. Exact batch lookup remained slower than `fastconstmap.VerifiedConstMap`; unchecked batch lookup was lower than `fastconstmap.ConstMap`. `OrdinalMap` uses more payload bytes per key and has slower construction because the builder validates and canonicalizes records before producing deterministic payload sections.

## Notes

- This benchmark remains standalone rather than part of `workspaces/benchmarks/run_all.sh` because `fastconstmap` is an optional Python dependency.
- The benchmark uses deterministic present-key probes. It does not measure missing-key behavior, mixed workloads, construction from serialized bytes, or total process RSS.
