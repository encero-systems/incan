//! Canonical static storage and the live-versus-detached contract of local storage bindings.

use super::{bir, body_named, build};

/// Scalar reads follow their operation: binary/positional written order, fixed named free-call declaration order.
#[test]
fn scalar_static_reads_precede_later_argument_effects() -> Result<(), Box<dyn std::error::Error>> {
    for expression in [
        "COUNT + change()",
        "combine(COUNT, change())",
        "combine(right=COUNT, left=change())",
    ] {
        let source = format!(
            "static COUNT: int = 1\n\ndef change() -> int:\n    COUNT = 2\n    return 10\n\ndef combine(left: int, right: int) -> int:\n    return left * 100 + right\n\ndef main() -> int:\n    return {expression}\n"
        );
        let module = build(&source, &["static_read_order"])?;
        let body = body_named(&module, "main")?;
        let read_position = body
            .block
            .stmts
            .iter()
            .position(|statement| {
                matches!(&statement.kind, bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Use(bir::Operand::Place(read)), ..
            } if read.place.global().is_some())
            })
            .ok_or("global value was not captured")?;
        let mutation_position = body
            .block
            .stmts
            .iter()
            .position(|statement| {
                matches!(&statement.kind, bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)), ..
            } if target.name == "change")
            })
            .ok_or("mutating call missing")?;
        if expression == "combine(right=COUNT, left=change())" {
            assert!(mutation_position < read_position, "{}", body.render_snapshot());
            assert!(body.render_snapshot().contains("evaluation_order=declaration"));
        } else {
            assert!(read_position < mutation_position, "{}", body.render_snapshot());
        }
    }
    Ok(())
}

/// Ordered list literals retain their checked element carrier and reject forged mismatched payloads.
#[test]
fn static_list_literals_retain_checked_elements() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "static ITEMS: list[int] = [1, 2]\nstatic FLAGS: list[bool] = []\n\ndef main() -> int:\n    return len(ITEMS)\n",
        &["m", "static_lists"],
    )?;
    let [items, flags] = module.static_declarations.as_slice() else {
        return Err("expected two retained list statics".into());
    };
    assert_eq!(
        items.initial,
        bir::StaticInitializer::List(vec![bir::Constant::Int(1), bir::Constant::Int(2)])
    );
    assert_eq!(flags.initial, bir::StaticInitializer::List(Vec::new()));
    assert!(module.is_well_formed_static_declaration(items));
    assert!(module.is_well_formed_static_declaration(flags));
    let mut malformed = items.clone();
    malformed.initial = bir::StaticInitializer::List(vec![bir::Constant::Bool(true)]);
    assert!(!module.is_well_formed_static_declaration(&malformed));
    let effectful = build(
        "def initial() -> int:\n    return 1\n\nstatic ITEMS: list[int] = [initial()]\n\ndef main() -> int:\n    return len(ITEMS)\n",
        &["m", "effectful_list"],
    )?;
    assert_eq!(effectful.static_declarations.len(), 1);
    Ok(())
}

/// Direct storage bindings read the static's current value; whole-binding writes detach their local value.
#[test]
fn static_alias_reads_retain_storage_until_whole_rebind() -> Result<(), Box<dyn std::error::Error>> {
    for (rebind, live) in [
        ("", true),
        ("    alias = \"snapshot\"\n", false),
        ("    alias += \"!\"\n", false),
    ] {
        let source = format!(
            "static TEXT: str = \"before\"\n\ndef main() -> str:\n    mut alias = TEXT\n    TEXT = \"after\"\n{rebind}    return alias\n"
        );
        let module = build(&source, &["m", "static_alias"])?;
        let returned = body_named(&module, "main")?
            .block
            .stmts
            .iter()
            .find_map(|stmt| match &stmt.kind {
                bir::StatementKind::Return {
                    value: Some(bir::Operand::Place(read)),
                } => Some(read),
                _ => None,
            });
        let Some(returned) = returned else {
            return Err("expected a retained return place".into());
        };
        assert_eq!(returned.place.global().is_some(), live, "{}", module.render_snapshot());
        if live {
            let storage = module.static_declarations.first().ok_or("missing static storage")?;
            assert_eq!(
                returned.place.global().map(|global| &global.identity),
                Some(&storage.canonical)
            );
            assert_eq!(returned.fact, bir::OwnershipFact::Clone);
        }
    }
    let conditional = build(
        "static TEXT: str = \"before\"\n\ndef main() -> str:\n    mut alias = TEXT\n    if true:\n        alias = \"snapshot\"\n    return alias\n",
        &["m", "conditional_static_alias"],
    )?;
    assert!(
        conditional
            .render_snapshot()
            .contains("control-flow-dependent static binding handle")
    );
    Ok(())
}

