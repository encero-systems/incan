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
    let provider_program = parsed(provider)?;
    let consumer_program = parsed(consumer)?;
    let dependencies = [("views", &provider_program)];
    let path = vec!["views".to_string()];
    let mut collected = TypeChecker::default();
    collected.register_dependency_module_path_segments("views", path.clone());
    let collected_errors = collected
        .check_with_imports(&consumer_program, &dependencies)
        .err()
        .unwrap_or_default();
    let mut builder = TypeChecker::default();
    builder.set_current_module_path(Some(path.clone()));
    builder
        .check_program(&provider_program)
        .map_err(|errors| format!("provider: {errors:?}"))?;
    let exports = Arc::from(crate::module::exported_symbols(&provider_program));
    let interface = builder.seal_current_module_dependency_interface("views", &provider_program, exports, true);
    let mut sealed = TypeChecker::default();
    sealed.register_dependency_module_path_segments("views", path);
    sealed.select_dependency_interfaces(vec![interface]);
    let sealed_errors = sealed
        .check_with_imports(&consumer_program, &dependencies)
        .err()
        .unwrap_or_default();
    Ok(vec![("collected", collected_errors), ("sealed", sealed_errors)])
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
