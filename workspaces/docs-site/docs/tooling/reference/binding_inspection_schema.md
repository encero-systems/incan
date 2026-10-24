# Binding inspection surfaces

Dev.8 has no `incan inspect bindings` command or binding-use receipt command.

Checked bindings can contribute `c_binding`, `c_binding_call` and `c_binding_facade` records to the [codegraph](codegraph_inspection.md). Source diagnostic reports are produced by `incan check PATH --format json`. [`std.interop`](../../language/reference/stdlib/interop.md) specifies the language binding contract.
