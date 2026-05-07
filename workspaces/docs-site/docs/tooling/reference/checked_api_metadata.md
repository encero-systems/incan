# Checked API Metadata

Checked API metadata is the compiler-produced JSON description of a package or module's public Incan API. It is intended for documentation generators, package browsers, editor tooling, and other consumers that need checked declarations without scraping source text or generated Rust.

Invoke the metadata command from a project root, project directory, or source file:

```bash
incan tools metadata api [PATH] --format json
```

Use `--format markdown` to render a compact generated API reference from the same checked metadata:

```bash
incan tools metadata api [PATH] --format markdown
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
  "package": {
    "name": "catalog",
    "version": "0.1.0"
  },
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
          "docstring_sections": {
            "summary": "Return the catalog label.",
            "params": [],
            "returns": null,
            "fields": [],
            "aliases": [],
            "decorators": []
          },
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

| Field            | Type           | Meaning                                                    |
| ---------------- | -------------- | ---------------------------------------------------------- |
| `schema_version` | number         | Metadata package schema version                            |
| `package`        | object or null | Project identity from `incan.toml`, when available         |
| `modules`        | array          | Checked metadata documents for the entry and local imports |

Each module document contains:

| Field            | Type   | Meaning                                           |
| ---------------- | ------ | ------------------------------------------------- |
| `schema_version` | number | Module metadata schema version                    |
| `module_path`    | array  | Logical module path segments                      |
| `declarations`   | array  | Public declarations visible from that source file |

`declarations` uses a `kind` discriminator. Current declaration kinds are `function`, `model`, `class`, `trait`, `enum`, `newtype`, `type_alias`, `const`, `static`, `alias`, and `partial`.

## Declaration Facts

The metadata is derived from parsed and typechecked semantics. Public declarations can include:

- stable source anchors: `anchor.id`, `anchor.span.start`, and `anchor.span.end`
- checked signatures, parameters, type parameters, bounds, receiver kind, and return type
- model and class fields, including model field `alias`, `description`, and `has_default`
- trait requirements and checked method signatures
- enum variants and value-enum raw values
- public import aliases with resolved `target_path` segments
- public partial callable presets with target provenance, preset metadata, projected callable parameters, return type, and async status
- raw docstring text when the declaration or method has a docstring
- parsed docstring sections in `docstring_sections`, including summary, parameters, returns, fields, aliases, and decorators
- decorator metadata with resolved decorator paths
- safe const values for public consts and safe decorator arguments

Types use the same structural `TypeRef` encoding as library manifest exports. For example, a non-generic type is encoded as `{"Named": {"name": "str"}}`, while a generic application is encoded as `{"Applied": {"name": "List", "args": [...]}}`.

When decorator processing exposes a public function as a callable-valued binding, metadata follows that checked binding. In that case, function metadata reports the callable binding's parameters and return type rather than the original source signature. Existing decorator metadata remains attached separately through `decorators`, so consumers that inspect marker decorators, safe decorator arguments, or docstring `Decorators:` sections can keep using that lane without inferring binding types from it.

Public partial declarations use `kind: "partial"`. A partial declaration remains distinct from a hand-written function or alias:

| Field         | Type   | Meaning                                                 |
| ------------- | ------ | ------------------------------------------------------- |
| `name`        | string | Exported partial name                                   |
| `target_path` | array  | Resolved target path segments                           |
| `target_kind` | string | Target category, such as `function`, `constructor`, or `partial` |
| `presets`     | array  | Preset names, checked types, and safe preset values      |
| `type_params` | array  | Remaining callable type parameters                      |
| `params`      | array  | Projected callable parameters                           |
| `return_type` | object | Checked callable return type                            |
| `is_async`    | bool   | Whether the projected callable is async                 |

Preset parameters are represented as ordinary defaulted parameters in `params`: `has_default: true` is the visual and semantic signal consumers should use for signature display. The `presets` array preserves provenance for tools that need to explain where the default came from.

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

The metadata command preserves raw docstring text for public declarations and checked methods and also emits parsed `docstring_sections` when a docstring is present. Recognized section headings are `Args:`, `Parameters:`, `Returns:`, `Fields:`, `Aliases:`, and `Decorators:`. Text before the first recognized heading becomes the `summary`.

Named sections use `name: description` entries:

```incan
pub def avg(values: List[float]) -> float:
    """
    Return the arithmetic mean.

    Args:
        values: Input values.

    Returns:
        float: Mean value.
    """
    return 0.0
```

`Returns:` may either be free-form prose or `type: description`. When a type spelling is present, the metadata command validates it against the checked return type.

Docstring validation is strict for mechanically checkable drift. If an `Args:` or `Fields:` section is present, every checked parameter or field must be documented and every documented name must exist. `Decorators:` and `Aliases:` entries must name decorators or aliases that exist in checked metadata. Drift diagnostics fail `incan tools metadata api` before JSON is printed.

## Editor Previews

The language server uses the same checked metadata extractor for hover previews after a document type-checks successfully. Hovering a public declaration, a checked public method, a public model/class field, or a public enum variant can show the checked signature, raw docstring text, field alias/description metadata, value-enum backing and raw-value metadata, derives, trait adoption, and safe const values. Public partial hover shows the projected callable signature plus target and preset provenance. If a decorated function's checked binding is callable-valued, its hover uses the same callable signature exposed by checked API metadata.

The LSP exposes these facts through `textDocument/hover`. Use `incan tools metadata api` when an integration needs the full JSON package.

If a document has parse or type errors, the LSP keeps reporting diagnostics and falls back to the older syntax-oriented hover details instead of presenting checked API metadata for an invalid program.

## Artifact and Model Boundaries

`incan tools metadata api` inspects source files or a project directory and emits JSON or generated Markdown. It does not build the project, emit generated Rust, or read a `.incnlib` artifact. Use `incan build --lib` for library artifact emission and `incan tools metadata model` for model bundle inspection.

The metadata JSON describes public declarations from checked Incan source and materialized contract models visible to the checked program. Model bundle schema, emit, materialization, and artifact inspection are documented separately in [Checked contract metadata](contract_metadata.md).

## Current Boundaries

Checked API metadata extraction does not inspect built `.incnlib` artifacts. Artifact inspection remains a separate tooling surface from source/project metadata extraction.

The extractor exposes only checked compiler facts and safe literal/const values. Unsupported decorator expressions are reported as `unsupported` metadata rather than evaluated, and consumers should not treat docstrings or decorator payloads as trusted executable input.
