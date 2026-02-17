//! Abstract Syntax Tree definitions for Incan
//!
//! This module defines all AST node types for the Incan language, following the grammar defined in our RFCs. The types
//! are organised into submodules by language component; everything is re-exported here so callers can continue to use
//! `use incan_syntax::ast::*`.

mod core;
mod decls;
mod exprs;
mod imports;
mod stmts;
mod types;
mod visitor;

pub use self::core::*;
pub use decls::*;
pub use exprs::*;
pub use imports::*;
pub use stmts::*;
pub use types::*;
pub use visitor::*;
