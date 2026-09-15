//! The Oven ring as one module, for the callers that still spell `crate::oven::…`: the store crate's receipt model
//! and modules and the rustc crate's modules are re-exported under their old paths until the driver is its own
//! crate.

pub use oven_rustc::{interop, legacy_cargo, loaf, loaf_mirror, native_contract, native_test, plan, rustc};
pub use oven_store::*;
