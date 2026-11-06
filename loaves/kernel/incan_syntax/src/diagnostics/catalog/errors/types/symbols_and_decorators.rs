//! Symbol resolution, decorator and namespace, and web route handler errors.

use super::*;

// -- Symbol resolution -------------------------------------------------------

pub fn unknown_symbol(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Unknown symbol '{}'", name), span)
        .with_hint("Did you forget to import it or define it?")
}

/// Report a source name that starts with the compiler's reserved `__incan_` prefix (#1769).
///
/// The compiler spells the items and locals it generates with that prefix, the original a decorator wraps among them,
/// so a source declaration, binding or import alias with the same prefix could collide with a generated name and stop
/// the build on a duplicate definition. `name` is the spelling the source used and `kind` the kind of name it declares
/// (`function`, `parameter`, `import alias`, ...), which the message and hint repeat. The hint suggests the name
/// without the prefix when what remains starts like an identifier. `INCAN-T0111` is its stable code.
pub fn reserved_compiler_name(name: &str, kind: &str, span: Span) -> CompileError {
    let prefix = conventions::RESERVED_COMPILER_NAME_PREFIX;
    let suggestion = name
        .strip_prefix(prefix)
        .filter(|rest| rest.starts_with(|first: char| first.is_ascii_alphabetic() || first == '_'));
    let hint = match suggestion {
        Some(rest) => format!("Rename the {kind} so it does not start with '{prefix}', for example '{rest}'"),
        None => format!("Rename the {kind} so it does not start with '{prefix}'"),
    };
    CompileError::type_error(
        format!("The {kind} '{name}' starts with '{prefix}', a prefix reserved for names the compiler generates"),
        span,
    )
    .with_stable_code("INCAN-T0111")
    .with_hint(hint)
    .with_note("The compiler names the items and locals it generates with this prefix, so a source name spelled the same way could collide with one of them")
}

/// Report an unknown type name written in a callable's signature or body annotation.
///
/// The likeliest cause there is a type parameter that was never declared, not a missing import, so the hint shows
/// where the declaration goes on `owner` (already-declared parameters are kept, the unknown name is appended). The
/// import remedy stays as the second hint because the name can still be a type declared elsewhere. See #1373.
pub fn unknown_type_parameter_candidate(
    name: &str,
    owner: &str,
    declared_type_params: &[String],
    span: Span,
) -> CompileError {
    let params = declared_type_params
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(name))
        .collect::<Vec<_>>()
        .join(", ");
    CompileError::type_error(format!("Unknown symbol '{}'", name), span)
        .with_hint(format!(
            "'{name}' is not declared as a type parameter of '{owner}'; did you mean `def {owner}[{params}](...)`?"
        ))
        .with_hint(format!(
            "If '{name}' is a type declared elsewhere, import it or define it"
        ))
}

pub fn duplicate_definition(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Duplicate definition of '{}'", name), span)
}

/// Report a source binding that attempts to replace a protected builtin spelling.
pub fn protected_builtin_binding(name: &str, builtin: BuiltinFnId, span: Span) -> CompileError {
    let canonical = builtins::as_str(builtin);
    CompileError::type_error(
        format!(
            "Cannot bind '{}': it is a protected builtin binding for '{}'",
            name, canonical
        ),
        span,
    )
    .with_hint(format!(
        "Choose a different name; '{}' and its registered aliases remain globally available",
        canonical
    ))
}

/// Report two active source bindings for one spelling in the same RFC 120 namespace.
pub fn duplicate_binding(name: &str, first_span: Span, second_span: Span) -> CompileError {
    CompileError::type_error(format!("Duplicate definition of '{}'", name), second_span)
        .with_related_span(first_span, format!("First binding of '{name}'"))
}

/// Report two imports whose same local spelling cannot be proven to name one canonical declaration.
pub fn ambiguous_import_binding(name: &str, first_span: Span, second_span: Span) -> CompileError {
    CompileError::type_error(format!("Ambiguous import binding '{}'", name), second_span)
        .with_related_span(first_span, format!("Conflicting import binding of '{name}'"))
        .with_hint("Use an explicit import alias so each declaration has a distinct local spelling")
}

