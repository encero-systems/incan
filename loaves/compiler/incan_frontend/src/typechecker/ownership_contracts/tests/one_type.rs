//! RFC 164 one type: a `Borrowed[T]` or `BorrowedMut[T]` contract presents the value type `T` to every caller, on
//! every dependency route, exactly as a caller in the declaring module sees it.

use super::*;
use std::sync::Arc;

/// Parse one module's source.
fn parsed(source: &str) -> Result<crate::ast::Program, Box<dyn std::error::Error>> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    Ok(crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?)
}

/// Check `consumer` against the provider module `views` on both dependency routes: collection of the provider's
/// source, and the interface the provider's own checker seals. Returns each route's diagnostics.
fn consumer_errors(
    provider: &str,
    consumer: &str,
) -> Result<Vec<(&'static str, Vec<crate::diagnostics::CompileError>)>, Box<dyn std::error::Error>> {
    Ok(consumer_checkers(provider, consumer)?
        .into_iter()
        .map(|(route, checker)| (route, checker.errors))
        .collect())
}

/// Check both authentic dependency routes while retaining their complete source-flow facts for ownership tests.
pub(super) fn consumer_checkers(
    provider: &str,
    consumer: &str,
) -> Result<Vec<(&'static str, TypeChecker)>, Box<dyn std::error::Error>> {
    consumer_checkers_with_package(provider, consumer, None)
}

/// Check both dependency routes inside one real package, so imported sibling-module identities remain Package.
pub(super) fn package_consumer_checkers(
    provider: &str,
    consumer: &str,
) -> Result<Vec<(&'static str, TypeChecker)>, Box<dyn std::error::Error>> {
    consumer_checkers_with_package(provider, consumer, Some("fixture_records"))
}

fn consumer_checkers_with_package(
    provider: &str,
    consumer: &str,
    package: Option<&str>,
) -> Result<Vec<(&'static str, TypeChecker)>, Box<dyn std::error::Error>> {
    let provider_program = parsed(provider)?;
    let consumer_program = parsed(consumer)?;
    let dependencies = [("views", &provider_program)];
    let path = vec!["views".to_string()];
    let mut collected = TypeChecker::default();
    collected.set_current_package_identity(package.map(str::to_owned));
    collected.register_dependency_module_path_segments("views", path.clone());
    if let Err(errors) = collected.check_with_imports(&consumer_program, &dependencies) {
        collected.errors = errors;
    }
    let mut builder = TypeChecker::default();
    builder.set_current_package_identity(package.map(str::to_owned));
    builder.set_current_module_path(Some(path.clone()));
    builder
        .check_program(&provider_program)
        .map_err(|errors| format!("provider: {errors:?}"))?;
    let exports = Arc::from(crate::module::exported_symbols(&provider_program));
    let interface = builder.seal_current_module_dependency_interface("views", &provider_program, exports, true);
    let mut sealed = TypeChecker::default();
    sealed.set_current_package_identity(package.map(str::to_owned));
    sealed.register_dependency_module_path_segments("views", path);
    sealed.select_dependency_interfaces(vec![interface]);
    if let Err(errors) = sealed.check_with_imports(&consumer_program, &dependencies) {
        sealed.errors = errors;
    }
    Ok(vec![("collected", collected), ("sealed", sealed)])
}

