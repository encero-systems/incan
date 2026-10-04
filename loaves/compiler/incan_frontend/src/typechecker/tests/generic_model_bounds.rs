//! A generic signature that names a bounded model or class with its own type parameter must declare the bound the
//! model declares for that position (#1280).

use super::*;

/// Return every diagnostic of a refused source, or say that it was accepted.
fn refused(source: &str) -> Result<Vec<CompileError>, String> {
    match check_str(source) {
        Ok(()) => Err("the checker accepted a signature missing a model's declared bound".to_string()),
        Err(errors) => Ok(errors),
    }
}

/// Return whether one diagnostic names the missing bound and the declaration to write.
fn names_missing_bound(
    errors: &[CompileError],
    argument: &str,
    type_param: &str,
    type_name: &str,
    bound: &str,
) -> bool {
    let message =
        format!("Type parameter '{argument}' is used as '{type_param}' of '{type_name}', which requires '{bound}'");
    let hint = format!("Declare the type parameter as '{argument} with {bound}'");
    errors
        .iter()
        .any(|error| error.message.contains(&message) && error.hints.iter().any(|candidate| candidate.contains(&hint)))
}

/// #1280: `consume[T](stream: Stream[T])` over `model Stream[R with Clone]` is refused and names `T with Clone`.
#[test]
fn generic_parameter_without_the_model_bound_is_refused_issue1280() -> Result<(), String> {
    let errors = refused(
        r#"
model Stream[R with Clone]:
    item: R


def consume[T](stream: Stream[T]) -> int:
    return 1


def main() -> None:
    println(consume(Stream(item=7)))
"#,
    )?;
    if names_missing_bound(&errors, "T", "R", "Stream", "Clone") {
        Ok(())
    } else {
        Err(format!(
            "expected a refusal naming 'T with Clone', got: {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        ))
    }
}

/// #1280: return types and method signatures are checked the same way, including a method whose owner parameter is
/// passed on unbounded.
#[test]
fn return_types_and_method_signatures_need_the_model_bound_issue1280() -> Result<(), String> {
    let errors = refused(
        r#"
model Stream[R with Clone]:
    item: R


def wrap[T](value: T) -> Stream[T]:
    return Stream(item=value)


model Holder[U]:
    value: U

    def stream(self) -> Stream[U]:
        return Stream(item=self.value)
"#,
    )?;
    if names_missing_bound(&errors, "T", "R", "Stream", "Clone")
        && names_missing_bound(&errors, "U", "R", "Stream", "Clone")
    {
        Ok(())
    } else {
        Err(format!(
            "expected refusals for both the function return type and the method return type, got: {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        ))
    }
}

/// #1280: a signature whose type parameter declares the bound, or that passes a concrete type, is accepted, as is a
/// model's own method naming the model with its already bounded parameter.
#[test]
fn declared_or_concrete_arguments_are_accepted_issue1280() -> Result<(), String> {
    check_str(
        r#"
model Stream[R with Clone]:
    item: R

    def again(self) -> Stream[R]:
        return Stream(item=self.item)


def consume[T with Clone](stream: Stream[T]) -> int:
    return 1


def concrete(stream: Stream[int]) -> int:
    return stream.item


def main() -> None:
    println(consume(Stream(item=7)))
    println(concrete(Stream(item=3)))
"#,
    )
    .map_err(|errors| {
        format!(
            "expected declared and concrete arguments to check, got: {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        )
    })
}

/// #1280: a standard-library model keeps its declared bound across the import: `ReaderChunks[R with BinaryReader]`
/// refuses a bare `R` and accepts one that declares `R with BinaryReader`.
#[test]
fn imported_stdlib_model_bound_is_required_issue1280() -> Result<(), String> {
    let errors = refused(
        r#"
from std.io import IoError, ReaderChunks


def count_chunks[R](chunks: ReaderChunks[R]) -> Result[int, IoError]:
    return Ok(0)
"#,
    )?;
    if !names_missing_bound(&errors, "R", "R", "ReaderChunks", "BinaryReader") {
        return Err(format!(
            "expected a refusal naming 'R with BinaryReader', got: {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        ));
    }
    check_str(
        r#"
from std.io import BinaryReader, IoError, ReaderChunks


def count_chunks[R with BinaryReader](chunks: ReaderChunks[R]) -> Result[int, IoError]:
    return Ok(0)
"#,
    )
    .map_err(|errors| {
        format!(
            "expected the declared BinaryReader bound to satisfy ReaderChunks, got: {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        )
    })
}

/// #1280: generic declarations are judged beyond signatures: a model field, an enum payload, a newtype's underlying
/// type, and a construction in a generic function body.
#[test]
fn fields_payloads_and_constructions_need_the_model_bound_issue1280() -> Result<(), String> {
    for (source, argument) in [
        (
            "model Stream[R with Clone]:\n    item: R\n\nmodel Holder[T]:\n    stream: Stream[T]\n",
            "T",
        ),
        (
            "model Stream[R with Clone]:\n    item: R\n\nenum Shape[S]:\n    Flowing(Stream[S])\n    Still\n",
            "S",
        ),
        (
            "model Stream[R with Clone]:\n    item: R\n\ntype Wrapped[W] = newtype Stream[W]\n",
            "W",
        ),
        (
            "model Stream[R with Clone]:\n    item: R\n\ndef build[T](value: T) -> int:\n    s = Stream(item=value)\n    return 1\n",
            "T",
        ),
    ] {
        let errors = refused(source)?;
        if !names_missing_bound(&errors, argument, "R", "Stream", "Clone") {
            return Err(format!(
                "expected a refusal naming '{argument} with Clone' for {source:?}, got: {:?}",
                errors.iter().map(|error| &error.message).collect::<Vec<_>>()
            ));
        }
    }
    check_str(
        "model Stream[R with Clone]:\n    item: R\n\nmodel Holder[T with Clone]:\n    stream: Stream[T]\n\ndef build[T with Clone](value: T) -> int:\n    s = Stream(item=value)\n    return 1\n",
    )
    .map_err(|errors| format!("declared bounds must be accepted, got: {errors:?}"))
}

/// #1280: a standard-library model's builtin bounds (`Deque[T with (Clone, Eq)]`) are required, and `T with Copy`
/// provides the `Clone` a model requires, since every `Copy` type is `Clone`.
#[test]
fn stdlib_builtin_bounds_and_copy_are_honored_issue1280() -> Result<(), String> {
    let errors = refused(
        r#"
from std.collections import Deque


def size[T with Clone](queue: Deque[T]) -> int:
    return 0
"#,
    )?;
    if !names_missing_bound(&errors, "T", "T", "Deque", "Eq") {
        return Err(format!(
            "expected a refusal naming 'T with Eq', got: {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        ));
    }
    check_str(
        r#"
from std.collections import Deque


model Stream[R with Clone]:
    item: R


def size[T with (Clone, Eq)](queue: Deque[T]) -> int:
    return 0


def copied[T with Copy](stream: Stream[T]) -> int:
    return 1
"#,
    )
    .map_err(|errors| format!("declared stdlib bounds and Copy must be accepted, got: {errors:?}"))
}

/// #1280: a bounded model declared in a dependency module keeps its bound in the consumer, under an alias too.
#[test]
fn dependency_model_bound_is_required_issue1280() -> Result<(), String> {
    let dependency = parse_program("pub model Stream[R with Clone]:\n    pub item: R\n", "dependency");
    for consumer_source in [
        "from streams import Stream\n\ndef consume[T](stream: Stream[T]) -> int:\n    return 1\n",
        "from streams import Stream as Flow\n\ndef consume[T](stream: Flow[T]) -> int:\n    return 1\n",
    ] {
        let consumer = parse_program(consumer_source, "consumer");
        let mut checker = TypeChecker::new();
        let errors = match checker.check_with_imports(&consumer, &[("streams", &dependency)]) {
            Ok(()) => return Err(format!("the checker accepted {consumer_source:?}")),
            Err(errors) => errors,
        };
        if !errors.iter().any(|error| {
            error.message.contains("Type parameter 'T' is used as 'R'") && error.message.contains("'Clone'")
        }) {
            return Err(format!(
                "expected a refusal naming 'T with Clone' for {consumer_source:?}, got: {:?}",
                errors.iter().map(|error| &error.message).collect::<Vec<_>>()
            ));
        }
    }
    Ok(())
}

/// #1280: each generic function that builds a bounded model over its own unbounded type parameter gets a diagnostic of
/// its own, even when an earlier function showed the same gap.
#[test]
fn each_function_reports_its_own_body_gap_issue1280() -> Result<(), String> {
    let source = r#"
model Stream[R with Clone]:
    item: R


def wrap[T](value: T) -> int:
    stream = Stream(item=value)
    return 1


def wrap2[T](value: T) -> int:
    stream = Stream(item=value)
    return 2
"#;
    let errors = refused(source)?;
    let second = source
        .find("def wrap2")
        .ok_or_else(|| "the source has no 'wrap2'".to_string())?;
    let gap = |error: &&CompileError| error.message.contains("Type parameter 'T' is used as 'R' of 'Stream'");
    let (first_function, second_function): (Vec<_>, Vec<_>) =
        errors.iter().filter(gap).partition(|error| error.span.start < second);
    if first_function.is_empty() || second_function.is_empty() {
        return Err(format!(
            "expected a refusal in both 'wrap' and 'wrap2', got: {:?}",
            errors
                .iter()
                .map(|error| (&error.message, error.span))
                .collect::<Vec<_>>()
        ));
    }
    Ok(())
}

/// #1280: two imported modules may each declare a `Node`; the bound the checked module requires is the one of the
/// `Node` its import names, whichever module is imported last.
#[test]
fn same_named_models_of_two_modules_keep_their_own_bounds_issue1280() -> Result<(), String> {
    let bounded = parse_program(
        "pub model Node[T with Clone]:\n    pub item: T\n\npub def helper() -> int:\n    return 1\n",
        "bounded",
    );
    let unbounded = parse_program("pub model Node[T]:\n    pub item: T\n", "unbounded");
    let orders: [[(&str, &crate::ast::Program); 2]; 2] = [
        [("a", &bounded), ("b", &unbounded)],
        [("b", &unbounded), ("a", &bounded)],
    ];
    for imports in orders {
        let order = imports.iter().map(|(name, _)| *name).collect::<Vec<_>>();
        let accepted_source =
            "from b import Node\nfrom a import helper\n\ndef f[T](n: Node[T]) -> int:\n    return helper()\n";
        let consumer = parse_program(accepted_source, "consumer");
        TypeChecker::new()
            .check_with_imports(&consumer, &imports)
            .map_err(|errors| format!("b's unbounded Node must be accepted with imports {order:?}, got: {errors:?}"))?;

        let refused_source = "from a import Node, helper\n\ndef f[T](n: Node[T]) -> int:\n    return helper()\n";
        let consumer = parse_program(refused_source, "consumer");
        let errors = match TypeChecker::new().check_with_imports(&consumer, &imports) {
            Ok(()) => return Err(format!("a's bounded Node must be refused with imports {order:?}")),
            Err(errors) => errors,
        };
        if !names_missing_bound(&errors, "T", "T", "Node", "Clone") {
            return Err(format!(
                "expected a refusal naming 'T with Clone' with imports {order:?}, got: {:?}",
                errors.iter().map(|error| &error.message).collect::<Vec<_>>()
            ));
        }
    }
    Ok(())
}

/// #1280: a standard-library model served by a compiled provider keeps the bound the provider publishes for it. The
/// import binds the provider's identity rather than the source declaration's, and the bounds are recorded under that
/// identity: `ReaderChunks[R]` from `std.io`, `Deque[T]` and `OrdinalMap[K]` from `std.collections` each refuse a type
/// parameter without the declared bound.
#[test]
fn provider_served_stdlib_model_bounds_are_required_issue1280() -> Result<(), Box<dyn std::error::Error>> {
    let package_name = "incan_stdlib_fixture";
    let cases = [
        (
            "io",
            "ReaderChunks",
            "R",
            "BinaryReader",
            "pub trait BinaryReader:\n  def read_byte(self) -> int: ...\n\npub model ReaderChunks[R with BinaryReader]:\n  pub reader: R\n",
            "from std.io import ReaderChunks\n\ndef count_chunks[R](chunks: ReaderChunks[R]) -> int:\n    return 0\n",
        ),
        (
            "collections",
            "Deque",
            "T",
            "Eq",
            "pub model Deque[T with (Clone, Eq)]:\n  pub item: T\n",
            "from std.collections import Deque\n\ndef size[T with Clone](queue: Deque[T]) -> int:\n    return 0\n",
        ),
        (
            "collections",
            "OrdinalMap",
            "K",
            "OrdinalKey",
            "pub trait OrdinalKey:\n  def ordinal_bytes(self) -> bytes: ...\n\npub class OrdinalMap[K with (Clone, OrdinalKey)]:\n  pub key: K\n",
            "from std.collections import OrdinalMap\n\ndef size[K with Clone](keys: OrdinalMap[K]) -> int:\n    return 0\n",
        ),
    ];
    for (module, model, param, bound, provider_source, consumer_source) in cases {
        let plan = super::stdlib_surfaces::sdk_provider_plan_for_module(package_name, module, model, provider_source)?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["consumer".to_string()]));
        checker.set_provider_plan(Arc::new(plan));
        let consumer = parse_program(consumer_source, "provider-served stdlib consumer");
        let errors = match checker.check_program(&consumer) {
            Ok(()) => return Err(format!("the checker accepted {model}[{param}] without '{bound}'").into()),
            Err(errors) => errors,
        };
        let imported = checker
            .type_info()
            .resolved_import_identity(model)
            .ok_or_else(|| format!("the import of {model} must carry an identity"))?;
        let provider_origin = SymbolOrigin::Package {
            library: package_name.to_string(),
            module_path: vec![module.to_string()],
        };
        if imported.origin != provider_origin {
            return Err(format!("{model} must bind the provider's identity, got {:?}", imported.origin).into());
        }
        if !names_missing_bound(&errors, param, param, model, bound) {
            return Err(format!(
                "expected a refusal naming '{param} with {bound}' for {model}, got: {:?}",
                errors.iter().map(|error| &error.message).collect::<Vec<_>>()
            )
            .into());
        }
    }
    Ok(())
}
