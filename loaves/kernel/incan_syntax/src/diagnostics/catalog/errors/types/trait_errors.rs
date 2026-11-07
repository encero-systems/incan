//! Trait conformance and validate-derive errors.

use super::*;

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

/// Refuse a type used as a callable when no checked constructor supplies its value semantics.
pub fn type_constructor_unavailable(type_name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Type '{type_name}' has no constructor"), span)
        .with_hint("Use an associated constructor or an explicit conversion method")
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

/// Emitted when a trait is written inside another type in a callable signature or a local annotation, such as
/// `Option[Serialize]` or `list[Serialize]` (#1866).
///
/// A trait stands for the types that adopt it only as a whole parameter or return type, where the value is one hidden
/// adopting type. Inside another type it has no value representation, so the annotation is refused rather than
/// generated as a bare trait.
pub fn trait_type_nested_in_annotation_unsupported(trait_type: &str, annotation: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Trait '{trait_type}' cannot be used inside the type '{annotation}'"),
        span,
    )
    .with_hint(format!(
        "Use a type that adopts '{trait_type}', or a type parameter bounded by it (`T with {trait_type}`) in its place"
    ))
    .with_note("A trait names the types that adopt it only as a whole parameter or return type")
}

/// Emitted when a model or class field stores a trait directly or inside a container.
pub fn trait_typed_field_annotation_unsupported(
    owner_kind: &str,
    owner_name: &str,
    field_name: &str,
    annotation: &str,
    trait_name: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!(
            "Trait-typed field '{owner_name}.{field_name}' has unsupported {owner_kind} storage type '{annotation}'"
        ),
        span,
    )
    .with_hint(format!(
        "Use a concrete type that adopts '{trait_name}' for the stored field"
    ))
    .with_note("Trait annotations are supported on callable boundaries and `with` adoption clauses, not stored fields")
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
pub fn duplicate_trait_instantiation(
    trait_name: &str,
    type_args: &str,
    first_span: Span,
    duplicate_span: Span,
) -> CompileError {
    CompileError::type_error(
        format!(
            "Trait '{}' is adopted more than once with type arguments [{}]",
            trait_name, type_args
        ),
        duplicate_span,
    )
    .with_related_span(first_span, "First matching trait adoption")
    .with_hint("Remove the duplicate `with` entry or use a different trait type argument")
}

/// Report `@rust.derive` attached to a declaration kind that cannot emit Rust derive attributes.
pub fn rust_derive_unsupported_attachment(kind: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("@rust.derive is not supported on {kind} declarations"), span)
        .with_hint("@rust.derive currently lowers only onto generated Rust structs and enums")
}

/// Report `@rust.derive` on a `rusttype`, which currently lowers to a Rust alias.
pub fn rust_derive_unsupported_rusttype(span: Span) -> CompileError {
    CompileError::type_error(
        "@rust.derive is not supported on rusttype declarations yet".to_string(),
        span,
    )
    .with_hint("rusttype currently lowers to a Rust type alias, and Rust derive attributes cannot attach to aliases")
}

/// Report an empty `@rust.derive(...)` invocation.
pub fn rust_derive_requires_positional_arg(span: Span) -> CompileError {
    CompileError::type_error(
        "@rust.derive requires at least one positional derive name".to_string(),
        span,
    )
    .with_hint("Use @rust.derive(Clone, Debug) or @rust.derive(\"serde::Serialize\")")
}

/// Report a named argument passed to `@rust.derive(...)`.
pub fn rust_derive_rejects_named_args(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("@rust.derive does not accept named argument '{name}'"), span)
        .with_hint("Pass Rust derive names positionally")
}

/// Report a derive argument that is not an identifier or Rust path string.
pub fn rust_derive_invalid_arg(span: Span) -> CompileError {
    CompileError::type_error(
        "@rust.derive arguments must be identifiers or Rust path strings".to_string(),
        span,
    )
    .with_hint(
        "Use a built-in derive name, an imported Rust derive binding, or a string path like \"serde::Serialize\"",
    )
}

