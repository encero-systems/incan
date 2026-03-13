//! Incan Compiler Frontend
//!
//! This module contains all frontend components:
//! - `lexer`: tokenization of source code
//! - `parser`: parsing tokens into AST
//! - `ast`: abstract syntax tree definitions
//! - `symbols`: symbol table and scope management
//! - `typechecker`: type checking and validation
//! - `diagnostics`: error reporting and lints
//! - `resolver`: module resolution for multi-file projects

// Syntax components are provided by the shared incan_syntax crate.
pub use incan_syntax::{ast, diagnostics, lexer, parser};

// Compiler-specific pieces remain local.
pub mod decorator_resolution;
pub mod library_exports;
pub mod module;
pub mod resolver;
pub mod surface_semantics;
pub mod symbols;
pub mod testing_markers;
pub mod typechecker;
