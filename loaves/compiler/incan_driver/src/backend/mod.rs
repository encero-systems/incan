//! Incan compiler backend support that survives the removal of Rust source emission.
//!
//! - `selection` - the backend-selection identity and execution receipt (#986) that native outputs record
//! - `c_abi` - the platform C ABI table

#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]

pub mod c_abi;
pub mod selection;

pub use selection::{
    BackendExecutionReceipt, BackendKind, BackendSelection, BackendSelectionError, digest_output, finalize_receipt,
    select_backend,
};
