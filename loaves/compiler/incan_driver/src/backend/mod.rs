//! Incan compiler backend support that survives the removal of Rust source emission.
//!
//! - `c_abi` - the platform C ABI table and checked C binding verification
//! - `c_abi_capabilities` - a declared interop target's toolchain and SDK capabilities, checked against the selection

#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]

pub mod c_abi;
pub mod c_abi_capabilities;
