# Rust Interoperability

Incan compiles to Rust, which means you can import from Rust crates and interoperate with Rust types.

## Importing Rust Crates

Use the `rust::` prefix to import from Rust crates:

```incan
# Import entire crate
import rust::serde_json as json

# Import specific items from Rust's standard library
from rust::std::time import Instant, Duration
from rust::std::collections import HashMap, HashSet

# Import from an external crate
from rust::uuid import Uuid

# Import nested items
import rust::serde_json::Value
```

### The `std` and `rust` namespaces

- `std::...` is reserved for **Incan's** standard library (e.g. `from std.web import App`).
- `rust::...` is for Rust crates from crates.io **and** Rust's standard library.

To use Rust's own standard library, use the `rust::std::` prefix:

```incan
import rust::std::fs
import rust::std::collections::BTreeMap
```

For example, if you would use `import std::fs`, this would refer to Incan's stdlib, **not** Rust's!

> **Note:** `rust::core::...` and `rust::alloc::...` are reserved for future `no_std`/target work and are not yet
> supported. The compiler will tell you to use `rust::std::...` instead.

### Paths and names that match Incan keywords

Rust modules and items sometimes use names that are reserved in Incan (`type`, `async`, and others). In `rust::` paths and in `from rust::... import ...` item lists, those spellings are still accepted. When you import a keyword-named symbol, bind it with `as` so you have a normal identifier in Incan source:

```incan
from rust::my_crate::proto import type as proto_type
```

The same rule applies to path segments after `rust::` (for example `rust::substrait::proto::type::Binary`).

## Dependency Management

When you use `import rust::crate_name`, Incan automatically adds the dependency to your generated `Cargo.toml`.
Dependencies are resolved using a three-tier precedence system:

1. **`incan.toml`** (highest priority): If the crate is configured in your project manifest, that spec is used.
2. **Inline annotations**: If you write `import rust::foo @ "1.0"`, that version is used.
3. **Known-good defaults**: For common crates (see table below), the compiler provides tested defaults.

If none of these apply, the compiler emits an error asking you to specify a version.

For the bigger picture, see: [Projects today](../../tooling/explanation/projects_today.md).

### Specifying versions and features

You can annotate any `rust::` import with a version requirement and optional feature list:

```incan
# Version only
import rust::my_crate @ "1.0"
from rust::obscure_lib @ "0.5" import Widget

# Version with features
import rust::tokio @ "1.0" with ["full"]
import rust::serde @ "1.0" with ["derive", "rc"]
from rust::sqlx @ "0.7" with ["runtime-tokio", "postgres"] import Pool
```

Version strings use [Cargo SemVer syntax](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html):

| Syntax      | Meaning                      | Example            |
| ----------- | ---------------------------- | ------------------ |
| `"1.2.3"`   | Caret (^1.2.3): >=1.2.3 <2.0 | `@ "1.0"`          |
| `"~1.2"`    | Tilde: >=1.2.0 <1.3.0        | `@ "~0.3"`         |
| `">=1, <2"` | Range                        | `@ ">=1.30, <2.0"` |
| `"=1.2.3"`  | Exact version                | `@ "=1.2.3"`       |

**Merging rules** when the same crate is imported in multiple files:

- Version strings must match exactly across all sites (mismatch is an error).
- Features are unioned automatically.

### Project-level dependencies (`incan.toml`)

For projects with multiple dependencies, use an `incan.toml` manifest instead of inline annotations.
This is the recommended approach for anything beyond single-file scripts:

```toml
[project]
name = "my_app"
version = "0.1.0"

[rust-dependencies]
tokio = { version = "1.35", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
my_crate = "2.0"  # assuming this is a rust crate you are referencing
```

When a crate is configured in `incan.toml`, inline version annotations for that crate are **not allowed** —
the manifest is the single source of truth. Bare imports (without `@`) are fine.

For the full manifest format, see:  
[Project configuration reference](../../tooling/reference/project_configuration.md).  
For a practical guide, see: [Managing dependencies](../../tooling/how-to/dependencies.md).

