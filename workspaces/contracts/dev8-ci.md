# Verify an installed dev.8 toolchain

The dev.8 workflow installs a packed release toolchain, builds this checkout's contract runner with that compiler, and runs all eight contract families with two workers. It compares case keys against `dev8-pass-baseline.txt`: a formerly passing case must still pass, including cases that disappear or become pending. Existing failures are reported without failing this regression gate. A runner build failure, incomplete report, duplicate result, missing family, or inconsistent total fails the job.

This verifies the installed release against the checkout's contracts. It does not rebuild the PR's compiler or prove self-hosting. DD-0010 requires that additional compiler build after self-hosting is proven. Release publication remains the maintainer's operation.

## Local archive

Install rustup first, then supply the archive and its matching Oven executable. The script installs Rust 1.98.0 with `rustc-dev` when absent, creates a fresh Incan home, and retains installation, build, and contract logs. It exposes only standard packages present in the archive; their source declarations come from this checkout. An older archive may consequently refuse a newer runner.

```sh
CI_DEV8_OUT="$PWD/target/dev8-evidence" scripts/ci-dev8.sh /path/toolchain.tar.zst /path/incan
```

The supported hosts are macOS arm64 and Linux x86_64. `CI_DEV8_JOBS` sets the worker budget, and `CI_DEV8_TIMEOUT` sets the per-command timeout in seconds (default 120). Every invocation creates a new evidence directory, prints its location, and leaves it available after failure. The local archive and supplied Oven are trusted local inputs; the archive checksum is recorded in that directory.

## CI release bundle

`incan toolchain pack` contains the compiler closure, but omits Oven, which is needed for `incan toolchain install`. Each host's OCI bundle therefore contains two files, `toolchain.tar.zst` and its matching `incan`. Both files are covered by the same manifest digest. The workflow pulls that pinned bundle with oras 1.3, then invokes the same script. The script refuses a reference without both its host's tag suffix and a SHA-256 digest.

The repository variable `DEV8_TOOLCHAINS` is a JSON object with keys `aarch64-apple-darwin` and `x86_64-unknown-linux-gnu`. Each value has this shape, using a real commit and manifest digest:

```text
ghcr.io/encero-systems/incan.pub/toolchain/incan:<commit>-<target>@sha256:<64 hex digits>
```

Leave the variable unset until the real release passes self-hosting. The contract jobs fail with an unset variable; they never substitute a mutable tag or a private bootstrap. The separate docs job runs US English checks and the strict docs build regardless of release availability.

## Publication handoff

The macOS candidate measured on October 24, 2026 (Europe/Berlin), contains the compiler and fixture source tree committed as `edf65667d3f413a5a0d9f8e560eb9a59968df95a`. It was packed and measured before committing that unchanged tree; the CI checkout record names parent `ed6475e26`. Its archive SHA-256 is `74dc9f59d1fb8168c21a197151389d4b66878cc7045e60b8782385940acb2b05` (461227351 bytes, ten standard packages). A fresh installation built the native contract runner and completed all eight families: **1126 PASS / 36 FAIL / 0 PEND**. An independent supplied-controller census has exactly the same PASS keys. All 1124 previous PASS keys survive; `behavior/emit_calls_and_builtins/std_fs_path_str_or_path_targets` and `behavior/snapshots_stdlib/std_fs_path` now pass. The complete contract logs name the remaining failures and their diagnostics. This establishes the candidate's installed runner build, not a compiler self-build or Linux parity.

The stored-current macOS archive checked for that measurement still names source `e5b69517d970dbab64b77e0ff8eb742b20b01219`, archive SHA-256 `702958a45b2da4c400d3b82b45e59b5db28f07b8cd60fb5e054ee31a9b142e7f`. Its fresh installation succeeds, but the runner build refuses `CommandExt.process_group`. That immutable archive does not contain the fixes above. The verified candidate is local evidence; it has not replaced the stored archive or been published.

Integrate the compiler fixes and select the full merged source SHA. Build both the `aarch64-apple-darwin` and `x86_64-unknown-linux-gnu` archives and their matching Oven executables from that same SHA. On each native host, verify a fresh installed runner build, a complete eight-family report, and equality with that host's direct PASS keys, preserving the committed baseline. Finish the separate compiler self-build verification before publication.

After self-hosting and release verification, the maintainer authenticates oras to GHCR and GitHub CLI to the repository, then runs `scripts/publish-dev8-toolchain.sh` with six operands: the full source commit, macOS archive, matching macOS Oven, Linux archive, matching Linux Oven, and a new JSON output path. Both bundles must come from that commit. The script pushes each annotated bundle once and reads its digest directly from the [oras push descriptor](https://oras.land/docs/how_to_guides/format_output/#oras-push). Only after both succeed does it write the receipt and set the single `DEV8_TOOLCHAINS` repository variable to that complete pair. OCI pushes are separate operations; a failed second push can leave the first bundle published without a completed pins file. If the variable update fails, the JSON receipt remains available and the prior variable value is preserved.

On Linux, use the Linux archive and Linux Oven on an x86_64 host, or set the variable and run the `ubuntu-latest` job. A macOS run does not establish Linux outcomes. Store records and release inputs must identify the same merged source SHA for both hosts before they can establish one verified release pair.

## Baseline maintenance

The comments in `dev8-pass-baseline.txt` identify the checkout, archive, Oven, runner, and measurement conditions. The baseline is measured evidence, not a list of desired results. Preserve its PASS keys when updating it; investigate a lost key before removing it. The baseline is shared by both hosts to require the same minimum behavior; Linux parity remains unverified until a Linux run succeeds.

To check an existing complete report without reinstalling or rebuilding:

```sh
scripts/check-dev8-baseline.sh workspaces/contracts/dev8-pass-baseline.txt /path/contracts.log
```