/// Primitive dictionary and set literals and the empty `Set()` constructor retain their checked elements; forged
/// mismatched payloads are malformed, and a dictionary of lists keeps a computed initializer.
#[test]
fn static_hashed_literals_retain_checked_elements() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "static COUNTS: dict[str, int] = {\"a\": 1}\nstatic SEEN: set[int] = Set()\nstatic WORDS: set[str] = {\"a\", \"b\"}\n\ndef main() -> int:\n    return len(COUNTS) + len(SEEN) + len(WORDS)\n",
        &["m", "static_hashed"],
    )?;
    let [counts, seen, words] = module.static_declarations.as_slice() else {
        return Err("expected three retained hashed statics".into());
    };
    assert_eq!(
        counts.initial,
        bir::StaticInitializer::Dict(vec![(bir::Constant::Str("a".into()), bir::Constant::Int(1))])
    );
    assert_eq!(seen.initial, bir::StaticInitializer::Set(Vec::new()));
    assert_eq!(
        words.initial,
        bir::StaticInitializer::Set(vec![bir::Constant::Str("a".into()), bir::Constant::Str("b".into())])
    );
    for declaration in [counts, seen, words] {
        assert!(module.is_well_formed_static_declaration(declaration));
    }
    let mut malformed = counts.clone();
    malformed.initial = bir::StaticInitializer::Dict(vec![(bir::Constant::Bool(true), bir::Constant::Int(1))]);
    assert!(!module.is_well_formed_static_declaration(&malformed));
    let nested = build(
        "static GROUPS: dict[str, list[int]] = {}\n\ndef main() -> int:\n    return len(GROUPS)\n",
        &["m", "nested_hashed"],
    )?;
    assert_eq!(nested.static_declarations.len(), 1);
    Ok(())
}

/// Computed initializers retain declaration order, canonical storage identity, and their closed checked carrier.
#[test]
fn computed_statics_retain_order_and_closed_initializer_bodies() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def initial(value: int) -> int:\n    return value\n\nstatic FIRST: int = initial(1)\nstatic SECOND: int = initial(FIRST + 1)\n",
        &["m", "computed_statics"],
    )?;
    let [first, second] = module.static_declarations.as_slice() else {
        return Err("expected both computed static declarations in source order".into());
    };
    for declaration in [first, second] {
        assert!(module.is_well_formed_static_declaration(declaration));
        let initializer = format!("{:?}", declaration.initial);
        assert!(initializer.starts_with("Computed("), "{initializer}");
        assert!(initializer.contains("params: []"), "{initializer}");
        assert!(initializer.contains("return_type: Primitive(Int)"), "{initializer}");
        assert!(initializer.contains(&format!("canonical: Some({:?})", declaration.canonical)));
    }
    assert!(format!("{:?}", first.canonical).contains("FIRST"));
    assert!(format!("{:?}", second.canonical).contains("SECOND"));
    assert_eq!(
        module.bodies.len(),
        1,
        "initializers must not become callable source declarations"
    );
    Ok(())
}

