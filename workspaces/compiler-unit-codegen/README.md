# Compiler unit codegen declarations

This Incan workspace validates the compiler's internal codegen declaration and projects it onto an existing schema-2 Loaf resolution. It has no dependency on the compiler's Rust libraries. Editing or testing this policy therefore does not require those libraries to be rebuilt.

The entrypoint takes `RESOLVED_INPUT DECLARATIONS PROFILE OUTPUT`, reads the original resolution without modifying it, and writes a separate projection only after validation. An identical existing projection is reused; a different existing destination is refused, including an input accidentally supplied as the output. `compiler-unit-codegen.json` declares SHA-256 optimization for the development closure while retaining assertions and overflow checks. The native adapter must bind these options into receipts before lookup and apply the exact options during compilation; policy tests alone do not prove that execution.

The [bootstrap workspace](../compiler-bootstrap/README.md) records the adapter boundary, runtime proof and remaining acceptance work for #1698. These exchange files are internal compiler preparation inputs. Application users do not need to invoke this projection command.
