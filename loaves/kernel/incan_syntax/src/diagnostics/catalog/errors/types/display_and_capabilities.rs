//! Display and RFC 104 capability errors.

use super::*;

// -- Display -----------------------------------------------------------------

/// Where a value is displayed: every position shares one display rule (#1748).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayPosition<'a> {
    /// An argument of `print` or `println`.
    Print {
        /// The spelling the call used (`print` or `println`).
        builtin: &'a str,
    },
    /// The argument of `str(...)`.
    Str,
    /// An f-string `{value}` part.
    Interpolation,
    /// A type argument bound to a type parameter that requires `Display`.
    Bound {
        /// The callee whose type parameter carries the bound.
        callee: &'a str,
        /// The type parameter that requires `Display`.
        type_param: &'a str,
    },
}

/// A value with no printed form in any display position (#1748).
///
/// A tuple, list, dict, set, `Option` or `Result` renders through its structure in every display position, a scalar or
/// a `str` renders through its own text, and a model, class, enum or newtype renders through the `Display` it provides:
/// a `__str__` method, the values of an enum that declares them, or the `message()` of an `Error` adopter. What remains
/// has no printed form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnprintableValue<'a> {
    /// A generic type parameter whose declaration does not require a display route.
    TypeParameter {
        /// The parameter's source name.
        name: &'a str,
    },
    /// A value of an anonymous union type (`int | str`), which prints once it is narrowed to one member.
    Union,
    /// A `Generator` value, whose items exist only as it is consumed.
    Generator,
    /// A function or closure value.
    Function,
    /// A `bytes` value, which has no text of its own.
    Bytes,
    /// A value of type `None`, such as the result of a function that returns `None` or the side of a `Result` that
    /// nothing gives another type.
    NoneValue,
    /// A model, class, enum or newtype value whose type provides no `Display`.
    Nominal {
        /// The type as the checker names it.
        type_name: &'a str,
    },
}

impl UnprintableValue<'_> {
    /// Name the value for a message: `the union value 'x'` when the source spells it as a plain name, otherwise the
    /// kind with its article (`a union value`), and a nominal type by name (`a value of type 'Point'`), so the article
    /// never depends on how a type name is pronounced.
    fn describe(self, name: Option<&str>) -> String {
        let kind = match self {
            Self::TypeParameter { name } => return format!("type parameter '{name}'"),
            Self::Union => "union value".to_string(),
            Self::Generator => "generator".to_string(),
            Self::Function => "function".to_string(),
            Self::Bytes => "bytes value".to_string(),
            Self::NoneValue => "None value".to_string(),
            Self::Nominal { type_name } => match name {
                Some(_) => format!("{type_name} value"),
                None => return format!("a value of type '{type_name}'"),
            },
        };
        match name {
            Some(name) => format!("the {kind} '{name}'"),
            None => format!("a {kind}"),
        }
    }
}

