//! The compiler driver: what a command does between argument parsing and rendering.
//!
//! Project discovery, the compilation session, module collection and typecheck orchestration, Cargo policy for
//! the compatibility path, and the rust-inspect workspace live here. Nothing in this tree parses `clap` arguments
//! or prints; `cli` renders what the driver returns, and the LSP and codegraph consume the same sessions. The
//! provider half of the former `commands/common.rs` — SDK store identity, inventory discovery, project
//! requirements — lives under `provider`, which this module consumes as a client.

pub mod build_unit;
pub mod cargo_policy;
pub mod diagnostics;
pub mod error;
pub mod lock;
pub mod metadata_packages;
pub mod modules;
pub mod project;
#[cfg(feature = "rust_inspect")]
pub mod rust_inspect_workspace;
pub mod session;
pub mod testing;
pub mod typecheck;
