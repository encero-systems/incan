//! Type mismatch, derive, and function and error-handling errors.

use super::*;

// -- Type mismatches ---------------------------------------------------------

/// Build the generic expected-versus-actual type mismatch diagnostic.
pub fn type_mismatch(expected: &str, found: &str, span: Span) -> CompileError {
    let mut error = CompileError::type_error(
        format!("Type mismatch: expected '{}', found '{}'", expected, found),
        span,
    )
    .with_expected_actual(expected, found);
    error = add_type_mismatch_hints(error, expected, found);
    error
}

/// Build the diagnostic for arithmetic over two different integer types (RFC 009).
///
/// Same-type integer arithmetic keeps its type, so an operation whose operands have two integer types (`i8 + i16`, or
/// `i8 + n` for an `int` value `n`) has no result type until one operand is converted to the other's type.
pub fn mixed_width_integer_arithmetic(left: &str, op: &str, right: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Mixed-width integer arithmetic: '{left} {op} {right}' has operands of two integer types"),
        span,
    )
    .with_note("Integer arithmetic takes two operands of one integer type and yields that type")
    .with_hint(
        "Convert one operand to the other operand's type first, with 'resize()' where the conversion is lossless or \
         'try_resize()' where the value may not fit",
    )
}

/// Build the diagnostic for comparing two integer types that no one integer type holds both values of (RFC 009).
///
/// Values of two integer types compare in the narrowest integer type both widen to without loss; `u128` beside a
/// signed type, and `isize` or `usize` beside another integer type, have none.
pub fn incomparable_integer_types(left: &str, op: &str, right: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Cannot compare '{left} {op} {right}': no integer type holds every value of both"),
        span,
    )
    .with_note("Two integer types compare in the narrowest integer type both widen to without loss")
    .with_hint("Convert one operand to the other operand's type first, with 'try_resize()'")
}

/// Build a type mismatch diagnostic for a return expression.
pub fn return_type_mismatch(expected: &str, found: &str, span: Span) -> CompileError {
    add_type_mismatch_hints(
        CompileError::type_error(
            format!("Return type mismatch: expected '{}', found '{}'", expected, found),
            span,
        )
        .with_expected_actual(expected, found)
        .with_note(format!(
            "This return expression must match the function return type '{}'",
            expected
        )),
        expected,
        found,
    )
}

/// Build a type mismatch diagnostic for assigning to a known binding.
pub fn assignment_type_mismatch(binding: &str, expected: &str, found: &str, span: Span) -> CompileError {
    add_type_mismatch_hints(
        CompileError::type_error(
            format!(
                "Assignment to '{}' has type mismatch: expected '{}', found '{}'",
                binding, expected, found
            ),
            span,
        )
        .with_expected_actual(expected, found)
        .with_note(format!("'{}' is declared as '{}'", binding, expected)),
        expected,
        found,
    )
}

/// Build a type mismatch diagnostic for a function call argument.
pub fn call_argument_type_mismatch(
    callee: &str,
    parameter: Option<&str>,
    expected: &str,
    found: &str,
    span: Span,
) -> CompileError {
    let message = match parameter {
        Some(parameter) => format!(
            "Argument '{}' of '{}' has type mismatch: expected '{}', found '{}'",
            parameter, callee, expected, found
        ),
        None => format!(
            "Argument to '{}' has type mismatch: expected '{}', found '{}'",
            callee, expected, found
        ),
    };
    let note = match parameter {
        Some(parameter) => format!(
            "Parameter '{}' of '{}' is declared as '{}'",
            parameter, callee, expected
        ),
        None => format!(
            "This argument is checked against '{}' when calling '{}'",
            expected, callee
        ),
    };
    add_type_mismatch_hints(
        CompileError::type_error(message, span)
            .with_expected_actual(expected, found)
            .with_note(note),
        expected,
        found,
    )
}