/// Report a displayed value that has no printed form (#1748).
///
/// `print`/`println` arguments, the argument of `str(...)`, f-string `{value}` parts and `Display` bounds share one
/// display rule, so the same value is refused in each; `position` words the message for the position the source used.
/// `name` is the operand when the source spells it as a plain name (or `self`), `None` for any other expression and for
/// a bound. The hint names what does display: a narrowed union member, a collected generator's list, a called
/// function's result, decoded bytes, or a type's `__str__` (and, outside a bound, its `{value:?}` structure, spelled
/// with the operand's own name when it has one). `INCAN-T0103` is its stable code.
pub fn value_has_no_printed_form(
    position: DisplayPosition<'_>,
    name: Option<&str>,
    value: UnprintableValue<'_>,
    span: Span,
) -> CompileError {
    let what = value.describe(name);
    let message = match position {
        DisplayPosition::Print { builtin } => format!("'{builtin}' cannot print {what}"),
        DisplayPosition::Str => format!("'str' cannot convert {what} to text"),
        DisplayPosition::Interpolation => format!("f-string cannot interpolate {what}"),
        DisplayPosition::Bound { callee, type_param } => {
            format!("Call to '{callee}' cannot bind {what} to '{type_param}', which requires 'Display'")
        }
    };
    let hint = match value {
        UnprintableValue::TypeParameter { name } => {
            format!("Add a Display bound: `{name} with Display`")
        }
        UnprintableValue::Union => {
            "Narrow it to one member first, with match or isinstance, and display that member".to_string()
        }
        UnprintableValue::Generator => {
            "Collect its items first with list(...); a list displays its elements".to_string()
        }
        UnprintableValue::Function => "Call it and display the result".to_string(),
        UnprintableValue::Bytes => {
            "Decode it to text first with decode(), or display its length with len(...)".to_string()
        }
        UnprintableValue::NoneValue => {
            "Display a value of another type; a Result side that nothing else types is None, so annotate the Result's type to give that side one".to_string()
        }
        UnprintableValue::Nominal { type_name } => {
            // Spell the structure form with the operand's own name; any other expression gets the format spec alone,
            // so the hint never names a binding the program does not have. A bound displays inside the callee, where
            // the caller has no f-string part to change.
            let structure = match (position, name) {
                (DisplayPosition::Bound { .. }, _) => String::new(),
                (_, Some(name)) => format!(", or interpolate its structure with f\"{{{name}:?}}\""),
                (_, None) => {
                    ", or interpolate its structure by adding the :? format spec to its f-string part".to_string()
                }
            };
            format!("Define __str__(self) -> str on '{type_name}' to give it a printed form{structure}")
        }
    };
    let note = match value {
        UnprintableValue::TypeParameter { .. } => {
            "A generic body may display a type parameter only when its declaration requires Display, Error or a trait that supplies __str__"
        }
        UnprintableValue::Nominal { .. } => {
            "A type provides Display through a __str__ method, the values of an enum that declares them, or the message() of an Error adopter; @derive(Display) provides nothing"
        }
        _ => {
            "print, println, str, an f-string {value} and a Display bound share one display rule, and it gives this value no printed form"
        }
    };
    CompileError::type_error(message, span)
        .with_stable_code("INCAN-T0103")
        .with_hint(hint)
        .with_note(note)
}

/// Report a displayed tuple, list, dict, set, frozen collection, `Option` or `Result` that holds a value with no
/// printed form (#1748).
///
/// Inside a structure every element displays as its `{value:?}` structure, and a generator, a function and a `bytes`
/// value have none, at any depth: the structure is refused in every display position as the element itself is.
/// `container` is the displayed value's type, `name` the operand when the source spells it as a plain name (or `self`),
/// and `element` the kind of the element with no printed form. `INCAN-T0103` is its stable code.
pub fn element_has_no_printed_form(
    position: DisplayPosition<'_>,
    name: Option<&str>,
    container: &str,
    element: UnprintableValue<'_>,
    span: Span,
) -> CompileError {
    let what = match name {
        Some(name) => format!("'{name}'"),
        None => format!("a value of type '{container}'"),
    };
    let element_what = element.describe(None);
    let message = match position {
        DisplayPosition::Print { builtin } => format!("'{builtin}' cannot print {what}, which holds {element_what}"),
        DisplayPosition::Str => format!("'str' cannot convert {what} to text, since it holds {element_what}"),
        DisplayPosition::Interpolation => format!("f-string cannot interpolate {what}, which holds {element_what}"),
        DisplayPosition::Bound { callee, type_param } => format!(
            "Call to '{callee}' cannot bind {what} to '{type_param}', which requires 'Display', since it holds {element_what}"
        ),
    };
    let hint = match element {
        UnprintableValue::Generator => "Collect the generator's items into a list first; a list displays its elements",
        UnprintableValue::Function => "Hold what the function returns instead of the function",
        UnprintableValue::Bytes => "Decode the bytes to text first with decode()",
        _ => "Display the elements that have a printed form",
    };
    CompileError::type_error(message, span)
        .with_stable_code("INCAN-T0103")
        .with_hint(hint)
        .with_note(
            "Inside a tuple, list, dict, set, frozen collection, Option or Result every element displays as its {value:?} structure; a generator, a function or bytes has none",
        )
}

