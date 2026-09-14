# Incan build times

Generated: 2026-08-21T09:46:57Z

Host: Darwin arm64

Rust: rustc 1.95.0 (59807616e 2026-04-14)

Best of 3 timed repeats per phase, except cold, which is measured once per toolchain because a
repeated cold build is no longer cold.

Incremental edits are verified by running the binary afterwards. `stale` means the toolchain reported a
successful build but the change never reached the binary, so no timing for it would be meaningful --
and its warm column is measuring a no-op rather than a rebuild.

| Toolchain | Cold (new user) | Warm (no changes) | Incremental (one edit) |
|---|---:|---:|---:|
| 0.4.0 | 3.68s | 50ms | stale |
| 0.5.0-rc7 | 922ms | 435ms | 514ms |

## 0.6.0-dev.4

Generated: 2026-09-14T07:51:40Z, same host, rustc 1.98.0 (88d9e12ae 2026-08-18), `scripts/bench_build_times.sh --warm-max-ms 400`, best of two runs. The machine was under ordinary desktop load; the same script on the same line at 02:18 the same morning, on an idle machine, measured 416 / 277 / 360 ms, and at the start of the line (`d4e560caa`, before #1111) 721 / 529 / 608 ms.

| Toolchain | Cold (new user) | Warm (no changes) | Incremental (one edit) |
|---|---:|---:|---:|
| 0.6.0-dev.4 | 462ms | 351ms | 431ms |

