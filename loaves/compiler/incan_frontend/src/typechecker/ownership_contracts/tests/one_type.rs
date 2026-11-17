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
    consumer_checkers_with_package_path(provider, consumer, None, vec!["views".to_string()])
}

/// Check both dependency routes inside one real package, so imported sibling-module identities remain Package.
pub(super) fn package_consumer_checkers(
    provider: &str,
    consumer: &str,
) -> Result<Vec<(&'static str, TypeChecker)>, Box<dyn std::error::Error>> {
    consumer_checkers_with_package_path(provider, consumer, Some("fixture_records"), vec!["views".to_string()])
}

/// Check both dependency routes while assigning the provider its exact package module identity.
fn consumer_checkers_with_package_path(
    provider: &str,
    consumer: &str,
    package: Option<&str>,
    path: Vec<String>,
) -> Result<Vec<(&'static str, TypeChecker)>, Box<dyn std::error::Error>> {
    let provider_program = parsed(provider)?;
    let consumer_program = parsed(consumer)?;
    let dependencies = [("views", &provider_program)];
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

/// Only the canonical `incan_stdlib_system::fs::path::Path.joinpath` call returns an independently owned lexical
/// value without an explicit `Owned` source signature. Same-spelled package methods, a different receiver nominal,
/// and a changed result nominal must retain the ordinary dependency refusal.
#[test]
fn standard_path_joinpath_independent_result_requires_exact_canonical_surface() -> Result<(), Box<dyn std::error::Error>>
{
    let path_module = vec!["fs".to_string(), "path".to_string()];
    let provider = "pub type Path = newtype str:\n    pub def joinpath(self, segment: str) -> Path:\n        return Path(self.0 + segment)\n";
    let consumer = "from views import Path\n\ndef owned_child(root: Borrowed[Path]) -> Owned[Path]:\n    return root.joinpath(\"/\").joinpath(\"owned\")\n";
    let consumer_program = parsed(consumer)?;
    let returned = consumer_program
        .declarations
        .iter()
        .find_map(|declaration| match &declaration.node {
            crate::ast::Declaration::Function(function) if function.name == "owned_child" => function.body.first(),
            _ => None,
        })
        .and_then(|statement| match &statement.node {
            crate::ast::Statement::Return(Some(returned)) => Some(returned),
            _ => None,
        })
        .ok_or("missing owned_child return")?;
    let Expr::MethodCall(outer_receiver, _, _, _) = &returned.node else {
        return Err("owned_child outer return is not a method call".into());
    };
    let Expr::MethodCall(inner_receiver, _, _, _) = &outer_receiver.node else {
        return Err("owned_child inner return is not a method call".into());
    };
    let receiver_spans = [
        (outer_receiver.span, inner_receiver.span),
        (returned.span, outer_receiver.span),
    ];
    for (route, checker) in
        consumer_checkers_with_package_path(provider, consumer, Some("incan_stdlib_system"), path_module.clone())?
    {
        let (owner, flow) = checker
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "owned_child")
            .ok_or_else(|| format!("{route}: missing owned_child flow"))?;
        let calls = checker
            .type_info
            .protocols
            .explicit_input_calls
            .iter()
            .filter(|(_, call)| call.callee.declaration_name == "joinpath" && call.owner.as_ref() == Some(owner))
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 2, "{route}");
        for (span, call) in &calls {
            let span = **span;
            let receiver_span = receiver_spans
                .iter()
                .find_map(|(call_span, receiver_span)| (*call_span == span).then_some(*receiver_span))
                .ok_or_else(|| format!("{route}: joinpath call lost its source receiver"))?;
            assert!(
                matches!(&call.callee.origin,
                    incan_semantics_core::SymbolOrigin::Package { library, module_path }
                        if library == "incan_stdlib_system"
                            && module_path.iter().map(String::as_str).eq(["fs", "path"])),
                "{route}: {:?}",
                call.callee
            );
            assert_eq!(call.kind, InputCallKind::Ordinary, "{route}");
            assert_eq!(call.receiver, Some(crate::ast::Receiver::Immutable), "{route}");
            assert_eq!(
                call.callee.namespace,
                incan_semantics_core::SymbolNamespace::Member,
                "{route}: callee namespace"
            );
            assert_eq!(
                call.callee.kind,
                incan_semantics_core::SemanticSourceTargetKind::Method,
                "{route}: callee kind"
            );
            assert_eq!(
                checker.type_info.resolved_builtin_method_call(span),
                None,
                "{route}: builtin method selection"
            );
            assert_eq!(checker.type_info.resolved_identity(span), Some(&call.callee), "{route}");
            assert_eq!(
                checker.type_info.call_site_callable_params(span),
                Some(call.parameters.as_slice()),
                "{route}"
            );
            let [parameter] = call.parameters.as_slice() else {
                return Err(format!("{route}: joinpath retained {} parameters", call.parameters.len()).into());
            };
            assert_eq!(parameter.kind, crate::ast::ParamKind::Normal, "{route}: parameter kind");
            assert!(!parameter.is_mut, "{route}: mutable parameter");
            assert_eq!(parameter.ty, ResolvedType::Str, "{route}: parameter type");
            let result_type = checker
                .type_info
                .expr_type(span)
                .ok_or_else(|| format!("{route}: joinpath result lost its checked type"))?;
            let mut result_path = Vec::new();
            let result_value_type = checker
                .explicit_ownership_value_type(result_type)
                .map(|value| {
                    result_path.push(0);
                    value
                })
                .unwrap_or_else(|| result_type.clone());
            let ResolvedType::Named(_) = result_value_type else {
                return Err(format!("{route}: joinpath result lost its Path type: {result_type:?}").into());
            };
            let result_identity = checker
                .type_info
                .expressions
                .expression_type_identities
                .get(&span)
                .and_then(|identities| identities.get(&result_path))
                .ok_or_else(|| format!("{route}: joinpath result lost its expression nominal identity"))?;
            assert_eq!(result_identity.declaration_name, "Path", "{route}");
            assert_eq!(
                result_identity.namespace,
                incan_semantics_core::SymbolNamespace::OrdinaryLexical,
                "{route}: result namespace"
            );
            assert_eq!(
                result_identity.kind,
                incan_semantics_core::SemanticSourceTargetKind::Newtype,
                "{route}"
            );
            assert!(
                matches!(&result_identity.origin,
                    incan_semantics_core::SymbolOrigin::Package { library, module_path }
                        if library == "incan_stdlib_system"
                            && module_path.iter().map(String::as_str).eq(["fs", "path"])),
                "{route}: {result_identity:?}"
            );
            let receiver_type = checker
                .type_info
                .expr_type(receiver_span)
                .ok_or_else(|| format!("{route}: joinpath receiver lost its checked type"))?;
            let mut receiver_path = Vec::new();
            let receiver_value_type = checker
                .explicit_ownership_value_type(receiver_type)
                .map(|value| {
                    receiver_path.push(0);
                    value
                })
                .unwrap_or_else(|| receiver_type.clone());
            let receiver_value_type = match receiver_value_type {
                ResolvedType::Ref(inner) => {
                    receiver_path.push(0);
                    *inner
                }
                other => other,
            };
            let ResolvedType::Named(_) = receiver_value_type else {
                return Err(format!(
                    "{route}: joinpath receiver is not canonical Path after value normalization: {receiver_type:?}"
                )
                .into());
            };
            let receiver_identity = checker
                .type_info
                .expressions
                .expression_type_identities
                .get(&receiver_span)
                .and_then(|identities| identities.get(&receiver_path))
                .ok_or_else(|| format!("{route}: joinpath receiver lost its expression nominal identity"))?;
            assert_eq!(receiver_identity, result_identity, "{route}: receiver/result identity");
            assert!(
                !checker.type_holds_borrow(checker.type_info.expr_type(span).ok_or("missing result")?),
                "{route}"
            );
            assert!(
                !checker.type_holds_native_loan(checker.type_info.expr_type(span).ok_or("missing result")?),
                "{route}"
            );
        }
        let InputFlow::Checked { graph, .. } = flow else {
            return Err(format!("{route}: canonical Path.joinpath was refused: {flow:?}").into());
        };
        for (span, _) in calls {
            assert!(
                graph.invocations.contains_key(span),
                "{route}: missing joinpath invocation"
            );
        }
        assert!(checker.errors.is_empty(), "{route}: {:?}", checker.errors);
    }

    let wrong_package =
        consumer_checkers_with_package_path(provider, consumer, Some("lookalike_system"), path_module.clone())?;
    for (route, checker) in wrong_package {
        assert_unavailable_reason(
            route,
            &checker,
            "owned_child",
            "call result needs retained dependency facts",
        )?;
    }

    let other_provider = "pub type Other = newtype str:\n    pub def joinpath(self, segment: str) -> Other:\n        return Other(self.0 + segment)\n";
    let other_consumer = "from views import Other\n\ndef owned_child(root: Borrowed[Other]) -> Owned[Other]:\n    return root.joinpath(\"/\").joinpath(\"owned\")\n";
    for (route, checker) in consumer_checkers_with_package_path(
        other_provider,
        other_consumer,
        Some("incan_stdlib_system"),
        path_module.clone(),
    )? {
        assert_unavailable_reason(
            route,
            &checker,
            "owned_child",
            "call result needs retained dependency facts",
        )?;
    }

    let changed_result_provider = "pub type Branch = newtype str:\n    pub def joinpath(self, segment: str) -> Branch:\n        return Branch(self.0 + segment)\n\npub type Path = newtype str:\n    pub def joinpath(self, segment: str) -> Branch:\n        return Branch(self.0 + segment)\n";
    let changed_result_consumer = "from views import Branch, Path\n\ndef owned_child(root: Borrowed[Path]) -> Owned[Branch]:\n    return root.joinpath(\"/\").joinpath(\"owned\")\n";
    for (route, checker) in consumer_checkers_with_package_path(
        changed_result_provider,
        changed_result_consumer,
        Some("incan_stdlib_system"),
        path_module,
    )? {
        assert_unavailable_reason(
            route,
            &checker,
            "owned_child",
            "call result needs retained dependency facts",
        )?;
    }
    Ok(())
}

