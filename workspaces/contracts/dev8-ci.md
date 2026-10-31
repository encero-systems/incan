# Verify an installed dev.8 toolchain

The dev.8 workflow installs a packed release toolchain, builds this checkout's contract runner with that compiler, and runs all eight contract families with two workers. It compares case keys against `dev8-pass-baseline.txt`: a formerly passing case must still pass, including cases that disappear or become pending. Existing failures are reported without failing this regression gate. A runner build failure, incomplete report, duplicate result, missing family, or inconsistent total fails the job.

CI also runs Oven's and each Loaf's test source files through the installed `incan test`, which accepts one source file per invocation, and compares the passing test ids against the committed `dev8-oven-tests.txt` list. The compiler record contracts compare against the frontend's native adapter, which their Loaf does not depend on, so `scripts/test-checker-records.sh` runs them in an isolated project that adds it. Removing six unused modules and their tests replaced the former 133-test count gate; a missing, duplicate or unexpected id fails the set comparison. `oven-tests.json` records the observed and expected counts. The installed LSP is mandatory: the graph family's existing `graph_lsp_contract.incn` starts it through `INCAN_LSP`, checks initialization, definitions, references and call hierarchy, then changes the buffer and requires rejection of the stale hierarchy item. CI first requires that case to pass independently and keeps its log, then runs the complete census with the same LSP selected.

This verifies the installed release against the checkout's contracts. It does not rebuild the PR's compiler or prove self-hosting. DD-0010 requires that additional compiler build after self-hosting is proven. Release publication remains the maintainer's operation.

## Local archive

Install rustup first, then supply the archive and its matching Oven and LSP executables. The script installs Rust 1.98.0 with `rustc-dev` when absent, creates a fresh Incan home, installs both tools in its `bin` directory, and retains installation, build, and contract logs. It exposes only standard packages present in the archive; their source declarations come from this checkout. An older archive may consequently refuse a newer runner.

```sh
CI_DEV8_OUT="$PWD/target/dev8-evidence" scripts/ci-dev8.sh /path/toolchain.tar.zst /path/incan /path/incan-lsp
```

The supported hosts are macOS arm64 and Linux x86_64. `CI_DEV8_JOBS` sets the worker budget, and `CI_DEV8_TIMEOUT` sets the per-command timeout in seconds (default 120). Every invocation creates a new evidence directory, prints its location, and leaves it available after failure. The local archive and supplied Oven are trusted local inputs; the archive checksum is recorded in that directory.

## CI release bundle

`incan toolchain pack` contains the compiler closure, but omits the command-line tools. Each host's OCI bundle therefore contains three files built from one source SHA: `toolchain.tar.zst`, `incan` (Oven), and `incan-lsp`. Build the LSP with the same installed driver using `incan build loaves/toolchain/incan-lsp --profile release`. All three files are covered by the same manifest digest. The workflow pulls that pinned bundle with oras 1.3, then invokes the same script. The script refuses a reference without both its host's tag suffix and a SHA-256 digest.

The repository variable `BUILD_TOOLCHAIN` names the released toolchain that CI builds with. It is a JSON object with a `version` key naming that release and one key per published host target. dev.8 publishes `aarch64-apple-darwin` and `x86_64-unknown-linux-gnu`. Each host value has this shape, using a real commit and manifest digest:

```text
ghcr.io/encero-systems/incan.pub/toolchain/incan:<commit>-<target>@sha256:<64 hex digits>
```

Leave the variable unset until the real release passes self-hosting. The contract jobs fail with an unset variable; they never substitute a mutable tag or a private bootstrap. The separate docs job runs US English checks and the strict docs build regardless of release availability.

## Publication handoff

The macOS candidate measured on October 24, 2026 (Europe/Berlin), contains the compiler and fixture source tree committed as `edf65667d3f413a5a0d9f8e560eb9a59968df95a`. It was packed and measured before committing that unchanged tree; the CI checkout record names parent `ed6475e26`. Its archive SHA-256 is `74dc9f59d1fb8168c21a197151389d4b66878cc7045e60b8782385940acb2b05` (461227351 bytes, ten standard packages). A fresh installation built the native contract runner and completed all eight families: **1126 PASS / 36 FAIL / 0 PEND**. An independent supplied-controller census has exactly the same PASS keys. All 1124 previous PASS keys survive; `behavior/emit_calls_and_builtins/std_fs_path_str_or_path_targets` and `behavior/snapshots_stdlib/std_fs_path` now pass. The complete contract logs name the remaining failures and their diagnostics. This establishes the candidate's installed runner build, not a compiler self-build or Linux parity.