/// A value an f-string `{value:?}` part cannot render, because its type has no `Debug` form (#1561).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UndebuggableValue<'a> {
    /// A `Generator` value, whose items exist only as it is consumed.
    Generator,
    /// A function or closure value.
    Function,
    /// A value whose type, or a type inside it, implements no `Debug`, such as a `JoinHandle[T]`.
    Type {
        /// The type that implements no `Debug`, as the checker names it.
        type_name: &'a str,
    },
}

/// Report an f-string `{value:?}` part whose value has no `Debug` form (#1561).
///
/// `{value:?}` renders a value's structure through `Debug`. A generator, a function, and a value whose type (or a type
/// inside it) implements no `Debug` have no such structure, so the part is refused the way a `{value}` part with no
/// printed form is. `name` is the operand when the source spells it as a plain name (or `self`), `None` for any other
/// expression. `INCAN-T0103` is its stable code.
pub fn value_has_no_debug_form(name: Option<&str>, value: UndebuggableValue<'_>, span: Span) -> CompileError {
    let what = match (value, name) {
        (UndebuggableValue::Generator, Some(name)) => format!("the generator '{name}'"),
        (UndebuggableValue::Generator, None) => "a generator".to_string(),
        (UndebuggableValue::Function, Some(name)) => format!("the function '{name}'"),
        (UndebuggableValue::Function, None) => "a function".to_string(),
        (UndebuggableValue::Type { type_name }, _) => format!("a value of type '{type_name}'"),
    };
    let hint = match value {
        UndebuggableValue::Generator => "Collect its items first with list(...); a list shows its elements",
        UndebuggableValue::Function => "Call it and interpolate the result",
        UndebuggableValue::Type { .. } => "Interpolate a value read from it instead, such as one of its fields",
    };
    CompileError::type_error(format!("f-string cannot interpolate {what} with ':?'"), span)
        .with_stable_code("INCAN-T0103")
        .with_hint(hint)
        .with_note("An f-string {value:?} part renders the value's structure through Debug, and this value has none")
}

pub fn tuple_field_assignment(span: Span) -> CompileError {
    CompileError::type_error("Cannot assign to tuple field - tuples are immutable".to_string(), span)
        .with_hint("Create a new tuple instead of modifying an existing one")
}

/// Report a `for` loop whose tuple pattern destructures an iteration item that is not a tuple at all.
///
/// Distinct from [`tuple_unpack_count_mismatch`], which reports a tuple of the wrong arity: here there is no
/// tuple to take elements from, so naming the actual item type is the useful part of the message.
pub fn for_pattern_expects_tuple_item(names: usize, item_ty: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Cannot destructure {names} values from iteration item of type '{item_ty}'"),
        span,
    )
    .with_hint("Iterate a sequence of tuples, or bind each item to a single name")
}

/// Build the error emitted when a statement destructures a value whose type is not a tuple at all.
///
/// The statement-level sibling of [`for_pattern_expects_tuple_item`]: same question, asked where `a, b = value`
/// binds names rather than where a loop binds an item. Naming the resolved type is the point of the diagnostic —
/// without it the failure only surfaced as a `rustc` field-projection error against a compiler-internal binding.
pub fn tuple_unpack_expects_tuple_value(names: usize, value_ty: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Cannot destructure {names} values from value of type '{value_ty}'"),
        span,
    )
    .with_hint("Assign a tuple of matching length, or bind the value to a single name")
}

