//! Type-system and semantic errors.
//!
//! Diagnostics raised during type checking: mismatches, missing implementations, trait conformance,
//! field/alias validation, mutability, and pattern matching.

use crate::ast::Span;
use incan_core::lang::derives::{self, DeriveId};

use crate::diagnostics::CompileError;

// -- Symbol resolution -------------------------------------------------------

pub fn unknown_symbol(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Unknown symbol '{}'", name), span)
        .with_hint("Did you forget to import it or define it?")
}

pub fn duplicate_definition(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Duplicate definition of '{}'", name), span)
}

/// Report a value enum declaration that attempts to use type parameters.
pub fn value_enum_type_params_not_supported(enum_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Value enum '{}' cannot declare type parameters", enum_name),
        span,
    )
    .with_hint("Value enums are concrete; remove the type parameters")
}

/// Report a value enum variant without its required raw literal value.
pub fn value_enum_variant_missing_value(enum_name: &str, variant_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Value enum variant '{}.{}' must have an explicit literal value",
            enum_name, variant_name
        ),
        span,
    )
    .with_hint(format!("Assign a value, for example: {variant_name} = ..."))
}

/// Report a value enum variant that carries tuple or struct payload fields.
pub fn value_enum_variant_payload_not_allowed(enum_name: &str, variant_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Value enum variant '{}.{}' cannot carry tuple or struct payloads",
            enum_name, variant_name
        ),
        span,
    )
    .with_hint("Value enum variants are simple value variants only")
}

/// Report a value enum raw literal whose kind does not match the enum backing type.
pub fn value_enum_literal_type_mismatch(enum_name: &str, expected: &str, found: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Value enum '{}' expects '{}' literal values, found '{}'",
            enum_name, expected, found
        ),
        span,
    )
    .with_hint(format!("Use a {expected} literal for each variant value"))
}

/// Report two value enum variants that declare the same raw value.
pub fn value_enum_duplicate_value(
    enum_name: &str,
    value: &str,
    first_variant: &str,
    second_variant: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!(
            "Duplicate value enum value {} in '{}' for variants '{}' and '{}'",
            value, enum_name, first_variant, second_variant
        ),
        span,
    )
    .with_hint("Each value enum raw value must be unique")
}

/// Report a value enum variant name that conflicts with generated helper names.
pub fn value_enum_reserved_generated_name(enum_name: &str, name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Value enum '{}' cannot use generated member name '{}' as a variant",
            enum_name, name
        ),
        span,
    )
    .with_hint("Rename the variant; 'value' and 'from_value' are reserved for generated helpers")
}

/// Report an assigned raw value on an ordinary non-value enum variant.
pub fn regular_enum_variant_value_not_allowed(enum_name: &str, variant_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Regular enum variant '{}.{}' cannot assign a raw value",
            enum_name, variant_name
        ),
        span,
    )
    .with_hint("Use `enum Name(str):` or `enum Name(int):` for value enums")
}

pub fn duplicate_call_argument(callee: &str, name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Duplicate argument '{name}' when calling '{callee}'"), span)
        .with_hint("Pass each fixed parameter at most once")
}

pub fn unknown_keyword_argument(callee: &str, name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Unexpected keyword argument '{name}' when calling '{callee}'"),
        span,
    )
    .with_hint("Add a `**kwargs` rest parameter to capture arbitrary keyword arguments")
}

pub fn call_unpack_without_rest(callee: &str, unpack: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Cannot use `{unpack}` unpacking when calling '{callee}' because the callee has no matching rest parameter"
        ),
        span,
    )
}

pub fn missing_required_argument(callee: &str, name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Missing required argument '{name}' when calling '{callee}'"),
        span,
    )
}

pub fn duplicate_rest_parameter(kind: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Only one `{kind}` rest parameter is allowed in a callable signature"),
        span,
    )
}

pub fn invalid_rest_parameter_order(message: &str, span: Span) -> CompileError {
    CompileError::type_error(message.to_string(), span)
}

pub fn rest_parameter_default_not_allowed(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Rest parameter '{name}' cannot declare a default value"), span)
}

// -- Decorators & namespaces -------------------------------------------------

pub fn unknown_decorator(path: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Unknown decorator '@{}'", path), span)
        .with_hint("Decorators must resolve to stdlib paths like @std.web.route")
        .with_hint("Import the module alias or use a fully qualified decorator path")
}

pub fn reserved_root_namespace(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("'{}' is a reserved root namespace", name), span)
        .with_hint("Choose a different name (reserved: std, rust)")
}

pub fn rust_allow_requires_positional_string(span: Span) -> CompileError {
    CompileError::type_error(
        "@rust.allow requires one or more positional string literal arguments".to_string(),
        span,
    )
    .with_hint("Example: @rust.allow(\"dead_code\", \"clippy::too_many_arguments\")")
}

