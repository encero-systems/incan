//! Rust backing for `std.derives.copying` (`Clone`, `Copy`, `Default`).
//!
//! The `@rust.extern` methods on these traits (`clone`, `default`) are implemented by Rust's
//! `#[derive(Clone, Copy, Default)]` proc macros. This module provides the namespace target for the
//! `rust.module("incan_stdlib::derives::copying")` directive.
