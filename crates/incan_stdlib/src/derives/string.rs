//! Rust backing for `std.derives.string` (`Debug`, `Display`).
//!
//! The `@rust.extern` method on `Debug` (`__repr__`) is implemented by Rust's `#[derive(Debug)]` proc macro.
//! `Display` has no extern methods — `__str__` is always user-provided. This module provides the namespace target for
//! the `rust.module("incan_stdlib::derives::string")` directive.
