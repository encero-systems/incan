# incan_web_macros

Procedural macros for the transitional Incan web runtime.

This crate supports compiler-generated Rust that targets `incan_stdlib::web`. It is toolchain-locked to the Incan compiler and stdlib runtime crate; it is not a standalone public web framework API.

## Boundary

The macros here generate Axum/inventory wiring for Incan web programs:

- `#[route(...)]` registers generated route handlers with `incan_stdlib::web::RouteEntry`.
- `#[derive(IntoResponse)]` delegates response conversion for tuple newtypes.
- `#[derive(FromRequestParts)]` delegates request extraction for tuple newtypes.

Those expansions are part of the current host-runtime bridge. If `std.web` changes ownership or moves more behavior into Incan source, these macros may change with the compiler.

## Development

Keep changes aligned with `crates/incan_stdlib/src/web.rs` and generated web code. Do not add reusable routing abstractions here unless the corresponding Incan stdlib surface has been defined first.

## License

Apache 2.0
