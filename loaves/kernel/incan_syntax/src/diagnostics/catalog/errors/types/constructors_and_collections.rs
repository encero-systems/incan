//! Constructor and pattern, indexing and collection, and tuple errors.

use super::*;

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

/// Report an attempt to bind a private class field through direct construction outside its declaring class.
pub fn private_class_constructor_field(class_name: &str, field: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Private field '{field}' cannot be set while constructing class '{class_name}' here"),
        span,
    )
    .with_stable_code("INCAN-T0135")
    .with_primary_label("private constructor field named here")
    .with_note("Only methods on the declaring class may name its private constructor fields")
    .with_hint(format!(
        "Call a `pub` constructor method on '{class_name}', or make fields intended for direct construction `pub`"
    ))
}

/// Report a newtype constructor call that does not pass exactly one positional underlying value.
pub fn newtype_constructor_shape(type_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Newtype constructor '{}' requires exactly one positional underlying value",
            type_name
        ),
        span,
    )
    .with_hint(format!("Use {}(<underlying value>)", type_name))
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

/// Report a collection literal whose own elements leave part of its type open (`[]`, `[None]`, `(None, 1)`, `{}`) at a
/// destination that holds two or more types of the literal's kind (`list[int] | list[str]`) (#1832).
///
/// Neither the destination nor the elements say which of those types the literal is, so it has none to take.
pub fn collection_literal_has_no_one_member(destination: &str, members: &[String], span: Span) -> CompileError {
    let quoted = members
        .iter()
        .map(|member| format!("'{member}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let example = members.first().map_or("one member", String::as_str);
    CompileError::type_error(
        format!(
            "Cannot tell which member of '{destination}' this literal is: it could be any of {quoted}, and its \
             elements do not say which"
        ),
        span,
    )
    .with_hint(format!(
        "Give the literal one member's type first, such as by binding it to a name annotated '{example}'"
    ))
}

/// Report a collection literal argument (`[]`, `{}`, `[None]`) whose parameter type names a type parameter of the
/// callee that no argument fixes, so the call has no type to instantiate it with (#1862).
pub fn generic_literal_argument_leaves_type_param_open(callee: &str, type_param: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Cannot infer type parameter '{type_param}' of '{callee}': this literal's elements do not say what it is, \
             and no other argument does"
        ),
        span,
    )
    .with_hint(format!(
        "Pass the type arguments explicitly, as in `{callee}[...](...)`, or give the literal a type first by binding \
         it to an annotated name"
    ))
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

/// Build the diagnostic emitted when `JsonValue` is indexed with an unsupported key type.
pub fn json_value_index_type_mismatch(found: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("JsonValue indices must be int or str, found '{found}'"), span)
        .with_hint("Use a string key for JSON objects or an integer index for JSON arrays")
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

/// Report that `list.append(value)` needs a cloneable element type because its argument reads a place.
///
/// An argument that reads a name, field or element, or builds a value from one whose type is not `Clone`, leaves the
/// list holding a copy so the place stays usable. A new value built only from call results, constructors, literals and
/// `Copy` or `Clone` parts moves into the list and is never reported (#1821).
pub fn list_append_requires_clone(elem_type: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("List.append requires element type '{}' to be Clone", elem_type),
        span,
    )
    .with_note(
        "List.append copies a value that reads a name, field or element, or is built from one, so the original stays usable",
    )
    .with_hint(
        "Add @derive(Clone) to the element type, append a Copy type, or append a value built only from call results, constructors and literals",
    )
}

/// Report that `list.repeat(value, count)` requires cloneable element values.
pub fn list_repeat_requires_clone(elem_type: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("list.repeat requires element type '{}' to be Clone", elem_type),
        span,
    )
    .with_note("list.repeat preserves the source value, so non-Copy values are cloned into the new list")
    .with_hint("Add @derive(Clone) to the element type or repeat a Copy type")
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

/// Report that `List.clone()` requires a cloneable element type.
pub fn list_clone_requires_clone(elem_type: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("List.clone requires element type '{}' to be Clone", elem_type),
        span,
    )
    .with_note("List.clone follows Rust Vec clone semantics, cloning each element into a new list")
    .with_hint("Add @derive(Clone) to the element type or clone a list of Copy elements")
}

