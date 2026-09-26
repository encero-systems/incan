//! Formatter tests: one module per subject, sharing the helpers in `helpers`.

use super::*;
use incan_syntax::ast::{Declaration, Program};
use incan_syntax::{lexer, parser};

mod helpers;

mod comments_and_blank_lines;
mod declarations;
mod entry_points_imports_and_spacing;
mod expressions_and_calls;
mod statements_and_patterns;

use helpers::{assert_format_round_trip_lex_parse, program_from_source};