pub fn rust_allow_rejects_named_args(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("@rust.allow does not accept named argument '{}'", name), span)
        .with_hint("Pass lint names as positional string literals")
}

pub fn rust_allow_invalid_lint_name(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Invalid Rust lint name '{}'", name), span)
        .with_hint("Use a Rust lint path like \"dead_code\" or \"clippy::too_many_arguments\"")
}

pub fn rust_allow_duplicate_lint(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Duplicate Rust lint '{}' in @rust.allow", name), span)
        .with_hint("Each @rust.allow invocation may list a lint only once")
}

pub fn rust_allow_broad_lint_group(name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Broad Rust lint group '{}' is not allowed in @rust.allow", name),
        span,
    )
    .with_hint("Suppress only specific rustc or Clippy lints")
}

pub fn rust_allow_unsupported_attachment(kind: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("@rust.allow cannot be used on {kind} declarations"), span)
        .with_hint("@rust.allow is supported on functions, methods, models, classes, enums, and newtypes")
}

// -- Type mismatches ---------------------------------------------------------

pub fn type_mismatch(expected: &str, found: &str, span: Span) -> CompileError {
    let mut error = CompileError::type_error(
        format!("Type mismatch: expected '{}', found '{}'", expected, found),
        span,
    );
    error = add_type_mismatch_hints(error, expected, found);
    error
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

// -- Functions & error handling ----------------------------------------------

/// Type error for using a **generic** function name in **value** position.
///
/// RFC 035 only supports monomorphically usable function references; passing `def id[T](...)` by name requires
/// inference/monomorphisation that is intentionally deferred. Users can wrap the call in a closure
/// (e.g. `(x) => id(x)`) at the use site.
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

// -- Mutability --------------------------------------------------------------

pub fn mutation_without_mut(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Cannot mutate '{}' - variable is immutable", name), span)
        .with_hint(format!("Declare with 'mut' to allow mutation: mut {} = ...", name))
        .with_note("In Incan, variables are immutable by default for safety")
        .with_note("This prevents accidental modifications and makes code easier to reason about")
}

pub fn self_mutation_without_mut(span: Span) -> CompileError {
    CompileError::type_error("Cannot mutate self - method takes immutable self".to_string(), span)
        .with_hint("Change the method signature to use 'mut self':")
        .with_hint("  def method(mut self) -> ReturnType:")
        .with_note("Methods that modify self must explicitly declare 'mut self'")
}

pub fn reassignment_without_mut(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Cannot reassign '{}' - variable is immutable", name), span)
        .with_hint(format!("Declare with 'mut' to allow reassignment: mut {} = ...", name))
        .with_hint("Or use a new variable name with 'let'")
        .with_note("Reassignment requires the variable to be declared as mutable")
}

pub fn static_initializer_requires_earlier_static(name: &str, current: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Static '{}' cannot reference '{}' before it is initialized",
            current, name
        ),
        span,
    )
    .with_hint("Module statics may only reference earlier statics in declaration order")
}

pub fn static_dependency_cycle(cycle: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Static dependency cycle detected: {}", cycle), span)
}

pub fn static_initializer_static_write_not_allowed(current: &str, target: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Static initializer for '{}' cannot assign to static '{}'",
            current, target
        ),
        span,
    )
    .with_hint("Static initializers may read earlier statics, but must not assign to any static")
}

pub fn imported_static_reassignment_not_allowed(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Cannot reassign imported static '{}'", name), span)
        .with_hint("Imported statics expose shared storage; mutate their contents instead of rebinding the name")
}

pub fn const_reassignment_suggests_static(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Cannot reassign const '{}'", name), span).with_hint(format!(
        "If this value needs module-owned mutable storage, declare it as `static {}: Type = ...`",
        name
    ))
}

// -- Match exhaustiveness ----------------------------------------------------

pub fn non_exhaustive_match(missing: &[String], span: Span) -> CompileError {
    let missing_str = missing.join(", ");
    CompileError::type_error(
        format!("Non-exhaustive match: missing patterns for {}", missing_str),
        span,
    )
    .with_hint("Add the missing cases or use '_' as a wildcard (use wildcards sparingly)")
}

// -- Traits ------------------------------------------------------------------

