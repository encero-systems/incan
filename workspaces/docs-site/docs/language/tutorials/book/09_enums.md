# 9. Enums and better `match`

<div class="inc-book-progress" aria-label="Chapter 9 of 13"><div class="inc-book-progress__meta"><strong>Chapter 9 of 13</strong><span>Enums and better match</span></div><div class="inc-book-progress__bar" aria-hidden="true"><span style="--inc-progress: 69.2%"></span></div></div>

Enums let you represent “one of several shapes” as a real type, and `match` lets you handle it exhaustively.

## A small command enum

```incan
enum Command:
    Add(int)
    Remove(int)
    List
```

!!! tip "Coming from Python?"
    Python’s `Enum` is mostly “named constants”. If you want variants that carry different data, Python often uses: strings, `dataclass` hierarchies, or `Union` types.

    In Incan, `enum` variants can carry data (like `Add(int)`), and `match` gives you an exhaustive, compiler-checked way to handle every case.

## Matching on enums

```incan
def run(cmd: Command) -> None:
    match cmd:
        Add(x) => println(f"add {x}")
        Remove(x) => println(f"remove {x}")
        List => println("list")

def main() -> None:
    run(Command.Add(1))     # outputs: add 1
    run(Command.Remove(1))  # outputs: remove 1
    run(Command.List)       # outputs: list
```

## Put enum-owned behavior on the enum

If a helper describes the enum itself, put it inside the enum body after the variants:

```incan
enum Command:
    Add(int)
    Remove(int)
    List

    def label(self) -> str:
        match self:
            Command.Add(_) => return "add"
            Command.Remove(_) => return "remove"
            Command.List => return "list"

    def default() -> Self:
        return Command.List
```

Call instance methods on values and associated functions on the enum type:

```incan
def main() -> None:
    cmd = Command.default()
    println(cmd.label())  # outputs: list
```

## Why this matters

Unlike a stringly-typed “command name”, enums:

- prevent typos
- make invalid states unrepresentable
- give you exhaustive `match` checking

## Try it

1. Add a `Clear` variant and update the `match` accordingly.
2. Add a `Rename(str)` variant and print the new name.
3. Add a `label(self) -> str` method so callers do not need a separate helper.

??? example "One possible solution"

    ```incan
    enum Command:
        Add(int)
        Remove(int)
        Rename(str)
        Clear
        List

        def label(self) -> str:
            match self:
                Command.Add(_) => return "add"
                Command.Remove(_) => return "remove"
                Command.Rename(_) => return "rename"
                Command.Clear => return "clear"
                Command.List => return "list"

    def run(cmd: Command) -> None:
        match cmd:
            Command.Add(x) => println(f"add {x}")
            Command.Remove(x) => println(f"remove {x}")
            Command.Rename(name) => println(f"rename {name}")
            Command.Clear => println("clear")
            Command.List => println("list")
    ```

## Where to learn more

- Enums deep dive: [Enums](../../explanation/enums.md)

<nav class="inc-prev-next" aria-label="Book chapter navigation"><a href="../08_collections_and_iteration/"><small>Previous chapter</small><strong>← 8. Collections</strong></a><a href="../10_models_vs_classes/"><small>Next chapter</small><strong>10. Models vs classes →</strong></a></nav>