/// Require one named callable to remain on the expected source-flow refusal route.
fn assert_unavailable_reason(
    route: &str,
    checker: &TypeChecker,
    callable: &str,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let state = checker
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == callable)
        .map(|(_, state)| state)
        .ok_or_else(|| format!("{route}: missing {callable} flow"))?;
    let InputFlow::Unavailable(unavailable) = state else {
        return Err(format!("{route}: {callable} unexpectedly acquired checked flow: {state:?}").into());
    };
    assert_eq!(unavailable.reason, expected, "{route}");
    Ok(())
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

    let method_provider = "pub class Encoder:\n    pub def encode(self) -> Result[int, str]:\n        return Ok(1)\n";
    let method_consumer = "from views import Encoder\n\ndef use_method(seed: Borrowed[int]) -> Result[int, int]:\n    encoder = Encoder()\n    return encoder.encode().map_err((_) => 1)\n";
    let method_program = parsed(method_consumer)?;
    let method_function = method_program
        .declarations
        .iter()
        .find_map(|declaration| match &declaration.node {
            crate::ast::Declaration::Function(function) if function.name == "use_method" => Some(function),
            _ => None,
        })
        .ok_or("missing use_method function")?;
    for (route, checker) in package_consumer_checkers(method_provider, method_consumer)? {
        let (owner, flow) = checker
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "use_method")
            .ok_or_else(|| format!("{route}: missing use_method flow"))?;
        let InputFlow::Checked { graph, .. } = flow else {
            return Err(format!("{route}: direct method Result did not retain checked flow: {flow:?}").into());
        };
        let (inner_span, inner) = checker
            .type_info
            .protocols
            .explicit_input_calls
            .iter()
            .find(|(_, call)| call.callee.declaration_name == "encode" && call.owner.as_ref() == Some(owner))
            .ok_or_else(|| format!("{route}: missing imported method producer"))?;
        assert_eq!(inner.kind, InputCallKind::Ordinary, "{route}");
        assert_eq!(inner.receiver, Some(crate::ast::Receiver::Immutable), "{route}");
        assert_eq!(
            inner.callee.kind,
            incan_semantics_core::SemanticSourceTargetKind::Method,
            "{route}"
        );
        let invocation = graph
            .invocations
            .get(inner_span)
            .ok_or_else(|| format!("{route}: method producer lost its invocation"))?;
        let receiver = invocation
            .receiver
            .ok_or_else(|| format!("{route}: method producer lost its receiver loan"))?;
        let loan = graph
            .loans
            .get(receiver.0)
            .ok_or_else(|| format!("{route}: method producer receiver loan is outside the graph"))?;
        assert_eq!(loan.kind, super::super::flow::LoanKind::Shared, "{route}");
        assert!(!loan.writable, "{route}");
        assert!(checker.errors.is_empty(), "{route}: {:?}", checker.errors);

        let mut corrupted = checker.type_info.clone();
        let InputFlow::Checked { graph, .. } = corrupted
            .protocols
            .explicit_input_flow
            .get_mut(owner)
            .ok_or_else(|| format!("{route}: missing copied method flow"))?
        else {
            return Err(format!("{route}: copied method flow is unavailable").into());
        };
        let receiver = graph
            .invocations
            .get(inner_span)
            .and_then(|invocation| invocation.receiver)
            .ok_or_else(|| format!("{route}: copied method producer lost its receiver"))?;
        graph
            .loans
            .get_mut(receiver.0)
            .ok_or_else(|| format!("{route}: copied method receiver loan is outside the graph"))?
            .span = *inner_span;
        assert!(
            corrupted.executable_function_input(owner, method_function).is_err(),
            "{route}: substituted method receiver span replayed"
        );
    }

    let stored_method_consumer = "from views import Encoder\n\ndef use_method(seed: Borrowed[int]) -> Result[int, int]:\n    encoder = Encoder()\n    result = encoder.encode()\n    return result.map_err((_) => 1)\n";
    for (route, checker) in package_consumer_checkers(method_provider, stored_method_consumer)? {
        let state = checker
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "use_method")
            .map(|(_, state)| state)
            .ok_or_else(|| format!("{route}: missing stored method Result flow"))?;
        if matches!(state, InputFlow::Checked { .. }) {
            return Err(format!("{route}: stored method Result acquired direct map_err admission").into());
        }
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
