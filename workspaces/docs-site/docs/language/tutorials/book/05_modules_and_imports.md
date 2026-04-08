# 5. Modules and imports

Once your program is more than one file, you’ll split it into modules and import what you need.

## A tiny multi-file project

Create a folder:

```text
my_project/
├── main.incn
└── strings.incn
```

`strings.incn`:

```incan
pub def shout(s: str) -> str:
    return s.strip().upper()
```

`main.incn`:

```incan
from strings import shout

def main() -> None:
    println(shout("  hello  "))
```

Run from `my_project/`:

```bash
incan run main.incn
```

### Visibility: exporting with `pub`

By default, definitions are **module-private** (only usable inside the same `.incn` file).
Prefix a declaration with `pub` to **export** it so other modules can import it.

!!! tip "Coming from Python?"
    In Python, most top-level definitions are effectively importable.
    In Incan, you typically make the intended “public API” of a module explicit with `pub`.

For more detail on `pub` (including how it affects `model`/`class` fields), see: [Models & Classes](../../explanation/models_and_classes/index.md).

## Import styles

Incan supports two styles you can mix:

```incan
# Python-style
from strings import shout

# Rust-style
import strings::shout
```

## Navigating folders (`..`, `super`, `crate`)

The reference documents how parent/root paths work:

- Parent: `..` (Python-style) or `super::` (Rust-style)
- Project root: `crate`

See: [Imports and modules (reference)](../../reference/imports_and_modules.md).

## Module-owned state with `static`

Sometimes a module does not just export functions and types.  
Sometimes it owns long-lived runtime state.

That is what `static` is for:

--8<-- "snippets/module_state.md"

Important:

- `static` is *runtime* state, not a compile-time constant
- imported `pub static` names refer to the same live storage
- `static` always needs an explicit type and initializer

Use `const` for baked compile-time data (constants).

## Try it

1. Add another function in `strings.incn` (for example `whisper`).
2. Mark it `pub` and import it into `main.incn`.
3. Call both functions.

??? example "One possible solution"

    ```incan
    # strings.incn
    pub def shout(s: str) -> str:
        return s.strip().upper()

    pub def whisper(s: str) -> str:
        return s.strip().lower()

    # main.incn
    from strings import shout, whisper

    def main() -> None:
        println(shout("  hello  "))    # outputs: HELLO
        println(whisper("  HELLO  "))  # outputs: hello
    ```

## What to learn next

- Explanation: [Imports and modules](../../explanation/imports_and_modules.md)
- Explanation: [Module static storage](../../explanation/static_storage.md)
- How-to: [Imports and modules (how-to)](../../how-to/imports_and_modules.md)
- How-to: [Module state](../../how-to/module_state.md)
- Reference: [Imports and modules (reference)](../../reference/imports_and_modules.md)
- Reference: [Static storage](../../reference/static_storage.md)

## Next

Back: [4. Control flow](04_control_flow.md)

Next chapter: [6. Errors (Result/Option and `?`)](06_errors.md)
