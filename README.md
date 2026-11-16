<p align="center">
  <img src="workspaces/docs-site/docs/shared/incapunk/incan_mark_440.png" alt="" width="170">
</p>

<p align="center">
  <img src="workspaces/docs-site/docs/shared/incapunk/wordmark_small_001.png" alt="Incan" width="220">
</p>

<p align="center">
  <b>Python-like readability. Rust-native performance.</b>
</p>

<p align="center">
  <a href="https://incan.io">Website</a> ·
  <a href="https://incan.io/latest/start_here/">Docs</a> ·
  <a href="https://incan.io/latest/tooling/how-to/install_and_run/">Install</a> ·
  <a href="https://incan.io/latest/language/tutorials/book/">The Book</a> ·
  <a href="https://github.com/encero-systems/incan/discussions">Discussions</a> ·
  <a href="https://www.linkedin.com/groups/42515004/">LinkedIn</a>
</p>

<p align="center">
  <a href="https://github.com/encero-systems/incan/actions/workflows/ci.yml"><img alt="CI status" src="https://img.shields.io/github/actions/workflow/status/encero-systems/incan/ci.yml?branch=main&style=flat-square&label=CI&labelColor=0d1117"></a>
  <a href="https://github.com/encero-systems/incan/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/encero-systems/incan?style=flat-square&color=ffc15a&labelColor=0d1117"></a>
  <a href="https://incan.io"><img alt="Documentation" src="https://img.shields.io/badge/docs-incan.io-48f0ef?style=flat-square&labelColor=0d1117"></a>
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/github/license/encero-systems/incan?style=flat-square&color=48f0ef&labelColor=0d1117"></a>
  <a href="#status"><img alt="Project status: beta" src="https://img.shields.io/badge/status-beta-ff5c69?style=flat-square&labelColor=0d1117"></a>
</p>

Incan is a statically typed language for writing clear, high-level application code that compiles to native code through the Rust compiler. It aims to feel lightweight and expressive while keeping the things that matter in large codebases explicit: types, errors, and mutability.

The current toolchain is designed to be easy to install, try, inspect, and diagnose without cloning the compiler repository first.

## Getting started

