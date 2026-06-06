# Incan Programming Language

Incan is a statically typed language for writing clear, high-level application code that compiles to native Rust. It aims to feel lightweight and expressive while keeping the things that matter in large codebases explicit: types, errors, and mutability.

## Getting started

Install the latest SDK release, create a starter project, run it, test it, and produce a release build:

```bash
curl -fsSL https://github.com/dannys-code-corner/incan/releases/latest/download/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"
incan new hello --yes
cd hello
incan run
incan test
incan build --release
```

You can also install through package-manager adapters that use the same release manifest and verified SDK archives:

```bash
brew install https://github.com/dannys-code-corner/incan/releases/latest/download/incan.rb
npm install -g incan-sdk
pipx install incan-sdk
```

If you are contributing to the compiler itself, clone this repository and use `make install` instead of the SDK installer.

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