/// Add smart hints based on the expected and found types.
fn add_type_mismatch_hints(mut error: CompileError, expected: &str, found: &str) -> CompileError {
    // Result/Option unwrapping hints
    if expected.starts_with("Result[") && !found.starts_with("Result[") {
        error = error.with_hint("Wrap the value with Ok(...) to return success");
        error = error.with_hint("Or use Err(...) to return an error");
        error = error.with_note("In Incan, functions that can fail return Result[T, E]");
    }

    if found.starts_with("Result[") && !expected.starts_with("Result[") {
        error = error.with_hint("Use the ? operator to unwrap: value?");
        error = error.with_hint("Or handle with match: match result: Ok(v) => ..., Err(e) => ...");
        error = error.with_note("Result must be explicitly unwrapped before use");
    }

    if expected.starts_with("Option[") && !found.starts_with("Option[") && found != "None" {
        error = error.with_hint("Wrap the value with Some(...) to make it optional");
    }

    if found.starts_with("Option[") && !expected.starts_with("Option[") {
        error = error.with_hint("Use .unwrap() if you're certain the value exists");
        error = error.with_hint("Or handle None with match: match opt: Some(v) => ..., None => ...");
    }

    if found == "None" && !expected.contains("Option") && expected != "None" {
        error = error.with_hint("None can only be used where Option[T] is expected");
    }

    // Numeric type hints
    if (expected == "int" && found == "float") || (expected == "float" && found == "int") {
        error = error.with_hint(format!(
            "Use explicit conversion: {}(...)",
            if expected == "int" { "int" } else { "float" }
        ));
    }

    if expected == "str" && found != "str" {
        error = error.with_hint("Use f-string or str() to convert to string");
    }

    // Decimal shape hints: a decimal is assignable only where no digit is lost (#1809)
    if let (Some((_, expected_precision, expected_scale)), Some((found_constructor, found_precision, found_scale))) =
        (rendered_decimal_shape(expected), rendered_decimal_shape(found))
    {
        let found_integer_digits = found_precision.saturating_sub(found_scale);
        let expected_integer_digits = expected_precision.saturating_sub(expected_scale);
        error = error.with_note(format!(
            "'{found}' holds up to {found_integer_digits} digit(s) before the point and {found_scale} after it; '{expected}' keeps {expected_integer_digits} before the point and {expected_scale} after it"
        ));
        error = error.with_hint(format!(
            "A decimal is assignable only to a decimal type that keeps at least its digits before and after the point, and the language defines no decimal rounding or resize: declare the target as '{found}', or as any {found_constructor}[p, s] with p - s >= {found_integer_digits} and s >= {found_scale}"
        ));
    }

    // Bool condition hints
    if expected == "bool" {
        if found.starts_with("Option[") {
            error = error.with_hint("Use 'is Some' or 'is None' to check Option values");
            error = error.with_hint("Example: if value is Some(v): ...");
        } else if found.starts_with("Result[") {
            error = error.with_hint("Use 'is Ok' or 'is Err' to check Result values");
            error = error.with_hint("Example: if result is Ok(v): ...");
        } else if found == "int" || found == "float" || found == "str" {
            error = error.with_hint("Use explicit comparison instead of truthiness");
            error = error.with_hint(match found {
                "int" | "float" => "Example: if value != 0: ...",
                "str" => "Example: if value != \"\": ...",
                _ => "Example: if value != default: ...",
            });
            error = error.with_note("Incan prefers explicit checks over implicit truthiness");
        }
    }

    if expected.starts_with("List[") && found.starts_with("List[") {
        error = error.with_hint("List element types must match exactly");
    }

    error
}

/// Parse a rendered decimal type (`decimal[10, 2]`, `decimal128[10, 2]`) into its constructor, precision and scale.
fn rendered_decimal_shape(rendered: &str) -> Option<(&str, u8, u8)> {
    let (constructor, arguments) = rendered.split_once('[')?;
    let (precision, scale) = arguments.strip_suffix(']')?.split_once(',')?;
    incan_lang::lang::types::numerics::decimal_constructor_from_str(constructor)?;
    Some((constructor, precision.trim().parse().ok()?, scale.trim().parse().ok()?))
}

pub fn field_type_mismatch(field: &str, expected: &str, found: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Cannot assign '{}' to field '{}' of type '{}'", found, field, expected),
        span,
    )
    .with_hint(format!(
        "Field '{}' expects type '{}', but got '{}'",
        field, expected, found
    ))
}

// -- Derives -----------------------------------------------------------------

pub fn unknown_derive(name: &str, span: Span) -> CompileError {
    let valid_derives = derives::DERIVES
        .iter()
        .map(|d| d.canonical)
        .collect::<Vec<_>>()
        .join(", ");
    CompileError::type_error(format!("Unknown derive '{}'", name), span)
        .with_hint(format!("Valid derives: {valid_derives}"))
        .with_hint("Hint: Use 'with TraitName' syntax for custom trait implementations")
}

pub fn derive_wrong_kind(name: &str, kind: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Cannot derive '{}' - it is a {}, not a trait", name, kind),
        span,
    )
    .with_hint("@derive() only works with traits like Debug, Eq, Clone".to_string())
    .with_hint(format!("Did you mean: `with {}` to implement a trait?", name))
}

/// Report `@derive(module)` on an imported module that has no RFC 024 `__derives__` declaration.
pub fn derive_module_missing_derives(name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Cannot derive module '{}' - it does not declare `__derives__`", name),
        span,
    )
    .with_hint("Add module-level `__derives__ = [TraitName]` metadata or derive an imported trait directly")
}

