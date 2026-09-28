//! A union member that names a nominal type is the declaration its name resolves to where the union is written, so
//! `Product | int` in two modules that each declare `Product` is two union types (#1796).

use super::helpers::parse_program;
use super::*;

/// One module declaring `Product`, `Answer = Product | int` and functions that take and build an `Answer`.
fn product_module(field_type: &str) -> String {
    format!(
        "pub model Product:\n    pub value: {field_type}\n\n\npub type Answer = Product | int\n\n\npub def label(answer: Answer) -> str:\n    return \"answer\"\n\n\npub def wrap(value: {field_type}) -> Answer:\n    return Product(value=value)\n"
    )
}

/// Check `consumer` as module `main` of a package whose modules `first` and `second` each declare `Product`.
fn check_consumer(consumer: &str) -> Result<(), Vec<CompileError>> {
    let first = parse_program(&product_module("int"), "first");
    let second = parse_program(&product_module("str"), "second");
    let main = parse_program(consumer, "main");
    let mut checker = TypeChecker::new();
    checker.register_dependency_module_path_segments("first", vec!["first".to_string()]);
    checker.register_dependency_module_path_segments("second", vec!["second".to_string()]);
    checker.set_current_module_path(Some(vec!["main".to_string()]));
    checker.check_with_imports(&main, &[("first", &first), ("second", &second)])
}

/// Return the messages of a consumer the checker must refuse.
fn refused_messages(consumer: &str) -> Result<Vec<String>, String> {
    match check_consumer(consumer) {
        Ok(()) => Err(format!("the checker must refuse:\n{consumer}")),
        Err(errors) => Ok(errors.into_iter().map(|error| error.message).collect()),
    }
}

/// #1796: `first.Answer` and `second.Answer` are `Product | int` over two different `Product` declarations, so a
/// value of one is refused where the other is expected, whether it is passed, returned from a call or assigned. The
/// checker compared union members by their bare names and accepted all three, and rustc refused the generated Rust.
#[test]
fn a_union_over_another_modules_same_named_type_is_refused_issue1796() -> Result<(), String> {
    for consumer in [
        "import first\nimport second\n\n\ndef take(answer: second.Answer) -> int:\n    return 0\n\n\ndef main() -> None:\n    value: first.Answer = 1\n    take(value)\n",
        "import first\nimport second\n\n\ndef main() -> None:\n    second.label(first.wrap(1))\n",
        "import second\nfrom first import Answer\n\n\ndef main() -> None:\n    value: second.Answer = 1\n    other: Answer = value\n",
        "import first\nfrom second import Product\n\n\ndef take(answer: Product | int) -> int:\n    return 0\n\n\ndef main() -> None:\n    take(first.wrap(1))\n",
        "import first\n\n\nmodel Product:\n    value: int\n\n\ndef take(answer: Product | int) -> int:\n    return 0\n\n\ndef main() -> None:\n    take(first.wrap(1))\n",
    ] {
        let messages = refused_messages(consumer)?;
        if !messages
            .iter()
            .any(|message| message.contains("first.Product") || message.contains("second.Product"))
        {
            return Err(format!(
                "the refusal must name the declaring modules: {messages:?}\n{consumer}"
            ));
        }
    }
    Ok(())
}

/// A plain value enters an imported union only when it is the declaration that the union's source module selected.
/// A same-named declaration imported into the consumer is not that member.
#[test]
fn a_plain_same_named_value_from_another_module_is_refused_for_an_imported_union() -> Result<(), String> {
    let source = "import first\nfrom second import Product\n\n\ndef main() -> None:\n    answer: first.Answer = Product(value=\"wrong module\")\n";
    let errors = match check_consumer(source) {
        Ok(()) => return Err("second.Product must not enter first.Answer".to_string()),
        Err(errors) => errors,
    };
    if !errors.iter().any(|error| {
        crate::diagnostics::code_for_error(error, crate::diagnostics::DiagnosticPhase::Typecheck) == "INCAN-T0001"
    }) {
        return Err(format!(
            "the incompatible union member must use INCAN-T0001: {errors:?}"
        ));
    }
    Ok(())
}

