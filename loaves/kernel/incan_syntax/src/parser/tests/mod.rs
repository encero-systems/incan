//! Parser unit tests.
//!
//! These tests focus on correctness of specific syntactic forms and on the parser’s error recovery behavior
//! (avoiding cascaded errors).

use super::*;
use crate::lexer;
use incan_lang::lang::types::collections::{self, CollectionTypeId};

mod helpers;

mod expressions;
mod fstrings;
mod functions_decorators_and_bindings;
mod modules_and_imports;
mod operator_precedence;
mod patterns_and_matching;
mod soft_keywords_and_vocab_blocks;
mod statements_and_blocks;
mod tuple_assignments;
mod type_declarations;
mod types_and_bounds;
mod vocab_scoped_symbols;

use helpers::{
    parse_str, parse_str_err, require_class_decl, require_function_decl, require_model_decl, require_newtype_decl,
    require_source_span, require_trait_decl,
};
