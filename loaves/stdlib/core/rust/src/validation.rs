//! Validation helpers used by RFC 017 validated newtype lowering.

use std::fmt::{self, Display};

/// Structured validation error payload for validated newtypes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub message: String,
    pub code: Option<String>,
}

impl ValidationError {
    /// Create a validation error without a machine-readable code.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
        }
    }

    /// Create a validation error with a machine-readable code.
    pub fn with_code(message: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: Some(code.into()),
        }
    }
}

impl Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(code) = &self.code {
            write!(f, "{code}: {}", self.message)
        } else {
            f.write_str(&self.message)
        }
    }
}

/// One field/path validation failure inside an aggregate validation error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationFailure {
    pub path: String,
    pub message: String,
}

impl ValidationFailure {
    /// Create a field/path validation failure from a displayable error.
    pub fn new(path: impl Into<String>, error: impl Display) -> Self {
        Self {
            path: path.into(),
            message: error.to_string(),
        }
    }
}

/// Aggregated validation failures for model/class construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationErrors {
    pub target: String,
    pub failures: Vec<ValidationFailure>,
}

impl Display for ValidationErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "ValidationError: {} validation failed with {} error(s)",
            self.target,
            self.failures.len()
        )?;
        for failure in &self.failures {
            writeln!(f, "  {}: {}", failure.path, failure.message)?;
        }
        Ok(())
    }
}

/// Builder used by generated Rust to collect field validation failures.
#[derive(Debug, Clone)]
pub struct ValidationErrorsBuilder {
    target: String,
    failures: Vec<ValidationFailure>,
}

impl ValidationErrorsBuilder {
    /// Create a validation-error builder for a target type.
    pub fn new(target: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            failures: Vec::new(),
        }
    }

    /// Add a validation failure for one field or path.
    pub fn push_field_error(&mut self, field: impl Into<String>, error: impl Display) {
        self.failures.push(ValidationFailure::new(field, error));
    }

    /// Return whether no validation failures have been collected.
    pub fn is_empty(&self) -> bool {
        self.failures.is_empty()
    }

    /// Raise an aggregate validation error if any failures were collected.
    pub fn raise_if_any(self) {
        if !self.failures.is_empty() {
            crate::errors::raise(ValidationErrors {
                target: self.target,
                failures: self.failures,
            });
        }
    }
}

/// Raise a validation error for a failed validated-newtype hook.
#[cold]
#[track_caller]
pub fn raise_validation_error(target: impl AsRef<str>, hook: impl AsRef<str>, error: impl Display) -> ! {
    let target = target.as_ref();
    let hook = hook.as_ref();
    crate::errors::raise(format!(
        "ValidationError: validated newtype construction failed for {target}::{hook}: {error}"
    ))
}

/// Raise a validation error for a failed named constraint.
#[cold]
#[track_caller]
pub fn raise_constraint_error(target: impl AsRef<str>, constraint: impl AsRef<str>) -> ! {
    let target = target.as_ref();
    let constraint = constraint.as_ref();
    crate::errors::raise(format!(
        "ValidationError: validated newtype construction failed for {target}: constraint {constraint} failed"
    ))
}
