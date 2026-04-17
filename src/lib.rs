#![forbid(unsafe_code)]
//! Incan Programming Language Compiler
//!
//! Incan combines Rust's safety and performance with Python's expressiveness.
//! This crate provides the compiler: frontend (lexer, parser, type checker),
//! backend (Rust code generation), and tooling (formatter, LSP).
//!
//! ## Panic Policy
//!
//! This codebase avoids `.unwrap()` and `.expect()` everywhere, including tests.
//! Use `Result`, `Option`, `?`, `ok_or`, and `map_err` instead.
//!
//! Generated Rust output may still contain panic-backed helpers and fallback paths.
//! That is generated program code, not compiler code.

pub mod backend;
#[cfg(feature = "cli")]
pub mod cli;
pub mod dependency_resolver;
pub mod format;
pub mod frontend;
pub mod library_manifest;
pub mod lockfile;
#[cfg(feature = "lsp")]
pub mod lsp;
pub mod manifest;
pub mod numeric;
pub mod numeric_adapters;
#[cfg(feature = "rust_inspect")]
pub mod rust_inspect;
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
