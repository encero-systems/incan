//! Rust backing for `std.derives.comparison` (`Eq`, `Ord`, `Hash`).
//!
//! The `@rust.extern` methods on these traits (`__eq__`, `__lt__`, `__hash__`) are implemented by Rust's
//! `#[derive(PartialEq, PartialOrd, Hash)]` proc macros. This module provides the namespace target for the
//! `rust.module("incan_stdlib::derives::comparison")` directive.
