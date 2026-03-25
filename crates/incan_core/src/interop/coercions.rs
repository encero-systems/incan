//! Built-in coercion matrix between Incan built-ins and Rust boundary targets (RFC 041).

/// Policy class for an admitted coercion edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoercionPolicy {
    /// Canonical exact lowering (`int -> i64`, `bool -> bool`, etc.).
    Exact,
    /// Borrow-based adaptation (`str -> &str`, `bytes -> &[u8]`).
    Borrow,
    /// Explicitly admitted lossy adaptation (`float -> f32` in the initial RFC matrix).
    Lossy,
}

/// Return the policy for an admitted builtin edge, or `None` when no implicit edge exists.
///
/// Parameters are normalized names (`int`, `float`, `str`, `bytes`, `None`) and Rust boundary type displays (`i64`,
/// `f32`, `String`, `&str`, `Vec<u8>`, `&[u8]`, `()`).
pub fn admitted_builtin_coercion(incan_type: &str, rust_target: &str) -> Option<CoercionPolicy> {
    match (incan_type, rust_target) {
        ("int", "i64") => Some(CoercionPolicy::Exact),
        ("float", "f64") => Some(CoercionPolicy::Exact),
        ("float", "f32") => Some(CoercionPolicy::Lossy),
        ("bool", "bool") => Some(CoercionPolicy::Exact),
        ("str", "String") | ("frozenstr", "String") => Some(CoercionPolicy::Exact),
        ("str", "&str") | ("frozenstr", "&str") => Some(CoercionPolicy::Borrow),
        ("bytes", "Vec<u8>") | ("frozenbytes", "Vec<u8>") => Some(CoercionPolicy::Exact),
        ("bytes", "&[u8]") | ("frozenbytes", "&[u8]") => Some(CoercionPolicy::Borrow),
        ("none", "()") | ("unit", "()") => Some(CoercionPolicy::Exact),
        _ => None,
    }
}
