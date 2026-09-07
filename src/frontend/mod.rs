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
pub mod body_ir;
pub mod contract_metadata;
pub mod decorator_resolution;
pub mod executable_resolution;
pub(crate) mod feature_metadata;
pub mod hir;
pub mod library_exports;
pub mod library_manifest_index;
pub mod module;
pub(crate) mod partial_projection;
pub mod registry_metadata;
pub(crate) mod resolved_type_subst;
pub(crate) mod rust_type_display;
pub mod surface_semantics;
pub mod symbols;
pub mod testing_markers;
pub mod typechecker;
pub mod vocab_ast_bridge;
pub mod vocab_desugar_pass;
