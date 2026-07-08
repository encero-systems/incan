//! Standard library for Incan-generated Rust code.
//!
//! This crate provides traits and utilities that generated Incan code depends on, including reflection capabilities,
//! JSON serialization helpers, and numeric operations.
//!
//! The stable boundary for Incan users is the `std.*` API declared by the Incan stdlib stubs. Rust modules in this
//! crate are runtime support for compiler-generated Rust and may contain transitional host implementations while the
//! corresponding Incan-language stdlib surface is still being built.

#![deny(clippy::unwrap_used)]

pub mod collections;
pub mod conversions;
pub mod environ;
pub mod errors;
pub mod frozen;
pub mod iter;
pub mod num;
pub mod prelude;
pub mod reflection;
pub mod storage;
pub mod strings;
pub mod testing;
pub mod validation;
pub mod version;

#[cfg(feature = "json")]
pub mod json;

#[cfg(feature = "async")]
pub mod r#async;

/// RFC 023: Incan `std.serde` namespace facade.
///
/// The `std.serde.json` module's `rust.module()` directive points here. Re-exports the JSON traits
/// so that `incan_stdlib::serde::ToJson` and `incan_stdlib::serde::FromJson` are available.
#[cfg(feature = "json")]
pub mod serde {
    pub use crate::json::{FromJson, ToJson};
}

/// Internal re-exports used by compiler-generated code.
///
/// These are **not** part of the user-facing stdlib API and may change alongside the compiler (toolchain-locked).
#[cfg(any(feature = "async", feature = "web"))]
pub mod __private {
    #[cfg(any(feature = "async", feature = "web"))]
    pub use tokio;
}

#[cfg(feature = "web")]
pub mod web;

// Re-export commonly used items
pub use reflection::{FieldInfo, HasClassName, HasFieldInfo, HasFieldMetadata, HasTypeClassName, HasTypeFieldMetadata};

#[cfg(feature = "json")]
pub use json::{FromJson, ToJson};

#[cfg(feature = "web")]
pub use web::{
    App, DELETE, GET, HEAD, HTTP_BAD_REQUEST, HTTP_CREATED, HTTP_FORBIDDEN, HTTP_INTERNAL_ERROR, HTTP_NO_CONTENT,
    HTTP_NOT_FOUND, HTTP_OK, HTTP_UNAUTHORIZED, OPTIONS, PATCH, POST, PUT, RouteEntry, response_html, response_ok,
    response_redirect, response_status,
};