/// Emitted when a trait's `with` supertrait graph contains a directed cycle (RFC 042).
pub fn supertrait_cycle(cycle: &[String], span: Span) -> CompileError {
    let path = cycle.join(" → ");
    let message = if cycle.len() == 1 {
        format!(
            "Supertrait cycle: trait '{}' declares itself in its `with` clause",
            cycle[0]
        )
    } else if cycle.is_empty() {
        "Supertrait cycle detected in trait hierarchy".to_string()
    } else {
        format!(
            "Supertrait cycle: {} → {}",
            path,
            cycle.first().map(String::as_str).unwrap_or("?")
        )
    };
    CompileError::type_error(message, span)
        .with_note("Break the cycle by removing or rearranging `with` clauses on these traits.")
}

/// Emitted when a supertrait bound names a type that is not a trait (RFC 042).
pub fn supertrait_bound_not_trait(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Supertrait bound '{}' is not a trait", name), span)
        .with_hint("Only trait names may appear in a trait's `with` clause")
}

/// Emitted when a supertrait bound supplies the wrong number of generic arguments.
pub fn supertrait_bound_arity_mismatch(name: &str, expected: usize, found: usize, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Supertrait bound '{}' expects {} type argument(s), found {}",
            name, expected, found
        ),
        span,
    )
    .with_hint("Match the supertrait generic arity in the `with` clause")
}

/// Emitted when a model/class trait adoption bound supplies the wrong number of generic arguments.
pub fn trait_adoption_bound_arity_mismatch(name: &str, expected: usize, found: usize, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Trait adoption '{}' expects {} type argument(s), found {}",
            name, expected, found
        ),
        span,
    )
    .with_hint("Match the trait generic arity in the model or class `with` clause")
}

/// Emitted when a supertrait bound is not a simple trait name or generic trait instantiation.
pub fn supertrait_bound_invalid(span: Span) -> CompileError {
    CompileError::type_error(
        "Supertrait bound must be a trait name or a generic trait instantiation (e.g. `DataSet[T]`)".to_string(),
        span,
    )
}

/// Emitted when a trait is used like a concrete type in a constructor call (RFC 042: traits are abstract).
pub fn cannot_instantiate_trait(trait_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Cannot construct trait '{}' — traits are abstract", trait_name),
        span,
    )
    .with_hint("Implement the trait on a model or class and construct that concrete type instead")
    .with_note("Trait names may only appear in type annotations and `with` adoption clauses")
}

/// Emitted when a local binding annotation names a trait type that has no local value representation yet.
pub fn trait_typed_local_annotation_unsupported(annotation: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Trait-typed local annotation '{}' is not supported", annotation),
        span,
    )
    .with_hint("Use the concrete implementing type for the local binding")
    .with_note("Trait annotations are currently supported on callable boundaries and `with` adoption clauses")
}

/// Emitted when two supertraits require the same field with incompatible types (RFC 042).
pub fn supertrait_requires_conflict(
    trait_name: &str,
    field: &str,
    existing: &str,
    other: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!(
            "Trait '{}' merges conflicting @requires for field '{}' (types '{}' vs '{}')",
            trait_name, field, existing, other
        ),
        span,
    )
    .with_hint("Adjust supertrait `@requires` types or the declaring trait's `with` clause so the field types agree")
}

/// Emitted when two independent supertraits declare the same method name with compatible-but-distinct signatures and
/// neither the adopting trait nor the concrete type resolves the ambiguity (RFC 042).
pub fn supertrait_method_ambiguity(
    adopted_trait: &str,
    method: &str,
    via_a: &str,
    via_b: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!(
            "Ambiguous trait method '{}' when adopting '{}' — supertraits '{}' and '{}' disagree",
            method, adopted_trait, via_a, via_b
        ),
        span,
    )
    .with_hint(format!(
        "Declare `def {method}(self, ...)` on '{}' or on the concrete type to disambiguate",
        adopted_trait
    ))
}

pub fn trait_conflict(trait_a: &str, trait_b: &str, method: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Conflicting implementations: both {} and {} define method '{}'",
            trait_a, trait_b, method
        ),
        span,
    )
    .with_hint(format!(
        "Resolve the conflict explicitly: {}.{}(self, ...)",
        trait_a, method
    ))
}

/// Report an identical generic trait adoption appearing more than once on one type.
pub fn duplicate_trait_instantiation(trait_name: &str, type_args: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Trait '{}' is adopted more than once with type arguments [{}]",
            trait_name, type_args
        ),
        span,
    )
    .with_hint("Remove the duplicate `with` entry or use a different trait type argument")
}

/// Report a same-name method requirement coming from unrelated adopted traits.
pub fn cross_trait_method_collision(trait_a: &str, trait_b: &str, method: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Ambiguous trait method '{}' from unrelated traits '{}' and '{}'",
            method, trait_a, trait_b
        ),
        span,
    )
    .with_hint("Use distinct method names for now; qualified trait-method calls are future work")
}

