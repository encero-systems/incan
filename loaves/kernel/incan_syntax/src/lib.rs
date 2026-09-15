//! Shared syntax frontend for the Incan language: lexer, parser, AST, diagnostics.
//!
//! This crate is dependency-light and intended for reuse across the compiler, formatter, LSP, and future interactive
//! tooling.
//!
//! ## Notes
//! - This crate is intentionally “syntax-only”: it does not do name resolution, type checking, or IR lowering.
//! - Vocabulary identity (keywords/operators/punctuation) comes from `incan_core::lang` registries.
//!
//! ## Examples
//! ```rust,no_run
//! use incan_syntax::{lexer, parser};
//! # fn main() {
//!
//! let tokens = lexer::lex("pass\n").expect("source should lex");
//! let program = parser::parse(&tokens).expect("tokens should parse");
//! assert_eq!(program.declarations.len(), 1);
//! # }
//! ```
//!
//! ## See also
//! - `incan_core::lang` for registry-backed language vocabulary (keywords/operators/punctuation/etc.).

pub mod ast;
pub mod diagnostics;
pub mod lexer;
pub mod parser;
pub mod scanners;
pub mod token_helpers;
