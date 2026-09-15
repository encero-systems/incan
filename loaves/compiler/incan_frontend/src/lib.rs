//! The Incan compiler frontend: the checked program and the provider facts it is checked against.
//!
//! The kernel supplies the syntax; this crate owns everything from the parsed module to the typed program:
//! - `lexer`: tokenization of source code
//! - `parser`: parsing tokens into AST
//! - `ast`: abstract syntax tree definitions
//! - `symbols`: symbol table and scope management
//! - `typechecker`: type checking and validation
//! - `diagnostics`: error reporting and lints
//! - `module`: canonical source-module resolution for multi-file projects
//! - `provider`: the compiled-provider catalog and active projection the typechecker resolves imports against
//! - `semantics_registry`: the surface-semantics packs both the frontend and the backend consult

// Syntax components are provided by the shared incan_syntax crate.
pub use incan_syntax::{ast, diagnostics, lexer, parser};
pub use parsed_module::ParsedModule;

// Compiler-specific pieces remain local.
pub mod api_metadata;
pub mod ast_walk;
pub mod body_ir;
pub mod contract_metadata;
pub mod decorator_resolution;
pub mod executable_resolution;
pub mod feature_metadata;
pub mod hir;
pub mod library_exports;
pub mod library_manifest;
pub mod library_manifest_index;
pub mod module;
pub mod numeric_adapters;
pub mod parsed_module;
pub mod partial_projection;
pub mod provider;
pub mod registry_metadata;
pub mod resolved_type_subst;
pub mod rust_type_display;
pub mod semantics_registry;
pub mod serde_usage;
pub mod surface_semantics;
pub mod symbols;
#[cfg(any(test, feature = "test_support"))]
pub mod test_support;
pub mod testing_markers;
pub mod typechecker;
pub mod vocab_ast_bridge;
pub mod vocab_desugar_pass;
