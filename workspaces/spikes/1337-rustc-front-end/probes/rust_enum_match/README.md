# Probe: matching Body IR-shaped Rust enums from Incan

Body IR's `StatementKind` mixes struct variants (`Assign { place, rvalue }`), tuple variants and unit variants. An Incan-written lowering has to match on all three. This probe wraps a Rust enum of those three shapes in a `rusttype` and matches it from Incan.

Run it with an Incan toolchain from the dev line: `incan oven bake --project .` then `incan run`. It prints:

```text
assign 3 4
return 7
continue
```

Struct variants match with named field patterns (`Stmt.Assign(place=place, value=value)`), tuple variants positionally, and unit variants bare; payloads bind with their Rust types.
