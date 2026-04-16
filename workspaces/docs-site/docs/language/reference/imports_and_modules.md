# Imports and modules (reference)

This page is the reference for import syntax, path rules, and prelude contents.

If you want the conceptual overview, see: [Imports and modules](imports_and_modules.md).

## Import syntax

Incan supports two styles that can be mixed freely.

### Python-style: `from module import ...`

```incan
# Import multiple items at once
from models import User, Product, Order

# Import with aliases
from utils import format_currency as fmt, validate_email as check_email
```

### Parenthesized import lists (single-line or multi-line)

Use parentheses when the list is long or for readability. This works for both regular modules and `rust::` imports.

```incan
from some_lib import (
  module_a as A,
  module_b,
  module_c as C,
  module_d,
)

from rust::polars import (
  A,
  B as b,
  pandas as pd,
  foo,
)
```

Trailing commas are allowed in parenthesized lists.

### Rust-style: `import module::item`

```incan
# Import a specific item
import models::User

# Import with an alias
import utils::format_currency as fmt
```

## Import path rules

### Child directory imports

You can use dots (Python-style) or `::` (Rust-style):

```incan
# Python-style: dots for nested paths
from db.models import User, Product

# Rust-style: :: for nested paths
import db::models::User
```

### Parent directory imports

Navigate to parent directories using `..` (Python-style) or `super` (Rust-style):

```incan
# Python-style: .. for parent
from ..common import Logger
from ...shared.utils import format_date

# Rust-style: super keyword
import super::common::Logger
import super::super::shared::utils::format_date
```

| Prefix                    | Meaning                               |
| ------------------------- | ------------------------------------- |
| `..` or `super::`         | Parent directory (one level up)       |
| `...` or `super::super::` | Grandparent directory (two levels up) |

### Absolute imports (project root)

Import from the project root using `crate`:

```incan
from crate.config import Settings
import crate::lib::database::Connection
```

The compiler finds the project root by looking for `Cargo.toml` or a `src/` directory.

### Path summary

| Incan path     | Meaning                | Rust equivalent |
| -------------- | ---------------------- | --------------- |
| `models`       | Same directory         | `models`        |
| `db.models`    | Child `db/models.incn` | `db::models`    |
| `..common`     | Parent’s `common.incn` | `super::common` |
| `super::utils` | Parent’s `utils.incn`  | `super::utils`  |
| `crate.config` | Root’s `config.incn`   | `crate::config` |

## The prelude

The prelude is a set of types and traits automatically available in every Incan file without explicit imports.

### Types always available

| Incan type     | Rust type       | Description                |
| -------------- | --------------- | -------------------------- |
| `int`          | `i64`           | 64-bit signed integer      |
| `float`        | `f64`           | 64-bit floating point      |
| `bool`         | `bool`          | Boolean                    |
| `str`          | `String`        | UTF-8 string               |
| `bytes`        | `Vec<u8>`       | Byte array                 |
| `List[T]`      | `Vec<T>`        | Dynamic array              |
| `Dict[K, V]`   | `HashMap<K, V>` | Hash map                   |
| `Set[T]`       | `HashSet<T>`    | Hash set                   |
| `Option[T]`    | `Option<T>`     | Optional value (Some/None) |
| `Result[T, E]` | `Result<T, E>`  | Success or error (Ok/Err)  |

### Type aliases and naming conventions

Many types have a canonical (generated-reference) name and a lowercase alias used in examples:

- Canonical: `List[T]`, `Dict[K, V]`, `Set[T]`
- Aliases: `list[T]`, `dict[K, V]`, `set[T]`
- Rust interop alias: `Vec[T]` (accepted as `List[T]`)

The generated language reference shows the canonical name and aliases in one place:
[Language reference (generated)](language.md).

### Built-in functions (always available)