/// Reject replacing one of the language's immutable builtin-function spellings.
pub fn immutable_builtin_redefinition(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Cannot redefine immutable built-in function '{name}'"), span)
        .with_hint("Use a different name; `print` and `println` are reserved language functions")
}

/// The declaration kind whose representation must store every declared type parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredTypeParamOwner {
    /// A `newtype` or `rusttype`: the parameter must appear in the underlying type.
    Newtype,
    /// An `enum`: the parameter must appear in at least one variant payload.
    Enum,
}

/// Report a declared type parameter that the declaration's representation does not store.
///
/// A model or class can carry such a parameter (it becomes a phantom marker, #1370), but a newtype has exactly its
/// underlying type and an enum has exactly its variant payloads, so a parameter neither mentions has nowhere to
/// live; the generated Rust would be rejected as an unused type parameter. Reported at the parameter's own span.
pub fn type_param_not_stored(owner: StoredTypeParamOwner, owner_name: &str, param: &str, span: Span) -> CompileError {
    let (kind, storage, hint) = match owner {
        StoredTypeParamOwner::Newtype => (
            "newtype",
            "its underlying type",
            format!("Use '{param}' in the underlying type, for example `newtype list[{param}]`, or remove it"),
        ),
        StoredTypeParamOwner::Enum => (
            "enum",
            "any variant payload",
            format!("Give a variant a payload that mentions '{param}', for example `Some({param})`, or remove it"),
        ),
    };
    CompileError::type_error(
        format!("Type parameter '{param}' of {kind} '{owner_name}' is not used by {storage}"),
        span,
    )
    .with_hint(hint)
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

/// Build the diagnostic for passing the same call argument more than once.
pub fn duplicate_call_argument(callee: &str, name: &str, first_span: Span, duplicate_span: Span) -> CompileError {
    CompileError::type_error(
        format!("Duplicate argument '{name}' when calling '{callee}'"),
        duplicate_span,
    )
    .with_related_span(first_span, format!("First argument named '{name}'"))
    .with_hint("Pass each fixed parameter at most once")
}

/// Build the diagnostic for a keyword argument that the callee does not accept.
pub fn unknown_keyword_argument(callee: &str, name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Unexpected keyword argument '{name}' when calling '{callee}'"),
        span,
    )
    .with_hint("Add a `**kwargs` rest parameter to capture arbitrary keyword arguments")
}

/// Build the diagnostic for unpacking call arguments into a callee without a rest parameter.
pub fn call_unpack_without_rest(callee: &str, unpack: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Cannot use `{unpack}` unpacking when calling '{callee}' because the callee has no matching rest parameter"
        ),
        span,
    )
}

/// Report a text encoding label outside the builtin `str.encode` / `bytes.decode` contract (#1668).
///
/// The builtin text return trip supports UTF-8 only; a literal label is rejected here, while a label that is only
/// known at run time raises `ValueError` from the generated program instead.
pub fn unsupported_text_encoding(callee: &str, encoding: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("{callee}() supports only UTF-8 in this release, got encoding '{encoding}'"),
        span,
    )
    .with_hint("Omit the argument or pass \"utf-8\"; std.fs and std.encoding cover other codecs")
}

/// Report a `bytes.decode` `errors` policy that is neither `strict` nor `replace` (#1668).
pub fn unsupported_decode_errors_policy(policy: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("bytes.decode() errors must be \"strict\" or \"replace\", got '{policy}'"),
        span,
    )
}

/// Build the diagnostic for omitting a required call argument.
pub fn missing_required_argument(callee: &str, name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Missing required argument '{name}' when calling '{callee}'"),
        span,
    )
}

/// Report a top-level partial preset expression that would require runtime evaluation.
pub fn unsafe_top_level_partial_preset(name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Top-level partial preset '{name}' must be declaration-safe"),
        span,
    )
    .with_hint("Use literals, const paths, or declaration-safe collection/model literals as top-level partial presets")
}

/// Build the diagnostic for declaring the same rest parameter twice.
pub fn duplicate_rest_parameter(kind: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Only one `{kind}` rest parameter is allowed in a callable signature"),
        span,
    )
}

/// Build the diagnostic for placing a rest parameter after an invalid parameter kind.
pub fn invalid_rest_parameter_order(message: &str, span: Span) -> CompileError {
    CompileError::type_error(message.to_string(), span)
}

