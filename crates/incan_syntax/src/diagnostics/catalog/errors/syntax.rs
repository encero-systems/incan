//! Parser and lexer diagnostics.
//!
//! Syntax-level errors raised during lexing and parsing, before type
//! checking begins. These typically use [`ErrorKind::Syntax`](crate::diagnostics::ErrorKind::Syntax).

use crate::ast::Span;

use crate::diagnostics::CompileError;

// -- Token-level -------------------------------------------------------------

pub fn expected_token(expected: &str, found: &str, span: Span) -> CompileError {
    CompileError::syntax(format!("Expected {}, found {}", expected, found), span)
}

pub fn expected_token_message(message: &str, found: &str, span: Span) -> CompileError {
    CompileError::syntax(format!("{message}, found {found}"), span)
}

pub fn unexpected_token(found: &str, span: Span) -> CompileError {
    CompileError::syntax(format!("Unexpected token: {}", found), span)
}

pub fn expected_identifier(found: &str, span: Span) -> CompileError {
    CompileError::syntax(format!("Expected identifier, found {found}"), span)
}

pub fn expected_string_literal(found: &str, span: Span) -> CompileError {
    CompileError::syntax(format!("Expected string literal, found {found}"), span)
}

pub fn expected_declaration(found: &str, span: Span) -> CompileError {
    CompileError::syntax(format!("Expected declaration, found {found}"), span)
}

pub fn expected_expression(found: &str, span: Span) -> CompileError {
    CompileError::syntax(format!("Expected expression, found {found}"), span)
}

pub fn expected_pattern(found: &str, span: Span) -> CompileError {
    CompileError::syntax(format!("Expected pattern, found {found}"), span)
}

pub fn expected_variant_name_after_dot(span: Span) -> CompileError {
    CompileError::syntax("Expected variant name after '.'".to_string(), span)
}

// -- Receivers & closures ----------------------------------------------------

pub fn invalid_receiver(span: Span) -> CompileError {
    CompileError::syntax("Invalid receiver - expected 'self' or 'mut self'".to_string(), span)
}

pub fn closure_params_must_be_identifiers(span: Span) -> CompileError {
    CompileError::syntax("Closure parameters must be identifiers".to_string(), span)
}

// -- Assignments -------------------------------------------------------------

pub fn invalid_assignment_target(span: Span) -> CompileError {
    CompileError::syntax("Invalid assignment target".to_string(), span)
}

pub fn invalid_compound_assignment_target(span: Span) -> CompileError {
    CompileError::syntax("Invalid compound assignment target".to_string(), span)
}

pub fn invalid_tuple_assignment_target(span: Span) -> CompileError {
    CompileError::syntax("Invalid assignment target in tuple assignment".to_string(), span)
}

// -- Indexing -----------------------------------------------------------------

pub fn empty_index_not_allowed(span: Span) -> CompileError {
    CompileError::syntax("Empty index is not allowed".to_string(), span)
}

// -- Imports & decorators ----------------------------------------------------

pub fn pub_modifier_not_allowed_on_import(span: Span) -> CompileError {
    CompileError::syntax("The 'pub' modifier is not supported on imports".to_string(), span)
}

pub fn decorator_path_expected(span: Span) -> CompileError {
    CompileError::syntax("Expected decorator path after '@'".to_string(), span)
}

pub fn import_path_expected_separator_after_crate(span: Span) -> CompileError {
    CompileError::syntax("Expected '::' or '.' after 'crate'".to_string(), span)
}

pub fn import_path_expected_separator_after_super(span: Span) -> CompileError {
    CompileError::syntax("Expected '::' or '.' after 'super'".to_string(), span)
}

// -- Enum variants -----------------------------------------------------------

pub fn enum_variant_mapped_values(span: Span) -> CompileError {
    CompileError::syntax("Enum variants cannot have mapped values".to_string(), span)
        .with_note("Enum bodies only accept variant identifiers, optionally with payload types")
        .with_hint(
            "To model a lookup table, use a function that returns records instead:\n\n  \
                 def all_categories() -> list[Category]:\n      \
                 return [Category(\"Groceries\"), Category(\"Utilities\")]",
        )
}

pub fn enum_variant_contains_dots(span: Span) -> CompileError {
    CompileError::syntax("Enum variants cannot contain dots".to_string(), span)
        .with_note("Enum bodies only accept simple identifiers, optionally with payload types")
        .with_hint("Use plain variant names: e.g. `Inflow` instead of `Cash.Inflow`")
}

