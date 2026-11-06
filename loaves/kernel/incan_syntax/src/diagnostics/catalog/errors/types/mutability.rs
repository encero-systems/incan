//! Mutability and match exhaustiveness errors.

use super::*;

// -- Mutability --------------------------------------------------------------

pub fn mutation_without_mut(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Cannot mutate '{}' - variable is immutable", name), span)
        .with_hint(format!("Declare with 'mut' to allow mutation: mut {} = ...", name))
        .with_note("In Incan, variables are immutable by default for safety")
        .with_note("This prevents accidental modifications and makes code easier to reason about")
}

/// The place a pattern or a `for` loop bound a name from, when that place does not permit a change through the name
/// (#1561).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOnlyScrutinee<'a> {
    /// A local binding declared without `mut`.
    Binding(&'a str),
    /// A parameter declared without `mut`.
    Parameter(&'a str),
    /// A module static, which a bound name cannot change in place.
    Static(&'a str),
    /// A local bound directly to a module static, which names the static's own storage.
    StaticAlias { alias: &'a str, static_name: &'a str },
    /// The items of a place a generator expression reads, which it holds as copies; the place's root name.
    GeneratorItems(&'a str),
    /// A local a closure captures, which the closure holds as its own copy.
    Captured(&'a str),
    /// `self` in a method that takes a plain `self` receiver.
    SelfReceiver,
    /// A dict value, which a pattern binds as a copy.
    DictValue,
}

/// Refuse a closure that captures local values where a function type holds only a named function or a closure that
/// captures nothing (#1561).
///
/// `what` names the value and ends where the sentence continues ("A closure that captures local values", "'g', which
/// holds a closure that captures local values,"); `slot` says where it is held ("stored in a list").
pub fn capturing_callable_in_function_slot(what: &str, slot: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("{what} cannot be {slot}"), span)
        .with_hint("Bind the closure to a new local and call it there, or pass it to a parameter its function only calls")
        .with_note(
            "A function type holds a named function or a closure that captures nothing. A closure that captures local values is held by a new local, by a function-typed parameter of a function of this module that only calls the parameter and is only called by name, and by the one 'return' of such a function",
        )
}

/// Refuse native callback storage when one named capture lacks the callee's required auto-trait evidence.
/// Missing metadata is an admission gap rather than a claim that the captured Rust type has a negative impl.
pub fn rust_callback_capture_missing_auto_trait(name: &str, ty: &str, marker: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Captured value '{name}' of type '{ty}' lacks retained `{marker}` evidence for this Rust callback"),
        span,
    )
    .with_note("The callback owns its captures; every captured value must satisfy the Rust callee's storage bounds")
    .with_hint(format!("Use captured values with a retained `{marker}` implementation"))
}

/// Refuse a method of a value as a local partial's target (RFC 084): an overloaded method, a generic method, or one
/// that takes `mut self`. `reason` says which, as the end of the sentence the message starts.
pub fn local_partial_method_target_refused(method: &str, reason: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("A local partial of the method '{method}' is not supported: {reason}"),
        span,
    )
}

/// Refuse a change, inside a closure, through a local the closure captures from its enclosing callable (#1561).
///
/// A closure reads each outer local it names as the value the local holds when the closure is constructed, so a change
/// through `name` would change the closure's own copy and never reach the local. A method that changes its receiver,
/// and a field or element write, are refused alike.
pub fn change_through_closure_capture(name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Cannot change '{name}' inside a closure - the closure captures its own copy of it"),
        span,
    )
    .with_hint(format!("Change '{name}' outside the closure"))
    .with_note("A closure reads each outer local it names as the value the local holds when the closure is constructed")
}

