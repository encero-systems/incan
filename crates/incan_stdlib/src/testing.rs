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
    crate::errors::__private::raise_runtime_misuse(&msg)
}

fn marker_runtime_misuse(marker: &str) -> ! {
    crate::errors::__private::raise_runtime_misuse(&format!(
        "std.testing.{marker} is marker metadata for `incan test` and is not executable runtime logic"
    ));
}

/// Marker runtime for `@std.testing.skip`.
///
/// `incan test` handles skip semantics during test discovery. Calling this at runtime is a misuse.
pub fn skip(_reason: String) {
    marker_runtime_misuse("skip");
}

/// Marker runtime for `@std.testing.skipif`.
///
/// `incan test` evaluates skipif conditions during discovery. Calling this at runtime is a misuse.
pub fn skipif(_condition: bool, _reason: String) {
    marker_runtime_misuse("skipif");
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

/// Marker runtime for `@std.testing.xfailif`.
///
/// `incan test` evaluates xfailif conditions during discovery. Calling this at runtime is a misuse.
pub fn xfailif(_condition: bool, _reason: String) {
    marker_runtime_misuse("xfailif");
}

/// Return the host platform identifier used by collection-time marker probes.
pub fn platform() -> String {
    std::env::consts::OS.to_string()
}

/// Runtime fallback for collection-time testing feature probes.
///
/// Runner features are only meaningful during `incan test` collection, so ordinary runtime calls return false.
pub fn feature(_name: String) -> bool {
    false
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
/// `incan test` uses resource metadata to avoid overlapping generated test batches that declare the same resource.
/// Calling this at runtime is a misuse.
pub fn resource(_name: String) {
    marker_runtime_misuse("resource");
}

/// Marker runtime for `@std.testing.serial`.
///
/// `incan test` uses serial metadata to run a generated test batch alone. Calling this at runtime is a misuse.
pub fn serial() {
    marker_runtime_misuse("serial");
}

/// Marker runtime for `@std.testing.timeout`.
///
/// `incan test` uses timeout metadata when running generated test batches. Calling this at runtime is a misuse.
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
    /// Create an environment helper with no recorded mutations.
    pub fn new() -> Self {
        Self { previous: Vec::new() }
    }

    /// Set an environment variable and remember its previous value for restoration.
    pub fn set(&mut self, key: String, value: String) {
        let old = std::env::var(&key).ok();
        self.previous.push((key.clone(), old));
        // SAFETY: process environment mutation is only sound when the caller ensures no other thread concurrently
        // reads or writes the environment. The generated runner invokes tests with libtest test-threads=1.
        unsafe { std::env::set_var(key, value) };
    }

    /// Remove an environment variable and remember its previous value for restoration.
    pub fn unset(&mut self, key: String) {
        let old = std::env::var(&key).ok();
        self.previous.push((key.clone(), old));
        // SAFETY: see `TestEnv::set`.
        unsafe { std::env::remove_var(key) };
    }

    /// Read the current process value for an environment variable.
    pub fn get(&self, key: String) -> Option<String> {
        std::env::var(key).ok()
    }
}

impl Default for TestEnv {
    /// Create the default environment helper.
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TestEnv {
    /// Restore environment variables changed through this helper in reverse order.
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

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::panic;

    use super::{fail_t, fixture, skip};

    fn panic_message(payload: &(dyn Any + Send)) -> Option<&str> {
        if let Some(message) = payload.downcast_ref::<String>() {
            Some(message.as_str())
        } else {
            payload.downcast_ref::<&str>().copied()
        }
    }

    #[test]
    fn fail_t_panics_with_the_given_message() -> Result<(), Box<dyn std::error::Error>> {
        let result = panic::catch_unwind(|| fail_t::<()>("custom failure".to_string()));

        match result {
            Ok(()) => Err(std::io::Error::other("fail_t returned instead of panicking").into()),
            Err(payload) => {
                assert_eq!(panic_message(payload.as_ref()), Some("custom failure"));
                Ok(())
            }
        }
    }

    #[test]
    fn marker_runtime_panics_explain_runner_only_usage() -> Result<(), Box<dyn std::error::Error>> {
        let result = panic::catch_unwind(|| skip("not implemented".to_string()));

        match result {
            Ok(()) => Err(std::io::Error::other("skip marker returned instead of panicking").into()),
            Err(payload) => {
                assert_eq!(
                    panic_message(payload.as_ref()),
                    Some("std.testing.skip is marker metadata for `incan test` and is not executable runtime logic"),
                );
                Ok(())
            }
        }
    }

    #[test]
    fn fixture_runtime_panics_explain_runner_only_usage() -> Result<(), Box<dyn std::error::Error>> {
        let result = panic::catch_unwind(fixture);

        match result {
            Ok(()) => Err(std::io::Error::other("fixture marker returned instead of panicking").into()),
            Err(payload) => {
                assert_eq!(
                    panic_message(payload.as_ref()),
                    Some("std.testing.fixture is marker metadata for `incan test` and is not executable runtime logic"),
                );
                Ok(())
            }
        }
    }
}
