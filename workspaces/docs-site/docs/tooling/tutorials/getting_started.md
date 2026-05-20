# Getting Started with Incan

## Prerequisites

- Rust (1.92+): install via [rustup](https://rustup.rs/)
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

To scaffold a full project with an entry point, test file, and manifest, use the standard project lifecycle path. It is the simplest path for most first projects.

```bash
incan new my_project --yes
cd my_project
```

This creates a ready-to-run layout:

```text
my_project/
├── src/
│   └── main.incn          # Entry point ("Hello from my_project!")
├── tests/
│   └── test_main.incn     # Starter test
├── README.md
├── .gitignore
└── incan.toml             # Project manifest
```

You can run it immediately:

```bash
incan run
incan test
```

When you run `incan new` in an interactive terminal without `--yes`, it prompts for the project name, version, description, author, and license. Use `incan init` instead when you are already inside an existing directory and want to add Incan project files there.

For the full walkthrough — adding modules and tests — see: [Your first project](your_first_project.md).

## Next Steps

- [Your first project](your_first_project.md) - Set up a real project with modules and tests
- [Incan Code Style Guide](../../language/reference/code_style.md) - Canonical source layout rules
- [Formatting with `incan fmt`](../how-to/formatting.md) - Formatter command usage
- [CLI Reference](../reference/cli_reference.md) - Commands, flags, and environment variables
- [Projects today](../explanation/projects_today.md) - Where builds go, what is regenerated, and what’s planned
- [Troubleshooting](../how-to/troubleshooting.md) - Common setup and “it didn’t work” fixes
- [Language: Start here](../../language/index.md) - Learn Incan syntax and patterns
- [Stability policy](../../stability.md) - Versioning expectations and “Since” semantics
- [Examples](https://github.com/dannys-code-corner/incan/tree/main/examples) - Sample programs
