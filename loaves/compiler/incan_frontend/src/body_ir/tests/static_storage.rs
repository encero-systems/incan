//! Canonical static storage and the live-versus-detached contract of local storage bindings.

use super::{bir, body_named, build};

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
    assert!(effectful.static_declarations.is_empty());
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
/// mismatched payloads are malformed, and a dictionary of lists keeps no initializer.
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
    assert!(nested.static_declarations.is_empty());
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
