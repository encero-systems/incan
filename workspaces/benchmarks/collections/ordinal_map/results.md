# OrdinalMap Benchmark Results

Generated locally on 2026-05-20 with:

```bash
PYTHON=/private/tmp/incan-fastconstmap-venv/bin/python bash workspaces/benchmarks/collections/ordinal_map/run.sh --keys 1000000 --probes 1000000
```

Corpus: 1,000,000 string keys and 1,000,000 deterministic present-key probes.

## Current RFC 101 Implementation

| implementation | lookup path | build ms | ns/lookup | batch ns/lookup | payload bytes/key | serialized bytes/key |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Python `dict` | exact | 91.750 | 491.024 | n/a | n/a | n/a |
| Python `fastconstmap.ConstMap` | unchecked | 222.000 | 275.913 | 51.335 | 9.044 | 9.044 |
| Python `fastconstmap.VerifiedConstMap` | verified | 201.581 | 248.705 | 50.141 | 18.088 | 18.088 |
| Incan `OrdinalMap[str]` | `get` exact | 822.159 | 115.139 | n/a | 28.278 | 28.278 |
| Incan `OrdinalMap[str]` | `require` exact | 822.159 | 109.385 | 66.591 | 28.278 | 28.278 |
| Incan `OrdinalMap[str]` | unchecked | 822.159 | 49.419 | 25.760 | 28.278 | 28.278 |

Incan `payload bytes/key` is `storage_bytes() / keys`: compact payload sections only. It is not total retained heap and does not include ordinary object/header overhead or runtime lookup caches.

This is a single local run, not a median over repeated samples. In this run, Incan exact single-key lookup was lower than `fastconstmap.VerifiedConstMap`, and unchecked single-key lookup was lower than `fastconstmap.ConstMap`. Exact batch lookup remained slower than `fastconstmap.VerifiedConstMap`; unchecked batch lookup was lower than `fastconstmap.ConstMap`. Construction remains slower because `OrdinalMap.from_keys` validates and canonicalizes records before building deterministic serialization sections.

## Prior Spike Baseline

The prior spike used a handwritten Rust index-table prototype and a fuller 1,000,000-record sweep. It is a performance reference comparison, not the stdlib implementation.

| implementation | lookup path | records | build ms | ns/lookup | bytes/key | value storage |
| --- | --- | ---: | ---: | ---: | ---: | --- |
| Rust `verified_fuse` | verified | 1,000,000 | 82.532 | 18.382 | 18.088 | `u64` |
| Rust `index_table_verified` | verified | 1,000,000 | 84.505 | 10.863 | 13.566 | `u32` |
| Rust `index_table_unchecked` | unchecked | 1,000,000 | 75.019 | 4.433 | 4.522 | `u32` |
| Python `dict` | exact | 1,000,000 | 3.512 | 424.771 | 111.758 | n/a |
| Python `fastconstmap.ConstMap` | unchecked | 1,000,000 | 120.723 | 39.400 | 9.044 | n/a |
| Python `fastconstmap.VerifiedConstMap` | verified | 1,000,000 | 123.711 | 109.107 | 18.088 | n/a |

The committed benchmark runner above is the reproducible RFC 101 measurement. These spike numbers are retained as design-history context.
