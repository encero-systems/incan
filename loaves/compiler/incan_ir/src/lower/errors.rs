//! Error types for AST to IR lowering.
//!
//! This module defines error types used during the lowering pass from AST to IR.
//! It provides both single errors (`LoweringError`) and error collections
//! (`LoweringErrors`) to support aggregated error reporting.

use super::super::IrSpan;

/// Error during AST lowering.
///
/// Represents a single error encountered during the lowering pass.
///
/// # Fields
///
/// * `message` - Human-readable error description
/// * `span` - Source location where the error occurred
#[derive(Debug, Clone)]
pub struct LoweringError {
    pub message: String,
    pub span: IrSpan,
}

impl std::fmt::Display for LoweringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "lowering error: {}", self.message)
    }
}

impl std::error::Error for LoweringError {}

/// Collection of multiple lowering errors.
///
/// This type allows returning multiple errors from a lowering pass,
/// similar to how the frontend typechecker reports multiple errors.
///
/// # Examples
///
/// ```rust,ignore
/// use incan::backend::ir::lower::{AstLowering, LoweringErrors};
///
/// let mut lowering = AstLowering::new_with_type_info(type_info);
/// match lowering.lower_program(&ast) {
///     Ok(ir) => { /* use ir */ }
///     Err(errors) => {
///         for err in errors.iter() {
///             eprintln!("Error: {}", err);
///         }
///     }
/// }
/// ```
#[derive(Debug)]
pub struct LoweringErrors(pub Vec<LoweringError>);

impl LoweringErrors {
    /// Create a new collection with a single error.
    ///
    /// # Parameters
    ///
    /// * `error` - The single error to wrap
    ///
    /// # Returns
    ///
    /// A `LoweringErrors` containing exactly one error.
    pub fn single(error: LoweringError) -> Self {
        Self(vec![error])
    }

    /// Create from a vector of errors.
    ///
    /// # Parameters
    ///
    /// * `errors` - Vector of errors to collect
    ///
    /// # Returns
    ///
    /// `Some(LoweringErrors)` if the vector is non-empty, `None` otherwise.
    pub fn from_vec(errors: Vec<LoweringError>) -> Option<Self> {
        if errors.is_empty() { None } else { Some(Self(errors)) }
    }

    /// Number of errors in this collection.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the collection is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterate over the errors.
    pub fn iter(&self) -> impl Iterator<Item = &LoweringError> {
        self.0.iter()
    }

    /// Get the first error (for backwards compatibility).
    ///
    /// Useful when callers only want to display a single error message.
    pub fn first(&self) -> Option<&LoweringError> {
        self.0.first()
    }
}

impl std::fmt::Display for LoweringErrors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.len() == 1 {
            write!(f, "{}", self.0[0])
        } else {
            writeln!(f, "{} lowering errors:", self.0.len())?;
            for (i, err) in self.0.iter().enumerate() {
                writeln!(f, "  {}: {}", i + 1, err)?;
            }
            Ok(())
        }
    }
}

impl std::error::Error for LoweringErrors {}

impl From<LoweringError> for LoweringErrors {
    fn from(e: LoweringError) -> Self {
        LoweringErrors::single(e)
    }
}