/// Build the diagnostic for putting a default value on a rest parameter.
pub fn rest_parameter_default_not_allowed(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Rest parameter '{name}' cannot declare a default value"), span)
}

// -- Decorators & namespaces -------------------------------------------------

pub fn unknown_decorator(path: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Unknown decorator '@{}'", path), span)
        .with_hint("Decorators must resolve to stdlib paths like @std.web.route")
        .with_hint("Import the module alias or use a fully qualified decorator path")
}

/// Report a decorator argument that an attribute macro cannot receive: only literals, constants and lists of them.
pub fn native_attribute_argument_unsupported(macro_path: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Arguments of the attribute macro `{macro_path}` must be literals, constants or lists of them"),
        span,
    )
}

/// Report a decorator argument naming a value that has no scalar literal form for an attribute macro.
pub fn native_attribute_argument_not_scalar(macro_path: &str, name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Argument `{name}` of the attribute macro `{macro_path}` has no literal form"),
        span,
    )
    .with_note("An attribute macro receives literals, scalar constants, bare names or lists of them")
    .with_hint("Write the list or value literally in the decorator")
}

/// Report a user-defined decorator on a declaration kind that keeps decorators compiler-owned.
pub fn user_defined_decorator_unsupported_target(path: &str, kind: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("User-defined decorator '@{path}' cannot be used on {kind} declarations"),
        span,
    )
    .with_hint("User-defined decorators are supported on functions and methods")
}

/// Report a decorator expression that resolves but is not callable.
pub fn decorator_not_callable(path: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("decorator '{path}' is not callable"), span)
}

/// Report a decorator factory expression whose result is not callable.
pub fn decorator_factory_not_callable(path: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("'{path}' does not return a callable"), span)
}

/// Report a decorator callable whose application result is not callable.
pub fn decorator_result_not_callable(path: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("decorator '{path}' must return a callable"), span)
}

/// Report a method decorator, or a function it returns in the method's place, that spells the decorated method's
/// receiver as `&Owner` or `&mut Owner` (#1790).
///
/// Incan source spells a method decorator's receiver the way the method spells it: `(Box, int) -> str` for a `self`
/// method and `(mut Box, int) -> int` for a `mut self` method, and the compiler decides how the receiver is passed.
/// `subject` names what spells it (`Method decorator '@as_int'`), `method` is the decorated method, `written` the
/// refused receiver spelling (`&Box`) and `replacement` the spelling to write instead, a callable shape or a parameter.
/// `INCAN-T0110` is its stable code.
pub fn method_decorator_receiver_spelling(
    subject: &str,
    method: &str,
    written: &str,
    replacement: &str,
    mutable_receiver: bool,
    span: Span,
) -> CompileError {
    let receiver = if mutable_receiver { "mut self" } else { "self" };
    CompileError::type_error(
        format!("{subject} spells the receiver of '{method}' as `{written}`"),
        span,
    )
    .with_stable_code("INCAN-T0110")
    .with_hint(format!(
        "Write `{replacement}` for a `{receiver}` method; the compiler decides how the receiver is passed"
    ))
    .with_note(
        "A method decorator spells the receiver the way the method does: the owner type for `self`, and the owner \
         type marked `mut` for `mut self`",
    )
}

/// Report a method-decorator shape or replacement whose receiver `mut` marker disagrees with the method (#1790).
///
/// A `mut self` method's changes to its receiver are visible to its caller, so every shape in its decorator chain and
/// every function a decorator returns in its place must take the receiver marked `mut`; a `self` method's must not.
/// `subject` names what carries the wrong marker (the decorator's accepted shape, its returned shape, or a replacement
/// function) and `fix` is the spelling to write instead.
pub fn method_decorator_receiver_mut_mismatch(
    subject: &str,
    method: &str,
    mutable_receiver: bool,
    fix: &str,
    span: Span,
) -> CompileError {
    let message = if mutable_receiver {
        format!("{subject} takes the receiver of '{method}' without `mut`, but '{method}' is a `mut self` method")
    } else {
        format!("{subject} marks the receiver of '{method}' `mut`, but '{method}' is a `self` method")
    };
    CompileError::type_error(message, span).with_hint(fix.to_string())
}