/// Report duplicate concrete methods that cannot be assigned to distinct same-family trait obligations.
pub fn duplicate_method_not_trait_backed(type_name: &str, method: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Type '{}' declares multiple '{}' methods that do not map to adopted trait instantiations",
            type_name, method
        ),
        span,
    )
    .with_hint(
        "Duplicate method names are only allowed when each implementation satisfies a same-family trait instantiation",
    )
}

/// Report a method call where arguments and expected type still leave multiple trait-backed overloads.
pub fn ambiguous_trait_method_call(method: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Ambiguous trait method call '{}'", method), span)
        .with_hint("Add an expected result type or make the call arguments select one trait instantiation")
}

pub fn missing_trait_method(trait_name: &str, method: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Trait '{}' requires method '{}' to be implemented", trait_name, method),
        span,
    )
    .with_hint(format!(
        "Add the required method: def {}(self, ...) -> ReturnType:",
        method
    ))
    .with_note("All required trait methods must be implemented")
}

/// Report a concrete type that has not implemented a required trait property.
pub fn missing_trait_property(trait_name: &str, property: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Trait '{}' requires property '{}' to be implemented",
            trait_name, property
        ),
        span,
    )
    .with_hint(format!(
        "Add the required property: property {} -> ReturnType:",
        property
    ))
    .with_note("All required trait properties must be implemented")
}

/// Report a body on a trait property requirement.
pub fn trait_property_body_not_supported(trait_name: &str, property: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Trait '{}' property '{}' cannot define a body", trait_name, property),
        span,
    )
    .with_hint(
        "Declare the abstract requirement as `property name -> Type` and provide the body in each implementation",
    )
}

pub fn trait_method_signature_mismatch(
    trait_name: &str,
    type_name: &str,
    method: &str,
    expected_sig: &str,
    found_sig: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!(
            "Trait '{}' requires '{}'::{} to match its signature",
            trait_name, type_name, method
        ),
        span,
    )
    .with_note(format!("Expected: {expected_sig}"))
    .with_note(format!("Found:    {found_sig}"))
    .with_hint("Update the method signature to match the trait requirement")
}

/// Report a computed property whose return type does not match the adopted trait requirement.
pub fn trait_property_signature_mismatch(
    trait_name: &str,
    type_name: &str,
    property: &str,
    expected: &str,
    found: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!(
            "Trait '{}' requires '{}'::{} to match its property type",
            trait_name, type_name, property
        ),
        span,
    )
    .with_note(format!("Expected: property {property} -> {expected}"))
    .with_note(format!("Found:    property {property} -> {found}"))
    .with_hint("Update the property return type to match the trait requirement")
}

/// Report incompatible same-name property requirements from two adopted traits.
pub fn trait_property_conflict(trait_a: &str, trait_b: &str, property: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Conflicting implementations: both {} and {} define property '{}'",
            trait_a, trait_b, property
        ),
        span,
    )
    .with_hint("Resolve the conflict by declaring a compatible property on the adopting trait or concrete type")
}

/// Report an ambiguous property requirement inherited through multiple supertraits.
pub fn supertrait_property_ambiguity(
    adopted_trait: &str,
    property: &str,
    via_a: &str,
    via_b: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!(
            "Ambiguous trait property '{}' when adopting '{}' — supertraits '{}' and '{}' disagree",
            property, adopted_trait, via_a, via_b
        ),
        span,
    )
    .with_hint(format!(
        "Declare `property {property} -> Type:` on '{}' or on the concrete type to disambiguate",
        adopted_trait
    ))
}

pub fn trait_required_field_type_mismatch(
    trait_name: &str,
    type_name: &str,
    field: &str,
    expected: &str,
    found: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!(
            "Trait '{}' requires field '{}' on '{}' to have type '{}'",
            trait_name, field, type_name, expected
        ),
        span,
    )
    .with_note(format!("Found: '{found}'"))
    .with_hint(format!("Change '{field}' to type '{expected}'"))
}

pub fn duplicate_trait_requires_field(field: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Duplicate @requires entry for field '{}'", field), span).with_hint(format!(
        "Remove the duplicate or keep a single @requires({field}: Type) entry"
    ))
}

pub fn trait_requires_missing_field(trait_name: &str, field: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Trait '{}' does not declare required field '{}'", trait_name, field),
        span,
    )
    .with_hint(format!("Add @requires({field}: Type) to trait '{}'", trait_name))
    .with_note("Trait default methods may only access fields declared in @requires(...)")
}