The stored-current macOS archive checked for that measurement still names source `e5b69517d970dbab64b77e0ff8eb742b20b01219`, archive SHA-256 `702958a45b2da4c400d3b82b45e59b5db28f07b8cd60fb5e054ee31a9b142e7f`. Its fresh installation succeeds, but the runner build refuses `CommandExt.process_group`. That immutable archive does not contain the fixes above. The verified candidate is local evidence; it has not replaced the stored archive or been published.

Integrate the compiler fixes and select the full merged source SHA. Build both the `aarch64-apple-darwin` and `x86_64-unknown-linux-gnu` archives and their matching Oven and LSP executables from that same SHA. On each native host, verify a fresh installed runner build, a complete eight-family report, and equality with that host's direct PASS keys, preserving the committed baseline. Finish the separate compiler self-build verification before publication.

After self-hosting and release verification, the maintainer authenticates oras to GHCR and GitHub CLI to the repository, then runs `scripts/publish-build-toolchain.sh` with the full source commit, the macOS archive with its matching Oven and LSP, the Linux archive with its matching Oven and LSP, and a new JSON output path. Every bundle must come from that commit. The script pushes each annotated bundle once and reads its digest directly from the [oras push descriptor](https://oras.land/docs/how_to_guides/format_output/#oras-push). Only after every push succeeds does it write the receipt and set the single `BUILD_TOOLCHAIN` repository variable to the complete set of pins. OCI pushes are separate operations; a failed second push can leave the first bundle published without a completed pins file. If the variable update fails, the JSON receipt remains available and the prior variable value is preserved.

On Linux, use the Linux archive and Linux Oven on an x86_64 host, or set the variable and run the `ubuntu-latest` job. A macOS run does not establish Linux outcomes. Store records and release inputs must identify the same merged source SHA for both hosts before they can establish one verified release pair.

## Host release rehearsal

`scripts/release-dev8.sh SHA NEW_OUT` creates a clean detached checkout of a full source SHA and builds a candidate without publishing it. Select an installed dev.8 seed with `INCAN_HOME` and set `RELEASE_DEV8_OVEN` to its Oven executable. The host needs `zsh` for the checkout's native build scripts.

The script builds natively from the clean checkout, with the checkout's own scripts over a lane laid out inside the output: `build-native-tools.sh oven` builds Oven with the seed, `relink-driver.sh --release` compiles the Incan lowering and the driver and installs the driver with its Rust ABI cache, and `build-native-runtime.sh` compiles and installs the runtime. It then builds the standard packages, rebuilds Oven and builds the LSP and runner with `build-native-tools.sh`, and packs the toolchain. Standard packages are compiled once and exposed to both profiles with identical Rust carrier identities. It runs all eight families directly, invokes `ci-dev8.sh` against the local three-file bundle in a fresh home, and requires every case's outcome to agree with the direct census as well as preserving the committed PASS baseline.

Use the integration head's installed toolchain as the seed. Before candidate assembly, the script measures its reference census on the clean checkout's cases. To reuse an already complete integration census for that SHA, set `RELEASE_DEV8_BASELINE_LOG` to its log; a companion file named `<log>.source-sha` must contain exactly the selected full SHA. Both the directly built candidate and fresh installation must match every reference outcome, including existing failures and pending cases. The receipt retains the reference log digest and both parity comparisons.

```sh
INCAN_HOME=/path/seed-home RELEASE_DEV8_OVEN=/path/incan \
  scripts/release-dev8.sh "$(git rev-parse HEAD)" /path/new-rehearsal
```

`receipt.json` records the SHA, host, three file digests, counts, Oven test count, outcome parity, compiler build mode and elapsed time. `timings.tsv` and per-step logs retain build and verification evidence. A failed prerequisite writes a refused receipt; absent counts remain null. A refused build never relabels the seed as a candidate. Remove disposable build and verification homes after reviewing the receipt when they are no longer needed. Linux must run the same command on its native host with matching seed artifacts; macOS evidence does not establish Linux results.

## Baseline maintenance

The comments in `dev8-pass-baseline.txt` identify the checkout, archive, Oven, runner, and measurement conditions. The baseline is measured evidence, not a list of desired results. Preserve its PASS keys when updating it; investigate a lost key before removing it. The baseline is shared by both hosts to require the same minimum behavior; Linux parity remains unverified until a Linux run succeeds.

To check an existing complete report without reinstalling or rebuilding:

```sh
scripts/check-dev8-baseline.sh workspaces/contracts/dev8-pass-baseline.txt /path/contracts.log
```