### Known-good defaults

The following crates have pre-configured versions with appropriate features. These defaults apply automatically when you
import a crate without a version annotation and without an `incan.toml` entry:

| Crate      | Version | Features                            |
| ---------- | ------- | ----------------------------------- |
| serde      | 1.0     | derive                              |
| serde_json | 1.0     | -                                   |
| tokio      | 1       | rt-multi-thread, macros, time, sync |
| time       | 0.3     | formatting, macros                  |
| chrono     | 0.4     | serde                               |
| reqwest    | 0.11    | json                                |
| uuid       | 1.0     | v4, serde                           |
| rand       | 0.8     | -                                   |
| regex      | 1.0     | -                                   |
| anyhow     | 1.0     | -                                   |
| thiserror  | 1.0     | -                                   |
| tracing    | 0.1     | -                                   |
| clap       | 4.0     | derive                              |
| log        | 0.4     | -                                   |
| env_logger | 0.10    | -                                   |
| sqlx       | 0.7     | runtime-tokio-native-tls, postgres  |
| futures    | 0.3     | -                                   |
| bytes      | 1.0     | -                                   |
| itertools  | 0.12    | -                                   |

You can override any of these via `incan.toml` or inline `@ "version"` annotations.

### Using unknown crates

If you import a crate not in the known-good list and provide no version, you'll see:

```text
error: unknown Rust crate `my_crate`: no version specified
  --> src/main.incn:5
    import rust::my_crate

hint: Add a version annotation: `import rust::my_crate @ "1.0"` or add it to incan.toml.
```

**Fix**: add an inline version annotation, or add the crate to your `incan.toml`.

## Rust-backed types with `rusttype`

Use `rusttype` when you want an Incan type that is directly backed by a Rust type:

```incan
from rust::std::string import String as RustString

type Name = rusttype RustString:
    def parse(raw: str) -> Result[Name, str]:
        ...

    def as_str(self) -> str:
        ...
```

This keeps Rust provenance explicit (`rust::...` import) while giving you an Incan-facing type name (`Name`) for docs, APIs, and rebinding.

### Qualified backing paths

When a `rust::` import binds a Rust module (or other namespace), you can name a concrete type inside it with `::` after that binding:

```incan
from rust::substrait::proto import type as proto_type

type Binary = rusttype proto_type::Binary
```

The first segment must resolve to a `rust::` import; the compiler builds the full Rust path for lowering and tooling.

Generics on qualified type paths (for example `binding::Wrapper[int]`) are not supported yet — import or spell a path to the concrete type without type arguments in that position.

### Declaring conversion edges with `interop:`

Inside a `rusttype` body, `interop:` declares explicit adapters across the Incan/Rust boundary:

```incan
from rust::std::string import String as RustString

type Name = rusttype RustString:
    def parse(raw: str) -> Result[Name, str]:
        ...

    def as_str(self) -> str:
        ...

    interop:
        from str try Name.parse
        into str via Name.as_str
```

Use:

- `from S via adapter` for infallible inbound adaptation (`S -> ThisType`)
- `from S try adapter` for fallible inbound adaptation (`S -> Result[ThisType, E]` or `S -> Option[ThisType]`)
- `into T via adapter` for outbound adaptation (`ThisType -> T`)

## Capability bounds with `std.rust`

Import Rust capability markers from `std.rust` and use them in generic `with` clauses:

```incan
from std.rust import Send, Sync

def run[T with Send, Sync](_value: T) -> None:
    pass
```

These are Incan-syntax bounds that lower to Rust-native predicates in generated code.

## Coercions at explicit Rust boundaries

When calling Rust functions or methods, the compiler can apply a bounded, compiler-managed coercion model for built-in types:

| Incan built-in | Canonical Rust lowering | Admitted Rust boundary targets |
| -------------- | ----------------------- | ------------------------------ |
| `int`          | `i64`                   | `i64`                          |
| `float`        | `f64`                   | `f64`, `f32`                   |
| `bool`         | `bool`                  | `bool`                         |
| `str`          | `String`                | `String`, `&str`               |
| `bytes`        | `Vec<u8>`               | `Vec<u8>`, `&[u8]`             |
| `None` / unit  | `()`                    | `()`                           |

