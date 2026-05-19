//! Incan language vocabulary registries.
//!
//! This module is the “front door” for language-level vocabulary: reserved keywords, operators, builtin functions,
//! builtin types, and punctuation.
//!
//! The design goal is to avoid stringly-typed checks scattered across the compiler/tooling. Instead, callers work with
//! **stable IDs** (e.g. `KeywordId`, `OperatorId`) and look up spellings/metadata via registry tables.
//!
//! ## Notes
//! - Registries are intentionally **pure**: no AST types, no IO, no side effects.
//! - The lexer/parser enforce syntax; registries provide spellings and metadata for shared use (diagnostics, docs,
//!   formatting, highlighting).
//!
//! ## Examples
//! ```rust
//! use incan_core::lang::keywords::{self, KeywordId};
//!
//! assert_eq!(keywords::from_str("if"), Some(KeywordId::If));
//! assert_eq!(keywords::as_str(KeywordId::If), "if");
//! ```
//!
//! ## See also
//! - `cargo run -p incan_core --bin generate_lang_reference` writes
//!   `workspaces/docs-site/docs/language/reference/language.md` from these registries. Do not edit that Markdown by
//!   hand; change the tables here and re-run the binary.

pub mod builtins;
pub mod conventions;
pub mod decorators;
pub mod derives;
pub mod enum_helpers;
pub mod errors;
pub mod features;
pub mod field_metadata;
pub mod highlighting;
pub mod keywords;
pub mod magic_methods;
pub mod operators;
pub mod punctuation;
pub mod registry;
pub mod rust_keywords;
pub mod stdlib;
pub mod surface;
pub mod trait_bounds;
pub mod trait_capabilities;
pub mod traits;
pub mod types;
