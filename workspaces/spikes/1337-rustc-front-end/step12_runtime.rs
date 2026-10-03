//! Step 12's runtime shim: the callable entry point the native route needs for `println`.
//!
//! The emitted-Rust route expands `println` into Rust's `println!` macro, which no MIR can call. The native route
//! calls a function instead, keeping `println!`'s semantics, including output capture under a test harness, by
//! invoking the macro inside it. The product function belongs in the stdlib's Rust facet; this shim stands in for it.

/// Print `text` and a newline to standard output, exactly as `println!("{text}")` does.
pub fn println(text: String) {
    println!("{text}");
}
