# Inspect checked C bindings

Check a source containing C bindings:

```bash
incan check src/main.incn --format json
```

Review the binding's source declaration and its declared header. Header paths are project-relative entries under `[[interop.c.targets]]`. The checker reports mismatched declarations as diagnostics.

For source relationships, use:

```bash
incan inspect codegraph . --format jsonl
```

Read binding-related records only when present and checked. Dev.8's CLI does not expose `inspect bindings`, binding-use receipts, interop deployment-plan inspection, baking or staging. A successful source check does not prove a mobile package was assembled or signed.

See [Checked C bindings](../../language/how-to/checked_c_bindings.md) and [Codegraph inspection](../reference/codegraph_inspection.md).