/// Report a `self`-method decorator chain the compiler cannot pass the receiver through (#1790).
///
/// A decorator whose shapes name a `self` method's receiver takes that receiver the way the method's generated wrapper
/// passes it, and so does every function it returns in the method's place. The compiler arranges that for private
/// functions of the method's module that are only used in the chain, and a decorator that only returns the callable it
/// accepts. `refused` says what is refused (`'parse' cannot be used here`), `reason` which rule that breaks, and `hint`
/// what to write instead. `INCAN-T0116` is its stable code.
pub fn method_decorator_receiver_not_planned(refused: &str, reason: &str, hint: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("{refused}: {reason}"), span)
        .with_stable_code("INCAN-T0116")
        .with_hint(hint.to_string())
        .with_note(
            "A decorator whose shapes name the receiver of a `self` method is a private function of the module that \
             declares the method's type, writes those shapes as callable types, returns the callable it accepts or a \
             private function of that module by name, and uses the callable it accepts only to return it; those \
             functions are used only in the decorator chain, and a returned function can also be called directly in \
             its module",
        )
}

/// Report a `mut` marker on an `int`, `float` or `bool` parameter of a function type (#1790).
///
/// Such a parameter is the function's own copy of the argument, so its changes never reach the caller and no `def` has
/// a type that marks it. `ty` is the parameter type as written.
pub fn mut_marker_on_copied_scalar(ty: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("`mut` cannot mark the `{ty}` parameter of a function type"),
        span,
    )
    .with_hint(format!("Write `{ty}` without `mut`"))
    .with_note(
        "An `int`, `float` or `bool` parameter is the function's own copy: `mut` lets the function change it, and the \
         change stays in the function, so its type does not mark the parameter",
    )
}

/// Report a type-valued decorator argument on a user-defined decorator factory.
pub fn decorator_type_argument_not_supported(path: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("User-defined decorator '{path}' cannot use type-valued decorator arguments"),
        span,
    )
    .with_hint("Use expression arguments such as name=value for decorator factories")
}

// -- Web route handlers ------------------------------------------------------

/// Report a `@route` handler whose declared return type has no response form (#1721).
///
/// A route handler's return value becomes the HTTP response, and only response types have that form: `str`, `None`,
/// `Json[...]`, `Html`, `Response`, a `Result` of those, or a wrapper that derives `IntoResponse`. A handler declared
/// `-> int` checked, but the route it registers had nothing to send and the build stopped on it. `handler` is the
/// function's name and `return_type` its declared return type as the source spells it. The hint lists the response
/// types and, for the common numeric case, the text form to return instead. `INCAN-T0107` is its stable code.
pub fn route_handler_return_not_response(handler: &str, return_type: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Route handler '{handler}' returns '{return_type}', which is not a response type"),
        span,
    )
    .with_stable_code("INCAN-T0107")
    .with_hint(
        "Return 'str', 'Json[...]', 'Html' or 'Response' from a route handler; to send a number or another value as \
         text, return 'str(value)'",
    )
    .with_note("A route handler's return value is the HTTP response, so only a response type can be returned")
}

/// Report a `@route` handler parameter that nothing in the request supplies (#1722).
///
/// The route binds a handler parameter in one of two ways: a `{name}` segment of the path binds the parameter of that
/// name, and a typed extractor (`Json[T]`, `Query[T]`, `Path[T]`, or a wrapper deriving `FromRequestParts`) reads the
/// request itself. A parameter that is neither has no value to receive, and the route cannot be built. `handler` is
/// the function's name, `parameter` the unbound parameter, `path` the route path as written, and `captures` the
/// segment names the path does bind, so the hint can name a spelling that would bind the parameter and, when the path
/// already captures other names, list them. `INCAN-T0108` is its stable code.
pub fn route_handler_parameter_unbound(
    handler: &str,
    parameter: &str,
    path: &str,
    captures: &[String],
    span: Span,
) -> CompileError {
    let bound_path = if path.ends_with('/') {
        format!("{path}{{{parameter}}}")
    } else {
        format!("{path}/{{{parameter}}}")
    };
    let error = CompileError::type_error(
        format!("Route handler '{handler}' has a parameter '{parameter}' that no segment of the path '{path}' binds"),
        span,
    )
    .with_stable_code("INCAN-T0108")
    .with_hint(format!(
        "Add a '{{{parameter}}}' segment to the path, '{bound_path}', or read the value from the request with a \
         'Query[...]' or 'Json[...]' parameter"
    ))
    .with_note(
        "A route handler's parameters come from the request: a '{name}' path segment binds the parameter named \
         'name', and a 'Json[T]', 'Query[T]' or 'Path[T]' parameter reads the body, the query string or the path",
    );
    if captures.is_empty() {
        error
    } else {
        let bound = captures
            .iter()
            .map(|capture| format!("'{capture}'"))
            .collect::<Vec<_>>()
            .join(", ");
        error.with_note(format!("The path binds {bound}"))
    }
}

