//! Rust-lowered capability bound registry (RFC 041).

/// Compiler-recognized Rust capability bounds writable in Incan `with` clauses.
pub const RUST_CAPABILITY_BOUNDS: &[&str] = &[
    "Send",
    "Sync",
    "Static",
    "Fn",
    "FnMut",
    "FnOnce",
    "RuntimeFuture",
    "RuntimeFnOnce",
    "RuntimeRaceCallback",
];

/// The capability markers that describe a callable: `Fn[Args...]`, `FnMut[Args...]` and `FnOnce[Args...]`.
///
/// Their type arguments are the callable's parameter list; the return type is whatever the value passed at the call
/// site returns. Rust spells that requirement only in its parenthesized form (`Fn(A) -> R`), so lowering gives these
/// markers the canonical `std.traits.callable.CallableN[Args..., R]` shape rather than a plain trait path (#1716).
pub const RUST_CALLABLE_CAPABILITY_BOUNDS: &[&str] = &["Fn", "FnMut", "FnOnce"];

/// The capability markers that require a future: a value the async runtime polls to completion.
///
/// `RuntimeFuture[T]` is satisfied by what an `async def` call returns (`work()`), by a task handle, and by any other
/// awaitable value. A function value is none of those, whatever it returns: `spawn(work)` passes the function where
/// the task belongs. The typechecker refuses a function value against these bounds instead of admitting it as it
/// admits every other type at a capability marker (#1772).
pub const RUST_FUTURE_CAPABILITY_BOUNDS: &[&str] = &["RuntimeFuture"];

/// Return `true` when `name` is a Rust-lowered capability marker.
#[must_use]
pub fn is_rust_capability_bound(name: &str) -> bool {
    RUST_CAPABILITY_BOUNDS.contains(&name)
}

/// Return `true` when `name` is a capability marker that requires a future (`RuntimeFuture`).
#[must_use]
pub fn is_rust_future_capability_bound(name: &str) -> bool {
    RUST_FUTURE_CAPABILITY_BOUNDS.contains(&name)
}

/// Return `true` when `name` is one of the callable capability markers (`Fn`, `FnMut`, `FnOnce`).
#[must_use]
pub fn is_rust_callable_capability_bound(name: &str) -> bool {
    RUST_CALLABLE_CAPABILITY_BOUNDS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callable_markers_are_capability_bounds() {
        for name in RUST_CALLABLE_CAPABILITY_BOUNDS {
            assert!(is_rust_capability_bound(name), "{name} must stay a capability marker");
            assert!(is_rust_callable_capability_bound(name));
        }
        assert!(!is_rust_callable_capability_bound("Send"));
        assert!(!is_rust_callable_capability_bound("RuntimeFnOnce"));
    }

    #[test]
    fn future_markers_are_capability_bounds() {
        for name in RUST_FUTURE_CAPABILITY_BOUNDS {
            assert!(is_rust_capability_bound(name), "{name} must stay a capability marker");
            assert!(is_rust_future_capability_bound(name));
        }
        assert!(!is_rust_future_capability_bound("RuntimeFnOnce"));
        assert!(!is_rust_future_capability_bound("Send"));
    }
}
