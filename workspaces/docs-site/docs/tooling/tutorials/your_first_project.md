# Your first Incan project

This tutorial walks you through setting up a real Incan project from scratch: create a new project, run it, split it into modules, and add tests.

<aside class="inc-tutorial-meta" aria-label="Tutorial details">
  <dl>
    <div><dt>Reader</dt><dd>New Incan project author</dd></div>
    <div><dt>Prerequisites</dt><dd>Getting Started completed</dd></div>
    <div><dt>Time</dt><dd>15–25 minutes</dd></div>
    <div><dt>Verified</dt><dd>Incan <code>&gt;=0.5.0-0,&lt;0.6.0</code> development docs; run and tests exercised with the prepared 0.5 release envelope</dd></div>
    <div><dt>Status</dt><dd>Release-envelope executable</dd></div>
    <div><dt>Outcome</dt><dd>A multi-module command-line project</dd></div>
    <div><dt>Artifacts</dt><dd>Manifest, modules, entry point, and tests</dd></div>
  </dl>
</aside>

## What you'll build

A small command-line tool called **greeter** with a greeting module and tests. Along the way you'll:

1. Scaffold a project with `incan new`
2. Split code into modules with imports
3. Write and run tests

---

## Step 1: Create the project

For a brand-new project, use `incan new`. In a terminal it can prompt for the project metadata:

```bash
incan new
```

Answer the prompts like this:

```text
Project name [incan_project]: greeter
Version [0.1.0]:
Description []: A small greeting command
Author (n to skip) []: n
License []: MIT
```

Then enter the new project directory:

```bash
cd greeter
```

If you are scripting the same setup, pass the metadata as flags and use `--yes`:

```bash
incan new greeter --description "A small greeting command" --license MIT --yes
cd greeter
```

`incan new` scaffolds a ready-to-run project:

```text
Created project 'greeter' at greeter

  src/main.incn          Entry point
  tests/test_main.incn   Starter test
  incan.toml             Project manifest

Run it:   incan run
Test it:  incan test
```

Your project layout:

```text
greeter/
├── src/
│   └── main.incn          # "Hello from greeter!"
├── tests/
│   └── test_main.incn     # Placeholder test
├── README.md              # Project README
├── .gitignore             # Ignores target/
└── incan.toml             # Manifest with project metadata and [project.scripts] main set
```

The generated `incan.toml` carries a `requires-incan` constraint for the current release line. Commit `incan.lock` once the project generates it so builds stay reproducible.

Try it immediately:

```bash
incan run
```

```text
Hello from greeter!
```

The generated `incan.toml` records the project metadata and already has `[project.scripts] main` pointing at `src/main.incn`, so commands like `incan lock` will work without a file argument later on:

```toml title="incan.toml"
[project]
name = "greeter"
version = "0.1.0"
description = "A small greeting command"
license = "MIT"
readme = "README.md"
requires-incan = ">=0.5.0-0,<0.6.0"

[project.scripts]
main = "src/main.incn"
```

!!! note "`incan new` versus `incan init`"
    Use `incan new` when you want Incan to create a project directory for you. Use `incan init` when you are already inside an existing directory and want to add an Incan manifest, entry point, and starter test there.

## Step 2: Add a module

Let's extract the greeting logic into its own module. Create `src/greet.incn`:

```incan title="src/greet.incn"
"""Greeting utilities."""

pub def greet(name: str) -> str:  # (1)
    return f"Hello, {name}!"
```

1. `pub` is an explicit module boundary. Without it, `greet` remains private to `greet.incn`.

Now update `src/main.incn` to use the `greet` function from the `greet.incn` module:

```incan title="src/main.incn"
from greet import greet  # (1)

pub def greeting() -> str:  # (2)
    return greet("World")

def main() -> None:
    println(greeting())
```

1. Project imports resolve from `src/`; the filename becomes the module name.
2. Export only the function the tests or another module need. `main` can remain private as the configured script entry point.

Run again:

```bash
incan run
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

pub def greeting() -> str:
    return greet("World")

def main() -> None:
    println(greeting())
    println(farewell("World"))
```

```bash
incan run
```

output:

```text
Hello, World!
Goodbye, World!
```

## Step 3: Write tests

`incan new` already created a placeholder test. Let's replace it with real tests for our greeting module. Update `tests/test_main.incn`:

```incan
from greet import greet, farewell  # (1)

from std.testing import assert_eq

def test_greet() -> None:
    assert_eq(greet("Alice"), "Hello, Alice!")

def test_greet_empty() -> None:
    assert_eq(greet(""), "Hello, !")

def test_farewell() -> None:
    assert_eq(farewell("Alice"), "Goodbye, Alice!")
```

1. Tests use the same `src/` module namespace as application code; they do not reach into files through relative paths.

Notice the import: `from greet import greet, farewell` — the exact same syntax as in `src/main.incn`. The test runner resolves imports against your project's source root (`src/`), so tests and source code share the same import paths.

Run the tests:

```bash
incan test
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
    Test files are found by name (`test_*.incn`) and test functions by name (`def test_*()`). See: [Testing](../how-to/testing.md).

## Your final project layout

```text
greeter/
├── src/
│   ├── main.incn          # Entry point
│   └── greet.incn         # Greeting module
├── tests/
│   └── test_main.incn     # Tests for greet module
├── README.md
├── .gitignore
└── incan.toml             # Project manifest
```

<section class="inc-learning-panel inc-learning-panel--complete inc-incus-slot" data-label="Complete" data-incus-category="success" markdown="1">

You now have a manifest-backed command-line project with a public module boundary and meaningful tests.

</section>

## Recap

| Step | What you did                 | Key command / concept        |
| ---- | ---------------------------- | ---------------------------- |
| 1    | Scaffolded a project         | `incan new`                  |
| 2    | Split code into modules      | `pub`, `from ... import ...` |
| 3    | Wrote and ran tests          | `incan test`                 |

## Next steps

- [Rust interop](../../language/how-to/rust_interop.md) — Use Rust crates from Incan code
- [Managing dependencies](../how-to/dependencies.md) — `incan.toml`, version annotations, and lock files
- [Project configuration reference](../reference/project_configuration.md) — Full `incan.toml` format
- [CI & automation](../how-to/ci_and_automation.md) — Locked builds, pipelines, and deployment
- [The Incan Book](../../language/tutorials/book/index.md) — Learn the language itself
