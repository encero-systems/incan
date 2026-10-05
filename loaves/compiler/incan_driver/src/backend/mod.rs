//! Incan Compiler Backend
//!
//! This module handles code generation from the typed AST to Rust source code.
//!
//! ## Architecture
//!
//! The backend uses a single unified pipeline:
//!
//! ```text
//! AST → AstLowering → IR → IrEmitter (syn/quote) → prettyplease → RustSource
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use incan_driver::backend::IrCodegen;
//!
//! let mut codegen = IrCodegen::new();
//! let rust_code = codegen.generate(&ast);
//! ```
//!
//! ## Module Organization
//!
//! - `ir` - the IR and its emission under their pre-ring path: a re-export of the `incan_ir` (types, lowering) and
//!   `incan_emit` (codegen entrypoint `IrCodegen`, emission, conversions) crates
//! - `project/` - Rust source projection and explicit publisher support (plan, generator, cargo_toml, runner)
//! - `selection` - backend-selection identity and execution receipt (#986), re-exported from `incan_emit`
//! - `c_abi` - the platform C ABI table

// Enforce explicit error handling in project generation code.
// XXX: codegen modules emit `.unwrap()` as string literals in generated Rust code.
// This is a KNOWN LIMITATION — generated code can panic at runtime on invalid data (e.g., missing dict keys, failed
// string parsing). See RFC 014 for the plan to improve error handling in generated code.
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]

// Public modules
pub mod c_abi;
pub mod ir;
pub mod project;
pub use incan_emit::selection;

// Re-export the unified codegen entrypoint
pub use ir::{GenerationError, IrCodegen};

// Backend-selection identity and execution receipt (#986)
pub use selection::{
    BackendExecutionReceipt, BackendKind, BackendSelection, BackendSelectionError, digest_output, finalize_receipt,
    select_backend,
};

// Project generation (public API)
pub use project::{
    CargoCommand, CompilationPlan, ExecutionResult, Executor, PlannedDirectory, PlannedFile, ProjectGenerator,
    RunProfile,
};

// For tests that need to verify lowering behavior
#[doc(hidden)]
pub use ir::{AstLowering, LoweringError};
