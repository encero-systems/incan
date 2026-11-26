# Inspect checked C bindings

Check a source containing C bindings:

```bash
incan check src/main.incn --format json
```

Review the binding's source declaration and its declared header. Header paths are project-relative entries under `[[interop.c.targets]]`. The checker reports mismatched declarations as diagnostics.

Review every checked binding of the project:

```bash
incan inspect bindings
incan inspect bindings --format json
```

Keep a redaction-safe record of which bindings the program uses, for example in CI, with the binding-use receipt. It names each binding by its descriptor identity, each direct native call and each public facade over a private raw bridge, without paths:

```bash
incan inspect bindings --format receipt
```

To tie the receipt to a declared target's locked contract, lock first, then name the target:

```bash
oven lock
incan inspect bindings --format receipt --target aarch64-linux-android
```

The receipt then carries the target's `locked_target_identity` and each binding's declared artifacts; a binding relation in `[[interop.c.targets.bindings]]` that names a binding the compiler did not produce is refused.

For what a build for a target links, use `oven inspect interop-plan --target TRIPLE`. For source relationships, use:

```bash
incan inspect codegraph . --format jsonl
```

A successful source check does not prove a mobile package was assembled or signed.

See [Checked C bindings](../../language/how-to/checked_c_bindings.md), [the CLI reference](../reference/cli_reference.md#incan-inspect-bindings) and [Codegraph inspection](../reference/codegraph_inspection.md).