/// A static entry write evaluates its value before its key, as legacy computes it before writing the entry, so the
/// key is the last read of the name it shares with the value.
#[test]
fn static_index_key_follows_its_value() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "static counts: dict[str, int] = {}\n\ndef record(name: str) -> None:\n    counts[name] = counts.get(name, 0) + 1\n",
        &["m", "static_index_key"],
    )?;
    let body = body_named(&module, "record")?;
    let statements = &body.block.stmts;
    let lookup = statements
        .iter()
        .position(|statement| matches!(statement.kind, bir::StatementKind::Call { .. }))
        .ok_or("missing value lookup")?;
    let write = statements
        .iter()
        .position(
            |statement| matches!(&statement.kind, bir::StatementKind::Assign { place, .. } if place.global().is_some()),
        )
        .ok_or("missing storage write")?;
    assert!(
        lookup < write,
        "the value must precede the write: {}",
        body.render_snapshot()
    );
    let bir::StatementKind::Assign { place, .. } = &statements[write].kind else {
        return Err("the storage write must be an assignment".into());
    };
    let [bir::PlaceElem::Index(key)] = place.projection.as_slice() else {
        return Err("the storage write must index the static once".into());
    };
    let bir::Operand::Place(read) = key.as_ref() else {
        return Err("the key must read the parameter".into());
    };
    assert_eq!(read.fact, bir::OwnershipFact::Move, "{}", body.render_snapshot());
    Ok(())
}

/// A fixed tuple spread must not defer an earlier scalar read until its effectful source has finished.
#[test]
fn tuple_spreads_capture_prior_scalar_arguments() -> Result<(), Box<dyn std::error::Error>> {
    for (declaration, invocation) in [
        (
            "def combine(first: int, second: int, third: int) -> int:\n    return first * 100 + second * 10 + third",
            "combine(COUNT, *pair())",
        ),
        (
            "def combine(first: int, *rest: int) -> int:\n    return first * 100 + rest[0] * 10 + rest[1]",
            "combine(COUNT, *pair())",
        ),
    ] {
        let source = format!(
            "static COUNT: int = 1\n\ndef pair() -> tuple[int, int]:\n    COUNT = 2\n    return (3, 4)\n\n{declaration}\n\ndef main() -> int:\n    return {invocation}\n"
        );
        let module = build(&source, &["tuple_spread_static_read"])?;
        let body = body_named(&module, "main")?;
        assert!(
            !body.render_snapshot().contains("unsupported("),
            "{}",
            body.render_snapshot()
        );
        let captured = body
            .block
            .stmts
            .iter()
            .position(|statement| {
                matches!(&statement.kind, bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Use(bir::Operand::Place(read)), ..
            } if read.place.global().is_some())
            })
            .ok_or("global argument was not captured")?;
        let calls = body
            .block
            .stmts
            .iter()
            .enumerate()
            .filter_map(|(index, statement)| {
                matches!(&statement.kind, bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)), ..
            } if target.name == "pair")
                .then_some(index)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            calls.len(),
            1,
            "tuple source must execute once: {}",
            body.render_snapshot()
        );
        assert!(captured < calls[0], "{}", body.render_snapshot());
    }
    Ok(())
}

