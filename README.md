# Incan Programming Language

Incan is a statically typed language for writing clear, high-level application code that compiles to native Rust. It aims to feel lightweight and expressive while keeping the things that matter in large codebases explicit: types, errors, and mutability.

The current toolchain is designed to be easy to install, try, inspect, and diagnose without cloning the compiler repository first.

## Getting started

Install the latest toolchain release before creating your first project:

```bash
curl -fsSL https://github.com/encero-systems/incan/releases/latest/download/install.sh | bash
export PATH="$HOME/.local/bin:$PATH"
incan --version
```

You can also install through package-manager adapters that use the same release manifest and verified toolchain archives:

```bash
brew tap encero-systems/tap
brew install incan
npm install -g @incan/toolchain
pipx install incan
```

Rust users can also build and install the release source through Cargo:

```bash
cargo install --git https://github.com/encero-systems/incan.git --tag v0.4.0 --locked --features lsp --bin incan --bin incan-lsp
```

Create a starter project, run it, test it, and produce a release build:

```bash
incan new hello --yes
cd hello
incan run
incan test
incan build --release
```

The direct installer links `incan` and `incan-lsp` into `~/.local/bin` by default and provisions the Rust backend through `rustup` when needed, including the `wasm32-wasip1` target used by packages with vocab companions. The npm and pipx packages delegate to that same installer and verified toolchain archive contract. Homebrew installs the prebuilt Incan commands through the generated formula, but Homebrew does not quietly run rustup into your home directory; make sure Rust and `wasm32-wasip1` are available when using the Homebrew path. Cargo installation compiles from source and is mainly for Rust users who prefer that workflow. See [Install and run Incan](workspaces/docs-site/docs/tooling/how-to/install_and_run.md) for supported hosts, dry-run installation, manifest pinning, Cargo installation, and source-build fallback instructions.

If you are contributing to the compiler itself, clone this repository and use `make install` instead of the toolchain installer.

## Tooling and inspection

The current toolchain includes these public surfaces for installation, first contact, diagnostics, and inspection:

- **Toolchain installation** through GitHub release artifacts, checksum-verified archives, `install.sh`, Homebrew, npm, and pipx adapters.
- **Starter project flow** through `incan new`, `incan run`, `incan test`, and `incan build --release`.
- **Stable diagnostics** through `incan check --format json` and `incan explain <CODE>`.
- **Build reports** through `incan build --report json`, including compiler version, project identity, generated paths, artifact paths, dependency summaries, Cargo policy flags, timings, and notes.
- **Generated Rust inspection** through `incan inspect rust --format json`, which reports the current Rust-backed compiler output without treating generated Rust as a stable ABI.
- **Codegraph export** through `incan inspect codegraph --format jsonl`, with compiler-backed files, modules, declarations, imports, exports, references, calls, diagnostics, spans, provenance, and degraded-state records.
- **Boundary parity hardening** across local, imported, re-exported, package, test-batch, generated-Rust, and vocab/tooling paths.

Read the [CLI reference](workspaces/docs-site/docs/tooling/reference/cli_reference.md) for detailed command contracts, or the [0.4 release notes](workspaces/docs-site/docs/release_notes/0_4.md) for release-specific change history.

These examples show the inspection commands most useful when evaluating a project:

```bash
incan check src/main.incn --format json
incan explain INCAN-T0001
incan build --report json
incan inspect rust src/main.incn --format json
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
- **Native performance**: the compiler emits Rust and builds a native binary.

## Who is this for?

- If you like the readability of Python but want stronger correctness tools and predictable performance, Incan is aimed at that workflow.
- If you like Rust but want a smaller surface syntax for everyday application code, Incan is built to stay close to Rust semantics while reducing boilerplate.
- If you like TypeScript or JavaScript tooling but want native binaries and Rust-backed execution for application code, Incan should feel familiar in its focus on typed APIs, editor feedback, and installable command-line tooling.

## Choose your path

- [Coming from Python](workspaces/docs-site/docs/start_here/coming_from_python.md): start with the pipx or direct installer path, then compare Python app patterns to typed Incan models, `Result`/`Option`, traits, tests, and Rust-backed deployment.
- [Coming from Rust](workspaces/docs-site/docs/start_here/coming_from_rust.md): start with the Cargo or direct installer path, then inspect how Incan keeps Rust-shaped errors, interop, generated Rust output, diagnostics, and native builds visible.
- [Coming from TypeScript or JavaScript](workspaces/docs-site/docs/start_here/coming_from_typescript_javascript.md): start with the npm or direct installer path, then compare typed app workflows, editor tooling, package scripts, diagnostics, and native artifact inspection.

## Status

> **⚠️ Beta Software ⚠️**  
> Incan is in active development. The language, compiler, and APIs may still change, although we will try to keep it stable as much as possible.  
> Feedback and contributions are of course welcome!

Docs policy: [Stability policy](workspaces/docs-site/docs/stability.md)

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

The docs site lives in `workspaces/docs-site/`.

Start here:

- Start here: `workspaces/docs-site/docs/start_here/index.md`
- Language: `workspaces/docs-site/docs/language/index.md`
- Tooling: `workspaces/docs-site/docs/tooling/index.md`
- Release notes: `workspaces/docs-site/docs/release_notes/0_4.md`

Build/serve locally:

```bash
make docs-build
make docs-serve
```

## Performance

Incan compiles to Rust and then to a native binary. Runtime performance can be close to Rust for many workloads, depending on current codegen and library behavior.

- Benchmarks: `workspaces/benchmarks/`
- Results: `workspaces/benchmarks/results/results.md`

| Benchmark                 | Incan | Rust  | Python   | Incan vs Python   |
|---------------------------|------:|------:|---------:|------------------:|
| Fibonacci (1M iterations) | 15ms  | 17ms  | 490ms    | **32.6×** faster  |
| Collatz (1M numbers)      | 152ms | 155ms | 9,043ms  | **59.4×** faster  |
| GCD (10M pairs)           | 277ms | 298ms | 2,037ms  | **7.3×** faster   |
| Mandelbrot (2K×2K)        | 250ms | 248ms | 12,268ms | **49.0×** faster  |
| N-Body (500K steps)       | 39ms  | 39ms  | 4,934ms  | **126.5×** faster |
| Prime Sieve (10M)         | 117ms | 120ms | 9,520ms  | **81.3×** faster  |
| Quicksort (1M elements)   | 79ms  | 78ms  | 2,435ms  | **30.8×** faster  |
| Mergesort (1M elements)   | 195ms | 196ms | 3,629ms  | **18.9×** faster  |

**Benchmark details:**

- **Machine:** Apple Silicon (results may vary)
- **Incan/Rust:** Release builds with optimizations
- **Python:** CPython 3.12
- **Methodology:** [hyperfine](https://github.com/sharkdp/hyperfine) with warmup runs

## Contributing

Contributions are welcome—docs, compiler, tooling, stdlib, and RFC work.

- Contributor docs: `workspaces/docs-site/docs/contributing/index.md`
- Repo guidelines: [CONTRIBUTING.md](CONTRIBUTING.md)

## License

Apache 2.0
