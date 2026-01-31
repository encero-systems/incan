//! Parser for the Incan programming language
//!
//! Converts a token stream into an AST following RFC 000: Incan Core Language RFC (Phase 1).
//!
//! ## Examples
//!
//! ```rust,no_run
//! use incan_syntax::{lexer, parser};
//!
//! let source = "def foo() -> int:\n    return 42\n";
//! let tokens = lexer::lex(source).unwrap();
//! let ast = parser::parse(&tokens).unwrap();
//! assert_eq!(ast.declarations.len(), 1);
//! ```

use crate::ast::*;
use crate::diagnostics::CompileError;
use crate::lexer::{FStringPart as LexFStringPart, Token, TokenKind};
use incan_core::lang::field_metadata::{self, FieldMetadataKey};
use incan_core::lang::keywords::KeywordId;
use incan_core::lang::operators::OperatorId;
use incan_core::lang::punctuation::PunctuationId;

// NOTE: This module is split across multiple files using `include!` to keep all parser
// methods in the same Rust module (preserving privacy + call patterns) while avoiding
// a single large source file.

include!("parser/core.rs");
include!("parser/helpers.rs");
include!("parser/decl.rs");
include!("parser/types.rs");
include!("parser/stmts.rs");
include!("parser/expr.rs");
include!("parser/util.rs");
include!("parser/api.rs");
include!("parser/tests.rs");
