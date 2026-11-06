//! Field and alias errors (RFC 021).

use super::*;

// -- Fields & aliases (RFC 021) ----------------------------------------------

pub fn missing_field(type_name: &str, field: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Type '{}' has no field '{}'", type_name, field), span)
}

/// Report a class field that holds its own class by value, which leaves the class without a finite layout.
pub fn direct_recursive_class_field(class_name: &str, field: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Class '{class_name}' holds itself through field '{field}' with no Option or collection in between"),
        span,
    )
    .with_hint("Store the field as `Option[...]` or in a collection such as `list[...]`")
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

/// Report a call to an unmarked method from outside the methods of its declaring type (#2570).
pub fn private_method(type_name: &str, method: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Method '{method}' on '{type_name}' is private"), span)
        .with_hint("Call this method from a method on the declaring type, or mark the method `pub`")
}

/// Report a call, from another module, to a method that takes the visibility of a trait that is not `pub` (#2570).
pub fn module_private_method(type_name: &str, method: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Method '{method}' on '{type_name}' is private to the module of its trait"),
        span,
    )
    .with_hint("Call this method from the module that declares its trait, or mark the trait `pub`")
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

/// Unsafe Rust calls have no author-facing safety contract, including inside checked C acknowledgment blocks.
pub fn rust_unsafe_call_not_supported(path: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Unsafe Rust call `rust::{path}` is unsupported: unsafe interop is out of scope"),
        span,
    )
    .with_note("RFC 005 excludes unsafe Rust calls; `unsafe:` acknowledges checked C bindings only")
    .with_hint("Use a safe Rust API whose signature the compiler can validate")
}

/// A source return or field type names a lifetime-bearing Rust type, whose lifetime contract no source type can state.
pub fn rust_borrow_without_lifetime_contract(path: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Rust type `rust::{path}` borrows data, so a source return or field type cannot name it"),
        span,
    )
    .with_stable_code("INCAN-T0120")
    .with_note("DD-0006 excludes returned and stored Rust values whose lifetime relationships are not retained")
    .with_hint("Keep the borrowed value inside the function that creates it, or return data that owns its contents")
}

/// A Rust associated call tries to specialize a receiver whose turbofish contains const-generic slots.
pub fn rust_receiver_const_generics_not_supported(path: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Rust receiver `rust::{path}` has const generic parameters that Incan cannot specialize"),
        span,
    )
    .with_hint("Use a Rust or Incan wrapper whose public receiver has type parameters only")
    .with_note("Incan v0.5 does not accept const values in call-site type-argument syntax")
}

/// Report a Rust associated call whose owner type arguments nothing in the program fixes (#1720).
///
/// `HashMap.new()` leaves `K` and `V` open; a later use of the binding fixes them, such as an insert, a typed parameter
/// or a typed return. Reads whose checking needs no open parameter, such as a length query, leave them open. This
/// refusal covers a discarded result or a binding with no type-determining use; a first use that needs them but fixes
/// nothing is refused with `INCAN-T0122` instead. `owner` is the receiver as the source spells it (`HashMap`), `method`
/// the associated function, `type_params` the owner's open parameters, and `binding` the local the result was bound to,
/// or `None` when the value was discarded. The hint shows both spellings that close the parameters, with example
/// types sized to the parameter count. `INCAN-T0105` is its stable code.
pub fn rust_owner_type_args_not_inferred(
    owner: &str,
    method: &str,
    type_params: &[String],
    binding: Option<&str>,
    span: Span,
) -> CompileError {
    const EXAMPLE_TYPES: [&str; 4] = ["str", "int", "float", "bool"];
    let params = type_params.join(", ");
    let examples = EXAMPLE_TYPES
        .iter()
        .cycle()
        .take(type_params.len())
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    let error = CompileError::type_error(
        format!("Cannot infer the type arguments '{params}' of '{owner}.{method}()'"),
        span,
    )
    .with_stable_code("INCAN-T0105");
    match binding {
        Some(name) => error
            .with_note(format!("'{name}' has no later use that determines these type arguments"))
            .with_hint(format!(
                "Write them in the call, '{owner}.{method}[{examples}]()', or annotate the binding, '{name}: {owner}[{examples}] = {owner}.{method}()'"
            )),
        None => error
            .with_note("The value is not bound to a name, so nothing later fixes them")
            .with_hint(format!("Write them in the call: '{owner}.{method}[{examples}]()'")),
    }
}

