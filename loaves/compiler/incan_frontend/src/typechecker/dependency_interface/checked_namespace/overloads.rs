//! An overloaded spelling in a checked namespace: the namespace publishes the overload set as its members, each under
//! its own canonical identity, so a consumer checked against the published namespace resolves and refuses each call
//! exactly as a consumer checked against the provider's source does.

use super::*;

/// A provider with two overload sets: `convert` selects by its argument type, and any call of `pick` with one
/// argument matches both of its overloads.
const PROVIDER: &str = concat!(
    "pub def convert(value: int) -> str:\n    return str(value)\n\n",
    "pub def convert(value: str) -> str:\n    return value\n\n",
    "pub def pick(value: int) -> int:\n    return value\n\n",
    "pub def pick(value: int, extra: int = 0) -> int:\n    return value + extra\n",
);

fn program(source: &str) -> Result<Program, String> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))
}

fn path(name: &str) -> Vec<String> {
    vec![name.to_owned()]
}

/// A checker for the module `name` that knows the provider's logical path.
fn configured(name: &str) -> TypeChecker {
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path(name)));
    checker.register_dependency_module_path_segments("conversions", path("conversions"));
    checker
}

/// The provider's source, its checker, and its namespace as a consumer installs it from the published record.
fn provider() -> Result<(Program, TypeChecker, Arc<ModuleDependencyInterface>), String> {
    let source = program(PROVIDER)?;
    let mut checker = configured("conversions");
    checker.check_program(&source).map_err(|errors| format!("{errors:?}"))?;
    let retained = CheckedNamespaceInterface::from_checked("conversions", &path("conversions"), &source, &checker)?
        .ok_or("an overloaded module published no checked namespace")?;
    let installed = CheckedNamespaceInterface::from_wire_value(retained.wire_value()?)?.installable()?;
    Ok((source, checker, installed))
}

/// The canonical identity of each overload of `name` the provider's own check declared, in source order.
fn declared_overloads(checker: &TypeChecker, name: &str) -> Result<Vec<CanonicalSymbolId>, String> {
    let symbol = checker.symbols.lookup(name).ok_or("missing overload set")?;
    let Some(Symbol {
        kind: SymbolKind::FunctionOverloads(overloads),
        ..
    }) = checker.symbols.get(symbol)
    else {
        return Err(format!("`{name}` is not an overload set"));
    };
    overloads
        .iter()
        .map(|overload| {
            checker
                .type_info()
                .declarations
                .function_bindings_by_span
                .get(&overload.span)
                .and_then(|binding| binding.identity.clone())
                .ok_or_else(|| format!("an overload of `{name}` has no declared identity"))
        })
        .collect()
}

/// Check `consumer` against the provider's source (cold) or its published namespace (warm).
fn check_consumer(
    consumer: &Program,
    provider: &Program,
    installed: &Arc<ModuleDependencyInterface>,
    warm: bool,
) -> Result<TypeChecker, Vec<CompileError>> {
    let mut checker = configured("main");
    if warm {
        checker.select_dependency_interfaces(vec![Arc::clone(installed)]);
        let empty = program("").map_err(|message| vec![CompileError::type_error(message, Default::default())])?;
        checker.check_with_imports(consumer, &[("conversions", &empty)])?;
    } else {
        checker.check_with_imports(consumer, &[("conversions", provider)])?;
    }
    Ok(checker)
}

/// An overloaded module publishes a checked namespace whose overload set keeps every member under its own identity.
#[test]
fn an_overloaded_module_publishes_each_overload_under_its_identity() -> Result<(), String> {
    let (_, checker, installed) = provider()?;
    for name in ["convert", "pick"] {
        let Some(SymbolKind::FunctionOverloads(published)) = installed.direct_member_symbols.get(name) else {
            return Err(format!("the published `{name}` is not an overload set"));
        };
        let identities = published
            .iter()
            .map(|overload| overload.identity.clone())
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| format!("a published overload of `{name}` has no identity"))?;
        assert_eq!(identities, declared_overloads(&checker, name)?, "{name}");
        let Some(Symbol {
            kind: SymbolKind::FunctionOverloads(declared),
            ..
        }) = checker
            .symbols
            .lookup(name)
            .and_then(|symbol| checker.symbols.get(symbol))
        else {
            return Err(format!("`{name}` is not a declared overload set"));
        };
        let emitted = |overloads: &[FunctionOverloadInfo]| {
            overloads
                .iter()
                .map(|overload| overload.info.emitted_name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(emitted(published), emitted(declared), "{name}: emitted overload names");
    }
    Ok(())
}

/// A consumer resolves each overloaded call through the published namespace to the overload a cold check selects.
#[test]
fn a_consumer_resolves_each_overload_through_the_published_namespace() -> Result<(), String> {
    let (source, checker, installed) = provider()?;
    let consumer = program(
        "from conversions import convert\n\ndef main() -> None:\n    println(convert(1))\n    println(convert(\"x\"))\n",
    )?;
    let declared = declared_overloads(&checker, "convert")?;
    let mut routes = Vec::new();
    for warm in [false, true] {
        let checked = check_consumer(&consumer, &source, &installed, warm).map_err(|errors| format!("{errors:?}"))?;
        let mut selected = checked
            .type_info()
            .references
            .resolved_identities
            .iter()
            .filter(|(_, identity)| declared.contains(identity))
            .map(|(span, identity)| (span.start, identity.clone()))
            .collect::<Vec<_>>();
        selected.sort();
        routes.push(selected.into_iter().map(|(_, identity)| identity).collect::<Vec<_>>());
    }
    assert_eq!(routes[0], declared, "a cold check selected other overloads");
    assert_eq!(routes[1], routes[0], "the published namespace resolved other overloads");
    Ok(())
}

/// A call two overloads both accept is refused through the published namespace with the cold check's diagnostic.
#[test]
fn an_ambiguous_overloaded_call_is_refused_through_the_published_namespace() -> Result<(), String> {
    let (source, _, installed) = provider()?;
    let consumer = program("from conversions import pick\n\ndef main() -> None:\n    println(pick(1))\n")?;
    let mut refusals = Vec::new();
    for warm in [false, true] {
        let errors = check_consumer(&consumer, &source, &installed, warm)
            .err()
            .ok_or("an ambiguous overloaded call was accepted")?;
        refusals.push(errors.iter().map(|error| error.message.clone()).collect::<Vec<_>>());
    }
    assert_eq!(
        refusals[0],
        vec!["Call to overloaded function 'pick' is ambiguous".to_owned()]
    );
    assert_eq!(refusals[1], refusals[0]);
    Ok(())
}