Example (`float -> f32` boundary adaptation):

```incan
from rust::std::time import Duration

def main() -> None:
    # `from_secs_f32` expects f32; Incan `float` can be adapted at this Rust boundary.
    d = Duration.from_secs_f32(1.5)
    println(d.as_secs_f32())
```

## Examples

### Working with JSON (serde_json)

```incan
import rust::serde_json as json
from rust::serde_json import Value

def parse_json(data: str) -> Value:
    return json.from_str(data).unwrap()

def main() -> None:
    data = '{"name": "Alice", "age": 30}'
    parsed = parse_json(data)
    println(f"Name: {parsed['name']}")
```

### Working with Time

```incan
from rust::std::time import Instant, Duration

def measure_operation() -> None:
    start = Instant.now()

    # Do some work
    for i in range(1000000):
        pass

    elapsed = start.elapsed()
    println(f"Operation took: {elapsed}")
```

### HTTP Requests (reqwest)

```incan
import std.async
import rust::reqwest

async def fetch_data(url: str) -> str:
    response = await reqwest.get(url)
    return await response.text()

async def main() -> None:
    data = await fetch_data("https://api.example.com/data")
    println(data)
```

### Using Collections

```incan
from rust::std::collections import HashMap, HashSet

def count_words(text: str) -> HashMap[str, int]:
    counts = HashMap.new()
    for word in text.split():
        count = counts.get(word).unwrap_or(0)
        counts.insert(word, count + 1)
    return counts
```

### Random Numbers

```incan
from rust::rand import Rng, thread_rng

def random_int(min: int, max: int) -> int:
    rng = thread_rng()
    return rng.gen_range(min..max)

def main() -> None:
    for _ in range(5):
        println(f"Random: {random_int(1, 100)}")
```

### UUIDs

```incan
from rust::uuid import Uuid

def generate_id() -> str:
    return Uuid.new_v4().to_string()

def main() -> None:
    id = generate_id()
    println(f"Generated ID: {id}")
```

## Type Mapping

Incan types map to canonical Rust types:

| Incan          | Rust            |
| -------------- | --------------- |
| `int`          | `i64`           |
| `float`        | `f64`           |
| `str`          | `String`        |
| `bytes`        | `Vec<u8>`       |
| `bool`         | `bool`          |
| `List[T]`      | `Vec<T>`        |
| `Dict[K, V]`   | `HashMap<K, V>` |
| `Set[T]`       | `HashSet<T>`    |
| `Option[T]`    | `Option<T>`     |
| `Result[T, E]` | `Result<T, E>`  |
| `None` / unit  | `()`            |

### String arguments and borrowing

!!! tip "Coming from Rust?"
    You never write `&str` or lifetimes in Incan. When you pass a `str` value to an external Rust function, the
    compiler automatically passes it as a borrowed `&str` — the most common pattern in Rust APIs.

    If a Rust function requires an owned `String` instead, append `.to_string()` at the call site:

    ```incan
    from rust::std::fs import write

    # Incan passes `path` and `content` as &str automatically
    write(path, content)

    # Force an owned String if the API requires it
    some_fn(path.to_string())
    ```

    This keeps interop ergonomic without exposing Rust borrow syntax in Incan code.

## Understanding Rust types (optional)

??? tip "Coming from Python?"
    If you're new to Rust types like `Vec`, `HashMap`, `String`, `Option`, and `Result`, see
    [Understanding Rust types (coming from Python)](rust_types_for_python_devs.md).

### Matching on Rust-backed enums and oneofs

When you wrap an imported Rust enum (or a prost-style `oneof`) in a `rusttype`, `match` uses the same qualified constructor patterns as for Incan enums (`Type.Variant(...)` in source; the compiler normalizes this to `Type::Variant` internally). Names you bind in those patterns are in scope in the arm body, so you can nest `Option` / `Result` matches the same way you would for native types.

