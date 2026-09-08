# Read and write TOML data

Use `std.toml` to read selected configuration fields or decode a complete model and produce TOML text. This is suitable for reading configuration or writing generated data. Serializing an existing document discards its comments, key order, and formatting.

## Read required and optional fields

Parse the source once, then state the type expected at each dotted path. Use `?` to propagate malformed input, missing required values, and type errors to the caller:

Click or focus the **+** markers for a closer look at each step.

```incan
"""Read project settings without declaring the complete manifest schema."""

from std import toml
from std.toml import TomlError


def describe_project(source: str) -> Result[str, TomlError]:
    """Read required project fields and supply a default only for an absent version."""
    document = toml.parse(source)?  # (1)!
    name: str = document.get("project.name")?  # (2)!
    targets = document.get[list[str]]("project.targets")?  # (3)!
    version = document.get_optional[str]("project.version")?.unwrap_or("unversioned")  # (4)!
    return Ok(f"{name} ({version}): {targets}")  # (5)!


def main() -> None:
    """Print project settings or a path-bearing diagnostic."""
    source = "[project]\nname = \"demo\"\ntargets = [\"linux\", \"macos\"]\n"
    match describe_project(source):  # (6)!
        Ok(description) => println(description)
        Err(error) => println(error.message())
```

1. `parse` reads the TOML text once and retains source locations for later diagnostics. The `?` extracts a successful document or returns its `TomlError` from `describe_project` immediately.
2. `name: str` supplies the type expected from `get`. This is equivalent to `name = document.get[str]("project.name")?`. The dotted path selects the `name` key inside the `project` table; a missing key or a value of the wrong type returns an error.
3. `get[list[str]]` requests a list whose members must all be strings. A wrong member reports its own path, such as `project.targets[2]`; the lookup does not silently convert it.
4. `get_optional[str]` returns `Result[Option[str], TomlError]`. First, `?` propagates an invalid present value. Then `unwrap_or` supplies the default only when the remaining `Option` is `None` because the key is absent.
5. The function promises `Result[str, TomlError]`, so wrap the completed description in `Ok`. The earlier `?` expressions already handle its error paths.
6. Handle the final result at the program boundary: print the description on success, or the diagnostic on failure. The field-reading code can use `?` without a separate `match` for every lookup.

If `project.version` is absent, the example uses `unversioned`. If it is present as an integer, the lookup returns an error; the default does not hide the invalid value. Likewise, an integer in `targets` reports the offending element's path, such as `project.targets[2]`.

For a key literally named `project.name`, use `document.get_key[str]("project.name")?`. Dotted lookup always traverses tables. To inspect an unknown shape, select a dynamic subtree with `document.get_value("project")?`, then use its `kind`, `as_table`, or `as_array` methods.

## Decode a model and serialize the result

Import the module and add `@derive(toml)` to each model in the document. Pass the original TOML text to `deserialize` so invalid fields can retain their source locations.

The following program decodes a project description and prints its serialized form:

```incan
"""Read a typed TOML document and serialize its project fields."""

from std import toml
from std.toml import TomlError


@derive(toml)  # (1)!
model Project:
    """Project identity read from the document."""

    name: str
    version: str


def rewrite_project(source: str) -> Result[str, TomlError]:
    """Validate project fields and produce TOML text."""
    project = toml.deserialize[Project](source)?  # (2)!
    return toml.serialize_pretty(project)  # (3)!


def main() -> None:
    """Print the serialized project or its input error."""
    source = "name = \"demo\"\nversion = \"0.1.0\"\n"
    match rewrite_project(source):
        Ok(text) => println(text)
        Err(error) => println(error.message())
```

1. `@derive(toml)` gives `Project` the TOML encoding and decoding support its fields require. Apply it to nested models too; no separate Rust derive is needed.
2. `deserialize[Project]` decodes the complete document into the declared model. Missing required fields, invalid field types, and malformed TOML return an error through `?`.
3. `serialize_pretty` returns `Result[str, TomlError]`, which already matches the function's return type. Return it directly. The output is newly formatted TOML; it does not preserve the input's comments, key order, or formatting.

For a file, read its text first using the [file I/O APIs](file_io.md), then pass that text to `rewrite_project`. Write the returned text only when replacing authored formatting is acceptable.

## Handle invalid input

Changing `version = "0.1.0"` to `version = 1` produces a decode error because the model requires a string. Missing required fields and malformed TOML also return `Err(TomlError)`. The `?` in `rewrite_project` propagates that error to the caller.

For a diagnostic with coordinates, inspect `error.line` and `error.column` before displaying them: both are optional. `error.message()` supplies the diagnostic text even when there is no source position.

## Report an application validation error

Keep the original `TomlValue` when you need to associate a policy failure with an input field. `document.get_value("project.name")?.location()` returns the selected value's original span without reparsing. Both `line` and `column` are one-based; columns count Unicode characters. A value without an authored span returns `None`.

See the [`std.toml` reference](../reference/stdlib/toml.md) for the complete function, value, and error contracts.
