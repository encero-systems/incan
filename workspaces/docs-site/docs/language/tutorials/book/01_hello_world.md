# 1. Hello world

Prerequisite: follow [Install, build, and run](../../../tooling/how-to/install_and_run.md).

<div class="inc-book-progress" aria-label="Chapter 1 of 13">
  <div class="inc-book-progress__meta"><strong>Chapter 1 of 13</strong><span>Hello world</span></div>
  <div class="inc-book-progress__bar" aria-hidden="true"><span style="--inc-progress: 7.7%"></span></div>
</div>

## Create a file

Create `hello.incn`:

```incan
def main() -> None:
    println("Hello, Incan!")
```

!!! tip "Coming from Python?"
    In Python you usually write `print("...")`. In Incan you have both:

    - `println("...")`: prints with a newline (used in most examples)
    - `print("...")`: prints without a newline

Tip: Incan uses indentation for blocks. The canonical style is **4 spaces** per indent level; see the [Incan Code Style Guide](../../reference/code_style.md) and run `incan fmt` to normalize source.

## Run it

```bash
incan run hello.incn
```

## When to make it a project

A single `hello.incn` file is the fastest way to try one language construct. Once you want tests, dependencies, a stable source root, release metadata, or repeatable project commands, create an Incan project:

```bash
incan new hello_project --yes
cd hello_project
```

This creates `incan.toml`, `src/main.incn`, `tests/test_main.incn`, `README.md`, and `.gitignore`. The manifest is the project metadata file; it names the project, records the project version and toolchain requirement, and declares the default entry point under `[project.scripts]`.

```toml title="incan.toml"
[project]
name = "hello_project"
version = "0.1.0"
requires-incan = ">=0.5.0-0,<0.6.0"

[project.scripts]
main = "src/main.incn"
```

Run the project entry point:

```bash
incan run
incan test
```

For the full lifecycle workflow, see [Project lifecycle](../../how-to/project_lifecycle.md).

## Try it

<section class="inc-learning-panel inc-learning-panel--exercise" data-label="Exercise" markdown="1">

1. Change the message you print.
2. Print two lines (two calls to `println`).
3. Use `print("...")` once to see the “no newline” behavior.

</section>

??? example "One possible solution"

    ```incan
    def main() -> None:
        print("Hello")
        println(", Incan!")
        println("Second line")
    ```

<section class="inc-learning-panel inc-learning-panel--complete inc-incus-slot" data-label="Complete" data-incus-category="success" markdown="1">

If your output contains two lines and one call used `print`, the first chapter is complete. You have edited and run an Incan program.

</section>

<nav class="inc-prev-next" aria-label="Book chapter navigation">
  <a href="../"><small>Book overview</small><strong>All 13 chapters</strong></a>
  <a href="../02_values_variables_and_types/"><small>Next chapter</small><strong>2. Values, variables, and types →</strong></a>
</nav>
