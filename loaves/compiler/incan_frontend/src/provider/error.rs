//! The failure a provider operation reports: a rendered message and nothing else.
//!
//! The provider does not know exit codes. Every failure it raised before the split was `CliError::failure`, so the
//! driver's conversion maps this to that same failure code and nothing above the provider changes. Keeping the
//! type here is what lets the provider sit below the driver as its own crate.

/// A provider operation that could not complete, rendered for the person who ran the command.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ProviderError {
    /// User-facing message, already formatted for display.
    pub message: String,
}

impl ProviderError {
    /// Fail with one rendered message.
    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Result of a provider operation.
pub type ProviderResult<T> = Result<T, ProviderError>;