/// #1796: the same declaration reached through any spelling is one union member, so a module's own union still
/// accepts its values, a union written with an imported or aliased `Product` is that module's union, and narrowing
/// and type patterns select the member by the declaration the written name resolves to.
#[test]
fn a_union_over_one_declaration_accepts_every_spelling_of_it_issue1796() -> Result<(), Vec<CompileError>> {
    for consumer in [
        "import first\nimport second\n\n\ndef main() -> None:\n    first.label(first.wrap(1))\n    second.label(second.wrap(\"one\"))\n",
        "import second\nfrom first import Answer, Product\n\n\ndef main() -> None:\n    value: Answer = Product(value=1)\n",
        "import first\nimport second\nfrom first import Product\n\n\ndef take(answer: Product | int) -> int:\n    return 0\n\n\ndef main() -> None:\n    take(first.wrap(1))\n",
        "import first\nimport second\nfrom first import Product as FirstProduct\n\n\ndef take(answer: FirstProduct | int) -> int:\n    return 0\n\n\ndef main() -> None:\n    take(first.wrap(1))\n    take(FirstProduct(value=2))\n",
        "import first\nimport second\n\n\ndef take(answer: first.Product | int) -> int:\n    return 0\n\n\ndef main() -> None:\n    take(first.wrap(1))\n",
        "import first\nimport second\nfrom first import Product\n\n\ndef read(answer: first.Answer) -> int:\n    if isinstance(answer, Product):\n        return answer.value\n    else:\n        return answer\n\n\ndef read_match(answer: first.Answer) -> int:\n    match answer:\n        Product(product) => return product.value\n        int(number) => return number\n",
    ] {
        check_consumer(consumer)?;
    }
    Ok(())
}

/// #1796: a type pattern whose name resolves to another module's `Product` names no member of `first.Answer`.
#[test]
fn a_type_pattern_for_another_modules_same_named_type_matches_no_member_issue1796() -> Result<(), String> {
    let messages = refused_messages(
        "import first\nfrom second import Product\n\n\ndef read(answer: first.Answer) -> int:\n    match answer:\n        Product(product) => return 1\n        int(number) => return number\n",
    )?;
    if messages.is_empty() {
        return Err("the pattern must be refused".to_string());
    }
    Ok(())
}

/// One module declaring a generic `Holder[T]` and `Held = Holder[int] | str`.
const HOLDER_MODULE: &str = "pub model Holder[T]:\n    pub item: T\n\n\npub type Held = Holder[int] | str\n\n\npub def hold() -> Held:\n    return Holder(item=1)\n";

/// Check `consumer` as module `main` of a package whose modules `third` and `fourth` each declare `Holder[T]`.
fn check_holder_consumer(consumer: &str) -> Result<(), Vec<CompileError>> {
    let third = parse_program(HOLDER_MODULE, "third");
    let fourth = parse_program(HOLDER_MODULE, "fourth");
    let main = parse_program(consumer, "main");
    let mut checker = TypeChecker::new();
    checker.register_dependency_module_path_segments("third", vec!["third".to_string()]);
    checker.register_dependency_module_path_segments("fourth", vec!["fourth".to_string()]);
    checker.set_current_module_path(Some(vec!["main".to_string()]));
    checker.check_with_imports(&main, &[("third", &third), ("fourth", &fourth)])
}

/// #1796: a generic member names its declaration too, so `Holder[int] | str` written in two modules that each
/// declare `Holder[T]` is two union types, while a value of either module's `Holder[int]` still enters its own.
#[test]
fn a_union_over_another_modules_same_named_generic_type_is_refused_issue1796() -> Result<(), String> {
    let refused = check_holder_consumer(
        "import third\nimport fourth\n\n\ndef take(held: fourth.Held) -> int:\n    return 0\n\n\ndef main() -> None:\n    take(third.hold())\n",
    );
    if refused.is_ok() {
        return Err("`third.Held` must be refused where `fourth.Held` is expected".to_string());
    }
    check_holder_consumer(
        "import third\nimport fourth\nfrom third import Holder\n\n\ndef take(held: third.Held) -> int:\n    return 0\n\n\ndef main() -> None:\n    take(Holder(item=2))\n    take(third.hold())\n",
    )
    .map_err(|errors| format!("one declaration's `Holder[int] | str` must accept its values: {errors:?}"))
}