pub fn trait_not_implemented(type_name: &str, trait_name: &str, span: Span) -> CompileError {
    let mut error = CompileError::type_error(
        format!("Type '{}' does not implement trait '{}'", type_name, trait_name),
        span,
    );

    if trait_name == "Error" {
        error = error.with_hint("Implement the Error trait with a message() method");
        error = error.with_hint("Example: def message(self) -> str: return self.msg");
        return error;
    }

    match derives::from_str(trait_name) {
        Some(DeriveId::Eq) | Some(DeriveId::PartialEq) => {
            error = error.with_hint("Add @derive(Eq) to enable equality comparison (==, !=)");
            error = error.with_hint("Or implement __eq__ manually for custom comparison logic");
        }
        Some(DeriveId::Ord) | Some(DeriveId::PartialOrd) => {
            error = error.with_hint("Add @derive(Ord) to enable ordering comparison (<, >, <=, >=)");
            error = error.with_hint("Or implement __lt__ manually for custom ordering");
        }
        Some(DeriveId::Hash) => {
            error = error.with_hint("Add @derive(Hash, Eq) to make this type hashable");
            error = error.with_note("Hash is required for Set membership and Dict keys");
        }
        Some(DeriveId::Clone) => {
            error = error.with_hint("Add @derive(Clone) to enable .clone() method");
        }
        Some(DeriveId::Copy) => {
            error = error.with_hint("Add @derive(Copy) to allow implicit copying for simple value types");
        }
        Some(DeriveId::Debug) => {
            error = error.with_hint("Add @derive(Debug) to enable {:?} formatting");
        }
        Some(DeriveId::Display) => {
            error = error.with_hint("Implement __str__ method for string representation");
            error = error.with_hint("Example: def __str__(self) -> str: return f\"{self.name}\"");
        }
        Some(DeriveId::Default) => {
            error = error.with_hint("Add @derive(Default) to enable Type.default()");
        }
        Some(DeriveId::Serialize) | Some(DeriveId::Deserialize) => {
            error = error.with_hint(format!("Add @derive({}) for JSON/serialization support", trait_name));
        }
        Some(DeriveId::Validate) => {
            error = error.with_hint("Add @derive(Validate) to enable validated construction via TypeName.new(...)");
            error = error.with_hint("Then implement: def validate(self) -> Result[Self, E]: ...");
        }
        None => {
            error = error.with_hint(format!(
                "Implement the {} trait or add 'with {}'",
                trait_name, trait_name
            ));
        }
    }

    error
}

pub fn generic_bound_not_satisfied(
    function_name: &str,
    type_param: &str,
    bound: &str,
    actual: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!(
            "Call to '{}' violates generic bound: type parameter '{}' requires '{}' but got '{}'",
            function_name, type_param, bound, actual
        ),
        span,
    )
    .with_hint(format!(
        "Ensure the argument type implements '{}' or widen '{}' bounds in '{}'",
        bound, type_param, function_name
    ))
}

pub fn cannot_compare(type_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Cannot compare values of type '{}' - Eq trait not implemented",
            type_name
        ),
        span,
    )
    .with_hint("Add @derive(Eq) to the type definition to enable comparison")
    .with_note("Comparison operators (==, !=) require the Eq trait")
}

pub fn cannot_order(type_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Cannot order values of type '{}' - Ord trait not implemented",
            type_name
        ),
        span,
    )
    .with_hint("Add @derive(Ord) to the type definition to enable ordering")
    .with_note("Ordering operators (<, >, <=, >=) require the Ord trait")
}

pub fn not_hashable(type_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Type '{}' cannot be used in Set or as Dict key - Hash trait not implemented",
            type_name
        ),
        span,
    )
    .with_hint("Add @derive(Hash, Eq) to make this type hashable")
    .with_note("Both Hash and Eq are required for Set membership and Dict keys")
}

// -- Validate derive ---------------------------------------------------------

pub fn validate_derive_missing_validate_method(type_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "@derive(Validate) requires '{}' to define method 'validate(self) -> Result[Self, E]'",
            type_name
        ),
        span,
    )
    .with_hint("Add: def validate(self) -> Result[Self, E]: ...")
    .with_note("Validated models must define a validation hook")
}

pub fn validate_derive_invalid_validate_signature(
    type_name: &str,
    expected: &str,
    found: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!(
            "@derive(Validate) requires '{}'::validate to have a specific signature",
            type_name
        ),
        span,
    )
    .with_note(format!("Expected: {expected}"))
    .with_note(format!("Found:    {found}"))
}

pub fn validate_derive_disallows_raw_construction(type_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Direct construction '{}'(...) is not allowed for @derive(Validate) models",
            type_name
        ),
        span,
    )
    .with_hint(format!("Use '{}.new(...)' instead", type_name))
    .with_note("This model opts into validated construction")
}

// -- Fields & aliases (RFC 021) ----------------------------------------------

pub fn missing_field(type_name: &str, field: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Type '{}' has no field '{}'", type_name, field), span)
}