/// Specialized by-value construction captures each global scalar/text value before later expressions mutate it.
#[test]
fn specialized_values_capture_prior_global_reads() -> Result<(), Box<dyn std::error::Error>> {
    let setup = "static COUNT: int = 1\n\nmodel Pair:\n    left: int\n    right: int\n\ndef change() -> int:\n    COUNT = 2\n    return 10\n\ndef combine(left: int, right: int) -> int:\n    return left * 100 + right\n\n";
    let expressions = [
        "values = [COUNT, change()]",
        "values = (COUNT, change())",
        "values = {COUNT, change()}",
        "values = {COUNT: change()}",
        "values = {0: COUNT, change(): 0}",
        "values = COUNT..change()",
        "values = f\"{COUNT}:{change()}\"",
        "values = partial Pair(left=COUNT, right=change())",
    ];
    let mut sources = expressions
        .iter()
        .map(|expression| format!("{setup}def main() -> None:\n    {expression}\n"))
        .collect::<Vec<_>>();
    sources.push("static TEXT: str = \"before\"\n\ndef change() -> str:\n    TEXT = \"after\"\n    return \"E\"\n\ndef main() -> None:\n    error = ValidationError(message=TEXT, code=change())\n".to_owned());
    for source in sources {
        let module = build(&source, &["specialized_value_order"])?;
        let body = body_named(&module, "main")?;
        let snapshot = body.render_snapshot();
        assert!(!snapshot.contains("unsupported"), "{source}\n{snapshot}");
        let read = body
            .block
            .stmts
            .iter()
            .position(|statement| {
                matches!(
                    &statement.kind,
                    bir::StatementKind::Assign { rvalue: bir::Rvalue::Use(bir::Operand::Place(read)), .. }
                        if read.place.global().is_some()
                )
            })
            .ok_or_else(|| format!("no captured global read: {source}\n{snapshot}"))?;
        let effects = body
            .block
            .stmts
            .iter()
            .enumerate()
            .filter_map(|(index, statement)| match &statement.kind {
                bir::StatementKind::Call {
                    callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                    ..
                } if target.name == "change" => Some(index),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(effects.len(), 1, "{source}\n{snapshot}");
        assert!(read < effects[0], "{source}\n{snapshot}");
    }
    Ok(())
}

/// The checked by-value index of a source hook is captured before its assigned-value expression mutates the global.
#[test]
fn index_hook_captures_value_arguments_before_later_effects() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"static INDEX: int = 0

model Row:
    value: int

    def __setitem__(self, index: int, value: int) -> None:
        println(index)
        println(value)

def change() -> int:
    INDEX = 1
    return 99

def main() -> None:
    row = Row(value=10)
    row[INDEX] = change()
"#,
        &["index_hook_read"],
    )?;
    let body = body_named(&module, "main")?;
    assert!(!body.render_snapshot().contains("Unsupported"));
    let read_position = body
        .block
        .stmts
        .iter()
        .position(|statement| {
            matches!(&statement.kind, bir::StatementKind::Assign {
            rvalue: bir::Rvalue::Use(bir::Operand::Place(read)), ..
        } if read.place.global().is_some())
        })
        .ok_or("hook index global value was not captured")?;
    let mutation_position = body
        .block
        .stmts
        .iter()
        .position(|statement| {
            matches!(&statement.kind, bir::StatementKind::Call {
            callee: bir::Callee::Function(bir::CallableTarget::Named(target)), ..
        } if target.name == "change")
        })
        .ok_or("mutating assigned-value call missing")?;
    assert!(read_position < mutation_position, "{}", body.render_snapshot());
    Ok(())
}

/// Iterator terminals capture each scalar argument before evaluating a later callback-producing expression.
/// Missing declaration-slot metadata still preserves receiver-first and left-to-right value evaluation.
#[test]
fn iterator_terminal_captures_global_argument_before_callback_effects() -> Result<(), Box<dyn std::error::Error>> {
    for method in ["fold", "reduce"] {
        let source = format!(
            "static COUNT: int = 1\n\ndef items() -> list[int]:\n    return [1]\n\ndef add(acc: int, item: int) -> int:\n    return acc + item + 10\n\ndef choose() -> (int, int) -> int:\n    COUNT = 2\n    return add\n\ndef main() -> int:\n    return items().iter().{method}(COUNT, choose())\n"
        );
        let module = build(&source, &["iterator_argument_read"])?;
        let body = body_named(&module, "main")?;
        let snapshot = body.render_snapshot();
        assert!(!snapshot.contains("unsupported("), "{snapshot}");
        let named_call_position = |name: &str| {
            body.block.stmts.iter().position(|statement| {
                matches!(&statement.kind, bir::StatementKind::Call {
                    callee: bir::Callee::Function(bir::CallableTarget::Named(target)), ..
                } if target.name == name)
            })
        };
        let receiver = named_call_position("items").ok_or("receiver call missing")?;
        let callback = named_call_position("choose").ok_or("callback producer missing")?;
        let captured = body
            .block
            .stmts
            .iter()
            .position(|statement| {
                matches!(&statement.kind, bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Use(bir::Operand::Place(read)), ..
            } if read.place.global().is_some())
            })
            .ok_or("iterator argument remained a deferred global read")?;
        let terminal = body
            .block
            .stmts
            .iter()
            .position(|statement| {
                matches!(&statement.kind, bir::StatementKind::Call {
                callee: bir::Callee::Method(target), ..
            } if target.name == method)
            })
            .ok_or("iterator terminal call missing")?;
        assert!(
            receiver < captured && captured < callback && callback < terminal,
            "{snapshot}"
        );
    }
    Ok(())
}
