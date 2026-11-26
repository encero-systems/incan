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

/// The auto traits a native callback's storage contract carries.
const RUST_AUTO_TRAITS: &[&str] = &["Send", "Sync"];

/// Return the callable trait (`Fn`, `FnMut` or `FnOnce`) a Rust trait path names.
///
/// Only the standard `std::ops` and `core::ops` paths qualify: a crate's own trait that is also called `Fn` is an
/// ordinary trait, not a callable bound.
#[must_use]
pub fn rust_callable_trait(path: &str) -> Option<&'static str> {
    let name = path
        .strip_prefix("std::ops::")
        .or_else(|| path.strip_prefix("core::ops::"))?;
    RUST_CALLABLE_CAPABILITY_BOUNDS
        .iter()
        .copied()
        .find(|marker| *marker == name)
}

/// Return the auto trait (`Send` or `Sync`) a Rust trait path names.
///
/// Only the standard `std::marker` and `core::marker` paths qualify: a crate's own `Send` is an ordinary trait.
#[must_use]
pub fn rust_auto_trait(path: &str) -> Option<&'static str> {
    let name = path
        .strip_prefix("std::marker::")
        .or_else(|| path.strip_prefix("core::marker::"))?;
    RUST_AUTO_TRAITS.iter().copied().find(|marker| *marker == name)
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
    fn only_standard_paths_name_callable_and_auto_traits() {
        assert_eq!(rust_callable_trait("std::ops::FnMut"), Some("FnMut"));
        assert_eq!(rust_callable_trait("core::ops::Fn"), Some("Fn"));
        assert_eq!(rust_callable_trait("my_crate::Fn"), None);
        assert_eq!(rust_auto_trait("std::marker::Send"), Some("Send"));
        assert_eq!(rust_auto_trait("core::marker::Sync"), Some("Sync"));
        assert_eq!(rust_auto_trait("my_crate::marker::Send"), None);
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