/// Report access to a class field that is not visible from the current member-access context.
pub fn private_field(type_name: &str, field: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Field '{field}' on '{type_name}' is private"), span)
        .with_hint("Access this field from a method on the declaring class, or mark the field `pub`")
}

/// Report access to a class computed property that is not visible from the current member-access context.
pub fn private_property(type_name: &str, property: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Property '{property}' on '{type_name}' is private"), span)
        .with_hint("Access this property from a method on the declaring class, or mark the property `pub`")
}

/// Report a computed property selected with method-call syntax.
pub fn property_called_as_method(property: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Computed property '{}' is not callable", property), span)
        .with_hint(format!("Use `.{property}` without parentheses"))
        .with_note("Computed properties are read with field-like syntax")
}

pub fn missing_method(type_name: &str, method: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Type '{}' has no method '{}(...)'", type_name, method), span)
        .with_hint("Check the method name spelling and receiver type")
        .with_hint("If this is your type, implement the method on the class/model/newtype")
}

/// The imported Rust item shape is known but currently unsupported in this language surface.
pub fn rust_item_shape_not_supported(path: &str, description: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Rust item `rust::{path}` has unsupported shape `{description}` for this operation"),
        span,
    )
    .with_hint("Import a module, type, function, or constant instead")
    .with_note("RFC 041 intentionally limits which Rust item shapes are typechecked directly")
}

/// `type X = rusttype Y` requires `Y` to resolve to a Rust-origin imported item.
pub fn rusttype_requires_rust_backing(type_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("`{type_name}` is declared as `rusttype`, but its backing type is not a resolved `rust::...` import"),
        span,
    )
    .with_hint("Import a concrete Rust item, e.g. `from rust::crate import TypeName`")
}

/// `interop:` blocks are only valid on `rusttype` declarations.
pub fn interop_block_requires_rusttype(type_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("`interop:` is only valid on `rusttype` declarations (found on `{type_name}`)"),
        span,
    )
    .with_hint("Use `type X = rusttype Y` when declaring host interop edges")
}

/// `interop:` adapters must be simple callable references (`name` or `Type.name`).
pub fn interop_adapter_ref_must_be_name_or_member(span: Span) -> CompileError {
    CompileError::type_error(
        "Interop adapter reference must be a callable name (`parse`) or member path (`Email.parse`)".to_string(),
        span,
    )
}

/// Qualified adapter refs must target the declaring rusttype surface.
pub fn interop_adapter_wrong_owner(type_name: &str, owner: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Interop adapter reference `{owner}.*` must use the declaring rusttype `{type_name}`"),
        span,
    )
    .with_hint(format!("Use `{type_name}.adapter_name` or a short-form adapter name"))
}

/// Short-form adapter names must resolve to exactly one callable.
pub fn ambiguous_interop_adapter_short_name(
    type_name: &str,
    adapter_name: &str,
    candidate_count: usize,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!(
            "Ambiguous short-form interop adapter `{adapter_name}` on `{type_name}` ({candidate_count} candidates)"
        ),
        span,
    )
    .with_hint(format!(
        "Use a qualified adapter reference, e.g. `{type_name}.{adapter_name}`"
    ))
}

/// `from S ...` adapters must be associated/free callables, not receiver methods.
pub fn interop_from_adapter_requires_associated_callable(type_name: &str, adapter: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("`from` interop adapter `{adapter}` on `{type_name}` cannot require `self`"),
        span,
    )
    .with_hint("Use an associated function or free callable that accepts the source type")
}

/// Adapter arity must match interop direction semantics.
pub fn interop_adapter_arity_mismatch(
    type_name: &str,
    adapter: &str,
    expected: usize,
    found: usize,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!("Interop adapter `{adapter}` on `{type_name}` expects {expected} argument(s), got {found}"),
        span,
    )
}

/// Adapter input type must match the declared edge source.
pub fn interop_adapter_input_mismatch(
    type_name: &str,
    adapter: &str,
    expected: &str,
    found: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!(
            "Interop adapter `{adapter}` on `{type_name}` has incompatible input type: expected `{expected}`, found `{found}`"
        ),
        span,
    )
}

/// Adapter output type must match the declared edge target.
pub fn interop_adapter_output_mismatch(
    type_name: &str,
    adapter: &str,
    expected: &str,
    found: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!(
            "Interop adapter `{adapter}` on `{type_name}` has incompatible output type: expected `{expected}`, found `{found}`"
        ),
        span,
    )
}

/// `via` adapters are infallible and must not return `Result`/`Option`.
pub fn interop_via_adapter_must_be_infallible(
    type_name: &str,
    adapter: &str,
    found_return: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!(
            "`via` interop adapter `{adapter}` on `{type_name}` must be infallible (found return type `{found_return}`)"
        ),
        span,
    )
    .with_hint("Use `try` for fallible adapters that return `Result[...]` or `Option[...]`")
}

