# Read and write TOML data

Use `std.toml` to read selected configuration fields or decode a complete model and produce TOML text. This is suitable for reading configuration or writing generated data. Serializing an existing document discards its comments, key order, and formatting.

## Read required and optional fields

Parse the source once, then state the type expected at each dotted path. Use `?` to propagate malformed input, missing required values, and type errors to the caller:

```incan
"""Read project settings without declaring the complete manifest schema."""

from std import toml
from std.toml import TomlError


def describe_project(source: str) -> Result[str, TomlError]:
    """Read required project fields and supply a default only for an absent version."""
    document = toml.parse(source)?
    name = document.get[str]("project.name")?
    targets = document.get[list[str]]("project.targets")?
    version = document.get_optional[str]("project.version")?.unwrap_or("unversioned")
    return Ok(f"{name} ({version}): {targets}")


def main() -> None:
    """Print project settings or a path-bearing diagnostic."""
    source = "[project]\nname = \"demo\"\ntargets = [\"linux\", \"macos\"]\n"
    match describe_project(source):
        Ok(description) => println(description)
        Err(error) => println(error.message())
```

If `project.version` is absent, the example uses `unversioned`. If it is present as an integer, the lookup returns an error; the default does not hide the invalid value. Likewise, an integer in `targets` reports the offending element's path, such as `project.targets[2]`.

For a key literally named `project.name`, use `document.get_key[str]("project.name")?`. Dotted lookup always traverses tables. To inspect an unknown shape, select a dynamic subtree with `document.get_value("project")?`, then use its `kind`, `as_table`, or `as_array` methods.

## Decode a model and serialize the result

Import the module and add `@derive(toml)` to each model in the document. Pass the original TOML text to `deserialize` so invalid fields can retain their source locations.

The following program decodes a project description and prints its serialized form:

```incan
"""Read a typed TOML document and serialize its project fields."""

from std import toml
from std.toml import TomlError


@derive(toml)
model Project:
    """Project identity read from the document."""

    name: str
    version: str


def rewrite_project(source: str) -> Result[str, TomlError]:
    """Validate project fields and produce TOML text."""
    project = toml.deserialize[Project](source)?
    return toml.serialize_pretty(project)


def main() -> None:
    """Print the serialized project or its input error."""
    source = "name = \"demo\"\nversion = \"0.1.0\"\n"
    match rewrite_project(source):
        Ok(text) => println(text)
        Err(error) => println(error.message())
```

For a file, read its text first using the [file I/O APIs](file_io.md), then pass that text to `rewrite_project`. Write the returned text only when replacing authored formatting is acceptable.

## Handle invalid input

Changing `version = "0.1.0"` to `version = 1` produces a decode error because the model requires a string. Missing required fields and malformed TOML also return `Err(TomlError)`. The `?` in `rewrite_project` propagates that error to the caller.

For a diagnostic with coordinates, inspect `error.line` and `error.column` before displaying them: both are optional. `error.message()` supplies the diagnostic text even when there is no source position.

## Report an application validation error

Keep the original `TomlValue` when you need to associate a policy failure with an input field. `document.get_value("project.name")?.location()` returns the selected value's original span without reparsing. Both `line` and `column` are one-based; columns count Unicode characters. A value without an authored span returns `None`.

See the [`std.toml` reference](../reference/stdlib/toml.md) for the complete function, value, and error contracts.
