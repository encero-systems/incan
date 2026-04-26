//! Testing helpers for Incan-generated Rust code.
//!
//! `crates/incan_stdlib/stdlib/testing.incn` is the source-of-truth surface API for `std.testing`.
//! This Rust module implements only host-boundary functions referenced by `@rust.extern` declarations in `std.testing`.

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

/// Marker runtime for `@std.testing.test`.
///
/// `incan test` handles explicit test discovery. Calling this at runtime is a misuse.
pub fn test() {
    marker_runtime_misuse("test");
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

/// Marker runtime for `@std.testing.mark`.
///
/// `incan test` handles marker selection during discovery. Calling this at runtime is a misuse.
pub fn mark(_name: String) {
    marker_runtime_misuse("mark");
}

/// Marker runtime for `@std.testing.resource`.
///
/// `incan test` handles resource scheduling. Calling this at runtime is a misuse.
pub fn resource(_name: String) {
    marker_runtime_misuse("resource");
}

/// Marker runtime for `@std.testing.serial`.
///
/// `incan test` handles serial scheduling. Calling this at runtime is a misuse.
pub fn serial() {
    marker_runtime_misuse("serial");
}

/// Marker runtime for `@std.testing.timeout`.
///
/// `incan test` handles test timeouts. Calling this at runtime is a misuse.
pub fn timeout(_duration: String) {
    marker_runtime_misuse("timeout");
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

/// Parameter case wrapper for decorator metadata.
///
/// When executed outside decorator metadata, this returns the wrapped value unchanged.
pub fn param_case<T>(value: T, _marks: Vec<String>, _id: String) -> T {
    value
}

/// Environment mutation fixture helper.
///
/// Values changed through this helper are restored when the helper is dropped.
pub struct TestEnv {
    previous: Vec<(String, Option<String>)>,
}

impl TestEnv {
    pub fn new() -> Self {
        Self { previous: Vec::new() }
    }

    pub fn set(&mut self, key: String, value: String) {
        let old = std::env::var(&key).ok();
        self.previous.push((key.clone(), old));
        // SAFETY: `incan test` runs generated test harnesses with libtest test-threads=1; this helper is intended for
        // isolated test processes and restores changes on drop.
        unsafe { std::env::set_var(key, value) };
    }

    pub fn unset(&mut self, key: String) {
        let old = std::env::var(&key).ok();
        self.previous.push((key.clone(), old));
        // SAFETY: see `TestEnv::set`.
        unsafe { std::env::remove_var(key) };
    }

    pub fn get(&self, key: String) -> Option<String> {
        std::env::var(key).ok()
    }
}

impl Default for TestEnv {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..).rev() {
            if let Some(value) = value {
                // SAFETY: see `TestEnv::set`.
                unsafe { std::env::set_var(key, value) };
            } else {
                // SAFETY: see `TestEnv::set`.
                unsafe { std::env::remove_var(key) };
            }
        }
    }
}