// -- Functions & error handling ----------------------------------------------

/// Type error for using a **generic** function name in **value** position.
///
/// RFC 035 only supports monomorphically usable function references; passing `def id[T](...)` by name requires
/// inference/monomorphization that is intentionally deferred. Users can wrap the call in a closure (e.g. `(x) =>
/// id(x)`) at the use site.
pub fn generic_function_reference(name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Cannot use generic function '{}' as a value — \
             generic function references are not yet supported",
            name
        ),
        span,
    )
    .with_hint("Wrap in a closure that supplies explicit type arguments: (x) => my_func(x)")
    .with_note("Only monomorphic (non-generic) functions can be passed by name (RFC 035)")
}

/// Type error for a local partial of a generic callable that leaves one of the callable's type parameters unfixed.
///
/// A local partial expression is a value, and a value is not generic: the partial instantiates its target with the type
/// arguments its presets fix or that it writes (RFC 084, RFC 054). `target` is the target as the partial spells it.
pub fn local_partial_type_param_unfixed(target: &str, type_param: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Local partial of '{target}' leaves type parameter '{type_param}' unfixed; a local partial is not generic"
        ),
        span,
    )
    .with_hint(format!(
        "Preset a parameter whose type fixes '{type_param}', or write the type arguments: partial {target}[...](...)"
    ))
}

/// Type error for using a type-like name in value position.
pub fn type_name_used_as_value(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Cannot use type '{name}' as a value"), span)
        .with_hint("Use a `Type[...]` parameter for type-token APIs, or pass the type as `helper[...]`")
        .with_note("Type tokens are explicit API values, not general runtime type objects")
}

/// Type error for applying an ordinary runtime operation to a compile-time type token.
pub fn type_token_operation_not_supported(operation: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Type tokens do not support {operation}"), span)
        .with_note("A `Type[T]` value carries checked type evidence for dispatch; it is not a runtime type object")
}

/// Type error for declarations whose alias-normalized token parameter surfaces are identical.
pub fn duplicate_type_token_overload(name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Overload '{name}' duplicates an existing alias-normalized type-token signature"),
        span,
    )
    .with_hint("Use a newtype when two type tokens need distinct overload identity")
}

pub fn missing_return_type(span: Span) -> CompileError {
    CompileError::type_error("Function is missing a return type".to_string(), span)
        .with_hint("Add a return type annotation: def name(...) -> Type:")
}

pub fn incompatible_error_type(expected: &str, found: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Cannot use '?' here: function returns Result[_, {}] but expression has error type '{}'",
            expected, found
        ),
        span,
    )
    .with_hint("Use map_err to convert the error type, or add a From implementation")
}

/// Build the diagnostic for combining setup unwrapping and fallible polling in one loop header.
pub fn result_wrapped_fallible_iterator_requires_local(span: Span) -> CompileError {
    CompileError::type_error(
        "A fallible loop cannot unwrap a Result containing a fallible iterator in the same header".to_string(),
        span,
    )
    .with_note(
        "The outer Result represents a one-time setup failure; the iterator Result represents a failure from each poll",
    )
    .with_hint("Unwrap setup first with `stream = open_stream()?`, then use `for item in stream?:`")
}

/// Build the diagnostic for using `try` in a function that does not return `Result`.
pub fn try_without_result_return(span: Span) -> CompileError {
    CompileError::type_error(
        "Cannot use '?' here: the enclosing function does not return Result[_, E]".to_string(),
        span,
    )
    .with_note("The '?' operator unwraps Ok(value) or returns early with Err(error)")
    .with_hint("Change the enclosing function return type to Result[T, E], or handle the Result with match")
}

pub fn testing_marker_runtime_call_not_supported(name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("'{}' is a test marker decorator and cannot be called at runtime", name),
        span,
    )
    .with_hint(format!("Use '@{}(...)' on a test or fixture declaration instead", name))
    .with_note("Marker semantics are consumed by `incan test` during discovery")
}

/// `@fixture(timeout=...)` tried to configure per-fixture timeout behavior, which RFC 004 intentionally excludes.
pub fn fixture_timeout_config_not_supported(name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Fixture '{name}' cannot declare per-fixture timeout configuration"),
        span,
    )
    .with_hint("Use test-runner timeout configuration or a test-level timeout marker instead")
    .with_note("RFC 004 keeps async fixture declaration metadata limited to scope, autouse, and async/yield shape")
}

/// An `async def` fixture omitted the mandatory `yield value` boundary.
pub fn async_fixture_requires_yield(name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Async fixture '{name}' must contain exactly one top-level `yield value` expression"),
        span,
    )
    .with_hint("Yield the fixture value between awaited setup and awaited teardown")
    .with_note("Async fixture setup runs before the yield; teardown after the yield is awaited by the test runner")
}