/// Build the error emitted when a loop pattern destructures a Rust interop item whose shape is unverifiable.
///
/// The loop-shaped sibling of [`tuple_unpack_rust_shape_unverified`], paired the same way
/// [`for_pattern_expects_tuple_item`] pairs with [`tuple_unpack_expects_tuple_value`]. Separate because the
/// remedy differs: a loop's reader changes what the sequence yields, not what a single assignment produces.
pub fn for_pattern_rust_shape_unverified(names: usize, item_ty: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Cannot verify Rust iteration item of type '{item_ty}' is tuple-shaped for destructuring {names} values"
        ),
        span,
    )
    .with_hint("Iterate a sequence of tuples the compiler can read, or bind each item to a single name")
}

/// Build the error emitted when destructuring a Rust interop value whose shape the compiler cannot establish.
///
/// Distinct from [`tuple_unpack_expects_tuple_value`], which reports a type known *not* to be a tuple. Here the
/// compiler simply cannot tell, and "not proven tuple-shaped" must refuse for the same reason a bare type variable
/// does: assuming otherwise re-emits a tuple-field projection into a value that may have no such fields.
pub fn tuple_unpack_rust_shape_unverified(names: usize, value_ty: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Cannot verify Rust value of type '{value_ty}' is tuple-shaped for destructuring {names} values"),
        span,
    )
    .with_hint("Bind the value to a single name, or return it from Rust as a tuple the compiler can read")
}

pub fn tuple_unpack_count_mismatch(expected: usize, found: usize, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Cannot unpack {} values from tuple with {} elements", expected, found),
        span,
    )
}

// -- RFC 104 capabilities -----------------------------------------------------

/// Build the error emitted when a `capability` declaration omits its `description` clause.
///
/// The grammar accepts the omission so that a missing description is reported here, against the declaration's own
/// span, rather than as a parse failure that cannot say which clause is missing. RFC 104 treats the description as
/// part of the authority's public contract: a policy author deciding whether to grant a capability reads it, so a
/// capability without one cannot be reviewed.
pub fn capability_description_required(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Capability '{name}' has no `description`"), span)
        .with_hint("Add `description = \"...\"` saying what granting this capability authorizes")
}

/// Build the error emitted when a `requires` entry is not a symbol reference.
///
/// RFC 104 requires these to be checked references to other capabilities rather than strings, so that a misspelling
/// is a compile error instead of a runtime denial discovered later. An entry that is a call, a literal, or any other
/// expression cannot name a declaration at all.
pub fn capability_requirement_not_a_reference(span: Span) -> CompileError {
    CompileError::type_error(
        "A capability `requires` entry must be a capability reference".to_string(),
        span,
    )
    .with_hint("Write the capability's name or dotted path, such as `host.http.request`, not an expression")
}

/// Build the error emitted when a `requires` entry names nothing the compiler can find.
///
/// This is deliberately an unresolved-symbol error at compile time. RFC 104 is explicit that a misspelled or
/// nonexistent capability reference must fail here rather than surviving to become a runtime capability denial,
/// which would surface far from the declaration that caused it.
pub fn capability_requirement_unresolved(path: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Unknown capability '{path}' in `requires`"), span)
        .with_hint("Declare the capability, or import the module that declares it")
}

/// Build the error emitted when a `requires` entry resolves to something that is not a capability.
///
/// Holding a capability never grants what it requires, so the entries are a documentation and policy surface rather
/// than a dispatch mechanism. A `requires` list naming a function or a type would describe an authority relationship
/// that cannot exist, and a policy reading it would draw a false conclusion.
pub fn capability_requirement_not_a_capability(path: &str, found: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("`requires` entry '{path}' is a {found}, not a capability"),
        span,
    )
    .with_hint("Reference a `capability` declaration, or remove the entry")
}

/// Build the error emitted when a `description` clause is present but is not compile-time text.
///
/// The description's audience is a person deciding whether to grant the capability, and there is no later moment at
/// which that review happens — so an expression to be evaluated at runtime cannot serve as one. Accepting a
/// non-string would also record no description at all, leaving a declaration that looks documented while carrying
/// nothing, which is worse than an obviously missing clause.
pub fn capability_description_must_be_text(name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Capability '{name}' has a `description` that is not a text literal"),
        span,
    )
    .with_hint("Write the description as a quoted string, such as `description = \"Issue a refund\"`")
}