/// `try` adapters must return `Result` or `Option`.
pub fn interop_try_adapter_requires_result_or_option(
    type_name: &str,
    adapter: &str,
    found_return: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!(
            "`try` interop adapter `{adapter}` on `{type_name}` must return `Result[...]` or `Option[...]` (found `{found_return}`)"
        ),
        span,
    )
}

/// Duplicate edge declarations are not allowed.
pub fn duplicate_interop_edge(
    type_name: &str,
    direction: &str,
    edge_ty: &str,
    first_span: Span,
    second_span: Span,
) -> CompileError {
    CompileError::type_error(
        format!("Duplicate interop edge `{direction} {edge_ty}` on `{type_name}`"),
        second_span,
    )
    .with_note(format!("First declaration at span: {:?}", first_span))
}

/// Conflicting edge declarations for the same directed type are rejected.
pub fn conflicting_interop_edge(
    type_name: &str,
    direction: &str,
    edge_ty: &str,
    first_adapter: &str,
    second_adapter: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!("Conflicting interop edges `{direction} {edge_ty}` on `{type_name}`"),
        span,
    )
    .with_note(format!(
        "Adapters `{first_adapter}` and `{second_adapter}` both target the same edge"
    ))
    .with_hint("Keep one canonical adapter edge for each directed type")
}

pub fn duplicate_alias(type_name: &str, alias: &str, first_span: Span, second_span: Span) -> CompileError {
    CompileError::type_error(
        format!("Duplicate alias '{}' on type '{}'", alias, type_name),
        second_span,
    )
    .with_note(format!(
        "Alias '{}' is already used by another field on '{}'",
        alias, type_name
    ))
    .with_note(format!("First alias occurrence at span: {:?}", first_span))
}

pub fn alias_collides_with_canonical(type_name: &str, alias: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Alias '{}' collides with a canonical field name on '{}'",
            alias, type_name
        ),
        span,
    )
    .with_hint("Choose a distinct alias or rename the canonical field")
}

pub fn alias_collides_with_method(type_name: &str, alias: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Alias '{}' collides with a method name on '{}'", alias, type_name),
        span,
    )
    .with_hint("Choose a distinct alias to avoid ambiguous member access")
}

pub fn alias_collides_with_builtin(type_name: &str, alias: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Alias '{}' collides with a builtin member on '{}'", alias, type_name),
        span,
    )
    .with_hint("Choose a distinct alias to avoid builtin member collisions")
}

pub fn empty_alias(span: Span) -> CompileError {
    CompileError::type_error(
        "Alias must be a non-empty, non-whitespace string literal".to_string(),
        span,
    )
}

/// RFC 021: Field aliases are only supported on `model` declarations, not `class`.
pub fn alias_not_supported_on_class(class_name: &str, field_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Field alias not supported on class '{}' field '{}'",
            class_name, field_name
        ),
        span,
    )
    .with_hint("Field aliases are only supported on `model` declarations (RFC 021)")
}

/// RFC 021: Field descriptions are only supported on `model` declarations, not `class`.
pub fn description_not_supported_on_class(class_name: &str, field_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Field description not supported on class '{}' field '{}'",
            class_name, field_name
        ),
        span,
    )
    .with_hint("Field descriptions are only supported on `model` declarations (RFC 021)")
}

// -- Constructors & patterns -------------------------------------------------

pub fn duplicate_constructor_field(type_name: &str, field: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Duplicate constructor argument: field '{}' is provided more than once for type '{}'",
            field, type_name
        ),
        span,
    )
    .with_hint("Remove the duplicate argument so each field is provided at most once")
}

pub fn duplicate_field_in_call(type_name: &str, field: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Duplicate constructor argument: field '{}' is provided more than once for type '{}'",
            field, type_name
        ),
        span,
    )
    .with_hint("Provide each field at most once (canonical name or alias)")
}

pub fn missing_required_constructor_field(type_name: &str, field: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Missing required field '{}' when constructing '{}'", field, type_name),
        span,
    )
    .with_hint(format!("Provide the field: {}(..., {}=..., ...)", type_name, field))
    .with_note("Fields without defaults must be provided during construction")
}

pub fn positional_constructor_args_not_supported(type_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Positional constructor arguments are not supported for '{}' (use named field arguments)",
            type_name
        ),
        span,
    )
    .with_hint(format!("Use named arguments: {}(field=value, ...)", type_name))
}

pub fn positional_pattern_not_supported(type_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Positional patterns are not supported for '{}' (use named field patterns)",
            type_name
        ),
        span,
    )
    .with_hint(format!("Use named fields: {}(field=pattern, ...)", type_name))
}

