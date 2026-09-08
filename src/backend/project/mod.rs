//! Rust source projection from checked Incan compilation inputs.
//!
//! [`ProjectGenerator`] emits the selected source modules and provider facades into `src/main.rs` or `src/lib.rs`.
//! It consumes the compiler's checked provider plan; native dependency selection and execution belong to the caller.
//! [`plan`] contains passive emitted-file descriptions, and [`runner`] contains captured native command outcomes.

pub mod generator;
pub mod plan;
pub mod runner;

/// Rust dependency key for the toolchain-owned runtime support crate used by emitted source.
pub(crate) const INCAN_STDLIB_CRATE_NAME: &str = "incan_stdlib";
/// Rust dependency key for the toolchain-owned derive crate used by emitted source.
pub(crate) const INCAN_DERIVE_CRATE_NAME: &str = "incan_derive";
/// Compiler-owned support crates required by generated source.
pub(crate) const GENERATED_TOOLCHAIN_SUPPORT_CRATES: [&str; 2] = [INCAN_STDLIB_CRATE_NAME, INCAN_DERIVE_CRATE_NAME];

// Re-export public types so `crate::backend::project::ProjectGenerator` (etc.) still works.
pub use generator::{ProjectGenerator, RunProfile};
pub use plan::{CompilationPlan, ExecutionResult, PlannedDirectory, PlannedFile};
