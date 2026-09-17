//! Compiler-owned inspection analysis.
//!
//! These modules produce the facts an inspection surface reports. They deliberately sit behind the compiler
//! boundary rather than inside a CLI command: RFC 120 records the failure mode, that "a check living in the CLI
//! could not agree with one living in the frontend by construction", and #1293 found a live instance of exactly
//! that. A CLI command selects an input and formats an answer; it does not compute one.
//!
//! The workspace layout rewrite (#1478) left this beside the driver's other inspection surfaces rather than in a
//! crate of its own; `rust_inspect` stays the Rust-signature substrate it consumes.

pub mod closure;
pub mod codegraph;
