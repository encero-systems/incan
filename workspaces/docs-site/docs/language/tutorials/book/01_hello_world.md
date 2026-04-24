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

Tip: Incan uses indentation for blocks. The canonical style is **4 spaces** per indent level; see the
[Incan Code Style Guide](../../reference/code_style.md) and run `incan fmt` to normalize source.

## Run it

If you installed to PATH:

```bash
incan run hello.incn
```

If you used the no-install fallback:

```bash
./target/release/incan run hello.incn
```

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