/// A trait-qualified call `Trait.method(receiver, ...)` names an imported Rust trait whose method signature is not
/// available, so the compiler cannot tell whether the receiver must be borrowed exclusively, shared, or moved.
///
/// The receiver mode is a fact of the Rust declaration. Guessing `&self` produces Rust that rustc rejects against
/// generated code the author never wrote, so the call is refused with the missing fact named instead (#1375).
pub fn rust_trait_receiver_mode_unavailable(trait_path: &str, method: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "Cannot determine the receiver of `rust::{trait_path}.{method}`: no signature metadata is available for \
             this Rust trait method"
        ),
        span,
    )
    .with_hint(format!(
        "Call it as a method on the receiver value (`value.{method}(...)`), or refresh Rust metadata for `{trait_path}`"
    ))
    .with_note("A trait-qualified call passes its receiver explicitly, and only the trait's declared receiver says whether that argument is `&mut self`, `&self`, or `self`")
}

/// A trait-qualified call names a method the inspected Rust trait does not declare.
///
/// Rust resolves `Trait::method` only against the trait's own associated items, so a method the trait inherits or
/// does not have at all fails native compilation; report it against the source call instead (#1375).
pub fn rust_trait_method_not_declared(trait_path: &str, method: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Rust trait `rust::{trait_path}` does not declare an associated function `{method}`"),
        span,
    )
    .with_hint("Qualify the call with the trait that declares the method, or call it as a method on the receiver value")
}

/// Why native callee selection found no single Rust callee for a call whose arguments check (#2481).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeCalleeGap<'a> {
    /// Type parameters of the callee that nothing in the call fixes, by name.
    OpenTypeParameters(&'a [String]),
    /// No admitted implementation of the method applies to the receiver and the arguments.
    NoCandidate,
    /// Several implementations apply equally, by native path.
    Ambiguous(&'a [String]),
}

/// A native method call whose arguments check, but for which no single Rust callee can be selected (#2481).
///
/// A native call compiles only through the callee the checker selects for it, so a call without one cannot be built;
/// it is refused at checking instead of at the build. `callee` names the requested method
/// (`rust::std::process::Command.args`) and `gap` says why selection failed. `INCAN-T0122` is its stable code.
pub fn rust_callee_not_selected(callee: &str, gap: NativeCalleeGap<'_>, span: Span) -> CompileError {
    let quoted = |names: &[String]| {
        let names = names.iter().map(|name| format!("`{name}`")).collect::<Vec<_>>();
        match names.as_slice() {
            [] => String::new(),
            [only] => only.clone(),
            [init @ .., last] => format!("{} and {last}", init.join(", ")),
        }
    };
    let (reason, note, hint) = match gap {
        NativeCalleeGap::OpenTypeParameters(names) => (
            match names {
                [_] => format!("its type parameter {} is not fixed by the call", quoted(names)),
                _ => format!("its type parameters {} are not fixed by the call", quoted(names)),
            },
            "Nothing in the receiver, the arguments, the type the result is expected to have or the first use of the \
             local it is bound to fixes them, and a native call compiles only through a callee whose type arguments \
             are all known",
            "Write the type arguments in the call, annotate the type the result is bound to, or pass an argument \
             whose type fixes them",
        ),
        NativeCalleeGap::NoCandidate => (
            "no admitted implementation of the method applies to this receiver and these arguments".to_string(),
            "An imported trait may declare the method while the receiver's type has no admitted implementation of \
             that trait",
            "Call the method on a value whose type implements it, or convert the receiver to such a type first",
        ),
        NativeCalleeGap::Ambiguous(candidates) => (
            format!("{} apply equally", quoted(candidates)),
            "Several implementations accept the receiver and the arguments, and nothing in the call chooses one",
            "Call a method or conversion whose signature names the type it produces",
        ),
    };
    CompileError::type_error(format!("Cannot select the Rust callee of `{callee}`: {reason}"), span)
        .with_stable_code("INCAN-T0122")
        .with_note(note)
        .with_hint(hint)
}

/// A Rust import is being used as a constructor, but the compiler lacks enough metadata to emit valid Rust.
pub fn rust_constructor_metadata_unavailable(path: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Cannot construct imported Rust item `rust::{path}` without constructor metadata"),
        span,
    )
    .with_hint("Use an associated Rust constructor method, a helper function, or refresh Rust metadata for this item")
    .with_note("Named-field Rust structs require field metadata so Incan can emit a Rust struct literal")
}