pub fn named_pattern_not_supported(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Named pattern fields are not supported for '{}'", name), span)
        .with_hint("Use positional patterns for enum variants and builtins")
}

/// Report an alternation whose alternatives do not bind the same names.
pub fn pattern_alternation_binding_mismatch(expected: &[String], found: &[String], span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Pattern alternation binding mismatch: expected bindings [{}], found [{}]",
            expected.join(", "),
            found.join(", ")
        ),
        span,
    )
    .with_hint("Every alternative in a pattern alternation must bind the same names")
}

/// Report an alternation whose same-named binding resolves to different types across alternatives.
pub fn pattern_alternation_binding_type_mismatch(name: &str, expected: &str, found: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Pattern alternation binding '{}' has incompatible types: expected '{}', found '{}'",
            name, expected, found
        ),
        span,
    )
    .with_hint("Use separate branches when alternatives bind the same name with different types")
}

pub fn duplicate_pattern_field(type_name: &str, field: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Duplicate pattern field: '{}' is matched more than once for '{}'",
            field, type_name
        ),
        span,
    )
    .with_hint("Remove the duplicate field from the pattern")
}

/// Constructor pattern in a `match` could not be tied to a known variant or payload typing (for example a typo in an
/// Incan enum variant name).
pub fn unknown_match_constructor_pattern(pattern: &str, subject_ty_display: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Constructor pattern '{}' does not resolve for this match (scrutinee type: {})",
            pattern, subject_ty_display
        ),
        span,
    )
    .with_hint(
        "Check the variant name and payload. For Rust-backed enums, prefer a `rusttype` wrapper or a catch-all arm (`_`).",
    )
}

// -- Indexing & collections --------------------------------------------------

pub fn not_indexable(type_name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Type '{}' is not indexable", type_name), span)
        .with_hint("Only List, Dict, str, and Tuple types support indexing")
}

pub fn tuple_index_requires_int_literal(span: Span) -> CompileError {
    CompileError::type_error(
        "Tuple indices must be an integer literal (e.g. t[0], t[-1])".to_string(),
        span,
    )
    .with_hint("Use a literal index so the compiler can validate bounds")
}

pub fn tuple_index_out_of_bounds(idx: i64, len: usize, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Tuple index {} is out of bounds for tuple of length {}", idx, len),
        span,
    )
    .with_hint("Tuple indices are checked at compile time")
}

pub fn index_type_mismatch(expected: &str, found: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Index type mismatch: expected '{}', found '{}'", expected, found),
        span,
    )
    .with_hint(format!("Use '{}' as the index type", expected))
}

pub fn index_value_type_mismatch(expected: &str, found: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Cannot assign '{}' to collection element of type '{}'", found, expected),
        span,
    )
    .with_hint(format!(
        "Collection elements are of type '{}', but got '{}'",
        expected, found
    ))
}

pub fn list_append_requires_clone(elem_type: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("List.append requires element type '{}' to be Clone", elem_type),
        span,
    )
    .with_note("List.append clones non-Copy values before pushing")
    .with_hint("Add @derive(Clone) to the element type or append a Copy type")
}

pub fn list_concat_requires_clone(elem_type: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("List concatenation requires element type '{}' to be Clone", elem_type),
        span,
    )
    .with_note("List + list preserves both source lists, so elements are cloned into the new list")
    .with_hint("Add @derive(Clone) to the element type or concatenate a list of Clone elements")
}

pub fn list_extend_requires_clone(elem_type: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("List.extend requires element type '{}' to be Clone", elem_type),
        span,
    )
    .with_note("List.extend preserves the source list, so elements are cloned into the receiver")
    .with_hint("Add @derive(Clone) to the element type or extend from a list of Clone elements")
}

pub fn string_index_assignment_not_allowed(span: Span) -> CompileError {
    CompileError::type_error("Strings are immutable - cannot assign to index".to_string(), span)
}

// -- Tuples ------------------------------------------------------------------

pub fn mutable_tuple(span: Span) -> CompileError {
    CompileError::type_error(
        "Tuples are immutable and cannot be declared with 'mut'".to_string(),
        span,
    )
    .with_hint("Remove 'mut' - tuples cannot be modified after creation")
}

pub fn tuple_field_assignment(span: Span) -> CompileError {
    CompileError::type_error("Cannot assign to tuple field - tuples are immutable".to_string(), span)
        .with_hint("Create a new tuple instead of modifying an existing one")
}

pub fn tuple_unpack_count_mismatch(expected: usize, found: usize, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Cannot unpack {} values from tuple with {} elements", expected, found),
        span,
    )
}
