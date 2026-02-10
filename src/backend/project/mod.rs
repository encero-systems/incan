//! Project generator — creates the output Rust project structure
//!
//! Generates:
//! - `Cargo.toml` with dependencies
//! - `src/main.rs` or `src/lib.rs`
//! - Invokes `cargo build`
//!
//! ## Cargo Dependency Policy
//!
//! The project generator receives **resolved dependency specs** from the dependency resolver,
//! including version requirements, features, sources, optional flags, and dev-only deps.
//! It does not perform resolution itself; it only renders `Cargo.toml` faithfully.
//!
//! ## Module Organisation
//!
//! - [`plan`] — [`CompilationPlan`], [`Executor`], [`ExecutionResult`] (separating "what" from "doing")
//! - [`generator`] — [`ProjectGenerator`] struct, setters, and `generate*()` methods
//! - [`cargo_toml`] — `Cargo.toml` rendering and dependency formatting
//! - [`runner`] — Build / run logic and result types ([`BuildResult`], [`RunResult`])

pub mod cargo_toml;
pub mod generator;
pub mod plan;
pub mod runner;

// Re-export public types so `crate::backend::project::ProjectGenerator` (etc.) still works.
pub use generator::ProjectGenerator;
pub use plan::{CargoCommand, CompilationPlan, ExecutionResult, Executor, PlannedDirectory, PlannedFile};
pub use runner::{BuildResult, RunResult};
