//! Compile-time version compatibility check between the Incan compiler and its stdlib.
//!
//! When the Incan compiler generates a Rust project from user code, that project depends on the `incan_stdlib` crate.
//! If the compiler and stdlib versions drift apart (e.g. a cached stdlib from a previous install), the generated code
//! could break in subtle ways at runtime.
//!
//! This module prevents that by providing a macro that the compiler emits into every generated `main.rs`:
//!
//! ```rust,ignore
//! incan_stdlib::__incan_stdlib_version_check!("0.3.0-dev.3");
//! ```
//!
//! The macro expands into a `const` assertion that compares the compiler version (baked in as a string literal) against
//! the stdlib version (read from `Cargo.toml` via `env!`). A mismatch becomes a **compile-time error** in the generated
//! Rust code, surfacing the problem before anything runs.

/// The version of this stdlib crate, read from `Cargo.toml` at compile time.
///
/// The compiler embeds its own version as a literal in the generated code, and the
/// `__incan_stdlib_version_check!` macro compares the two.
pub const INCAN_STDLIB_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Byte-level string equality usable in `const` contexts.
///
/// Rust nightly currently does not stabilise `PartialEq` as a const trait, so `a == b` on `&str` inside a `const` block
/// is a compiler error. This function works around that by comparing raw `&[u8]` slices element by element — primitive
/// `u8` equality is always const-stable.
#[doc(hidden)]
pub const fn const_str_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Compile-time assertion that the compiler and stdlib versions match.
///
/// Emitted by the Incan compiler into every generated `main.rs`. Expands to a `const _: () = { ... }` block that panics
/// (= compile error) when the versions differ. Example output on mismatch:
///
/// ```text
/// Incan compiler/std lib version mismatch: compiler 0.3.0-dev.4, stdlib 0.3.0-dev.3
/// ```
#[doc(hidden)]
#[macro_export]
macro_rules! __incan_stdlib_version_check {
    ($compiler_version:literal) => {
        const _: () = {
            if !$crate::version::const_str_eq(
                $compiler_version.as_bytes(),
                $crate::version::INCAN_STDLIB_VERSION.as_bytes(),
            ) {
                panic!(concat!(
                    "Incan compiler/std lib version mismatch: compiler ",
                    $compiler_version,
                    ", stdlib ",
                    env!("CARGO_PKG_VERSION")
                ));
            }
        };
    };
}