```incan
type PlanRel = rusttype rust::my_crate::proto::PlanRel:
    def noop(self) -> None:
        ...

def inspect(x: Option[PlanRel]) -> None:
    match x:
        Some(inner) =>
            match inner:
                PlanRel.Root(root) =>
                    # `root` is a normal binding here (payload typed as a composed Rust path)
                    println("matched Root")
                _ =>
                    pass
        None =>
            pass
```

For a single tuple-field variant, the typechecker models the payload as a `RustPath` of the form `{backing_rust_path}::{Variant}` (aligned with how member paths are composed for imported ADTs). Multiple positional sub-patterns are accepted but typed permissively until richer Rust metadata is available.

If the scrutinee is typed as a bare imported Rust path (not a `rusttype` alias), the type prefix in the pattern is not cross-checked against that path; prefer spelling the `rusttype` wrapper and matching on `PlanRel.Variant(...)` so the prefix matches your Incan type name.

## Limitations

1. **Lifetime annotations**: Rust's borrow checker and lifetime annotations are not exposed in Incan.
    Types that require explicit lifetime management may not work directly.

2. **Generic bounds**: Capability bounds from `std.rust` (`Send`, `Sync`, `Static`, `Fn`, `FnMut`, `FnOnce`) are supported via `with` clauses. Custom trait bounds and more complex generic patterns may need wrapper functions.

3. **Unsafe code**: Incan cannot call unsafe Rust functions directly.
    If you need unsafe operations, create a safe wrapper in Rust first.

4. **Macros**: Rust macros are not directly callable. Use the expanded form or a wrapper function.

5. **`match` exhaustiveness on `rusttype` enums**: Compiler exhaustiveness for `match` is driven by Incan `enum` definitions plus `Option` / `Result`. A `rusttype` wrapping a Rust enum does not supply the same variant list, so you will not get “non-exhaustive match” diagnostics for missing Rust variants. Use a catch-all arm (`_`) when you must accept variants you do not list explicitly.

## Best Practices

1. **Use `incan fmt` to fix import style**: the formatter always normalizes `rust::` imports to `::` notation.
    If you (or a collaborator) wrote `from rust.serde_json import Value`, running `incan fmt` silently rewrites it
    to `from rust::serde_json import Value`.

2. **Prefer Incan types**: Use Incan's built-in types when possible. Use Rust types only when you need
    specific functionality.

3. **Handle Results**: Rust crate functions often return `Result`. Use `?` or explicit matching:

    ```incan
    def safe_parse(s: str) -> Result[int, str]:
        return s.parse()  # Returns Result

    def main() -> None:
        match safe_parse("42"):
            case Ok(n):
                println(f"Parsed: {n}")
            case Err(e):
                println(f"Error: {e}")
    ```

4. **Async compatibility**: If using async Rust crates, make sure your Incan functions are also async.

5. **Error types**: Rust's error types can be complex. Consider using `anyhow` for simple error handling:

    ```incan
    from rust::anyhow import Result, Context

    def read_config(path: str) -> Result[Config]:
        content = fs.read_to_string(path).context("Failed to read config")?
        return parse_config(content)
    ```

## See Also

- [Managing dependencies](../../tooling/how-to/dependencies.md) - Adding crates, locking, CI
- [Project configuration reference](../../tooling/reference/project_configuration.md) - Full `incan.toml` format
- [RFC 041] - `rusttype`, `interop`, capability bounds
- [Error Handling](../explanation/error_handling.md) - Working with `Result` types
- [Derives & Traits](../reference/derives_and_traits.md) - Drop trait for custom cleanup
- [File I/O](file_io.md) - Reading, writing, and path handling
- [Async Programming](async_programming.md) - Async/await with Tokio
- [Imports & Modules](imports_and_modules.md) - Module system, imports, and built-in functions
- [Web Framework](../tutorials/web_framework.md) - Building web apps with Axum

--8<-- "_snippets/rfcs_refs.md"