/// Report a Rust derive name that cannot be resolved to a built-in or imported derive macro.
pub fn rust_derive_unresolved(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Rust derive '{name}' is not resolved"), span)
        .with_hint("Import third-party derives through rust:: imports or declare its Loaf under [dependencies]")
}

/// Report a Rust derive that would generate the same impl as an explicit `with` adoption.
pub fn rust_derive_conflicts_with_trait_adoption(derive_name: &str, trait_name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("@rust.derive({derive_name}) conflicts with explicit `with {trait_name}` adoption"),
        span,
    )
    .with_hint("Use either Rust derive passthrough or an explicit trait adoption, not both")
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

/// Report `pub` on a method that implements a method of an adopted trait, which takes the trait's visibility (#2570).
pub fn pub_modifier_not_allowed_on_trait_implementation(
    type_name: &str,
    trait_name: &str,
    method: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!("Method '{method}' on '{type_name}' implements trait '{trait_name}' and cannot be marked `pub`"),
        span,
    )
    .with_hint(format!(
        "Remove `pub`; a method that implements a trait method takes the visibility of trait '{trait_name}'"
    ))
}

/// Report `pub` on a language hook, a method the language calls by its name, which is public by rule (#2570).
pub fn pub_modifier_not_allowed_on_language_hook(type_name: &str, method: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Method '{method}' on '{type_name}' is a language hook and cannot be marked `pub`"),
        span,
    )
    .with_hint(format!(
        "Remove `pub`; the language calls '{method}' by its name, so it is public wherever '{type_name}' is"
    ))
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

/// Report an abstract method spelling used where a concrete type method body is required.
pub fn concrete_method_requires_body(method: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Method '{}' must have a body outside trait declarations", method),
        span,
    )
    .with_hint("Use `:` followed by an indented method body")
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

/// Report a model, class, enum or newtype adopting `Awaitable[T]`: the wrapper form of RFC 039 has no realization.
///
/// `Awaitable[T]` is realized only as a generic bound (`F with Awaitable[T]`), where every future-like value
/// satisfies it. Nothing can build a declared type as an awaitable wrapper, and a model, class or enum always derives
/// `Clone` and `Debug`, which an awaitable handle such as `JoinHandle[T]` cannot be.
pub fn awaitable_adoption_not_realized(type_name: &str, expected_output: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Type '{type_name}' cannot adopt Awaitable[{expected_output}]: a declared type is never an awaitable wrapper"),
        span,
    )
    .with_hint("Await the JoinHandle[T] itself, or take the awaitable through a generic bound (`F with Awaitable[T]`)")
    .with_note(
        "Awaitable[T] is realized only as a bound over future-like values; a model, class or enum also always derives Clone and Debug, which an awaitable handle such as JoinHandle[T] cannot be",
    )
}

/// Report a required Rust associated type that has no adopting-type declaration.
pub fn missing_rust_associated_type(trait_name: &str, associated_type: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Trait '{trait_name}' requires associated type '{associated_type}' to be implemented"),
        span,
    )
    .with_hint(format!(
        "Add `type {associated_type} for {trait_name} = ...` in the adopting type body"
    ))
    .with_note("Rust trait associated types must be explicit when metadata exposes the requirement")
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

/// Report that a type does not meet a required trait and add the most useful source-level remedy.
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
        Some(DeriveId::Validate) => {
            error = error.with_hint("Add @derive(Validate) to enable validated construction via TypeName.new(...)");
            error = error.with_hint("Then implement: def validate(self) -> Result[Self, E]: ...");
        }
        Some(DeriveId::Descriptor) => {
            error = error.with_hint(
                "@derive(Descriptor) enables structural registry snapshots; it does not add a runtime trait",
            );
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

/// Why an `Fn`-family capability marker cannot stand where it was written (#1716).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallableMarkerRefusal<'a> {
    /// The marker names more parameters than a marker can spell today.
    ParameterCount {
        /// How many parameters the marker names.
        count: usize,
        /// The most a marker can name.
        limit: usize,
    },
    /// The marker bounds a type parameter of a nominal declaration rather than of a function or method.
    NominalOwner {
        /// The declaration kind as the source spells it (`model`, `class`, `enum`, `trait`, `newtype`, `type`).
        owner_kind: &'a str,
        /// The declaration's name.
        owner_name: &'a str,
    },
}

