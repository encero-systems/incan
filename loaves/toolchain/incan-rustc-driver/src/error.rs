//! Refusals at the plain-data-to-native-body boundary.

/// A malformed plan must be rejected before any native compilation effect.
#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    /// An index does not name a local in the containing function.
    #[error("function `{function}` refers to unknown local {index}")]
    UnknownLocal { function: String, index: i64 },
    /// An edge does not name a basic block in the containing function.
    #[error("function `{function}` refers to unknown block {index}")]
    UnknownBlock { function: String, index: i64 },
    /// A function reference is absent from the crate and admitted external declarations.
    #[error("unknown callee `{0}`")]
    UnknownCallee(String),
    /// A well-indexed node violates the scalar plan's type or control-flow contract.
    #[error("invalid plan in `{function}`: {reason}")]
    Invalid { function: String, reason: String },
    /// A well-formed plan needs a native item the direct route does not provide yet, such as a trait impl legacy
    /// synthesizes for a source type that the checker proved the call relies on. This is a named route gap, not a plan
    /// defect, so it is reported the way lowering reports an unsupported construct.
    #[error("unsupported native instantiation of `{function}`: {reason}")]
    Unsupported { function: String, reason: String },
}