/// Refuse a `__hash__` call through a type parameter's `Hash` bound (#1561).
///
/// A `Hash` bound is met only by `@derive(Hash)` or `Hash` in `@rust.derive(...)`, and neither gives a type a
/// `__hash__` method (#1822), so the method the stdlib `Hash` trait declares has nothing to call through the bound.
pub fn hash_method_through_hash_bound(type_param: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("'__hash__' cannot be called on '{type_param}' through its 'Hash' bound"),
        span,
    )
    .with_hint("Bound the type parameter by a trait that declares '__hash__', or use the value as a set element or dict key")
    .with_note(
        "A 'Hash' bound is met only by '@derive(Hash)' or 'Hash' in '@rust.derive(...)', which provide no '__hash__' method; a method named '__hash__' is an ordinary method of the type that defines it",
    )
}

/// Refuse a call argument that binds a type parameter of the callee to a type another argument's binding does not
/// unify with (#1561).
///
/// `bound` is the type the call binds the parameter to and `found` the type this argument binds it to: in
/// `take(1, "s")` for `def take[T](x: T, y: T)`, `"s"` binds `T` to `str` and the `1` binds it to `int`.
pub fn conflicting_type_argument_bindings(
    callee: &str,
    type_param: &str,
    bound: &str,
    found: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(format!("Type mismatch: expected '{bound}', found '{found}'"), span)
        .with_expected_actual(bound, found)
        .with_note(format!(
            "Another argument of '{callee}' binds type parameter '{type_param}' to '{bound}', and '{type_param}' is one type in a call"
        ))
        .with_hint(format!(
            "Pass arguments of one type for '{type_param}', or write the type argument: {callee}[...](...)"
        ))
}

/// Refuse a call or construction that leaves a type parameter of `owner` undetermined: no written type argument, no
/// `sources` (`argument`, `field` or `payload`) and no expected type fixes it, and the checker never chooses one
/// (#1561, #2762). The caller adds the declared parameter's location and the written form that fixes it.
pub fn type_param_not_determined(owner: &str, type_param: &str, sources: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Type parameter '{type_param}' of '{owner}' is not determined: no {sources} or expected type fixes it"),
        span,
    )
    .with_stable_code(crate::diagnostics::stable::TYPECHECK_CODE)
    .with_primary_label(format!("'{type_param}' is not determined here"))
}

/// Refuse a method call on a value of a type parameter whose bounds declare no method of that name (#1561).
///
/// Inside the declaration that introduces it, a type parameter has the methods its bounds' traits declare, and
/// `.clone()`, which the generated Rust bounds by `Clone`. Any other method has nothing to call for a type argument
/// that lacks it, so the generated Rust refuses it.
pub fn type_parameter_method_not_declared(type_param: &str, method: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Type parameter '{type_param}' has no method '{method}(...)': no bound of '{type_param}' declares it"),
        span,
    )
    .with_hint(format!(
        "Bound '{type_param}' by a trait that declares '{method}' (`{type_param} with Trait`), or declare the value \
         with a concrete type"
    ))
}

/// Refuse an RFC 088 iterator adapter or terminal called on a value that is iterable but is not an iterator (#1561).
///
/// A list, set, frozen collection or `Iterable[T]` provides `iter()`; the adapters and terminals are methods of the
/// `Iterator[T]` that `iter()` returns, and the collection has no method of their name.
pub fn iterator_method_on_an_iterable(type_name: &str, method: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Type '{type_name}' has no method '{method}(...)'"), span)
        .with_hint(format!("Call it on the value's iterator: `.iter().{method}(...)`"))
        .with_note(format!(
            "'{method}' is a method of 'Iterator[T]'; a collection or 'Iterable[T]' value provides 'iter()', which returns its iterator"
        ))
}
