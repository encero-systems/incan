//! Tokio-backed time adapters for `std.async.time`.

use std::fmt;

/// Timeout error used by the public async timing helpers.
#[must_use]
#[derive(Clone, Copy, Default)]
pub struct TimeoutError;

impl TimeoutError {
    /// Incan-facing error message.
    pub fn message(&self) -> String {
        "operation timed out".to_string()
    }

    /// Timeout errors do not have an underlying cause.
    pub fn source(&self) -> Option<String> {
        None
    }
}

impl fmt::Debug for TimeoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TimeoutError")
    }
}

impl fmt::Display for TimeoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("operation timed out")
    }
}

impl std::error::Error for TimeoutError {}

/// Clamp a floating-point second value to a valid `Duration`, treating negative/NaN/infinity as zero.
pub fn clamp_seconds(seconds: f64) -> std::time::Duration {
    if !seconds.is_finite() || seconds.is_sign_negative() {
        return std::time::Duration::from_secs(0);
    }

    std::time::Duration::from_secs_f64(seconds)
}

pub fn clamp_millis(milliseconds: i64) -> std::time::Duration {
    let millis = if milliseconds.is_negative() {
        0
    } else {
        milliseconds as u64
    };

    std::time::Duration::from_millis(millis)
}

#[cfg(test)]
mod tests {
    use super::{clamp_millis, clamp_seconds};

    #[tokio::test]
    async fn clamp_seconds_normalizes_negative_and_infinite() {
        assert_eq!(clamp_seconds(-1.0), std::time::Duration::from_secs(0));
        assert_eq!(clamp_seconds(f64::INFINITY), std::time::Duration::from_secs(0));
        assert_eq!(clamp_seconds(f64::NAN), std::time::Duration::from_secs(0));
        assert!(clamp_seconds(0.25).as_nanos() > 0);
    }

    #[tokio::test]
    async fn clamp_millis_normalizes_negative() {
        assert_eq!(clamp_millis(-1), std::time::Duration::from_millis(0));
        assert_eq!(clamp_millis(500), std::time::Duration::from_millis(500));
    }
}
