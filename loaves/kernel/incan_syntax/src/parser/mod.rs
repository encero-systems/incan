//! Parser for the Incan programming language
//!
//! Converts a token stream into an AST following RFC 000: Incan Core Language RFC (Phase 1).
//!
//! ## Examples
//!
//! ```rust
//! use incan_syntax::{lexer, parser};
//! # fn main() -> Result<(), String> {
//!
//! let source = "def foo() -> int:\n    return 42\n";
//! let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
//! let ast = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
//! assert_eq!(ast.declarations.len(), 1);
//! # Ok(())
//! # }
//! ```

use crate::ast::*;
use crate::diagnostics::{CompileError, errors};
use crate::lexer::{FStringPart as LexFStringPart, Token, TokenKind};
use incan_lang::lang::conventions::validate_package_feature_identifier;
use incan_lang::lang::field_metadata::{self, FieldMetadataKey};
use incan_lang::lang::keywords::{KeywordId, KeywordSurfaceKind};
use incan_lang::lang::operators::OperatorId;
use incan_lang::lang::punctuation::PunctuationId;
use incan_semantics_core::SurfaceFeatureKey;

// NOTE: This module is split across multiple files using `include!` to keep all parser methods in the same Rust module
// (preserving privacy + call patterns) while avoiding a single large source file.

include!("core.rs");
include!("helpers.rs");
include!("decl/mod.rs");
include!("types.rs");
include!("stmts.rs");
include!("expr.rs");
include!("prefix_and_power.rs");
include!("embedded/mod.rs");
include!("util.rs");
include!("api.rs");
#[cfg(test)]
mod tests;
