//! JSON, serde and ordinal-map runtime: the Rust facet of the `data` standard library component.
//!
//! Generated code links this crate when its program reaches `std.json`, `std.serde` or `std.collections`; the
//! compiler writes the checked `std.json` facade into every generated crate today, so in practice every program
//! links it. It carries the serde-backed `ToJson`/`FromJson` traits and `JsonValue`, the `std.serde` namespace
//! facade, and the xxh3 key helpers that the generated `OrdinalMap` splices in. Everything here builds on
//! `incan_std_primitives`; nothing in the primitive facet depends on this one.

#![deny(clippy::unwrap_used)]

pub mod collections;
pub mod json;

/// Compatibility reexports for native consumers of the JSON traits.
pub mod serde {
    pub use crate::json::{FromJson, ToJson};
}

pub use json::{FromJson, ToJson};