/// An async fixture used a nested or repeated yield boundary.
pub fn async_fixture_invalid_yield_shape(name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Async fixture '{name}' must use exactly one top-level `yield value` expression"),
        span,
    )
    .with_hint("Place a single `yield value` statement in the fixture body")
    .with_note("Nested or repeated yields cannot define one deterministic setup/teardown boundary")
}

/// An async fixture used `yield` without a yielded value.
pub fn async_fixture_yield_requires_value(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Async fixture '{name}' must yield the fixture value"), span)
        .with_hint("Use `yield value` so dependents receive the fixture value")
}

/// A `yield` expression appeared outside a generator function or fixture declaration.
pub fn yield_outside_generator(span: Span) -> CompileError {
    CompileError::type_error(
        "`yield` is only valid in generator functions or fixtures".to_string(),
        span,
    )
    .with_hint("Declare the enclosing function as returning `Generator[T]`, or use a fixture declaration")
}

/// A function declared `Generator[T]` but did not contain a yield expression.
pub fn generator_requires_yield(name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Generator function '{name}' must contain at least one `yield value`"),
        span,
    )
    .with_hint("Add `yield value` for the declared `Generator[T]` element type")
}

/// A generator used bare `yield`, which cannot produce the declared element type.
pub fn generator_yield_requires_value(span: Span) -> CompileError {
    CompileError::type_error("Generator `yield` must include a value".to_string(), span)
        .with_hint("Use `yield value` so the generator can produce its declared element type")
}

/// A generator attempted to return a final value, which RFC 006 does not support.
pub fn generator_return_value_not_supported(span: Span) -> CompileError {
    CompileError::type_error("Generator functions cannot use `return value`".to_string(), span)
        .with_hint("Use bare `return` to terminate iteration early")
}

pub fn try_on_non_result(found: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Cannot use '?' on type '{}' - expected Result[T, E]", found),
        span,
    )
    .with_note("The '?' operator only works on Result types")
    .with_hint("The ? operator unwraps Ok(value) or returns early with Err(error)")
    .with_hint("Example: let user = get_user(id)?  # Returns Err if get_user fails")
    .with_note(if found.starts_with("Option[") {
        "For Option types, use .ok_or(error)? to convert to Result first"
    } else {
        "If this operation can fail, the function should return Result[T, E]"
    })
}

/// `await` appeared outside an `async def`, outside an async method, or inside a closure body (closures are never
/// async).
pub fn await_outside_async(span: Span) -> CompileError {
    CompileError::type_error(
        "Cannot use 'await' outside of an async function or async method".to_string(),
        span,
    )
    .with_note("'await' is only valid inside `async def` and async method bodies")
    .with_hint("Declare the enclosing function or method with the `async` keyword (after importing `std.async`)")
}

/// An async callable was invoked in a context where the returned future is not awaited directly.
pub fn async_call_without_await(callable: &str, span: Span) -> CompileError {
    CompileError::warning(format!("Async call `{callable}` is not awaited"), span)
        .with_note("Calling an async function or method produces work that should usually be awaited")
        .with_hint(format!(
            "Use `await {callable}(...)` when the result should be observed here"
        ))
}

/// `break` appeared outside any enclosing loop body.
pub fn break_outside_loop(span: Span) -> CompileError {
    CompileError::type_error("`break` is only valid inside loops".to_string(), span)
        .with_hint("Use `break` only inside `for`, `while`, or `loop:` bodies")
}

/// `break <value>` appeared inside a loop form that does not yield a value.
pub fn break_value_requires_loop_expression(span: Span) -> CompileError {
    CompileError::type_error(
        "`break <value>` is only valid inside `loop:` expressions".to_string(),
        span,
    )
    .with_hint("Use plain `break` for `for`, `while`, and statement-form `loop:`")
    .with_note("Only `loop:` expressions can produce a value for the surrounding expression")
}

/// `continue` appeared outside any enclosing loop body.
pub fn continue_outside_loop(span: Span) -> CompileError {
    CompileError::type_error("`continue` is only valid inside loops".to_string(), span)
        .with_hint("Use `continue` only inside `for`, `while`, or `loop:` bodies")
}

/// A `loop:` expression never produced a reachable `break`, so its result type cannot be determined yet.
pub fn loop_expression_requires_break(span: Span) -> CompileError {
    CompileError::type_error("loop expression must contain at least one `break`".to_string(), span)
        .with_hint("Add `break <expr>` to produce a value, or use statement-form `loop:` if the loop never returns")
        .with_note(
            "Incan does not define a bottom (`Never`) type yet, so non-terminating loop expressions are rejected",
        )
}