/// Where a list whose items a `for` loop takes out is used again (#1844).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakenListReuse {
    /// A read of the list after the loop.
    AfterLoop,
    /// A read of the list inside the body of the loop that takes its items.
    InsideLoop,
    /// An enclosing loop that runs the `for` loop again over the list it already emptied.
    RepeatedLoop,
    /// A closure that captured the list before the loop, and can still read it.
    CapturedBeforeLoop,
    /// A read that a repeat of an enclosing loop reaches after the loop took the items, before the enclosing loop
    /// assigns the list a new one.
    ReadBeforeRefill,
}

/// Report a list used again after a `for` loop took its items out (#1844).
///
/// The loop body hands each item on by value and `item_ty` can be neither copied nor cloned, so the loop takes the
/// items out of `list`. `use_span` is the use: the read, the read inside the capturing closure, or, when an enclosing
/// loop repeats the loop, the loop's iterable. `related_span` is the loop's iterable, or, for the repeat, the list's
/// definition outside the enclosing loop. `INCAN-T0119` is its stable code.
pub fn taken_list_used_again(
    list: &str,
    item_ty: &str,
    reuse: TakenListReuse,
    use_span: Span,
    related_span: Span,
) -> CompileError {
    let loop_label = format!("the `for` loop takes the items of `{list}` here");
    let (message, related_label, hint) = match reuse {
        TakenListReuse::AfterLoop => (
            format!("`{list}` is used after the `for` loop that took its items"),
            loop_label,
            format!(
                "Read what the later code needs from `{list}` before the loop, or collect the results in a new list \
                 inside it"
            ),
        ),
        TakenListReuse::InsideLoop => (
            format!("`{list}` is used inside the `for` loop that takes its items"),
            loop_label,
            format!("Read what the loop needs from `{list}` before the loop starts"),
        ),
        TakenListReuse::RepeatedLoop => (
            format!("the `for` loop that takes the items of `{list}` runs again when the enclosing loop repeats"),
            format!("`{list}` is defined outside the enclosing loop"),
            format!("Build `{list}` inside the enclosing loop, so each pass iterates a list of its own"),
        ),
        TakenListReuse::CapturedBeforeLoop => (
            format!("a closure captures `{list}` before the `for` loop that takes its items"),
            loop_label,
            format!("Read what the closure needs from `{list}` into a binding of its own before the loop"),
        ),
        TakenListReuse::ReadBeforeRefill => (
            format!(
                "`{list}` is used where a repeat of the enclosing loop reaches it after the `for` loop took its items"
            ),
            loop_label,
            format!(
                "Assign `{list}` a new list before this use in each pass, or read what it needs before the enclosing loop"
            ),
        ),
    };
    CompileError::type_error(message, use_span)
        .with_stable_code("INCAN-T0119")
        .with_related_span(related_span, related_label)
        .with_note(format!(
            "The loop body hands each `{item_ty}` item on by value, and a `{item_ty}` can be neither copied nor \
             cloned, so the loop takes the items out of `{list}`"
        ))
        .with_hint(hint)
}

