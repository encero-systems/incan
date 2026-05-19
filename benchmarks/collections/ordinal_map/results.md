# OrdinalMap Benchmark Results

Generated locally on 2026-05-19 with:

```bash
PYTHON=/private/tmp/incan-fastconstmap-venv/bin/python bash benchmarks/collections/ordinal_map/run.sh --keys 1000000 --probes 1000000
```

Corpus: 1,000,000 string keys and 1,000,000 deterministic present-key probes.

## Current RFC 101 Implementation

| implementation | lookup path | build ms | ns/lookup | batch ns/lookup | payload bytes/key | serialized bytes/key |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Python `dict` | exact | 119.617 | 643.945 | n/a | n/a | n/a |
| Python `fastconstmap.ConstMap` | unchecked | 239.752 | 310.888 | 62.469 | 9.044 | 9.044 |
| Python `fastconstmap.VerifiedConstMap` | verified | 238.422 | 372.085 | 74.676 | 18.088 | 18.088 |
| Incan `OrdinalMap[str]` | `get` exact | 941.892 | 436.191 | n/a | 28.278 | 28.278 |
| Incan `OrdinalMap[str]` | `require` exact | 941.892 | 476.893 | 442.310 | 28.278 | 28.278 |
| Incan `OrdinalMap[str]` | unchecked | 941.892 | 219.302 | 160.212 | 28.278 | 28.278 |

Incan `payload bytes/key` is `storage_bytes() / keys`: compact payload sections only. It is not total retained heap and does not include ordinary object/header overhead or the runtime list caches used by the current implementation.

This is a single local run, not a median over repeated samples. In this run, the current stdlib implementation's exact single-key lookup was slower than `fastconstmap.VerifiedConstMap`, while unchecked single-key lookup was lower than `fastconstmap.ConstMap`. Batch lookup is slower than `fastconstmap` in this implementation because the public Incan call path still preserves list and string values with clones at batch boundaries. Construction remains slower because `OrdinalMap.from_keys` is pure Incan code that validates/canonicalizes records and builds deterministic serialization sections.

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

The prior spike artifacts are historical local worktree outputs rather than committed benchmark inputs. Re-run this directory's benchmark script for committed, reproducible RFC 101 measurements.