```incan
# Output
println(value)      # Print with newline
print(value)        # Print without newline

# Collections
len(collection)     # Get length

# Iteration
range(n)            # Iterator 0..n
range(start, end)   # Iterator start..end
range(start, end, step)  # Iterator start..end with a custom step (Python-like)
range(start..end)   # Iterator start..end (Rust-style range literal)
range(start..=end)  # Iterator start..=end (inclusive end)
enumerate(iter)     # Iterator with indices
zip(iter1, iter2)   # Pair up two iterators

# Type conversion (Python-like)
dict()              # Empty Dict
dict(mapping)       # Convert to Dict
list()              # Empty List
list(iterable)      # Convert to List
set()               # Empty Set
set(iterable)       # Convert to Set
```

## Special import: `import this`

`import this` is always available and prints the Incan “Zen” design principles when imported:

```bash
incan run -c "import this"
```

## Incan standard library (`std.*`)

<!-- TODO: move this to its own section -->

Incan's standard library lives under the `std` namespace. Import modules and items from it just like any other module.
The compiler activates features (e.g. async runtime, web framework) automatically based on which `std.*` modules you
import — no manual feature flags needed.

### Available modules

| Module           | Description                                   | Activates feature |
| ---------------- | --------------------------------------------- | ----------------- |
| `std.web`        | Web framework (routes, responses, extractors) | `web` (Axum)      |
| `std.testing`    | Test fixtures and assertions                  | —                 |
| `std.async`      | Async utilities (activates `async`/`await`)   | —                 |
| `std.serde.json` | JSON serialization/deserialization            | `json`            |
| `std.reflection` | Reflection helpers (`FieldInfo`, etc.)        | —                 |
| `std.derives.*`  | Derive helpers (`string`, `comparison`, ...)  | —                 |
| `std.traits.*`   | Core traits (`ops`, `convert`, `error`, ...)  | —                 |
| `std.math`       | Math constants and functions                  | —                 |

### Soft keywords

Some language keywords are **import-activated** (soft keywords). They behave like identifiers by default and only become
reserved keywords after importing a particular `std.*` namespace.

Currently:

- `async` and `await` are activated by importing `std.async` (for example `import std.async` or
  `from std.async.time import sleep`).

If you forget the import, you’ll get a targeted diagnostic telling you what to add.

Example:

```incan
async def work() -> None:
    await sleep(1.0)
```

```text
error: `async` is only available after importing `std.async`

hint: Add `import std.async` or `from std.async import ...`
```

### Import examples

```incan
# Import items from the web framework
from std.web import App, route, Response, Json, GET, POST

# Import test fixtures
from std.testing import fixture

# Import async time helpers (also activates `async`/`await`)
from std.async.time import sleep

# Import JSON helpers
from std.serde.json import json_stringify, json_parse

async def do_work() -> None:
    await sleep(0.5)
```

### Reserved root namespaces

The names `std` and `rust` are reserved at the root level. You cannot shadow them with local modules or aliases:

```incan
# ERROR: 'std' is a reserved root namespace
import models as std
```

## Stdlib module: `std.math`

See the stdlib reference page: [Standard library reference: `std.math`](stdlib/math.md).

You must import `std.math` before use:

```incan
import std.math

def main() -> None:
    println(f"pi={math.PI}")
```

### Available constants

| Constant        | Description                 |
| --------------- | --------------------------- |
| `math.PI`       | π (3.14159...)              |
| `math.E`        | Euler’s number (2.71828...) |
| `math.TAU`      | τ = 2π (6.28318...)         |
| `math.INFINITY` | Positive infinity           |
| `math.NAN`      | Not a Number                |

### Available functions

