//! Testing helpers for Incan-generated Rust code.
//!
//! `crates/incan_stdlib/stdlib/testing.incn` is the source-of-truth surface API for `std.testing`.
//! This Rust module implements only host-boundary functions referenced by `@rust.extern` declarations in `std.testing`.

/// Explicitly fail a test with a message.
///
/// # Panics
///
/// Always panics with the provided `msg`.
pub fn fail(msg: String) {
    panic!("{}", msg);
}

/// Generic panic primitive used by `std.testing` helpers with non-`None` return types.
///
/// # Panics
///
/// Always panics with the provided `msg`.
pub fn fail_t<T>(msg: String) -> T {
    panic!("{}", msg);
}

fn marker_runtime_misuse(marker: &str) -> ! {
    panic!("std.testing.{marker} is marker metadata for `incan test` and is not executable runtime logic");
}

/// Marker runtime for `@std.testing.skip`.
///
/// `incan test` handles skip semantics during test discovery. Calling this at runtime is a misuse.
pub fn skip(_reason: String) {
    marker_runtime_misuse("skip");
}

/// Marker runtime for `@std.testing.xfail`.
///
/// `incan test` handles xfail semantics during test discovery/execution. Calling this at runtime is a misuse.
pub fn xfail(_reason: String) {
    marker_runtime_misuse("xfail");
}

/// Marker runtime for `@std.testing.slow`.
///
/// `incan test` handles slow-test filtering. Calling this at runtime is a misuse.
pub fn slow() {
    marker_runtime_misuse("slow");
}

/// Marker runtime for `@std.testing.fixture`.
///
/// `incan test` consumes fixture metadata during discovery. Calling this at runtime is a misuse.
pub fn fixture() {
    marker_runtime_misuse("fixture");
}

/// Marker runtime for `@std.testing.parametrize`.
///
/// Parameter expansion is handled by `incan test`; calling this at runtime is a misuse.
pub fn parametrize<T>(_argnames: String, _argvalues: Vec<T>) {
    marker_runtime_misuse("parametrize");
}

/// Placeholder host boundary for `std.testing.assert_raises`.
///
/// # Panics
///
/// Always panics because `assert_raises` is not implemented yet.
pub fn assert_raises<E>(_block: fn() -> (), msg: String) -> E {
    if msg.is_empty() {
        fail_t::<E>("std.testing.assert_raises is not implemented yet".to_string());
    }
    fail_t::<E>(msg)
}
