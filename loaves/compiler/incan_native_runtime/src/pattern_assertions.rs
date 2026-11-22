//! Carrier-pattern assertion failures and `unwrap` panics, with the messages compiled code has always raised.

use std::fmt::Debug;

use incan_std_testing::fail_t;

/// `assert value is Some(...)` met None.
pub fn missing_some() {
    missing_some_with("assertion failed: expected Some, got None".to_owned());
}

/// `assert value is Some(...), message` met None.
pub fn missing_some_with(message: String) {
    fail_t(message)
}

/// `assert value is None` met Some.
pub fn unexpected_some() {
    unexpected_some_with("assertion failed: expected None, got Some".to_owned());
}

/// `assert value is None, message` met Some.
pub fn unexpected_some_with(message: String) {
    fail_t(message)
}

/// `assert value is Ok(...)` met Err.
pub fn missing_ok() {
    missing_ok_with("assertion failed: expected Ok, got Err".to_owned());
}

/// `assert value is Ok(...), message` met Err.
pub fn missing_ok_with(message: String) {
    fail_t(message)
}

/// `assert value is Err(...)` met Ok.
pub fn missing_err() {
    missing_err_with("assertion failed: expected Err, got Ok".to_owned());
}

/// `assert value is Err(...), message` met Ok.
pub fn missing_err_with(message: String) {
    fail_t(message)
}

/// The standard None panic, with its static text payload.
pub fn unwrap_none() {
    std::panic::panic_any("called `Option::unwrap()` on a `None` value");
}

/// The standard Err panic: the error formatted once through Debug, as an owned payload.
pub fn unwrap_err<E: Debug>(error: E) {
    std::panic::panic_any(format!("called `Result::unwrap()` on an `Err` value: {error:?}"));
}
