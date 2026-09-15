//! The compiler driver: what a command does between argument parsing and rendering.
//!
//! Project discovery, the compilation session, module collection and typecheck orchestration, Cargo policy for
//! the compatibility path, the rust-inspect workspace, the build pipeline, the generated project (`backend`), the
//! inspection analyses (`inspect`), the generated-output cache and the replacement-compatibility corpus live here.
//! Nothing in this crate parses `clap` arguments or prints; the binaries render what the driver returns, and the LSP
//! and codegraph consume the same sessions. The provider is `incan_provider`, which this crate consumes as a client.

pub mod backend;
#[cfg(feature = "cli")]
pub mod build;
pub mod build_report;
pub mod build_unit;
pub mod cargo_policy;
pub mod diagnostics;
pub mod error;
pub mod generated_cache;
pub mod inspect;
pub mod interop_plan;
pub mod lock;
pub mod metadata_packages;
pub mod modules;
pub mod oven_store;
pub mod project;
pub mod replacement_compatibility;
#[cfg(feature = "rust_inspect")]
pub mod rust_inspect_workspace;
pub mod session;
pub mod shadow_support;
pub mod testing;
#[cfg(test)]
mod tests;
pub mod typecheck;

pub use backend::IrCodegen;
pub use backend::project::ProjectGenerator;
