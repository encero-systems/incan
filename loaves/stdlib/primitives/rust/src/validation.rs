//! Native validation carrier and divergence helpers used by RFC 017 validated-newtype lowering.
//!
//! This boundary stays in Rust because `ValidationError` is a compiler-owned builtin carrier passed through generic
//! native `Result` values, and rustc MIR construction must name its concrete layout. Incan-authored runtime functions
//! own validation flow and call only these carrier and divergence operations.

use std::any::Any;
use std::fmt::{self, Display};
use std::sync::Arc;

/// Structured validation error payload for validated newtypes.
///
/// Cloning this umbrella shares retained originals through `Arc`; it never requires or invokes `Clone` on a concrete
/// error payload.
#[derive(Clone)]
pub struct ValidationError {
    pub message: String,
    pub code: Option<String>,
    members: Vec<ValidationMember>,
}

/// One retained concrete validation failure and its optional construction location, shared without cloning its
/// payload when the compatibility carrier itself is cloned.
#[derive(Clone)]
struct ValidationMember {
    location: Option<String>,
    message: String,
    original: Arc<dyn Any>,
}

/// Move one concrete payload into shared type-erased storage without cloning or requiring a payload trait beyond
/// concrete identity.
fn shared_original<T: Any>(original: Box<T>) -> Arc<dyn Any> {
    let original: Arc<T> = Arc::from(original);
    original
}

impl fmt::Debug for ValidationMember {
    /// Describe retained diagnostics without requiring or exposing a concrete payload's `Debug` implementation.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidationMember")
            .field("location", &self.location)
            .field("message", &self.message)
            .field("original_type", &self.original.as_ref().type_id())
            .finish()
    }
}

impl fmt::Debug for ValidationError {
    /// Describe the umbrella and retained member identities without requiring the original payloads to be cloneable
    /// or printable.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidationError")
            .field("message", &self.message)
            .field("code", &self.code)
            .field("members", &self.members)
            .finish()
    }
}

impl ValidationError {
    /// Create a validation error without a machine-readable code.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
            members: Vec::new(),
        }
    }

    /// Create a validation error with a machine-readable code.
    pub fn with_code(message: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: Some(code.into()),
            members: Vec::new(),
        }
    }

    /// Capture one original concrete error without imposing `Clone`, `Debug` or serialization on its payload.
    pub fn from_original<T: Any>(message: String, original: Box<T>) -> Self {
        Self {
            message: message.clone(),
            code: None,
            members: vec![ValidationMember {
                location: None,
                message,
                original: shared_original(original),
            }],
        }
    }

    /// Capture one original concrete error at a structured field or model location.
    pub fn from_located_original<T: Any>(location: String, message: String, original: Box<T>) -> Self {
        Self {
            message: message.clone(),
            code: None,
            members: vec![ValidationMember {
                location: Some(location),
                message,
                original: shared_original(original),
            }],
        }
    }

    /// Retain another original failure after existing members, preserving aggregate order.
    pub fn push_original<T: Any>(&mut self, message: String, original: Box<T>) {
        self.members.push(ValidationMember {
            location: None,
            message,
            original: shared_original(original),
        });
    }

    /// Retain another located original failure after existing members, preserving aggregate order.
    pub fn push_located_original<T: Any>(&mut self, location: String, message: String, original: Box<T>) {
        self.members.push(ValidationMember {
            location: Some(location),
            message,
            original: shared_original(original),
        });
    }

    /// Return this umbrella with one additional located original failure.
    ///
    /// This consuming form is the Incan-facing aggregate operation for #2472 while mutable native callable receivers
    /// are not yet representable by Body IR. It preserves member order without cloning any retained payload.
    pub fn with_located_original<T: Any>(mut self, location: String, message: String, original: Box<T>) -> Self {
        self.push_located_original(location, message, original);
        self
    }

    /// Return the number of original concrete failures retained by this umbrella.
    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    /// Borrow member `index` only when its concrete identity is exactly `T`.
    pub fn original_at<T: Any>(&self, index: usize) -> Option<&T> {
        self.members.get(index)?.original.downcast_ref::<T>()
    }

    /// Borrow the first member only when its concrete identity is exactly `T`.
    pub fn original<T: Any>(&self) -> Option<&T> {
        self.original_at(0)
    }

    /// Borrow member `index`'s structured location when one was captured.
    pub fn member_location(&self, index: usize) -> Option<&str> {
        self.members.get(index)?.location.as_deref()
    }

    /// Borrow member `index`'s human-readable message without consuming its original payload.
    pub fn member_message(&self, index: usize) -> Option<&str> {
        self.members.get(index).map(|member| member.message.as_str())
    }
}

impl Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(code) = &self.code {
            write!(f, "{code}: {}", self.message)?;
        } else {
            f.write_str(&self.message)?;
        }
        if self.members.len() > 1 {
            for member in &self.members {
                f.write_str("\n  ")?;
                if let Some(location) = &member.location {
                    write!(f, "{location}: ")?;
                }
                f.write_str(&member.message)?;
            }
        }
        Ok(())
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
