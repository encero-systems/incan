# 1. Hello world

Prerequisite: follow [Install, build, and run](../../../tooling/how-to/install_and_run.md).

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

If you installed to PATH:

```bash
incan run hello.incn
```

If you used the no-install fallback:

```bash
./target/release/incan run hello.incn
```

## When to make it a project

A single `hello.incn` file is the fastest way to try the language. Once you want tests, dependencies, a stable source root, release metadata, or repeatable project commands, create an Incan project. This is easy to do using the incan cli:

```bash
mkdir hello_project
cd hello_project
incan init --name hello_project --yes
```

This creates `incan.toml`, `src/main.incn`, `tests/test_main.incn`, `README.md`, and `.gitignore`. The manifest is the project metadata file; it names the project, records the project version, and declares the default entry point under `[project.scripts]`.

```toml title="incan.toml"
[project]
name = "hello_project"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"
```

Run the project entry point:

```bash
incan run
```

For the full lifecycle workflow, see [Project lifecycle](../../how-to/project_lifecycle.md).

## Try it

1. Change the message you print.
2. Print two lines (two calls to `println`).
3. Use `print("...")` once to see the “no newline” behavior.

??? example "One possible solution"

    ```incan
    def main() -> None:
        print("Hello")
        println(", Incan!")
        println("Second line")
    ```

## Next

Next chapter: [2. Values, variables, and types](02_values_variables_and_types.md).
