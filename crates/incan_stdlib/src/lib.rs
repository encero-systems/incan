//! Standard library for Incan-generated Rust code.
//!
//! This crate provides traits and utilities that generated Incan code depends on, including reflection capabilities,
//! JSON serialization helpers, and numeric operations.

#![deny(clippy::unwrap_used)]

pub mod collections;
pub mod conversions;
pub mod errors;
pub mod frozen;
pub mod iter;
pub mod num;
pub mod prelude;
pub mod reflection;
pub mod strings;
pub mod testing;

#[cfg(feature = "json")]
pub mod json;

#[cfg(feature = "web")]
pub mod web;

// Re-export commonly used items
pub use reflection::{FieldInfo, HasFieldInfo};

#[cfg(feature = "json")]
pub use json::{FromJson, ToJson};

#[cfg(feature = "web")]
pub use web::{App, DELETE, GET, HEAD, Json, OPTIONS, PATCH, POST, PUT, Response};

// Testing helpers (always available)
pub use testing::{assert, assert_eq, assert_false, assert_ne, assert_true, fail};