/// What a `for` loop that has to take its items out iterates instead of a list it can take them from (#1561).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UntakeableIterable<'a> {
    /// A `mut` parameter, whose items belong to the caller.
    MutParameter(&'a str),
    /// The variable of an enclosing `for` loop that reads its items in place.
    InPlaceLoopVariable(&'a str),
    /// Another binding that is not a local list, such as a static.
    Binding(&'a str),
    /// An element of a list or a value of a dict.
    Element,
    /// A field.
    Field,
    /// The result of a `list`, `dict` or `set` method, such as `values()`, by method name.
    CollectionMethod(&'a str),
    /// `enumerate(...)` or `zip(...)`, by name, which pair up the items of what they iterate.
    Adapter(&'a str),
    /// Any other expression.
    Other,
}

/// Refuse a `for` loop that has to take its items out of what it iterates and cannot (#1561).
///
/// The loop body hands each item on by value and `item_ty` can be neither copied nor cloned, so the loop has to take
/// the items out of `iterable`, which is only possible for a local list, the variable of an enclosing loop that owns
/// its items, a parameter not marked `mut`, or a new list such as a call result. `span` is the iterable.
/// `INCAN-T0119` is its stable code.
pub fn loop_cannot_take_items(iterable: UntakeableIterable<'_>, item_ty: &str, span: Span) -> CompileError {
    let what = match iterable {
        UntakeableIterable::MutParameter(name) => {
            format!("`{name}`, a `mut` parameter whose items belong to the caller")
        }
        UntakeableIterable::InPlaceLoopVariable(name) => {
            format!("`{name}`, the variable of a `for` loop that reads its items in place")
        }
        UntakeableIterable::Binding(name) => format!("`{name}`"),
        UntakeableIterable::Element => "a list element or dict value".to_string(),
        UntakeableIterable::Field => "a field".to_string(),
        UntakeableIterable::CollectionMethod(method) => format!("`{method}()`, which reads the collection in place"),
        UntakeableIterable::Adapter(name) => format!("`{name}(...)`, which reads what it iterates in place"),
        UntakeableIterable::Other => "this iterable".to_string(),
    };
    CompileError::type_error(
        format!("the `for` loop cannot take its `{item_ty}` items out of {what}"),
        span,
    )
    .with_stable_code("INCAN-T0119")
    .with_note(format!(
        "The loop body hands each `{item_ty}` item on by value, and a `{item_ty}` can be neither copied nor cloned, so \
         the loop has to take the items out of what it iterates: a local list, the variable of an enclosing loop that \
         owns its items, a parameter not marked `mut`, or a new list such as a call result"
    ))
    .with_hint("Iterate a local list or a parameter not marked `mut` that holds the items, or take the items out one at a time with `pop()`")
}

pub fn string_index_assignment_not_allowed(span: Span) -> CompileError {
    CompileError::type_error("Strings are immutable - cannot assign to index".to_string(), span)
}

// -- Tuples ------------------------------------------------------------------

/// Report a tuple annotation written without its element types (#1717).
///
/// `Tuple` names a family of types, one per element list, so a bare spelling names no type at all and the build
/// has nothing to emit for it. `spelling` is the word the source used (`Tuple` or `tuple`) so the hint keeps the
/// author's casing. `INCAN-T0104` is its stable code.
pub fn tuple_annotation_requires_element_types(spelling: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Tuple annotation '{spelling}' is missing its element types"),
        span,
    )
    .with_stable_code("INCAN-T0104")
    .with_hint(format!(
        "Write one type per element, for example '{spelling}[int, str]' for a pair of an int and a str"
    ))
}

/// Report a builtin collection annotation written without its type arguments (#1717, #1749).
///
/// `list`, `dict`, `set`, `tuple`, `Option`, `Result`, the frozen collections and `Generator` each name a family of
/// types, one per argument list, so a bare spelling names no type at all and the build has nothing to emit for it.
/// `spelling` is the word the source used (`List`, `list`, `Option`, ...) so the message and the hint keep the
/// author's casing; the family decides which type arguments the hint spells. `INCAN-T0104` is its stable code.
pub fn collection_annotation_requires_type_arguments(spelling: &str, span: Span) -> CompileError {
    let family = collections::from_str(spelling);
    let (missing, arguments) = match family {
        Some(CollectionTypeId::Tuple) => return tuple_annotation_requires_element_types(spelling, span),
        Some(CollectionTypeId::Dict | CollectionTypeId::FrozenDict) => ("key and value types", "str, int"),
        Some(CollectionTypeId::Result) => ("value and error types", "int, str"),
        Some(CollectionTypeId::Option) => ("value type", "int"),
        Some(
            CollectionTypeId::List
            | CollectionTypeId::Set
            | CollectionTypeId::FrozenList
            | CollectionTypeId::FrozenSet
            | CollectionTypeId::Generator,
        )
        | None => ("element type", "int"),
    };
    let family_name = match family {
        Some(family) => collections::as_str(family),
        None => spelling,
    };
    CompileError::type_error(
        format!("{family_name} annotation '{spelling}' is missing its {missing}"),
        span,
    )
    .with_stable_code("INCAN-T0104")
    .with_hint(format!(
        "Write the type arguments, for example '{spelling}[{arguments}]'"
    ))
}
