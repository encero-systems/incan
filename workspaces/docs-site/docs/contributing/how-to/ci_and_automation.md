# Run repository checks

Use an installed dev.8 toolchain for compiler work. The repository has no Cargo project, and its Makefile provides hygiene and documentation checks rather than a compiler bootstrap.

## Run the local gate

From the repository root:

```bash
make pre-commit-fast
make reference-contract-check
make docs-build
```

The fast gate checks Rust formatting, US English, changed rustdocs and the skill table. Reference review and strict documentation builds are separate checks.

## Prove behavior

Use the contract runner with the installed Oven executable:

```bash
incan run workspaces/contracts/src/main.incn -- incan workspaces/contracts/fixtures --jobs 2 check
```

Choose one case or area during development. Run the affected families at commit points. The [contract family guide](https://github.com/encero-systems/incan/blob/main/workspaces/contracts/README.md) defines selectors, expectations and pending cases.

## Check examples

```bash
make examples
```

Use an installed dev.8 toolchain; Oven prepares dependencies automatically. Tune the per-command deadline with `INCAN_EXAMPLES_TIMEOUT` (default 120 seconds). The runner needs GNU `timeout` or `gtimeout` from coreutils and writes `target/examples/examples.tsv`. Entries in `examples/check-only.txt` are checked; other entry points run. Every failure, including a timeout, fails the target.

## Package a toolchain

```bash
incan toolchain pack incan-0.6.0-dev.8.tar.zst
```

Install the resulting archive into a separate home on a compatible host before testing relocation. Local Rust 1.98.0 with `rustc-dev` must match the packed driver's identity. See [Install and run](../../tooling/how-to/install_and_run.md).
