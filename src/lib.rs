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
pub use incan_frontend::compiler_stack;
pub(crate) use incan_provider::compiled_sdk;
pub use incan_provider::dependency_resolver;
pub mod driver;
pub use incan_format as format;
pub use incan_frontend as frontend;
pub(crate) mod generated_cache;
pub mod inspect;
pub use incan_frontend::library_manifest;
pub mod lockfile;
#[cfg(feature = "lsp")]
pub mod lsp;
pub use oven_model::manifest;
pub mod numeric;
pub use incan_ir::numeric_adapters;
pub mod oven;
pub use incan_oven_facet as oven_facet;
pub use oven_model::oven_interop;
pub use oven_model::project_lifecycle;
pub mod provider;
pub mod replacement_compatibility;
#[cfg(feature = "rust_inspect")]
pub mod rust_inspect;
pub use incan_core::version;
pub(crate) use oven_model::toolchain_layout;
pub use oven_model::workspace;

pub use frontend::ast;
pub use frontend::diagnostics;
pub use frontend::lexer;
pub use frontend::parser;
pub use frontend::symbols;
pub use frontend::typechecker;

pub use backend::IrCodegen;
pub use backend::project::ProjectGenerator;

pub use format::{FormatConfig, FormatError, check_formatted, format_diff, format_source, format_source_with_config};
