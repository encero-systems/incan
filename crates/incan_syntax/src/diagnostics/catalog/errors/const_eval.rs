//! Const-expression evaluation and builtin function diagnostics.
//!
//! Errors from the compile-time constant evaluator (RFC 008/009) and
//! builtin function arity/type checks.

use crate::ast::Span;
use incan_core::errors::IncanError;

use crate::diagnostics::CompileError;

// -- Const evaluation --------------------------------------------------------

pub fn const_missing_type_annotation(name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Cannot infer type for const '{}'; add an explicit type annotation",
            name
        ),
        span,
    )
}

pub fn const_dependency_cycle(cycle: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Const dependency cycle detected: {}", cycle), span)
}

pub fn const_non_const_name(name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Non-const name '{}' is not allowed in a const initializer", name),
        span,
    )
}

pub fn const_unary_op_not_supported(op: &str, ty: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Unary '{}' is not supported for type '{}'", op, ty), span)
}

pub fn const_binary_op_not_supported(op: &str, left: &str, right: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Binary operator '{}' is not supported for types '{}' and '{}'",
            op, left, right
        ),
        span,
    )
}

pub fn const_compare_incompatible(left: &str, right: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Cannot compare '{}' with '{}'", left, right), span)
}

pub fn const_logical_op_requires_bool(op: &str, left: &str, right: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Logical operator '{}' requires bool operands (got '{}' and '{}')",
            op, left, right
        ),
        span,
    )
}

pub fn const_operator_not_allowed(op: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Operator '{}' is not allowed inside const initializers (phase 1)", op),
        span,
    )
}

pub fn const_empty_list_type_inference(span: Span) -> CompileError {
    CompileError::type_error(
        "Cannot infer type for empty const list; annotate as FrozenList[T]".to_string(),
        span,
    )
}

pub fn const_empty_set_type_inference(span: Span) -> CompileError {
    CompileError::type_error(
        "Cannot infer type for empty const set; annotate as FrozenSet[T]".to_string(),
        span,
    )
}

pub fn const_empty_dict_type_inference(span: Span) -> CompileError {
    CompileError::type_error(
        "Cannot infer type for empty const dict; annotate as FrozenDict[K, V]".to_string(),
        span,
    )
}

pub fn const_indexing_requires_string(span: Span) -> CompileError {
    CompileError::type_error(
        "Indexing is only supported for strings in const initializers".to_string(),
        span,
    )
}

pub fn const_slicing_requires_string(span: Span) -> CompileError {
    CompileError::type_error(
        "Slicing is only supported for strings in const initializers".to_string(),
        span,
    )
}

pub fn const_string_index_requires_int(found: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("String index must be int (got '{}')", found), span)
}

pub fn const_slice_component_requires_int(component: &str, found: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Slice {} must be int (got '{}')", component, found), span)
}

pub fn const_string_index_out_of_range(span: Span) -> CompileError {
    CompileError::type_error(IncanError::string_index_out_of_range().to_string(), span)
}

pub fn const_slice_step_zero(span: Span) -> CompileError {
    CompileError::type_error(IncanError::slice_step_zero().to_string(), span)
}

pub fn const_expression_not_allowed(span: Span) -> CompileError {
    CompileError::type_error(
        "Expression is not allowed inside const initializers (phase 1)".to_string(),
        span,
    )
}

pub fn const_self_not_allowed(span: Span) -> CompileError {
    CompileError::type_error("self is not allowed inside const initializers".to_string(), span)
}

pub fn const_none_type_inference(span: Span) -> CompileError {
    CompileError::type_error(
        "Cannot infer type for None in const initializer; add an explicit type annotation".to_string(),
        span,
    )
}

// -- Builtin function calls --------------------------------------------------

pub fn constructor_single_arg_required(name: &str, found: usize, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "{}() expects exactly one argument (positional or named `value`), got {}",
            name, found
        ),
        span,
    )
}

pub fn builtin_arity(name: &str, expected: usize, found: usize, span: Span) -> CompileError {
    CompileError::type_error(format!("{name}() expects {expected} argument(s), got {found}"), span)
}

pub fn builtin_expects_list(name: &str, found: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("{name}() expects a list, got {}", found), span)
}

pub fn builtin_list_element_type_not_supported(name: &str, found: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("{name}() does not support list element type {}", found), span)
}

pub fn builtin_bool_type_not_supported(found: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("bool() does not support type {}", found), span)
}
