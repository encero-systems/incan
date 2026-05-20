# Install, build, and run (make-first)

This page documents the canonical way to build/install/run Incan from this repository.

## Prerequisites

- Rust (1.92+): install via [rustup](https://rustup.rs/)
- `git`: to clone the repository
- `make`: for the canonical make-first workflow

These instructions assume a Unix-like shell environment (macOS/Linux). If you’re on Windows, use WSL or adapt the commands.

## Get the repo (run from repo root)

Run the commands on this page from the repository root (the directory containing `Makefile`):

```bash
git clone https://github.com/dannys-code-corner/incan.git
cd incan
```

## Canonical command flow

--8<-- "_snippets/commands/install_build_run.md"

## Run a file

If you used `make install`:

```bash
incan run hello.incn
```

If you used the no-install fallback:

```bash
./target/release/incan run hello.incn
```
