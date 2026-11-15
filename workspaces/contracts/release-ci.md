# Verify an installed release toolchain

Release qualification starts from the immutable host bundle in `build-toolchain.json`, builds the selected checkout's compiler and complete shipped closure, and verifies the candidate directly and through a fresh installation. Installing the released toolchain is a seed selection; it is not evidence that the checkout compiler passed. DD-0010's fixed-point mode becomes a required release-workflow stage once the separate reproducibility work lands.

## Installed verification

`scripts/verify_installation.incn` verifies an already installed toolchain. It builds this checkout's contract runner, runs the committed Oven test inventory and every production Loaf's Rust-facet suite, requires the live language-server contract, and runs all eight contract families. The census is complete only when it contains one final total, unique result keys, and every family. Every key in `pass-baseline.txt` must remain `PASS`; existing failures remain visible without weakening that lower bound.

```sh
incan run scripts/verify_installation.incn -- target/release-verification --jobs 2 --timeout 120
```

The selected installation comes from `INCAN_HOME`; `INCAN_LSP` may select its matching server. Each invocation creates and retains one `run.*` evidence directory below the supplied output. It contains the compiler, Oven, Rust-facet, LSP, and contract logs; input digests; `oven-tests.json`; counts; and the summary. The supported release hosts are macOS arm64 and Linux x86_64.

The composite install action performs only work that must happen before an Incan command exists: read the checked-in target pin, install Rust 1.98.0 with `rustc-dev`, pull the immutable OCI bundle with oras, install its archive, and export the installed commands. Any requested verification then runs the Incan program above.

## Release qualification

`scripts/release.incn` creates a clean detached checkout of a full source SHA and builds the candidate without publishing it. The released Oven that runs the program and the toolchain under `INCAN_HOME` are the seed. Worker and timeout choices are arguments, and the fixed-point gate is explicit:

```sh
incan run scripts/release.incn -- "$(git rev-parse HEAD)" target/release \
  --prove-fixed-point --jobs 6 --timeout 600
```

The seed builds only the conductor and the support helper. Both link the seed's installed native crates, and the seed links such a crate only into builds of the sources it was built from. When the source changes one of those crates (`loaves/kernel/incan_lang`), pass `--seed-source SEED_SHA`, the commit the seed was built from: the two seed-built tools then take that crate's sources from the seed's commit and everything else from the source SHA. Every later step, and so every shipped artifact, builds the source SHA alone.

The program builds the source support closure, checked bootstrap interfaces, compiler, runtime, standard Loaves, Oven, language server, and contract runner. It writes `receipt.json` with schema `incan.release-rehearsal/1`, `timings.tsv`, one retained log per phase, `bundle/{incan,incan-lsp,toolchain.tar.zst}`, and `ci/run.*/` installed-verification evidence. A failure leaves a refused receipt naming the current phase. Normal qualification omits `--prove-fixed-point`; release workflows pass the flag because a published release must reproduce DD-0010's driver, runtime, lowering, standard-library floor, Oven, and runner bytes.

The candidate census must have the same case keys as the seed census over the same committed fixture tree, preserve every seed PASS, and preserve the committed baseline. A fresh installation at another source, home, and Rust-sysroot path must reproduce every direct candidate outcome. Linux additionally pulls the pinned Clang artifact with oras and unpacks it through Incan libraries; macOS evidence does not establish that branch.

The toolchain version directory remains `0.6.0-dev.8` because it is the installed release identity, not an orchestration name.

## Publication

Publication is a separate maintainer operation. After both native hosts pass release qualification, authenticate oras and run:

```sh
incan run scripts/publish_build_toolchain.incn -- \
  COMMIT MAC_ARCHIVE MAC_OVEN MAC_LSP LINUX_ARCHIVE LINUX_OVEN LINUX_LSP
```

The program validates both complete bundles before publishing either, pushes each annotated OCI artifact once, reads the digest returned by that push, and atomically rewrites `build-toolchain.json` only after both pushes succeed. It does not change repository variables. A failed second push can leave the first immutable artifact present, but the checked-in pin remains unchanged.

## Baseline maintenance

`pass-baseline.txt` is measured evidence, not a desired-result list. Preserve its keys when updating it and investigate any lost key before removal. `oven-tests.txt` is likewise the exact expected passing test-id set for the installed release gate. Linux and macOS share the same minimum semantic baseline; each host still requires native release evidence.
