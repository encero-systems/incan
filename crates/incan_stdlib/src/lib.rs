//! Standard library for Incan-generated Rust code.
//!
//! This crate provides traits and utilities that generated Incan code depends on, including reflection capabilities,
//! JSON serialization helpers, and numeric operations.

#![deny(clippy::unwrap_used)]

pub mod collections;
pub mod conversions;
pub mod derives;
pub mod errors;
pub mod frozen;
pub mod iter;
pub mod num;
pub mod prelude;
pub mod reflection;
pub mod strings;
pub mod testing;
pub mod version;

#[cfg(feature = "json")]
pub mod json;

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

/// RFC 022/023: Incan `std.async` namespace facade.
///
/// Generated Rust may import `incan_stdlib::r#async` when users write `import std.async`.
/// This module provides a stable namespace surface backed by Tokio re-exports.
#[cfg(any(feature = "async", feature = "web"))]
pub mod r#async {
    pub mod time {
        pub use crate::__private::tokio::time::{Duration, sleep, timeout};
    }

    pub mod task {
        pub use crate::__private::tokio::task::{JoinHandle, spawn, spawn_blocking, yield_now};
    }

    pub mod channel {
        pub use crate::__private::tokio::sync::mpsc::{
            Receiver, Sender, UnboundedReceiver, UnboundedSender, channel, unbounded_channel,
        };
        pub use crate::__private::tokio::sync::oneshot::{
            Receiver as OneshotReceiver, Sender as OneshotSender, channel as oneshot,
        };
    }

    pub mod sync {
        pub use crate::__private::tokio::sync::{Barrier, Mutex, RwLock, Semaphore};
    }

    pub mod prelude {
        pub use super::channel::{
            OneshotReceiver, OneshotSender, Receiver, Sender, channel, oneshot, unbounded_channel,
        };
        pub use super::sync::{Barrier, Mutex, RwLock, Semaphore};
        pub use super::task::{JoinHandle, spawn, spawn_blocking, yield_now};
        pub use super::time::{Duration, sleep, timeout};
    }
}

#[cfg(feature = "web")]
pub mod web;

// Re-export commonly used items
pub use reflection::{FieldInfo, HasFieldInfo};

#[cfg(feature = "json")]
pub use json::{FromJson, ToJson};

#[cfg(feature = "web")]
pub use web::{
    App, DELETE, GET, HEAD, HTTP_BAD_REQUEST, HTTP_CREATED, HTTP_FORBIDDEN, HTTP_INTERNAL_ERROR, HTTP_NO_CONTENT,
    HTTP_NOT_FOUND, HTTP_OK, HTTP_UNAUTHORIZED, OPTIONS, PATCH, POST, PUT, RouteEntry, response_html, response_ok,
    response_redirect, response_status,
};
