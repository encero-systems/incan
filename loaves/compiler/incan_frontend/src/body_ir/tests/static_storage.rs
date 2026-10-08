//! Canonical static storage and the live-versus-detached contract of local storage bindings.

use super::{bir, body_named, build};

/// Static assignments finish RHS calls before consuming the final owned index binding, matching legacy storage writes.
#[test]
fn static_index_key_follows_rhs_call() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "static counts: dict[str, int] = {}\n\ndef record(name: str) -> None:\n    counts[name] = counts.get(name, 0) + 1\n",
        &["m", "static_index_key"],
    )?;
    let body = body_named(&module, "record")?;
    let call = body
        .block
        .stmts
        .iter()
        .position(|statement| matches!(statement.kind, bir::StatementKind::Call { .. }))
        .ok_or("missing RHS lookup")?;
    let index = body
        .block
        .stmts
        .iter()
        .find_map(|statement| {
            let bir::StatementKind::Assign { place, .. } = &statement.kind else {
                return None;
            };
            if place.global().is_none() {
                return None;
            }
            let [bir::PlaceElem::Index(index)] = place.projection.as_slice() else {
                return None;
            };
            let bir::Operand::Place(read) = index.as_ref() else {
                return None;
            };
            Some(read)
        })
        .ok_or("missing indexed static destination")?;
    assert_eq!(index.fact, bir::OwnershipFact::Move, "{}", body.render_snapshot());
    let assignment = body
        .block
        .stmts
        .iter()
        .position(|statement| {
            matches!(&statement.kind,
                bir::StatementKind::Assign { place, .. } if place.global().is_some()
            )
        })
        .ok_or("missing storage write")?;
    assert!(
        call < assignment,
        "the RHS lookup must precede the storage write: {}",
        body.render_snapshot()
    );
    Ok(())
}

/// Primitive hashed literals retain exact checked keys and values, including the canonical empty set constructor.
#[test]
fn static_hashed_literals_retain_checked_elements() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "static COUNTS: dict[str, int] = {\"a\": 1}\nstatic SEEN: set[int] = Set()\nstatic WORDS: set[str] = {\"a\", \"b\"}\n\ndef main() -> int:\n    return len(COUNTS)\n",
        &["m", "static_hashed"],
    )?;
    assert_eq!(module.static_declarations.len(), 3);
    for declaration in &module.static_declarations {
        assert!(module.is_well_formed_static_declaration(declaration));
    }
    let mut malformed = module.static_declarations[0].clone();
    malformed.initial = bir::StaticInitializer::Dict(vec![(bir::Constant::Bool(true), bir::Constant::Int(1))]);
    assert!(!module.is_well_formed_static_declaration(&malformed));
    let nested = build(
        "static VALUES: dict[str, list[int]] = {}\n\ndef main() -> int:\n    return len(VALUES)\n",
        &["m", "nested_hashed"],
    )?;
    assert!(nested.static_declarations.is_empty());
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
