//! Incan Compiler Frontend
//!
//! This module contains all frontend components:
//! - `lexer`: tokenization of source code
//! - `parser`: parsing tokens into AST
//! - `ast`: abstract syntax tree definitions
//! - `symbols`: symbol table and scope management
//! - `typechecker`: type checking and validation
//! - `diagnostics`: error reporting and lints
//! - `module`: canonical source-module resolution for multi-file projects

// Syntax components are provided by the shared incan_syntax crate.
pub use incan_syntax::{ast, diagnostics, lexer, parser};

// Compiler-specific pieces remain local.
pub mod api_metadata;
pub(crate) mod ast_walk;
pub mod contract_metadata;
pub mod decorator_resolution;
pub mod library_exports;
pub mod library_manifest_index;
pub mod module;
pub(crate) mod resolved_type_subst;
pub mod surface_semantics;
pub mod symbols;
pub mod testing_markers;
pub mod typechecker;
pub mod vocab_ast_bridge;
pub mod vocab_desugar_pass;
