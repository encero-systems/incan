# Getting Started with Incan

## Prerequisites

- Rust (1.85+): install via [rustup](https://rustup.rs/)
- `git`: to clone the repository
- `make`: for the canonical make-first workflow

These instructions assume a Unix-like shell environment (macOS/Linux). If you’re on Windows, use WSL:

- WSL install guide: `https://learn.microsoft.com/windows/wsl/install`

## Install/build/run (canonical)

Follow: [Install, build, and run](../how-to/install_and_run.md).

## Your First Program

Create a file `hello.incn`:

```incan
def main() -> None:
    println("Hello, Incan!")
```

Run it:

If you used `make install`:

```bash
incan run hello.incn
```

If you used the no-install fallback:

```bash
./target/release/incan run hello.incn
```

## Project Structure

To scaffold a full project with an entry point, test file, and manifest:

```bash
mkdir my_project && cd my_project
incan init
```

This creates a ready-to-run layout:

```text
my_project/
├── src/
│   └── main.incn          # Entry point ("Hello from my_project!")
├── tests/
│   └── test_main.incn     # Starter test
└── incan.toml             # Project manifest
```

You can run it immediately:

```bash
incan run src/main.incn
incan test tests/
```

For the full walkthrough — adding modules, Rust crate dependencies, and lock files — see:
[Your first project](your_first_project.md).

## Next Steps

- [Your first project](your_first_project.md) - Set up a real project with modules, dependencies, tests, and
  lock files
- [Formatting Guide](../how-to/formatting.md) - Code style and `incan fmt`
- [CLI Reference](../reference/cli_reference.md) - Commands, flags, and environment variables
- [Projects today](../explanation/projects_today.md) - Where builds go, what is regenerated, and what’s planned
- [Troubleshooting](../how-to/troubleshooting.md) - Common setup and “it didn’t work” fixes
- [Language: Start here](../../language/index.md) - Learn Incan syntax and patterns
- [Stability policy](../../stability.md) - Versioning expectations and “Since” semantics
- [Examples](https://github.com/dannys-code-corner/incan/tree/main/examples) - Sample programs
