# Checked API Metadata

Checked API metadata is the compiler-produced JSON description of a package or module's public Incan API. It is intended for documentation generators, package browsers, editor tooling, and other consumers that need checked declarations without scraping source text or generated Rust.

Invoke the metadata command from a project root, project directory, or source file:

```bash
incan tools metadata api [PATH] --format json
```

When `PATH` is a directory, `src/lib.incn` is the preferred entry point and `src/main.incn` is the fallback. The command type-checks the target before writing JSON. Type errors are reported as compiler diagnostics and no metadata package is printed.

## Example

For a project with this `src/lib.incn`:

```incan
pub const DEFAULT_LABEL = "catalog"

@rust.allow("dead_code")
pub def label() -> str:
    """Return the catalog label."""
    return DEFAULT_LABEL
```

Run:

```bash
incan tools metadata api . --format json
```

Output:

```json
{
  "schema_version": 1,
  "modules": [
    {
      "schema_version": 1,
      "module_path": [
        "lib"
      ],
      "declarations": [
        {
          "kind": "const",
          "name": "DEFAULT_LABEL",
          "anchor": {
            "id": "lib::DEFAULT_LABEL",
            "span": {
              "start": 0,
              "end": 35
            }
          },
          "ty": {
            "Named": {
              "name": "FrozenStr"
            }
          },
          "value": {
            "kind": "string",
            "value": "catalog"
          }
        },
        {
          "kind": "function",
          "name": "label",
          "anchor": {
            "id": "lib::label",
            "span": {
              "start": 37,
              "end": 147
            }
          },
          "docstring": "Return the catalog label.",
          "decorators": [
            {
              "path": [
                "rust",
                "allow"
              ],
              "source_name": "rust.allow",
              "anchor": {
                "start": 37,
                "end": 61
              },
              "args": [
                {
                  "kind": "positional",
                  "value": {
                    "kind": "literal",
                    "value": {
                      "kind": "string",
                      "value": "dead_code"
                    }
                  }
                }
              ]
            }
          ],
          "type_params": [],
          "params": [],
          "return_type": {
            "Named": {
              "name": "str"
            }
          },
          "is_async": false
        }
      ]
    }
  ]
}
```

## Package Shape

The top-level JSON object is a metadata package:

| Field            | Type   | Meaning                                                    |
| ---------------- | ------ | ---------------------------------------------------------- |
| `schema_version` | number | Metadata package schema version                            |
| `modules`        | array  | Checked metadata documents for the entry and local imports |

Each module document contains:

| Field            | Type   | Meaning                                           |
| ---------------- | ------ | ------------------------------------------------- |
| `schema_version` | number | Module metadata schema version                    |
| `module_path`    | array  | Logical module path segments                      |
| `declarations`   | array  | Public declarations visible from that source file |

`declarations` uses a `kind` discriminator. Current declaration kinds are `function`, `model`, `class`, `trait`, `enum`, `newtype`, `type_alias`, `const`, `static`, and `alias`.

## Declaration Facts

The metadata is derived from parsed and typechecked semantics. Public declarations can include:

- stable source anchors: `anchor.id`, `anchor.span.start`, and `anchor.span.end`
- checked signatures, parameters, type parameters, bounds, receiver kind, and return type
- model and class fields, including model field `alias`, `description`, and `has_default`
- trait requirements and checked method signatures
- enum variants and value-enum raw values
- public import aliases with resolved `target_path` segments
- raw docstring text when the declaration or method has a docstring
- decorator metadata with resolved decorator paths
- safe const values for public consts and safe decorator arguments

Types use the same structural `TypeRef` encoding as library manifest exports. For example, a non-generic type is encoded as `{"Named": {"name": "str"}}`, while a generic application is encoded as `{"Applied": {"name": "List", "args": [...]}}`.

## Safe Values

Metadata only carries values that the compiler can expose without executing user code:

| Kind     | Meaning                               |
| -------- | ------------------------------------- |
| `int`    | Integer literal or checked const      |
| `float`  | Floating-point literal or const       |
| `bool`   | Boolean literal or const              |
| `string` | String literal or frozen string const |
| `bytes`  | Bytes literal or frozen bytes const   |
| `none`   | Literal `None`                        |

Decorator arguments that are not literals, type arguments, or const references are reported as `unsupported` metadata values instead of being evaluated.

## Docstrings

The metadata command preserves raw docstring text for public declarations and checked methods. It does not currently parse docstring sections or validate documented parameters, returns, fields, aliases, or decorator facts. Consumers that need rendered reference documentation should treat docstring parsing as a separate documentation-generation step.

## Current Boundaries

Checked API metadata extraction does not emit Incan source, inspect built `.incnlib` artifacts, or materialize contract-backed models. Those are separate compiler/tooling surfaces from the JSON metadata command.