pub fn enum_variant_assigned_values(span: Span) -> CompileError {
    CompileError::syntax("Enum variants cannot have assigned values".to_string(), span)
        .with_note("Incan enums are algebraic types, not integer-valued enums")
        .with_hint(
            "If you need key-value data, use a model instead:\n\n  \
                 model Color:\n      \
                 name: str\n      \
                 value: int",
        )
}

pub fn enum_variant_type_annotations(span: Span) -> CompileError {
    CompileError::syntax("Enum variants cannot have type annotations".to_string(), span)
        .with_note("Enum variants use parenthesized payloads, not field-style declarations")
        .with_hint(
            "If you need typed fields, use a model instead:\n\n  \
                 model MyRecord:\n      \
                 name: str\n\n  \
                 Or use a payload: `MyVariant(str)`",
        )
}

// -- Method & field declarations ---------------------------------------------

pub fn method_decl_expected_body(span: Span) -> CompileError {
    CompileError::syntax(
        "Expected ':' after return type or newline for abstract method".to_string(),
        span,
    )
}

pub fn decorators_on_fields_not_supported(span: Span) -> CompileError {
    CompileError::syntax("Decorators on fields are not supported".to_string(), span)
}

pub fn unknown_field_metadata_key(key: &str, span: Span) -> CompileError {
    CompileError::syntax(format!("Unknown field metadata key '{key}'"), span)
}

pub fn duplicate_field_metadata_key(key: &str, span: Span) -> CompileError {
    CompileError::syntax(format!("Duplicate '{}' metadata key", key), span)
}

pub fn field_alias_as_conflict(span: Span) -> CompileError {
    CompileError::syntax("Cannot combine 'alias=\"...\"' with 'as \"...\"'".to_string(), span)
}

// -- String / literal lexer errors -------------------------------------------

pub fn unterminated_string(span: Span) -> CompileError {
    CompileError::new("Unterminated string".to_string(), span)
}

pub fn unterminated_string_newline(span: Span) -> CompileError {
    CompileError::new(
        "Unterminated string (newline in single-quoted string)".to_string(),
        span,
    )
}

pub fn unterminated_escape_sequence(span: Span) -> CompileError {
    CompileError::new("Unterminated escape sequence".to_string(), span)
}

pub fn unterminated_byte_string(span: Span) -> CompileError {
    CompileError::new("Unterminated byte string".to_string(), span)
}

pub fn unterminated_byte_string_newline(span: Span) -> CompileError {
    CompileError::new("Unterminated byte string (newline in string)".to_string(), span)
}

pub fn invalid_hex_escape(hex: &str, span: Span) -> CompileError {
    CompileError::new(format!("Invalid hex escape: \\x{}", hex), span)
}

pub fn non_ascii_in_byte_string(ch: char, span: Span) -> CompileError {
    CompileError::new(format!("Non-ASCII character in byte string: '{}'", ch), span)
}

pub fn unterminated_fstring(span: Span) -> CompileError {
    CompileError::new("Unterminated f-string".to_string(), span)
}

pub fn unmatched_right_brace_in_fstring(span: Span) -> CompileError {
    CompileError::new("Unmatched '}' in f-string".to_string(), span)
}

pub fn unterminated_fstring_escape(span: Span) -> CompileError {
    CompileError::new("Unterminated escape in f-string".to_string(), span)
}

pub fn invalid_float_literal(value: &str, span: Span) -> CompileError {
    CompileError::new(format!("Invalid float literal: {}", value), span)
}

pub fn invalid_integer_literal(value: &str, span: Span) -> CompileError {
    CompileError::new(format!("Invalid integer literal: {}", value), span)
}

pub fn unexpected_character(ch: char, span: Span) -> CompileError {
    CompileError::new(format!("Unexpected character '{}'", ch), span)
}

pub fn unexpected_bang(span: Span) -> CompileError {
    CompileError::new("Unexpected character '!'".to_string(), span)
}

pub fn unmatched_closing_bracket(span: Span) -> CompileError {
    CompileError::new("Unmatched closing bracket".to_string(), span)
}

pub fn inconsistent_indentation(expected: usize, found: usize, span: Span) -> CompileError {
    CompileError::new(
        format!("Inconsistent indentation: expected {} spaces, got {}", expected, found),
        span,
    )
}
