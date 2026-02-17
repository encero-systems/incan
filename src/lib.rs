#![forbid(unsafe_code)]
//! Incan Programming Language Compiler
//!
//! Incan combines Rust's safety and performance with Python's expressiveness.
//! This crate provides the compiler: frontend (lexer, parser, type checker),
//! backend (Rust code generation), and tooling (formatter, LSP).
//!
//! ## Panic Policy
//!
//! This codebase follows explicit error handling:
//!
//! - **Production code**: Use `Result` or `Option` with `?` / `ok_or` / `map_err`. The `cli` and `backend` modules
//!   enforce `#![deny(clippy::unwrap_used)]`.
//!
//! - **Test code**: `.unwrap()` and `.expect()` are acceptable in tests.
//!
//! - **Generated code**: The codegen modules emit `.unwrap()` as *string literals* in generated Rust code. This is
//!   acceptable (these are output strings, not actual method calls in the compiler).
//!
//! - **True invariants**: If a panic represents a compiler bug (logic error), use `.expect("INVARIANT: reason")` with a
//!   clear explanation.

pub mod backend;
#[cfg(feature = "cli")]
pub mod cli;
pub mod dependency_resolver;
pub mod format;
pub mod frontend;
pub mod lockfile;
#[cfg(feature = "lsp")]
pub mod lsp;
pub mod manifest;
pub mod numeric;
pub mod numeric_adapters;
pub(crate) mod semantics_registry;
pub mod version;

pub use frontend::ast;
pub use frontend::diagnostics;
pub use frontend::lexer;
pub use frontend::parser;
pub use frontend::symbols;
pub use frontend::typechecker;

pub use backend::IrCodegen;
pub use backend::project::ProjectGenerator;

pub use format::{FormatConfig, check_formatted, format_diff, format_source, format_source_with_config};