/// Report an `Fn`-family capability marker written where no generated code can spell it (#1716).
///
/// A marker names a callable's parameter list and leaves the return type to the call that passes the value.
/// `marker` is the bound as the source spells it (`Fn[int, int, int]`), `type_param` the parameter it bounds, and
/// `parameter_types` the marker's type arguments rendered one by one, which the hint reuses to spell the
/// alternatives. A marker with more parameters than the limit has no bound to become; a marker on a nominal
/// declaration's type parameter has no call to learn its return type from, so the hint names the bound that
/// spells one. `INCAN-T0106` is its stable code.
pub fn callable_marker_not_supported(
    marker: &str,
    type_param: &str,
    parameter_types: &[String],
    refusal: CallableMarkerRefusal<'_>,
    span: Span,
) -> CompileError {
    let marker_name = marker.split('[').next().unwrap_or(marker);
    let parameters = parameter_types.join(", ");
    match refusal {
        CallableMarkerRefusal::ParameterCount { count, limit } => CompileError::type_error(
            format!("Callable marker '{marker}' on '{type_param}' names {count} parameters; a marker takes at most {limit}"),
            span,
        )
        .with_stable_code("INCAN-T0106")
        .with_hint(format!(
            "Write at most {limit} parameters in the marker, or gather the parameters into one model and write \
             '{marker_name}[ThatModel]'"
        ))
        .with_note("The marker's type arguments are the callable's parameter list; its return type comes from the value passed"),
        CallableMarkerRefusal::NominalOwner { owner_kind, owner_name } => CompileError::type_error(
            format!("Callable marker '{marker}' cannot bound type parameter '{type_param}' of {owner_kind} '{owner_name}'"),
            span,
        )
        .with_stable_code("INCAN-T0106")
        .with_hint(format!(
            "Name the return type: bound '{type_param}' with 'Callable{}[{parameters}{}R]' from std.traits.callable, with \
             'R' the return type, or give the field a function type '({parameters}) -> R'",
            parameter_types.len(),
            if parameter_types.is_empty() { "" } else { ", " }
        ))
        .with_note(format!(
            "A callable marker leaves its return type to the call that passes the value; a {owner_kind} has no such call"
        )),
    }
}

/// What the name in a `with` bound resolved to, which decides the remedy a bound violation can offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenericBoundTarget {
    /// The bound names a trait (source, builtin, or imported Rust), so a type argument can satisfy it.
    Trait,
    /// The bound names a type such as `float` or a model, which only that type satisfies (#1033).
    Type,
}

/// Where the type bound to a generic parameter at a call site came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeArgumentOrigin {
    /// Written in the call's bracket list (`cast[int](...)`).
    Explicit,
    /// Inferred from the value arguments or the expected result type.
    Inferred,
}

/// Report a call whose bound type argument does not satisfy a `with` bound on the callee's type parameter.
///
/// The hint names the type that failed rather than "the argument type", because with an explicit bracket list the
/// value arguments are not what is wrong, and it branches on what the bound names: a trait can be implemented or the
/// bound widened, while a type in bound position is satisfied only by that type. See #1373 and #1033.
pub fn generic_bound_not_satisfied(
    function_name: &str,
    type_param: &str,
    bound: &str,
    bound_target: GenericBoundTarget,
    actual: &str,
    actual_origin: TypeArgumentOrigin,
    span: Span,
) -> CompileError {
    let error = CompileError::type_error(
        format!(
            "Call to '{}' violates generic bound: type parameter '{}' requires '{}' but got '{}'",
            function_name, type_param, bound, actual
        ),
        span,
    );
    match (bound_target, actual_origin) {
        (GenericBoundTarget::Type, _) => error.with_hint(format!(
            "'{bound}' is a type, so only '{bound}' itself satisfies this bound; pass '{bound}' as the type \
             argument for '{type_param}', or call an overload whose bound accepts '{actual}'"
        )),
        (GenericBoundTarget::Trait, TypeArgumentOrigin::Explicit) => error.with_hint(format!(
            "Type argument '{actual}' for '{type_param}' must implement '{bound}'; pass a type that implements it \
             or widen the '{type_param}' bounds in '{function_name}'"
        )),
        (GenericBoundTarget::Trait, TypeArgumentOrigin::Inferred) => error.with_hint(format!(
            "Ensure the argument type '{actual}' implements '{bound}' or widen the '{type_param}' bounds in \
             '{function_name}'"
        )),
    }
}

