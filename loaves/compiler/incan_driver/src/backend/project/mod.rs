//! Project generator — creates the output Rust source projection
//!
//! Generates:
//! - `Cargo.toml` with dependencies
//! - `src/main.rs` or `src/lib.rs`
//! - Retains `Cargo.toml` only as an inspectable compatibility projection or explicit publisher input
//!
//! ## Cargo Dependency Policy
//!
//! The project generator receives **resolved dependency specs** from the dependency resolver, including version
//! requirements, features, sources, optional flags, and dev-only deps. It does not perform resolution itself; it only
//! renders `Cargo.toml` faithfully.
//!
//! ## Module Organisation
//!
//! - [`plan`] — [`CompilationPlan`], [`Executor`], [`ExecutionResult`] (separating "what" from "doing")
//! - [`generator`] — [`ProjectGenerator`] struct, setters, and `generate*()` methods
//! - [`cargo_toml`] — `Cargo.toml` rendering and dependency formatting
//! - [`runner`] — Cargo-lock projection support for the explicit publisher boundary

pub mod cargo_toml;
pub mod generator;
pub mod lock_projection;
pub mod plan;
pub mod runner;
#[cfg(test)]
mod tests;

// Re-export public types so `crate::backend::project::ProjectGenerator` (etc.) still works.
pub use generator::{ProjectGenerator, RunProfile};
pub use plan::{CargoCommand, CompilationPlan, ExecutionResult, Executor, PlannedDirectory, PlannedFile};
