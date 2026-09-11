//! Compiler-owned inspection analysis.
//!
//! These modules produce the facts an inspection surface reports. They deliberately sit behind the compiler
//! boundary rather than inside a CLI command: RFC 120 records the failure mode, that "a check living in the CLI
//! could not agree with one living in the frontend by construction", and #1293 found a live instance of exactly
//! that. A CLI command selects an input and formats an answer; it does not compute one.
//!
//! Destined for `compiler/incan_inspect` under `loaves/LAYOUT.md`, alongside `rust_inspect`.

pub mod codegraph;