/// Refuse a change through a name a `match`, `if let` or `while let` pattern, or a `for` loop, binds from a place that
/// does not permit it (#1561).
///
/// A name a pattern or a loop binds from a place is part of that place, so a change through the name is a change to the
/// place: a `mut` binding or parameter, a field or list element of one, or `self` in a `mut self` method takes the
/// change. The place named by `place` does not, so the change is refused rather than applied to a copy or left to fail
/// in the build. `binding` is the name the change goes through.
pub fn change_through_binding_of_read_only_place(
    binding: &str,
    place: ReadOnlyScrutinee<'_>,
    span: Span,
) -> CompileError {
    let (source, hint) = match place {
        ReadOnlyScrutinee::Binding(name) => (
            format!("'{name}', which is immutable"),
            format!("Declare '{name}' with 'mut' to change it through '{binding}': mut {name} = ..."),
        ),
        ReadOnlyScrutinee::Parameter(name) => (
            format!("the parameter '{name}', which is not declared 'mut'"),
            format!("Declare the parameter 'mut' to change the caller's value through '{binding}': mut {name}: ..."),
        ),
        ReadOnlyScrutinee::Static(name) => (
            format!("the static '{name}', which is changed only through its own name"),
            format!("Change a copy of its value in a 'mut' variable, then assign the copy back to '{name}'"),
        ),
        ReadOnlyScrutinee::Captured(name) => (
            format!("'{name}', which the closure captures as its own copy"),
            format!("Change '{name}' outside the closure"),
        ),
        ReadOnlyScrutinee::GeneratorItems(place) => (
            format!("'{place}' by a generator expression, which holds copies of the items it reads"),
            format!("Change the items of '{place}' in a list comprehension or a 'for' loop"),
        ),
        ReadOnlyScrutinee::StaticAlias { alias, static_name } => (
            format!("'{alias}', which names the static '{static_name}', which is changed only through its own name"),
            format!("Change a copy of its value in a 'mut' variable, then assign the copy back to '{static_name}'"),
        ),
        ReadOnlyScrutinee::SelfReceiver => (
            "'self', which this method takes as plain 'self'".to_string(),
            format!("Declare the method with 'mut self' to change 'self' through '{binding}'"),
        ),
        ReadOnlyScrutinee::DictValue => (
            "a dict value, which a pattern binds as a copy".to_string(),
            "Bind the value to a 'mut' variable, change it, and store it back into the dict".to_string(),
        ),
    };
    CompileError::type_error(format!("Cannot change '{binding}' - it is bound from {source}"), span)
        .with_hint(hint)
        .with_note(
            "A name a pattern or a `for` loop binds from a place is part of that place, so a change through the name changes the place",
        )
}

/// How a method body changes the object its plain `self` receiver names (#1723).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfMutation<'a> {
    /// The body assigns to a place rooted at the receiver, `self.width *= factor` or `self.items[0] = value`.
    Assignment,
    /// The body calls a method that changes a place rooted at the receiver, `self.items.pop()`.
    MutatingCall {
        /// The method the body calls on the place.
        callee: &'a str,
    },
}

/// Report a method that changes the object it is called on while its receiver is a plain `self` (#1723).
///
/// The receiver spelling is a promise the generated code keeps literally: a plain `self` method reads the object
/// only, so an assignment to `self.width` or a `self.items.pop()` inside it has no way to write. `method` is the
/// method being checked, `target` the place it changes as the source spells it (`self.width`, `self.items[0]`), and
/// `mutation` says whether the body assigns to that place or calls a method that changes it. The hint spells the
/// receiver to declare; `INCAN-T0102` is its stable code.
pub fn self_mutation_requires_mut_self(
    method: &str,
    target: &str,
    mutation: SelfMutation<'_>,
    span: Span,
) -> CompileError {
    let message = match mutation {
        SelfMutation::Assignment => format!("Method '{method}' assigns to '{target}' but takes 'self'"),
        SelfMutation::MutatingCall { callee } => {
            format!("Method '{method}' calls '{target}.{callee}()', which changes '{target}', but takes 'self'")
        }
    };
    CompileError::type_error(message, span)
        .with_stable_code("INCAN-T0102")
        .with_hint(format!("Declare the receiver as 'mut self': def {method}(mut self, ...)"))
        .with_note("A method that changes the object it is called on says so in its receiver; a plain 'self' method only reads it")
}

/// The argument an `INCAN-T0117` refusal names, which decides the remedy its hint spells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutArgumentPlace {
    /// A binding declared without `mut`, by name.
    Binding(String),
    /// A field reached from a binding declared without `mut`.
    Field,
    /// An element of a list or a dict, which a call receives as a copy of the stored value.
    Element,
    /// A module static, which a call receives as a copy of its current value.
    Static,
    /// A `for` loop variable, which cannot be declared `mut`: its name, and whether the loop's pattern destructures
    /// each element (`for xs, n in pairs:`).
    LoopVariable { name: String, destructured: bool },
    /// A local a closure captures from its enclosing callable, which the closure holds as its own copy.
    Captured(String),
}

/// How an `INCAN-T0117` refusal names the `mut` parameter: by its declared name, or by its position when the callee is
/// known only by a callable type whose parameters have no names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutParameterLabel<'a> {
    /// A declared parameter name.
    Named(&'a str),
    /// A 1-based position in the callable type's parameter list.
    Position(usize),
}

