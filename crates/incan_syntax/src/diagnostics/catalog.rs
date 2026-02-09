//! Error and lint constructor catalog.
//!
//! Every user-facing diagnostic the compiler can emit has a named constructor
//! in [`errors`] (for hard errors) or [`lints`] (for advisory warnings).
//! Centralising them here makes it easy to keep messages consistent and to
//! audit the full set of diagnostics the compiler produces.

pub mod errors;
pub mod lints;