/// Report a call that matched none of an overload set's candidates.
///
/// One diagnostic carries what every candidate wanted, so the reader is not told only what the first-declared
/// overload required. `candidates` pairs each candidate's rendered signature with the first reason it was rejected,
/// in declaration order. See #1373.
pub fn no_overload_accepts_call(function_name: &str, candidates: &[(String, String)], span: Span) -> CompileError {
    let mut error = CompileError::type_error(
        format!(
            "Call to '{}' matches none of its {} overloads",
            function_name,
            candidates.len()
        ),
        span,
    );
    for (signature, reason) in candidates {
        error = error.with_note(format!("candidate `{signature}` rejected: {reason}"));
    }
    error.with_hint(format!(
        "Change the call to satisfy one candidate, or add an overload of '{function_name}' for this call"
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

/// Report `/`, `//`, `%` or `**` applied to values of a type parameter (#1715).
///
/// These four operators follow the language's numeric rules (true division yields `float`, floor division and modulo
/// round toward negative infinity, power picks its result type from the exponent), which the concrete numeric types
/// carry and no trait abstracts. `+`, `-` and `*` become inferred bounds on the type parameter; for these four there
/// is no bound a type argument could satisfy, so the function could never be compiled for any argument. `operator`
/// is the operator as written, `type_param` the parameter's name and `dunder` the RFC 028 hook (`__mod__`) that a
/// bound trait could define to make the operator resolve through a trait instead. `INCAN-T0109` is its stable code.
pub fn operator_has_no_type_parameter_bound(
    operator: &str,
    type_param: &str,
    dunder: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!("Operator '{operator}' cannot be applied to values of type parameter '{type_param}'"),
        span,
    )
    .with_stable_code("INCAN-T0109")
    .with_hint(format!(
        "Declare the operands as 'int' or 'float' instead of '{type_param}', or bound '{type_param}' by a trait that \
         defines '{dunder}' so the operator resolves through that trait"
    ))
    .with_note(
        "'/', '//', '%' and '**' follow the language's numeric rules, which only the concrete numeric types carry; \
         unlike '+', '-' and '*', no bound on a type parameter stands for them",
    )
}

/// Report a `dict.get(key)` whose result is kept although the stored value type cannot be copied.
///
/// `get` answers with the stored value. A result that is only read (a `match`, `if let` or `while let` whose bindings
/// are only the argument of `len`, `print` or `println`, an f-string interpolation, or the receiver of a Rust method
/// with a shared receiver whose result is not kept, or are unused) needs no copy; any other result is the lookup's own
/// copy of the value, which `value_type` does not provide. `INCAN-T0118` is its stable code.
pub fn kept_dict_lookup_value_cannot_be_copied(value_type: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("`get` here has to return its own `{value_type}`, and `{value_type}` cannot be copied"),
        span,
    )
    .with_stable_code("INCAN-T0118")
    .with_hint(
        "Read the value where it is stored instead: `match table.get(key):` or `if let Some(item) = table.get(key):`, \
         where `item` is only passed to `len`, `print` or `println`, interpolated in an f-string, used to call a Rust \
         method with a shared receiver whose result is not kept (an Incan method taking `self` does not count), or not \
         used",
    )
    .with_note("`get` returns the stored value; a result that is returned, bound, passed on or changed is a copy of it")
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