impl MutParameterLabel<'_> {
    /// Spell the parameter inside a sentence: `'items'` or `at position 1`.
    fn in_sentence(self) -> String {
        match self {
            Self::Named(name) => format!("'{name}'"),
            Self::Position(position) => format!("at position {position}"),
        }
    }

    /// Name a `mut` variable a remedy binds the value to.
    fn binding_name(self) -> String {
        match self {
            Self::Named(name) => name.to_string(),
            Self::Position(_) => "value".to_string(),
        }
    }
}

/// Whether an `INCAN-T0117` refusal's callee is known to change the `mut` parameter or may change it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutParameterChange {
    /// The body that runs changes the parameter.
    Changes,
    /// The body that runs is not known at the call (a method reached by trait dispatch, a callable known only by its
    /// type, a declaration whose body the check does not read), and it may change the parameter.
    MayChange,
}

/// Refuse an argument for a `mut` parameter whose changes reach the caller when the callee changes it and the change
/// would fail to reach the argument (#1773).
///
/// A `mut` parameter of any type but `int`, `float`, `bool` or a Rust type, and not a rest parameter, shows the
/// callee's changes to the caller. When the callee does change it, the argument must be a place the caller may change:
/// an immutable binding or a field of one cannot be changed, and an element or a static reaches the callee as a copy,
/// so the change would be lost. `parameter` and `callee` name the declaration and `place` the argument, which picks the
/// remedy; a callee known only by its callable type names the parameter by position. `change` says whether the callee
/// is known to change the parameter or may change it. `INCAN-T0117` is its stable code.
pub fn immutable_argument_to_mut_parameter(
    parameter: MutParameterLabel<'_>,
    callee: &str,
    change: MutParameterChange,
    place: MutArgumentPlace,
    span: Span,
) -> CompileError {
    let hint = mut_argument_hint(&place, parameter);
    let parameter = parameter.in_sentence();
    CompileError::type_error(
        format!("Argument for the 'mut' parameter {parameter} of '{callee}' must be a mutable binding"),
        span,
    )
    .with_stable_code("INCAN-T0117")
    .with_hint(hint)
    .with_note(format!(
        "'{callee}' {} the parameter {parameter}, and its changes are visible to the caller, so the caller passes a binding declared with 'mut'",
        match change {
            MutParameterChange::Changes => "changes",
            MutParameterChange::MayChange => "may change",
        }
    ))
}

/// Refuse an immutable argument for a `mut` parameter whose changes reach the caller when the callee does not change
/// the parameter but the argument cannot be copied (#1561).
///
/// An argument the caller may not change reaches a parameter the call never changes as a copy of its value. A value of
/// `value_type`, such as a `Generator`, has no copy, so the argument has to be a place the caller may change, as for a
/// parameter the call changes. `parameter`, `callee` and `place` are as for [`immutable_argument_to_mut_parameter`].
/// `INCAN-T0117` is its stable code.
pub fn uncopyable_argument_to_mut_parameter(
    parameter: MutParameterLabel<'_>,
    callee: &str,
    value_type: &str,
    place: MutArgumentPlace,
    span: Span,
) -> CompileError {
    let hint = mut_argument_hint(&place, parameter);
    let parameter = parameter.in_sentence();
    CompileError::type_error(
        format!("Argument for the 'mut' parameter {parameter} of '{callee}' must be a mutable binding"),
        span,
    )
    .with_stable_code("INCAN-T0117")
    .with_hint(hint)
    .with_note(format!(
        "'{callee}' does not change the parameter {parameter}, so an argument the caller may not change is passed as a copy, and a '{value_type}' cannot be copied"
    ))
}

/// Refuse a native field projected through a shared owner when a source parameter requests exclusive access.
///
/// Even a read-only callee cannot form the `&mut` required by its parameter from the retained shared native loan.
/// `INCAN-T0117` remains the stable code for mutable-argument authority failures.
pub fn shared_native_field_to_mut_parameter(
    parameter: MutParameterLabel<'_>,
    callee: &str,
    span: Span,
) -> CompileError {
    let binding = parameter.binding_name();
    let parameter = parameter.in_sentence();
    CompileError::type_error(
        format!("Argument for the 'mut' parameter {parameter} of '{callee}' requires exclusive access"),
        span,
    )
    .with_stable_code("INCAN-T0117")
    .with_hint(format!(
        "Remove 'mut' from the read-only parameter, or copy the field into a 'mut' binding first: mut {binding} = ..."
    ))
    .with_note("The native field is reached through a shared owner, which cannot grant an exclusive loan")
}