| Function                                       | Description                   |
| ---------------------------------------------- | ----------------------------- |
| `math.sqrt(x)`                                 | Square root                   |
| `math.abs(x)`                                  | Absolute value                |
| `math.floor(x)`                                | Largest integer ≤ x           |
| `math.ceil(x)`                                 | Smallest integer ≥ x          |
| `math.round(x)`                                | Round to nearest integer      |
| `math.pow(x, y)`                               | x raised to power y           |
| `math.exp(x)`                                  | e^x                           |
| `math.log(x)`                                  | Natural logarithm (ln)        |
| `math.log10(x)`                                | Base-10 logarithm             |
| `math.log2(x)`                                 | Base-2 logarithm              |
| `math.sin(x)`, `math.cos(x)`, `math.tan(x)`    | Trig (radians)                |
| `math.asin(x)`, `math.acos(x)`, `math.atan(x)` | Inverse trig                  |
| `math.sinh(x)`, `math.cosh(x)`, `math.tanh(x)` | Hyperbolic                    |
| `math.atan2(y, x)`                             | Two-argument arctangent       |
| `math.hypot(x, y)`                             | Euclidean distance √(x² + y²) |

## Stdlib module: `std.async`

See generated/curated stdlib signatures: [Standard library reference: `std.async`](stdlib/async.md).

`std.async` includes runtime support for asynchronous programming and activates the `async`/`await` soft keywords when imported.

You can import time helpers directly:

```incan
from std.async.time import sleep, timeout
```

Or import a complete surface from the prelude:

```incan
from std.async.prelude import *
```

### Time helpers

| Function                | Description                      |
| ----------------------- | -------------------------------- |
| `sleep`, `sleep_ms`     | Delay the current task           |
| `timeout`, `timeout_ms` | Bound async work with a deadline |

### Concurrency helpers

| API                                       | Description                  |
| ----------------------------------------- | ---------------------------- |
| `spawn`, `spawn_blocking`                 | Start async or blocking work |
| `channel`, `unbounded_channel`, `oneshot` | Message passing primitives   |
| `select_timeout`                          | Timeout-based select utility |
| `yield_now`                               | Yield to scheduler           |

## Rust standard library access

To import from Rust’s standard library, use the `rust::` prefix:

```incan
import rust::std::fs
import rust::std::env
import rust::std::path::Path
import rust::std::time
```

!!! warning "The `std` root is reserved"
    Bare `import std::fs` refers to **Incan’s** standard library, not Rust’s.
    Always use the `rust::std::` prefix when you need Rust’s stdlib.

Note: using these requires understanding the underlying Rust types. Prefer Incan built-ins (`read_file`, `write_file`,
etc.) where available.

## Rust crates vs Incan modules (important)

- **External crates**: Prefer `rust::...` imports (e.g. `import rust::serde_json`), which also enables automatic
  dependency management for generated `Cargo.toml`.
- **Incan project modules** (multi-file projects): imports like `from db.schema import Database` refer to modules in
  the current crate and are emitted as `crate::db::schema::Database` in generated Rust so they compile reliably from
  submodules.

### Version and feature annotations (Rust crates only)

Rust crate imports support optional version and feature annotations using `@` and `with`:

```text
import rust::CRATE [@ "VERSION"] [with ["FEATURE", ...]]
from rust::CRATE [@ "VERSION"] [with ["FEATURE", ...]] import ITEMS
```

Examples:

```incan
# Version only
import rust::my_crate @ "1.0"

# Version with features
import rust::tokio @ "1.0" with ["full"]
from rust::sqlx @ "0.7" with ["runtime-tokio", "postgres"] import Pool
```

Rules:

- `@ "VERSION"` uses [Cargo SemVer syntax](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html).
- `with [...]` requires `@` (you cannot specify features without a version).
- If the crate is configured in `incan.toml`, inline annotations are **not allowed**.
- When the same crate is imported in multiple files, versions must match and features are unioned.

These annotations only apply to `rust::` imports. Incan module imports (`from models import User`) do not support version
or feature annotations.

See: [Rust interop](../how-to/rust_interop.md) for practical guidance and examples.

## Current status and limitations

Supported:

- Python-style imports: `from module import item1, item2`
- Rust-style imports: `import module::item`
- Nested paths
- Parent navigation (`..` / `super`)
- Root imports (`crate`)
- Aliases (`as`)
- Public re-exports in source modules: `pub from module import Item` (allowed in files under `src/`)

Limitations (current):

1. No wildcard imports (`from module import *`)
2. No circular imports
