# Binding inspection schema

Checked bindings contribute `c_binding`, `c_binding_call` and `c_binding_facade` records to the [codegraph](codegraph_inspection.md). This page specifies the members a `c_binding` record carries in `resources`, `symbols`, `enums` and `structs`. [`std.interop`](../../language/reference/stdlib/interop.md) specifies the language binding contract.

## Binding fields

Each resource has `name`, `native` and `release` strings.

Each symbol has its Incan `name`, its native linker spelling in `native`, `parameters`, a structural `return_type`, `buffers` and `outcomes`. Each parameter has a `name` and a structural `type`. A buffer has `pointer_parameter`, `length_parameter` and an exact scalar `element` spelling; `buffers` is omitted when empty. An outcome has the binding-local `result` spelling and the output-parameter names in `initializes`, `updates` and `invalidates`.

An enum has a binding-local `name`, a canonical scalar `carrier`, and `variants`, each with an Incan `name` and a C constant `native` spelling. A structure has a binding-local `name`, its C spelling in `native`, and `fields`, each with a `name` and a structural `type`.

## Structural types

A structural type is an object with a `kind` discriminator:

| `kind` | Additional fields | Meaning |
| --- | --- | --- |
| `scalar` | `spelling` | Canonical C vocabulary spelling such as `c.i32`. |
| `pointer` | `mutable`, `pointee` | Required C pointer and its nested pointee type. |
| `struct` | `name` | Binding-local plain structure. |
| `resource` | `access`, `resource` | Opaque resource with `owned`, `borrowed` or `borrowed_mut` access. |
| `output` | `mode`, `value` | Compiler-managed `out` or `in_out` storage and its nested value type. |
| `nullable` | `value` | Nullable form of the nested resource type. |
| `void` | none | C `void` result. |