/// A fresh imported `Result` call may feed the compiler-selected `Result.map_err` combinator. The inner Package
/// function keeps its ordinary call fact; the builtin combinator publishes a call fact authenticated by its typed
/// operation, Member identity, callback signature and result. Same-spelled source methods and returned-view receivers
/// remain on their existing refusal routes.
#[test]
fn builtin_result_map_err_requires_fresh_selected_package_result() -> Result<(), Box<dyn std::error::Error>> {
    let provider = "pub def encode(value: int) -> Result[int, str]:\n    return Ok(value)\n";
    let consumer = "from views import encode\n\ndef use(value: Borrowed[int]) -> Result[int, int]:\n    return encode(value).map_err((_) => 1)\n";
    for (route, checker) in package_consumer_checkers(provider, consumer)? {
        let (owner, flow) = checker
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "use")
            .ok_or_else(|| format!("{route}: missing use flow"))?;
        let inner_span = checker
            .type_info
            .protocols
            .explicit_input_calls
            .iter()
            .find(|(_, call)| call.callee.declaration_name == "encode" && call.owner.as_ref() == Some(owner))
            .map(|(span, _)| *span)
            .ok_or_else(|| format!("{route}: missing imported encode call"))?;
        let inner = &checker.type_info.protocols.explicit_input_calls[&inner_span];
        assert!(
            matches!(
                &inner.callee.origin,
                incan_semantics_core::SymbolOrigin::Package { library, module_path }
                    if library == "fixture_records" && module_path.iter().map(String::as_str).eq(["views"])
            ),
            "{route}: imported call lost Package origin: {:?}",
            inner.callee
        );
        assert_eq!(
            inner.callee.kind,
            incan_semantics_core::SemanticSourceTargetKind::Function,
            "{route}"
        );
        assert_eq!(inner.parameters.len(), 1, "{route}");
        assert_eq!(inner.parameters[0].ty, crate::symbols::ResolvedType::Int, "{route}");

        let outer_span = checker
            .type_info
            .calls
            .resolved_builtin_method_calls
            .iter()
            .find(|(_, operation)| {
                **operation
                    == incan_lang::lang::builtins::BuiltinMethodId::Result(
                        incan_lang::lang::surface::result_methods::ResultMethodId::MapErr,
                    )
            })
            .map(|(span, _)| *span)
            .ok_or_else(|| format!("{route}: missing selected Result.map_err operation"))?;
        let outer_identity = checker
            .type_info
            .resolved_identity(outer_span)
            .ok_or_else(|| format!("{route}: Result.map_err lost its builtin identity"))?;
        assert_eq!(
            outer_identity.namespace,
            incan_semantics_core::SymbolNamespace::Member,
            "{route}"
        );
        assert_eq!(
            outer_identity.origin,
            incan_semantics_core::SymbolOrigin::Builtin,
            "{route}"
        );
        assert_eq!(outer_identity.declaration_name, "Result.map_err", "{route}");
        assert_eq!(
            outer_identity.kind,
            incan_semantics_core::SemanticSourceTargetKind::Method,
            "{route}"
        );
        assert_ne!(outer_span, inner_span, "{route}");
        let outer = checker
            .type_info
            .protocols
            .explicit_input_calls
            .get(&outer_span)
            .ok_or_else(|| format!("{route}: selected Result.map_err has no InputCall"))?;
        assert_eq!(outer.owner.as_ref(), Some(owner), "{route}");
        assert_eq!(&outer.callee, outer_identity, "{route}");
        assert_eq!(outer.receiver, Some(crate::ast::Receiver::Immutable), "{route}");
        let [callback] = outer.parameters.as_slice() else {
            return Err(format!("{route}: Result.map_err retained the wrong callback arity").into());
        };
        let crate::symbols::ResolvedType::Function(callback_parameters, callback_result) = &callback.ty else {
            return Err(format!("{route}: Result.map_err callback is not a checked function").into());
        };
        assert_eq!(callback_parameters.len(), 1, "{route}");
        assert_eq!(callback_parameters[0].ty, crate::symbols::ResolvedType::Str, "{route}");
        assert_eq!(callback_result.as_ref(), &crate::symbols::ResolvedType::Int, "{route}");
        assert_eq!(
            checker.type_info.expr_type(outer_span),
            Some(&crate::symbols::ResolvedType::Generic(
                "Result".into(),
                vec![crate::symbols::ResolvedType::Int, crate::symbols::ResolvedType::Int],
            )),
            "{route}"
        );
        assert!(matches!(flow, InputFlow::Checked { .. }), "{route}: {flow:?}");
        assert!(checker.errors.is_empty(), "{route}: {:?}", checker.errors);
    }

    let user_source = "model Mapper:\n    value: int\n\n    def map_err(self, replacement: str) -> int:\n        return len(replacement)\n\ndef user_map(value: Borrowed[str]) -> int:\n    mapper = Mapper(value=1)\n    return mapper.map_err(value)\n";
    let user_program = parsed(user_source)?;
    let mut user = TypeChecker::default();
    if let Err(errors) = user.check_program(&user_program) {
        user.errors = errors;
    }
    let user_state = user
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == "user_map")
        .map(|(_, state)| state)
        .ok_or("missing user-selected map_err flow")?;
    let InputFlow::Unavailable(user_unavailable) = user_state else {
        return Err(format!("user-selected map_err acquired builtin admission: {user_state:?}").into());
    };
    assert_eq!(
        user_unavailable.reason,
        "ordinary argument requires inferred access or mutable binding facts"
    );
    assert_eq!(
        user_source.get(user_unavailable.span.start..user_unavailable.span.end),
        Some("value")
    );
    assert!(!user.errors.is_empty());
    assert!(
        user.errors
            .iter()
            .all(|error| error.stable_code() == Some("INCAN-T0123"))
    );

    let view_source = "def result_view(value: Borrowed[Result[int, str]]) -> Borrowed[Result[int, str]] from value:\n    return value\n\ndef mapped_view(value: Borrowed[Result[int, str]]) -> Result[int, int]:\n    return result_view(value).map_err((_) => 1)\n";
    let view_program = parsed(view_source)?;
    let mut view = TypeChecker::default();
    if let Err(errors) = view.check_program(&view_program) {
        view.errors = errors;
    }
    let view_state = view
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == "mapped_view")
        .map(|(_, state)| state)
        .ok_or("missing returned-view Result receiver flow")?;
    let InputFlow::Unavailable(view_unavailable) = view_state else {
        return Err(format!("returned-view Result receiver acquired builtin admission: {view_state:?}").into());
    };
    assert_eq!(
        view_unavailable.reason,
        "call has no selected explicit input access facts"
    );
    assert_eq!(
        view_source.get(view_unavailable.span.start..view_unavailable.span.end),
        Some("result_view(value).map_err((_) => 1)")
    );
    assert!(!view.errors.is_empty());
    assert!(
        view.errors
            .iter()
            .all(|error| error.stable_code() == Some("INCAN-T0123"))
    );
    Ok(())
}

