//! Incan compiler backend support that survives the removal of Rust source emission.
//!
//! - `c_abi` - the platform C ABI table and checked C binding verification

#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]

pub mod c_abi;