/// A Rust type's admitted metadata names no associated item, variant or field of the written name.
pub fn rust_associated_item_metadata_unavailable(path: &str, item: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Rust type `rust::{path}` has no admitted associated item `{item}`"),
        span,
    )
    .with_hint("Check the item's spelling, or call the associated function or method that provides the value")
    .with_note("Rust associated items are read only from the type's admitted metadata")
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

/// A rusttype alias cannot author a new impl for a foreign Rust trait on the foreign backing type.
pub fn rusttype_foreign_trait_impl_orphan(type_name: &str, trait_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!(
            "`{type_name}` cannot implement foreign Rust trait `{trait_name}` because its `rusttype` backing type is foreign"
        ),
        span,
    )
    .with_hint("Use body-less adoption only when the backing Rust type already implements the trait")
    .with_note("Rust coherence treats a type alias as the backing type, not as a new local type")
}

/// Rusttype forwarding needs metadata proof before lowering can skip the illegal alias impl.
pub fn rusttype_forwarding_requires_metadata(type_name: &str, trait_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Cannot forward Rust trait `{trait_name}` for rusttype `{type_name}` without metadata proof"),
        span,
    )
    .with_hint("Provide Rust ABI metadata for the backing type or use a local newtype wrapper instead")
}

/// Metadata proves the backing type does not satisfy a body-less rusttype trait adoption.
pub fn rusttype_forwarding_trait_not_implemented(
    type_name: &str,
    backing_type: &str,
    trait_name: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!(
            "Backing type `{backing_type}` does not implement Rust trait `{trait_name}` for rusttype `{type_name}`"
        ),
        span,
    )
    .with_hint("Remove the adoption, choose a backing type that implements the trait, or use a local newtype wrapper")
}

/// Forwarding generic Rust trait instantiations needs metadata richer than the current trait path list.
pub fn rusttype_forwarding_generic_trait_blocked(type_name: &str, trait_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Cannot safely forward generic Rust trait `{trait_name}` for rusttype `{type_name}` yet"),
        span,
    )
    .with_hint("Use a local newtype wrapper for a handwritten impl when Rust metadata omits trait type arguments")
}

/// A class implements a Rust trait only when both are concrete and the trait declares nothing but methods.
pub fn class_rust_trait_adoption_unsupported(class_name: &str, trait_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Class `{class_name}` cannot implement Rust trait `{trait_name}`"),
        span,
    )
    .with_hint("A class implements a Rust trait without type parameters, associated types or associated constants")
}

/// A class method fills a Rust trait method only as a concrete instance method with a body.
pub fn class_rust_trait_method_unsupported(
    class_name: &str,
    method_name: &str,
    trait_name: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!("Method `{method_name}` of class `{class_name}` cannot implement Rust trait `{trait_name}`"),
        span,
    )
    .with_hint("Implement a Rust trait method as an instance method with a body and without type parameters")
}

/// Awaitable-to-Future rusttype bridging is intentionally gated until pin/projection metadata exists.
pub fn awaitable_future_bridge_blocked(type_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("`Awaitable[T]` to Rust `Future` bridging is not implemented for rusttype `{type_name}` yet"),
        span,
    )
    .with_hint("Keep the Rust Future adapter in handwritten Rust for now")
    .with_note("Safe generation needs Pin projection and Future output metadata that this slice does not have")
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
    .with_related_span(first_span, "First interop edge declaration")
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

/// Build the diagnostic for two fields that claim the same serialized alias.
pub fn duplicate_alias(type_name: &str, alias: &str, first_span: Span, second_span: Span) -> CompileError {
    CompileError::type_error(
        format!("Duplicate alias '{}' on type '{}'", alias, type_name),
        second_span,
    )
    .with_related_span(first_span, format!("First field alias '{alias}'"))
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
