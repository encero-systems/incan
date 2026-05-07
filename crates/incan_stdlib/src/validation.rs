//! Validation helpers used by RFC 017 validated newtype lowering.

use std::fmt::{self, Display};

/// Structured validation error payload for validated newtypes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub message: String,
    pub code: Option<String>,
}

impl ValidationError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
        }
    }

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
    pub fn new(target: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            failures: Vec::new(),
        }
    }

    pub fn push_field_error(&mut self, field: impl Into<String>, error: impl Display) {
        self.failures.push(ValidationFailure::new(field, error));
    }

    pub fn is_empty(&self) -> bool {
        self.failures.is_empty()
    }

    pub fn raise_if_any(self) {
        if !self.failures.is_empty() {
            crate::errors::raise(ValidationErrors {
                target: self.target,
                failures: self.failures,
            });
        }
    }
}

#[cold]
#[track_caller]
pub fn raise_validation_error(target: impl AsRef<str>, hook: impl AsRef<str>, error: impl Display) -> ! {
    let target = target.as_ref();
    let hook = hook.as_ref();
    crate::errors::raise(format!(
        "ValidationError: validated newtype construction failed for {target}::{hook}: {error}"
    ))
}

#[cold]
#[track_caller]
pub fn raise_constraint_error(target: impl AsRef<str>, constraint: impl AsRef<str>) -> ! {
    let target = target.as_ref();
    let constraint = constraint.as_ref();
    crate::errors::raise(format!(
        "ValidationError: validated newtype construction failed for {target}: constraint {constraint} failed"
    ))
}