/// Spell the remedy for an `INCAN-T0117` argument: declare the binding `mut`, or pass a `mut` variable in its place.
fn mut_argument_hint(place: &MutArgumentPlace, parameter: MutParameterLabel<'_>) -> String {
    let binding = parameter.binding_name();
    match place {
        MutArgumentPlace::Binding(name) => format!("Declare '{name}' with 'mut' where it is bound: mut {name} = ..."),
        MutArgumentPlace::Field => "Declare the binding the field belongs to with 'mut'".to_string(),
        MutArgumentPlace::Element | MutArgumentPlace::Static => format!(
            "Bind the value to a 'mut' variable, pass the variable, and store it back: mut {binding} = ..., then assign it to the element or static"
        ),
        MutArgumentPlace::LoopVariable {
            name,
            destructured: false,
        } => format!(
            "A 'for' loop variable cannot be declared 'mut': loop over the indexes instead, bind each element to a 'mut' variable, pass that variable, and store it back: mut {name} = ...[i], then ...[i] = {name}"
        ),
        MutArgumentPlace::LoopVariable {
            name,
            destructured: true,
        } => format!("A 'for' loop variable cannot be declared 'mut', so '{name}' cannot be passed to this parameter"),
        MutArgumentPlace::Captured(name) => format!(
            "A closure holds its own copy of '{name}', which it captures, so the call could not change '{name}': make the call outside the closure"
        ),
    }
}

/// Refuse rebinding a `mut` parameter whose changes reach the caller inside its own body (#1773).
///
/// The caller sees what the callee does to such a parameter's value, not a new value bound to its name, so an
/// assignment or compound assignment to the parameter itself could only be lost. Changes in place (appending, element
/// or field assignment) stay allowed; a parameter of type `int`, `float` or `bool` is the callee's own copy and may be
/// rebound freely.
pub fn caller_visible_mut_parameter_rebinding(name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Cannot rebind the 'mut' parameter '{name}': its changes are visible to the caller"),
        span,
    )
    .with_hint(format!(
        "Change '{name}' in place (append to it, assign its elements or fields), or bind the new value to a new name"
    ))
    .with_note("The caller sees changes made to the value it passed, not a new value bound to the parameter's name")
}

/// How the hint of a refused hold on a caller-visible `mut` parameter spells an independent copy of its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutParameterCopy {
    /// A copy written as one expression, such as `list(items)`.
    Expression(String),
    /// No single expression copies a value of the parameter's type; the copy is a new value built from it.
    NewValue,
}

/// Refuse holding a `mut` parameter whose changes reach the caller in a new binding or another value (#1773).
///
/// `other = items`, a literal, comprehension, field store, construction or `partial` preset holding `items`, a
/// `match`, `if`, `break` or `yield` value that is `items`, a `match items:` arm that binds it and a closure that
/// returns it, changes it or passes it on to a parameter that may change it would each hold the parameter's value under
/// another name. Whether such a holder shares the caller's value or copies it is not defined, so the parameter is used
/// only directly. `copy` spells the independent copy the hint offers.
pub fn caller_visible_mut_parameter_held(name: &str, copy: &MutParameterCopy, span: Span) -> CompileError {
    let copy = match copy {
        MutParameterCopy::Expression(expression) => format!("write {expression}"),
        MutParameterCopy::NewValue => format!("build a new value from '{name}'"),
    };
    CompileError::type_error(
        format!("The 'mut' parameter '{name}' cannot be bound to another name or held in another value"),
        span,
    )
    .with_hint(format!(
        "Change '{name}' directly, or pass it to a function that takes a 'mut' parameter; for an independent copy, {copy}"
    ))
    .with_note(format!(
        "'{name}' shows its changes to the caller; whether another name or value holding it shares the caller's value or copies it is not defined, so it is used only directly"
    ))
}

/// A Rust interop parameter requires an exclusive borrow of an immutable Incan binding.
pub fn mutable_rust_borrow_requires_mut(name: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("Rust parameter requires a mutable borrow of '{name}'"), span).with_hint(format!(
        "Declare with 'mut' before passing '{name}' to this Rust method"
    ))
}

/// A checked C binding parameter requires an exclusive borrow of an immutable Incan resource binding.
pub fn mutable_c_borrow_requires_mut(name: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("C binding parameter requires a mutable borrow of '{name}'"),
        span,
    )
    .with_hint(format!(
        "Declare with 'mut' before passing '{name}' to this C binding symbol"
    ))
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
