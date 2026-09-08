# Read and write TOML data

Use `std.toml` to read a document into a model and produce TOML text from that model. This is suitable for reading configuration or writing generated data. Serializing an existing document discards its comments, key order, and formatting.

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

## Read a document with an open schema

When the complete structure is not known, call `toml.parse(source)` and inspect the resulting `TomlValue`. Retain a successful lookup when traversing a subtree repeatedly, because `get` and `get_index` return owned copies. For an application validation error, `toml.locate(source, path)` can recover the selected value's source span from the original text.

See the [`std.toml` reference](../reference/stdlib/toml.md) for the complete function, value, and error contracts.