Incan 0.6 installs from a release bundle for your host. A bundle holds the `incan` command, the `incan-lsp` language server and the toolchain archive; the [release page](https://github.com/encero-systems/incan/releases) names the bundle for each host. You need Rust 1.98.0 with `rustc-dev`, the compiler Incan builds through, and [ORAS](https://oras.land) 1.3 to pull the bundle:

```bash
rustup toolchain install 1.98.0 --profile minimal --component rustc-dev
oras pull ghcr.io/encero-systems/loaves.pub/toolchain/incan:<bundle for your host> --output incan-bundle
chmod +x incan-bundle/incan incan-bundle/incan-lsp
incan-bundle/incan toolchain install incan-bundle/toolchain.tar.zst
cp incan-bundle/incan-lsp "$HOME/.incan/bin/"
export PATH="$HOME/.incan/bin:$PATH"
incan --version
```

`incan toolchain install` first checks for Rust 1.98.0 and `rustc-dev`, and names the `rustup` command to run when either is missing. It verifies the archive before it replaces an installation, installs the toolchain under `~/.incan`, and puts `incan` and `oven` in `~/.incan/bin`. Set `INCAN_HOME` before installing to use another home. Installation never installs Rust or changes your default Rust toolchain. The archive carries the standard library, so a project needs nothing else.

Create a project, then check, run, test and build it. Every command takes an explicit path:

```bash
oven new hello
cd hello
incan check src/main.incn
incan run src/main.incn
incan test tests/test_main.incn
incan build src/main.incn --profile release
```

`oven new` writes a starter project with `loaf.toml`, `src/main.incn` and `tests/test_main.incn`. The release build writes the executable to `target/incan/release/bin/hello`. See [Install and run Incan](https://incan.io/latest/tooling/how-to/install_and_run/) for installing into a separate home and packing an installed toolchain. The 0.5 installer, Homebrew, npm and pipx channels install Incan 0.5.1; [Move a project from 0.5 to 0.6](https://incan.io/latest/tooling/how-to/move_from_0_5/) lists what changed.

If you are contributing to the compiler itself, see [Use a development checkout](https://incan.io/latest/tooling/how-to/install_and_run/#use-a-development-checkout).

## Tooling and inspection

The current toolchain includes these public surfaces for installation, first contact, diagnostics, and inspection:

- **Toolchain installation** from a release bundle for each host: `incan toolchain install` verifies an archive and the local Rust compiler before it replaces an installation, and `incan toolchain pack` packs an installed toolchain as a relocatable archive.
- **Project loop** through `incan check`, `incan run`, `incan test`, and `incan build`, each with an explicit path. `oven` runs the same project operations.
- **Stable diagnostics** through `incan check --format json` and `incan explain <CODE>`.
- **Build inputs and reuse** through `oven describe-unit PATH`, which reports the selected compilation inputs, and `OVEN_TRACE=-`, which reports executable reuse decisions.
- **Codegraph export** through `incan inspect codegraph --format jsonl`, with compiler-backed files, modules, declarations, imports, exports, references, calls, diagnostics, spans, provenance, and degraded-state records.
- **Boundary parity hardening** across local, imported, re-exported, package, test-batch, and vocab/tooling paths.

Read the [CLI reference](https://incan.io/latest/tooling/reference/cli_reference/) for detailed command contracts, or the [0.6 release notes](https://incan.io/latest/release_notes/0_6/) for release-specific change history.

These examples show the inspection commands most useful when evaluating a project:

```bash
incan check src/main.incn --format json
incan explain INCAN-T0001
oven describe-unit src/main.incn
incan inspect codegraph src --format jsonl
```

## Positioning

Python won because it made application code readable and fast to write. Incan starts from that same readability premise, but changes the foundation: static types, explicit errors, explicit mutability, and Rust-native compilation.

Incan is not a Python compatibility runtime or a faster Python interpreter. It is for new application code where teams want Python-like ergonomics without Python's runtime, packaging, and deployment tradeoffs.

As AI tools generate more code, those constraints matter more. Incan gives developers and agents a smaller, typed, auditable language surface that compiles into the Rust ecosystem.

## Why Incan?

- **Readable by default**: concise syntax for modeling data and writing “glue code” without ceremony.
- **Explicit error handling**: `Result`, `Option`, and `?` keep failure paths visible and reviewable.
- **Strong domain types**: `newtype` and `model` make invariants and intent first-class.
- **Deterministic composition**: traits are for behavior contracts and predictable composition.
- **Rust interop when you need it**: call into Rust crates for ecosystems and performance-sensitive utilities.
- **Native performance**: the compiler builds a native binary through the pinned Rust compiler.

## Who is this for?

- If you like the readability of Python but want stronger correctness tools and predictable performance, Incan is aimed at that workflow.
- If you like Rust but want a smaller surface syntax for everyday application code, Incan is built to stay close to Rust semantics while reducing boilerplate.
- If you like TypeScript or JavaScript tooling but want native binaries and Rust-backed execution for application code, Incan should feel familiar in its focus on typed APIs, editor feedback, and installable command-line tooling.

## Choose your path

- [Coming from Python](https://incan.io/latest/start_here/coming_from_python/): install as [Getting started](#getting-started) shows, then compare Python app patterns to typed Incan models, `Result`/`Option`, traits, tests, and Rust-backed deployment.
- [Coming from Rust](https://incan.io/latest/start_here/coming_from_rust/): install as [Getting started](#getting-started) shows, then inspect how Incan keeps Rust-shaped errors, interop, diagnostics, and native builds visible.
- [Coming from TypeScript or JavaScript](https://incan.io/latest/start_here/coming_from_typescript_javascript/): install as [Getting started](#getting-started) shows, then compare typed app workflows, editor tooling, package scripts, diagnostics, and native artifact inspection.

## Status

> **⚠️ Beta Software ⚠️**  
> Incan is in active development. The language, compiler, and APIs may still change, although we will try to keep it stable as much as possible.  
> Feedback and contributions are of course welcome!

Docs policy: [Stability policy](https://incan.io/latest/stability/)

## A small example

```incan
enum AppError:
    InvalidInput(str)

type Email = newtype str:
    def from_str(v: str) -> Result[Email, AppError]:
        if "@" not in v:
            return Err(AppError.InvalidInput("missing @"))
        return Ok(Email(v.lower()))

@derive(Debug, Eq, Clone)
model User:
    id: int
    email: Email
    is_active: bool = true

trait Loggable:
    def log(self, msg: str) -> None:
        println(f"[{self.name}] {msg}")

class UserService with Loggable:
    name: str
    users: Dict[int, User]

    def create(mut self, email_str: str) -> Result[User, AppError]:
        email = Email.from_str(email_str)?
        user = User(id=len(self.users) + 1, email=email)
        self.users[user.id] = user
        self.log(f"created user {user.id}")
        return Ok(user)
```

## Documentation

The docs site is published at [incan.io](https://incan.io); its source lives in `workspaces/docs-site/`.

- [Start here](https://incan.io/latest/start_here/)
- [Language](https://incan.io/latest/language/)
- [Tooling](https://incan.io/latest/tooling/)
- [Release notes](https://incan.io/latest/release_notes/)

Build/serve locally:

```bash
make docs
make docs-serve
```

## Performance

Incan compiles to a native binary through the Rust compiler, so Rust is the meaningful reference point: on these workloads Incan runs within roughly 20% of hand-written Rust, and matches or beats it on some. Python is included for scale, not as the target.

- Benchmarks: `workspaces/benchmarks/`
- Results: `workspaces/benchmarks/results/results.md`

| Benchmark                 | Incan | Rust  | Python  | Incan vs Rust |
|---------------------------|------:|------:|--------:|--------------:|
| Fibonacci (1M iterations) | 3ms   | 4ms   | 42ms    | ~1×           |
| Collatz (1M numbers)      | 94ms  | 92ms  | 4,209ms | 1.02×         |
| GCD (10M pairs)           | 92ms  | 84ms  | 819ms   | 1.10×         |
| Mandelbrot (2K×2K)        | 113ms | 113ms | 4,655ms | 1.00×         |
| N-Body (500K steps)       | 20ms  | 17ms  | 1,507ms | 1.18×         |
| Prime Sieve (50M)         | 141ms | 124ms | 3,163ms | 1.14×         |
| Quicksort (1M elements)   | 56ms  | 48ms  | 1,000ms | 1.17×         |
| Mergesort (1M elements)   | 83ms  | 130ms | 1,300ms | **0.64×**     |

A ratio below 1.00× means Incan is faster than the hand-written Rust implementation of the same algorithm; Mergesort is the clear case. Fibonacci runs in single-digit milliseconds, where a one-millisecond difference is measurement noise rather than a meaningful gap, so it is reported as approximately even.

**Benchmark details:**

- **Machine:** Apple Silicon (results may vary)
- **Incan/Rust:** Release builds with optimizations
- **Python:** CPython 3.14
- **Methodology:** [hyperfine](https://github.com/sharkdp/hyperfine) with warmup runs
- **Reproduce:** `make benchmarks` (add `SKIP_PYTHON=true` for Incan vs Rust only)

## Community

- LinkedIn group: [Incan Programming Language Community](https://www.linkedin.com/groups/42515004/)
- GitHub Discussions: [Ask questions and share what you're building](https://github.com/encero-systems/incan/discussions)

## Contributing

Contributions are welcome—docs, compiler, tooling, stdlib, and RFC work.

- [Contributor docs](https://incan.io/latest/contributing/)
- Repo guidelines: [CONTRIBUTING.md](CONTRIBUTING.md)

## License

Apache 2.0