/// A function with a `Borrowed[str]` input, called from another module, takes an owned local with no `&`, as a call
/// in its own module does (shape B1).
#[test]
fn imported_explicit_input_takes_its_value_type() -> Result<(), Box<dyn std::error::Error>> {
    let provider = "pub def text_size(content: Borrowed[str]) -> int:\n    return len(content)\n\npub def bump(mut counter: BorrowedMut[int]) -> None:\n    counter += 1\n";
    let consumer = "from views import bump, text_size\n\ndef main() -> int:\n    text = \"abc\"\n    mut count = 0\n    bump(count)\n    return text_size(text) + count\n";
    for (route, errors) in consumer_errors(provider, consumer)? {
        assert!(errors.is_empty(), "{route}: {errors:?}");
    }
    Ok(())
}

/// A generic explicit result imported from another module reads as the value it views: fields, methods, indexing,
/// iteration and comparison act as on that value (shape B2).
#[test]
fn imported_explicit_result_reads_as_its_value_type() -> Result<(), Box<dyn std::error::Error>> {
    let provider = "pub model Entry:\n    pub name: str\n    pub count: int\n\n    pub def label(self) -> str:\n        return self.name\n\npub def borrowed_table[T](values: Borrowed[list[T]]) -> Borrowed[list[T]] from values:\n    return values\n";
    let consumer = "from views import Entry, borrowed_table\n\ndef main() -> int:\n    entries = [Entry(name=\"a\", count=1)]\n    table = borrowed_table(entries)\n    mut total = len(table)\n    for entry in table:\n        total += entry.count\n    if table[0].label() == \"a\" and table == entries:\n        total += table[0].count\n    return total\n";
    for (route, errors) in consumer_errors(provider, consumer)? {
        assert!(errors.is_empty(), "{route}: {errors:?}");
    }
    Ok(())
}

