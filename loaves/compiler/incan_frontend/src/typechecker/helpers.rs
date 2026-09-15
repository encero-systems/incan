//! Provide shared helpers for the typechecker.
//!
//! This module centralizes small, reusable predicates and utilities used across typechecking
//! submodules, to avoid code duplication and keep semantics consistent.

mod consts;
mod strings;
mod symbols;
mod types;

pub use consts::*;
pub use strings::*;
pub use types::*;
