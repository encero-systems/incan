//! Const-expression evaluation and builtin function diagnostics.
//!
//! Errors from the compile-time constant evaluator (RFC 008/009) and builtin function arity/type checks.

use crate::ast::Span;
use incan_lang::errors::IncanError;

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

/// Report a `const` annotated with a mutable container type (`list`, `dict`, `set`) at the annotation itself.
///
/// A const is deeply immutable and is represented by the frozen wrapper (`FrozenList[T]`, `FrozenDict[K, V]`,
/// `FrozenSet[T]`; RFC 030), which does not read where the mutable container is expected. Accepting the written
/// annotation would silently retype the binding and surface the contradiction only at its first use site, in a
/// diagnostic naming a type the author never wrote (#1488). `written` is the annotation as the source spells it and
/// `frozen` the representation the const actually has; `span` is the annotation's own span.
pub fn const_mutable_collection_annotation(name: &str, written: &str, frozen: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("const '{name}' is annotated '{written}', but a const is deeply immutable and is represented as '{frozen}'"),
        span,
    )
    .with_note("A frozen collection does not read where the mutable container is expected, so the written annotation could not be honoured at any use site")
    .with_hint(format!("Annotate the const as '{frozen}', or omit the annotation to infer it"))
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

/// Report that a builtin-style call supplied more arguments than its surface contract accepts.
pub fn builtin_max_arity(name: &str, max: usize, found: usize, span: Span) -> CompileError {
    CompileError::type_error(format!("{name}() expects at most {max} argument(s), got {found}"), span)
}

/// Report an explicit bracket list whose length differs from the callee's type parameter count.
///
/// RFC 054 makes an explicit list arity-complete, with `_` as the slot to infer, so a short list is completed with
/// `_` in the hint rather than described as an unsupported partial application. `written` holds the type arguments
/// as the call spelled them, in order. See #1373.
pub fn explicit_type_arg_arity(name: &str, type_params: &[String], written: &[String], span: Span) -> CompileError {
    let expected = type_params.len();
    let found = written.len();
    let error = CompileError::type_error(
        format!("{name} expects {expected} explicit type argument(s), got {found}"),
        span,
    );
    if expected == 0 {
        return error.with_hint(format!(
            "'{name}' declares no type parameters; remove the type argument list"
        ));
    }
    let error = error.with_note(format!(
        "'{name}' declares type parameters [{}]; an explicit list binds every one of them in that order",
        type_params.join(", ")
    ));
    if found < expected {
        let completed = written
            .iter()
            .map(String::as_str)
            .chain(std::iter::repeat_n("_", expected - found))
            .collect::<Vec<_>>()
            .join(", ");
        let missing = type_params[found..].join(", ");
        error.with_hint(format!(
            "Write `_` for a parameter the value arguments determine ({missing}): {name}[{completed}](...)"
        ))
    } else {
        error.with_hint(format!("Remove the extra type argument(s); '{name}' takes {expected}"))
    }
}

pub fn call_site_type_inference_unresolved(callee: &str, type_param: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Could not infer type parameter '{type_param}' for '{callee}' from value arguments; replace `_` with an explicit type or adjust the call"
        ),
        span,
    )
}

pub fn explicit_call_site_type_args_not_supported(span: Span) -> CompileError {
    CompileError::type_error(
        "Explicit call-site type arguments are not supported for this call form".to_string(),
        span,
    )
}

pub fn builtin_expects_list(name: &str, found: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("{name}() expects a list, got {}", found), span)
}

/// Report a direct `zip(left, right)` operand that cannot be adapted to the source-owned iterator protocol.
pub fn builtin_zip_argument_not_supported(position: usize, found: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("zip() argument {position} must be a list, FrozenList, or Iterator, got {found}"),
        span,
    )
}

pub fn builtin_list_element_type_not_supported(name: &str, found: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("{name}() does not support list element type {}", found), span)
}

pub fn builtin_bool_type_not_supported(found: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("bool() does not support type {}", found), span)
}
