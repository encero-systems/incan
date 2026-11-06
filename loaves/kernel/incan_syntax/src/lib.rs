//! Shared syntax frontend for the Incan language: lexer, parser, AST, diagnostics.
//!
//! This crate is dependency-light and intended for reuse across the compiler, formatter, LSP, and future interactive
//! tooling.
//!
//! ## Notes
//! - This crate is intentionally “syntax-only”: it does not do name resolution, type checking, or IR lowering.
//! - Vocabulary identity (keywords/operators/punctuation) comes from `incan_lang::lang` registries.
//!
//! ## Examples
//! ```rust
//! use incan_syntax::{lexer, parser};
//! # fn main() -> Result<(), String> {
//!
//! let tokens = lexer::lex("def main() -> None:\n    pass\n")
//!     .map_err(|errors| format!("lex errors: {errors:?}"))?;
//! let program = parser::parse(&tokens).map_err(|errors| format!("parse errors: {errors:?}"))?;
//! assert_eq!(program.declarations.len(), 1);
//! # Ok(())
//! # }
//! ```
//!
//! ## See also
//! - `incan_lang::lang` for registry-backed language vocabulary (keywords/operators/punctuation/etc.).

pub mod ast;
pub mod diagnostics;
pub mod lexer;
pub mod parser;
pub mod scanners;
pub mod token_helpers;
