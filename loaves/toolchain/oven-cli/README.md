# `oven-cli`

Ring: **toolchain**

The `oven` binary: the command surface from RFC 118, authored against the Oven API rather than by moving `incan oven`'s handlers.

## Sources

None yet. `incan oven …`, `incan lock` and `incan tools` stay in `incan-cli`: `tools` is a semantic product RFC 118's ownership table gives to `incan`, `lock` is a wrapper over the driver's lock resolution (which this crate may not import), and `oven.rs` is mostly this repository's test-harness tooling, none of it in RFC 118's `oven` surface. The package is authored when RFC 118 lands; the decision is recorded on #1481 (cut 3).

## May depend on

`oven`, plus `compiler/incan_oven_facet` for the Incan provider wiring

This directory is a layout skeleton. It holds no code yet; `src/` is a placeholder for the conventional crate root.
