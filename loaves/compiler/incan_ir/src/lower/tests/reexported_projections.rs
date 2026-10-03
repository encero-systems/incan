//! Which import item binds a re-exported projection: the first item of each projection a module re-exports is a
//! public item spelled by the declaration's own name (#1750, #1751, #1764).

use super::import_paths::{lower_module_at, lowered_imports, parse_module};
use super::*;
use crate::decl::{IrImportItem, Visibility};

/// The provider every test re-exports from: a function and a public `alias` declaration of it.
const PROVIDER: &str = "pub def scale(value: int) -> int:\n    return value * 2\n\n\npub scale_alias = alias scale\n";

/// Return each lowered import item in declaration order, with the visibility of its import.
fn import_items_in_order(ir: &IrProgram) -> Vec<(Visibility, IrImportItem)> {
    lowered_imports(ir)
        .into_iter()
        .flat_map(|(_, _, visibility, items)| items.into_iter().map(move |item| (visibility, item)))
        .collect()
}

/// Return the name, alias and visibility of each import item whose identity is the declaration `declaration_name`.
fn items_of(ir: &IrProgram, declaration_name: &str) -> Vec<(String, Option<String>, Visibility)> {
    import_items_in_order(ir)
        .into_iter()
        .filter(|(_, item)| {
            item.canonical
                .as_ref()
                .is_some_and(|identity| identity.declaration_name == declaration_name)
        })
        .map(|(visibility, item)| (item.name, item.alias, visibility))
        .collect()
}

/// #1750: a facade re-exporting an `alias` declaration without its target binds the projection under the target's
/// own spelling, exposing the alias's name; so does a further re-export renaming the alias again.
#[test]
fn alias_declaration_reexported_without_its_target_binds_the_projection_issue1750() -> Result<(), String> {
    let provider = parse_module(PROVIDER, "provider")?;
    let facade = parse_module("pub from provider import scale_alias\n", "facade")?;
    let public_api = parse_module("pub from facade import scale_alias as exported_scale\n", "public_api")?;

    let facade_ir = lower_module_at(&["facade"], &facade, &[("provider", &provider)])?;
    assert_eq!(
        items_of(&facade_ir, "scale"),
        vec![("scale".to_string(), Some("scale_alias".to_string()), Visibility::Public)]
    );

    let public_api_ir = lower_module_at(
        &["public_api"],
        &public_api,
        &[("provider", &provider), ("facade", &facade)],
    )?;
    assert_eq!(
        items_of(&public_api_ir, "scale"),
        vec![(
            "scale".to_string(),
            Some("exported_scale".to_string()),
            Visibility::Public
        )]
    );
    Ok(())
}

/// #1750: when the facade re-exports the target as well, the target's item binds the projection first, whichever
/// order the source lists them in, and the alias keeps its own spelling as the second name.
#[test]
fn target_reexported_beside_its_alias_binds_the_projection_first_issue1750() -> Result<(), String> {
    let provider = parse_module(PROVIDER, "provider")?;
    for source in [
        "pub from provider import scale_alias, scale\n",
        "pub from provider import scale, scale_alias\n",
    ] {
        let facade = parse_module(source, "facade")?;
        let ir = lower_module_at(&["facade"], &facade, &[("provider", &provider)])?;
        assert_eq!(
            items_of(&ir, "scale"),
            vec![
                ("scale".to_string(), None, Visibility::Public),
                ("scale_alias".to_string(), None, Visibility::Public),
            ],
            "{source}"
        );
    }
    Ok(())
}

/// #1750: an earlier private import of the target used by the facade stands behind the public re-export of its
/// alias, which moves ahead of it in an import of its own and binds the projection publicly.
#[test]
fn private_import_before_a_public_reexport_stands_behind_it_issue1750() -> Result<(), String> {
    let provider = parse_module(PROVIDER, "provider")?;
    let facade = parse_module(
        "from provider import scale\npub from provider import scale_alias\n\n\npub def doubled(value: int) -> int:\n    return scale(value)\n",
        "facade",
    )?;
    let ir = lower_module_at(&["facade"], &facade, &[("provider", &provider)])?;
    assert_eq!(
        items_of(&ir, "scale"),
        vec![
            ("scale".to_string(), Some("scale_alias".to_string()), Visibility::Public),
            ("scale".to_string(), None, Visibility::Private),
        ]
    );
    Ok(())
}

/// #1764: an alias of a module member beside a direct import of the member binds the projection publicly, under the
/// member's own spelling with the alias's name, ahead of the private import.
#[test]
fn module_member_alias_beside_a_direct_import_binds_publicly_issue1764() -> Result<(), String> {
    let helpers = parse_module("pub def double(value: int) -> int:\n    return value * 2\n", "helpers")?;
    let facade = parse_module(
        "from helpers import double\nimport helpers as h\n\npub twice = h.double\n\n\npub def quadruple(value: int) -> int:\n    return double(double(value))\n",
        "facade",
    )?;
    let ir = lower_module_at(&["facade"], &facade, &[("helpers", &helpers)])?;
    assert_eq!(
        items_of(&ir, "double"),
        vec![
            ("double".to_string(), Some("twice".to_string()), Visibility::Public),
            ("double".to_string(), None, Visibility::Private),
        ]
    );
    Ok(())
}

/// A module that only imports a projection privately keeps its imports as written.
#[test]
fn private_imports_keep_their_order_and_spelling() -> Result<(), String> {
    let provider = parse_module(PROVIDER, "provider")?;
    let consumer = parse_module(
        "from provider import scale_alias\nfrom provider import scale\n\n\ndef run() -> int:\n    return scale(1) + scale_alias(2)\n",
        "consumer",
    )?;
    let ir = lower_module_at(&["consumer"], &consumer, &[("provider", &provider)])?;
    assert_eq!(
        items_of(&ir, "scale"),
        vec![
            ("scale_alias".to_string(), None, Visibility::Private),
            ("scale".to_string(), None, Visibility::Private),
        ]
    );
    Ok(())
}
