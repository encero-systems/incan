# Checked Contract Metadata

Checked contract metadata is the tooling surface for canonical model bundles, contract-backed model materialization, and artifact introspection. It is separate from checked API metadata: model bundles describe structural row-shaped contracts that can be projected to Incan `model` source, while API metadata describes checked public declarations already present in source.

## Project Configuration

Declare model bundle JSON files in `incan.toml`:

```toml
[tool.incan.metadata]
model-bundles = ["contracts/order_summary.json"]
```

Paths are resolved relative to the project root unless they are absolute.

## Bundle Shape

A bundle may be stored as a single canonical model object:

```json
{
  "schema_version": 1,
  "stable_model_id": "orders.summary",
  "logical_type_name": "OrderSummary",
  "publishable": true,
  "fields": [
    {
      "name": "order_id",
      "type": "str",
      "alias": "orderId",
      "description": "Stable order identifier"
    },
    {
      "name": "coupon_code",
      "type": "str",
      "nullable": true
    }
  ]
}
```

Or as a package object with `model_bundles`:

```json
{
  "schema_version": 1,
  "model_bundles": [
    {
      "schema_version": 1,
      "stable_model_id": "orders.summary",
      "logical_type_name": "OrderSummary",
      "publishable": true,
      "fields": [
        {
          "name": "order_id",
          "type": "str"
        }
      ]
    }
  ]
}
```

Fields:

| Field               | Type           | Meaning                                                               |
| ------------------- | -------------- | --------------------------------------------------------------------- |
| `schema_version`    | number         | Bundle or package schema version.                                     |
| `stable_model_id`   | string or null | Artifact-facing identity for publishable bundles.                     |
| `logical_type_name` | string         | Incan model type name.                                                |
| `publishable`       | bool           | Whether the bundle may be embedded into artifact contract metadata.   |
| `fields`            | array          | Ordered canonical field list.                                         |
| `field.name`        | string         | Incan field name.                                                     |
| `field.type`        | string         | Incan type spelling.                                                  |
| `field.nullable`    | bool           | When true, emit uses `Option[T]` unless the type is already optional. |
| `field.alias`       | string or null | Field alias used as the wire name.                                    |
| `field.description` | string or null | Field description surfaced through model reflection.                  |
| `field.metadata`    | object         | Producer metadata that does not affect type identity.                 |

Publishable bundles require `stable_model_id`. Bundle validation rejects duplicate logical type names, duplicate stable ids, duplicate field names, duplicate aliases, empty aliases/descriptions, empty metadata keys, and unknown or opaque type spellings.

## Materialization

When a project declares model bundles, build and run commands materialize them as ordinary public `model` declarations before typechecking the project entry point. Materialized models participate in constructors, member access, model lowering, JSON alias metadata, and `__fields__()` reflection the same way handwritten models do for the represented field subset.

```incan
def main() -> None:
    let row = OrderSummary(order_id="o-1", coupon_code=None)
    println(row.order_id)
```

If a materialized model name collides with a visible source declaration or another bundle, the compiler reports a hard error instead of shadowing or mangling the generated name.

## Model Emit

Use `incan tools metadata model` to project a model bundle to formatted Incan source:

```bash
incan tools metadata model path/to/project OrderSummary --format incan
incan tools metadata model path/to/project orders.summary --format json
incan tools metadata model contracts/order_summary.json OrderSummary
```

The first positional argument is a project directory, bundle JSON file, source file inside a project, or `.incnlib` artifact. The second positional argument is either `logical_type_name` or `stable_model_id`.

`--format incan` prints formatted model source:

```incan
pub model OrderSummary:
    order_id [alias="orderId", description="Stable order identifier"]: str
    coupon_code: Option[str]
```

`--format json` prints the canonical bundle JSON for the selected model.

Projection is intentionally lossy. It preserves logical type name, field order, type spelling, nullability, alias, and description. It does not reconstruct comments, imports, author-only formatting, producer implementation details, or metadata fields that do not have an Incan model syntax.

## Artifact Inspection

`incan build --lib` embeds publishable model bundles and checked API metadata into the `.incnlib` manifest under `contract_metadata`. Tooling can inspect a built artifact without requiring the original source checkout:

```bash
incan build --lib
incan tools metadata model target/lib/my_package.incnlib OrderSummary --format incan
```

Artifacts that do not carry checked model metadata are reported as non-introspectable for model emit instead of being reconstructed from generated Rust or machine code.

## LSP Command

The language server registers `workspace/executeCommand` command `incan.metadata.model.emit`. Clients pass either one object argument or positional arguments:

```json
{
  "command": "incan.metadata.model.emit",
  "arguments": [
    {
      "uri": "file:///path/to/project/src/main.incn",
      "model": "OrderSummary",
      "format": "incan"
    }
  ]
}
```

The command returns a JSON object containing `format`, `model`, `stableModelId`, and either `source` for Incan output or `bundle` for JSON output. The same validation and formatter path as the CLI command are used.
