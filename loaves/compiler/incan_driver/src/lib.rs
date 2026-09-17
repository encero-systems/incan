//! The compiler driver: what a command does between argument parsing and rendering.
//!
//! Project discovery, the compilation session, module collection and typecheck orchestration, Cargo policy for
//! the compatibility path, the rust-inspect workspace, the build pipeline, the generated project (`backend`), the
//! inspection analyses (`inspect`), the generated-output cache and the replacement-compatibility corpus live here.
//! Nothing in this crate parses `clap` arguments; the binaries render what the driver returns, and the LSP and
//! codegraph consume the same sessions. The provider is `incan_provider`, which this crate consumes as a client.
//!
//! The driver is not yet silent, and an embedder should know where it speaks. Warnings that have no record to ride
//! home in are still written to stderr where they arise — the ignored `Cargo.toml` beside a `loaf.toml` in
//! [`project`], typecheck warnings in [`diagnostics`] and [`testing::module_graph`] — and the build pipeline and the
//! generated-project runner print their progress and the program's output as they go. Moving those onto the returned
//! records is the remaining half of the boundary; the moves that made this crate did not change any of it.

#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
pub mod backend;
// The build pipeline still rides the `cli` feature the root crate gave it: it links the generator's managed-cache
// lease, which is gated the same way, and it reads Rust metadata on every route. Cutting it loose from `cli` is the
// remaining #1479 work; until then a `--no-default-features` driver has no build pipeline.
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
