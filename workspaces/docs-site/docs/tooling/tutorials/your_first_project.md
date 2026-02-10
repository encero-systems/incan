# Your first Incan project

This tutorial walks you through setting up a real Incan project from scratch — from `incan init`
to a running, tested program with multiple modules.

**Prerequisites**: [Getting started](getting_started.md) (Incan installed and `incan run hello.incn` works).

**Time**: ~10 minutes.

## What you'll build

A small command-line tool called **greeter** with a greeting module and tests. Along the way you'll:

1. Scaffold a project with `incan init`
2. Split code into modules with imports
3. Write and run tests

---

## Step 1: Create the project

```bash
mkdir greeter && cd greeter
incan init
```

`incan init` scaffolds a ready-to-run project:

```text
Created project 'greeter' at .

  src/main.incn          Entry point
  tests/test_main.incn   Starter test
  incan.toml             Project manifest

Run it:   incan run src/main.incn
Test it:  incan test tests/
```

Your project layout:

```text
greeter/
├── src/
│   └── main.incn          # "Hello from greeter!"
├── tests/
│   └── test_main.incn     # Placeholder test
└── incan.toml             # Manifest with [project.scripts] main set
```

Try it immediately:

```bash
incan run src/main.incn
```

```text
Hello from greeter!
```

The generated `incan.toml` already has `[project.scripts] main` pointing at `src/main.incn`, so commands like `incan lock`
will work without a file argument later on.

## Step 2: Add a module

Let's extract the greeting logic into its own module. Create `src/greet.incn`:

```incan title="src/greet.incn"
"""Greeting utilities."""

pub def greet(name: str) -> str:
    return f"Hello, {name}!"
```

Note the `pub` keyword — without it, `greet` would be private to its module and you couldn't
import it.

Now update `src/main.incn` to use the `greet` function from the `greet.incn` module:

```incan title="src/main.incn"
from greet import greet

def main() -> None:
    println(greet("World"))
```

Run again:

```bash
incan run src/main.incn
```

output:

```text
Hello, World!
```

### Adding more functions

Let's add a second function. Update `src/greet.incn` to add the `farewell` function:

```incan title="src/greet.incn"
"""Greeting utilities."""

pub def greet(name: str) -> str:
    return f"Hello, {name}!"

pub def farewell(name: str) -> str:
    return f"Goodbye, {name}!"
```

And update `src/main.incn` to use both:

```incan title="src/main.incn"
from greet import greet, farewell

def main() -> None:
    println(greet("World"))
    println(farewell("World"))
```

```bash
incan run src/main.incn
```

output:

```text
Hello, World!
Goodbye, World!
```

## Step 3: Write tests

`incan init` already created a placeholder test. Let's replace it with real tests for our
greeting module. Update `tests/test_main.incn`:

```incan
from greet import greet, farewell

from std.testing import assert_eq

def test_greet() -> None:
    assert_eq(greet("Alice"), "Hello, Alice!")

def test_greet_empty() -> None:
    assert_eq(greet(""), "Hello, !")

def test_farewell() -> None:
    assert_eq(farewell("Alice"), "Goodbye, Alice!")
```

Notice the import: `from greet import greet, farewell` — the exact same syntax as in
`src/main.incn`. The test runner resolves imports against your project's source root
(`src/`), so tests and source code share the same import paths.

Run the tests:

```bash
incan test tests/
```

You should see output like:

```text
=================== test session starts ===================
collected 3 item(s)

test_main.incn::test_greet PASSED
test_main.incn::test_greet_empty PASSED
test_main.incn::test_farewell PASSED

=================== 3 passed in 2.69s ===================
```

!!! tip "Test discovery"
    Test files are found by name (`test_*.incn`) and test functions by name (`def test_*()`).
    See: [Testing](../how-to/testing.md).

## Your final project layout

```text
greeter/
├── src/
│   ├── main.incn          # Entry point
│   └── greet.incn         # Greeting module
├── tests/
│   └── test_main.incn     # Tests for greet module
└── incan.toml             # Project manifest
```

## Recap

| Step | What you did                 | Key command / concept        |
| ---- | ---------------------------- | ---------------------------- |
| 1    | Scaffolded a project         | `incan init`                 |
| 2    | Split code into modules      | `pub`, `from ... import ...` |
| 3    | Wrote and ran tests          | `incan test tests/`          |

## Next steps

- [Rust interop](../../language/how-to/rust_interop.md) — Use Rust crates from Incan code
- [Managing dependencies](../how-to/dependencies.md) — `incan.toml`, version annotations, and lock files
- [Project configuration reference](../reference/project_configuration.md) — Full `incan.toml` format
- [CI & automation](../how-to/ci_and_automation.md) — Locked builds, pipelines, and deployment
- [The Incan Book](../../language/tutorials/book/index.md) — Learn the language itself