/// Report a `@route` handler whose JSON-carrying wrapper has a payload type with no JSON form (#1768).
///
/// A `Json[T]`, `Query[T]` or `Path[T]` parameter deserializes the request into `T` through serde, and a `Json[T]`
/// return serializes `T` as the response body, so `T`, and every model or class it is built from, must derive `json`.
/// A model or class declared without it checked and then could not be registered as a handler. `handler` is the
/// function's name, `wrapper` the declared wrapper type as written, and `payload` the model or class that lacks the
/// derive, or, when `builtin` is set, the `decimal` or frozen type that has no JSON form at all (#1561).
/// `INCAN-T0112` is its stable code.
pub fn route_payload_without_json_form(
    handler: &str,
    wrapper: &str,
    payload: &str,
    builtin: bool,
    span: Span,
) -> CompileError {
    let hint = if builtin {
        format!("Use a type with a JSON form in place of '{payload}', such as 'str', 'float', 'list' or 'dict'")
    } else {
        format!(
            "Derive JSON support on '{payload}': add '@derive(json)' above its declaration, with 'from std.serde import json'"
        )
    };
    CompileError::type_error(
        format!("Route handler '{handler}' uses '{wrapper}', but '{payload}' has no JSON form"),
        span,
    )
    .with_stable_code("INCAN-T0112")
    .with_hint(hint)
    .with_note(
        "A route's 'Json[T]', 'Query[T]' and 'Path[T]' parameters deserialize the request into 'T', and a 'Json[T]' \
         return serializes 'T' as the response body, so every model or class in 'T' must derive 'json'",
    )
}

/// Report a malformed `ValidationError(...)` constructor call.
pub fn validation_error_constructor_shape(span: Span) -> CompileError {
    CompileError::type_error(
        "ValidationError requires exactly one message argument and an optional code".to_string(),
        span,
    )
    .with_hint("Use ValidationError(\"message\") or ValidationError(message=\"message\")")
}

/// Report a `from_underlying` hook that does not satisfy the RFC 017 contract.
pub fn invalid_newtype_validation_hook(newtype_name: &str, detail: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Invalid '{newtype_name}.from_underlying' validation hook: {detail}"),
        span,
    )
    .with_hint(format!(
        "Use: def from_underlying(value: <underlying>) -> Result[{newtype_name}, ValidationError]"
    ))
}

/// Report an implicit coercion attempt into a newtype that opted out.
pub fn implicit_newtype_coercion_disabled(newtype_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Implicit coercion into newtype '{newtype_name}' is disabled"),
        span,
    )
    .with_hint("Use an explicit constructor or from_underlying call at this site")
}

/// Report a cycle in validated-newtype underlying/coercion resolution.
pub fn newtype_coercion_cycle(newtype_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Validated-newtype coercion cycle detected while resolving '{newtype_name}'"),
        span,
    )
    .with_hint("Break the newtype-underlying cycle before relying on implicit coercion")
}

pub fn reserved_root_namespace(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("'{}' is a reserved root namespace", name), span)
        .with_hint("Choose a different name (reserved: std, rust)")
}

/// Build the diagnostic for malformed RFC 171 testing-marker declaration metadata.
pub fn invalid_testing_marker_annotation(reason: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Invalid @marker annotation: {reason}"), span).with_hint(
        "Use @marker(kind=\"name\", runner_only=true); fixture markers may also declare scope_arg, autouse_arg, and three scopes",
    )
}