/// A native `&mut T` value is a `BorrowedMut[T]` value and lends shared access as a `Borrowed[T]` value, so it passes
/// to either explicit parameter, of a function or a method, with no adjustment. A native `&T` grants no exclusive
/// access, so passing it for `BorrowedMut[T]` is refused (shape A).
#[test]
fn native_reference_is_an_explicit_view_value() -> Result<(), Box<dyn std::error::Error>> {
    let declarations = "model State:\n    count: int\n\n    def bump(self, mut state: BorrowedMut[State]) -> int:\n        state.count += self.count\n        return state.count\n\ndef bump(mut state: BorrowedMut[State]) -> int:\n    state.count += 1\n    return state.count\n\ndef peek(state: Borrowed[State]) -> int:\n    return state.count\n\n";
    let admitted = format!(
        "{declarations}def exclusive(state: &mut State, step: State) -> int:\n    return bump(state) + peek(state) + step.bump(state)\n\ndef shared(state: &State) -> int:\n    return peek(state)\n"
    );
    let checker = checked(&admitted)?;
    let mismatches: Vec<_> = checker
        .errors
        .iter()
        .filter(|error| error.message.contains("type mismatch"))
        .collect();
    assert!(mismatches.is_empty(), "{mismatches:?}");
    let refused = format!("{declarations}def shared(state: &State) -> int:\n    return bump(state)\n");
    let checker = checked(&refused)?;
    assert!(
        checker
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0126")),
        "{:?}",
        checker.errors
    );
    Ok(())
}

/// A function with `Borrowed[T]` parameters is a callable value of the type a native signature spells with `&T`: the
/// two callable types are one type (shape C). An owned parameter still differs from a borrowed one.
#[test]
fn explicit_view_parameters_are_native_reference_parameters() -> Result<(), Box<dyn std::error::Error>> {
    let declarations = "def compare(left: Borrowed[float], right: Borrowed[float]) -> int:\n    return 0\n\ndef owned(left: Owned[float], right: Owned[float]) -> int:\n    return 0\n\ndef apply(order: (&float, &float) -> int) -> int:\n    return 0\n\n";
    let admitted = format!("{declarations}def main() -> int:\n    return apply(compare)\n");
    let checker = checked(&admitted)?;
    let mismatches: Vec<_> = checker
        .errors
        .iter()
        .filter(|error| error.message.contains("mismatch"))
        .collect();
    assert!(mismatches.is_empty(), "{mismatches:?}");
    let refused = format!("{declarations}def main() -> int:\n    return apply(owned)\n");
    let checker = checked(&refused)?;
    assert!(
        checker.errors.iter().any(|error| error.message.contains("mismatch")),
        "{:?}",
        checker.errors
    );
    Ok(())
}

/// An imported explicit input whose value type names a nominal the caller does not import is still the same
/// contract: the caller's spelling of the parameter is its value type (shape B1).
#[test]
fn imported_explicit_input_names_a_nominal_its_caller_does_not_import() -> Result<(), Box<dyn std::error::Error>> {
    let provider = "pub model Entry:\n    pub count: int\n\npub def entries() -> list[Entry]:\n    return [Entry(count=1)]\n\npub def total(values: Borrowed[list[Entry]]) -> int:\n    return len(values)\n";
    let consumer =
        "from views import entries, total\n\ndef main() -> int:\n    items = entries()\n    return total(items)\n";
    for (route, errors) in consumer_errors(provider, consumer)? {
        assert!(errors.is_empty(), "{route}: {errors:?}");
    }
    Ok(())
}

/// A property's `Borrowed[T]` result is a receiver-tied view that reads as its value type `T` at every access:
/// indexing, iteration, a method and comparison act as on the field it views (shape B2).
#[test]
fn property_view_result_reads_as_its_value_type() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Frame:\n    nominals: list[int]\n\n    property nominals_view -> Borrowed[list[int]] from self.nominals:\n        return self.nominals\n\ndef main() -> int:\n    frame = Frame(nominals=[1, 2])\n    mut total = frame.nominals_view[0] + len(frame.nominals_view)\n    for value in frame.nominals_view:\n        total += value\n    if frame.nominals_view == [1, 2] and frame.nominals_view.contains(1):\n        total += 1\n    return total\n";
    let checker = checked(source)?;
    let reads: Vec<_> = checker
        .errors
        .iter()
        .filter(|error| error.stable_code().is_none())
        .collect();
    assert!(reads.is_empty(), "{reads:?}");
    Ok(())
}
