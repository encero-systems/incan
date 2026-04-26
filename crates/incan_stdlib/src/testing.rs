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
