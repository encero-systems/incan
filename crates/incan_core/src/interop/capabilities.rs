//! Rust-lowered capability bound registry (RFC 041).

/// Compiler-recognized Rust capability bounds writable in Incan `with` clauses.
pub const RUST_CAPABILITY_BOUNDS: &[&str] = &["Send", "Sync", "Static", "Fn", "FnMut", "FnOnce"];

/// Return `true` when `name` is a Rust-lowered capability marker.
#[must_use]
pub fn is_rust_capability_bound(name: &str) -> bool {
    RUST_CAPABILITY_BOUNDS.contains(&name)
}
