//! A module of a crate of several records the crate path of each type another module declares that it names without
//! binding it, and keeps the spelling of every type it binds (#1561).

use super::*;
use crate::lower::CrateNominalContext;
use std::sync::Arc;

/// Parse one module, keeping lexer and parser failures as test errors.
fn parse_module(source: &str) -> Result<ast::Program, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))
}

/// Check and lower `main_source` as the crate root beside modules `ids`, which declares `EvidenceId`, `Evidence` and
/// `Kind`, and `records`, which imports them into a model's fields, returning the root's recorded paths.
fn unbound_paths(main_source: &str) -> Result<std::collections::HashMap<String, String>, String> {
    let ids = parse_module(
        "pub type EvidenceId = newtype str\n\n\npub model Evidence:\n    pub id: str\n\n\npub enum Kind:\n    Primary\n",
    )?;
    let records = parse_module(
        "from ids import EvidenceId, Evidence, Kind\n\n\npub model Decision:\n    pub ids: list[EvidenceId]\n    pub items: list[Evidence]\n    pub kinds: list[Kind]\n",
    )?;
    let main = parse_module(main_source)?;
    let mut checker = TypeChecker::new();
    checker.register_dependency_module_path_segments("ids", vec!["ids".to_string()]);
    checker.register_dependency_module_path_segments("records", vec!["records".to_string()]);
    checker
        .check_with_imports(&main, &[("ids", &ids), ("records", &records)])
        .map_err(|errors| format!("main should typecheck: {errors:?}"))?;
    let context = CrateNominalContext::from_modules([
        (Vec::new(), Vec::new(), &main),
        (vec![vec!["ids".to_string()]], vec!["ids".to_string()], &ids),
        (vec![vec!["records".to_string()]], vec!["records".to_string()], &records),
    ]);
    let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
    lowering.set_crate_nominal_context(Some(Arc::new(context)));
    let ir = lowering
        .lower_program(&main)
        .map_err(|errors| format!("main lowering failed: {errors:?}"))?;
    Ok(ir.unbound_nominal_type_paths)
}

/// The root names `EvidenceId`, `Evidence` and `Kind` only through `Decision`'s fields: each is recorded at the module
/// that declares it. A type the root imports, under its own name or another, declares, or uses as the name of a type
/// parameter of one of its declarations keeps its spelling.
#[test]
fn types_a_module_names_without_binding_them_are_placed_at_their_declaring_module_issue1561() -> Result<(), String> {
    let unbound = unbound_paths(
        "from records import Decision\n\n\ndef main() -> None:\n    d = Decision(ids=[], items=[], kinds=[])\n    println(len(d.ids))\n",
    )?;
    for (name, path) in [
        ("EvidenceId", "crate::ids::EvidenceId"),
        ("Evidence", "crate::ids::Evidence"),
        ("Kind", "crate::ids::Kind"),
    ] {
        assert_eq!(unbound.get(name).map(String::as_str), Some(path), "{unbound:?}");
    }
    assert!(
        !unbound.contains_key("Decision"),
        "an imported type is bound: {unbound:?}"
    );

    let bound = unbound_paths(
        "from ids import Kind\nfrom ids import Evidence as Proof\nfrom records import Decision\n\n\nmodel EvidenceId:\n    raw: str\n\n\ndef first[Proof](xs: list[Proof]) -> Proof:\n    return xs[0]\n\n\ndef main() -> None:\n    d = Decision(ids=[], items=[], kinds=[])\n    println(len(d.kinds))\n",
    )?;
    assert!(!bound.contains_key("Kind"), "an import binds its name: {bound:?}");
    assert!(
        !bound.contains_key("EvidenceId"),
        "a declaration binds its name: {bound:?}"
    );
    assert_eq!(
        bound.get("Evidence").map(String::as_str),
        Some("crate::ids::Evidence"),
        "an import under another name leaves the declaration's own name unbound: {bound:?}"
    );
    Ok(())
}

/// A type parameter named like a type another module declares keeps that name's spelling in the whole module, since a
/// bare name cannot say which of the two a use means.
#[test]
fn a_type_parameter_named_like_an_unbound_type_keeps_its_spelling_issue1561() -> Result<(), String> {
    let unbound = unbound_paths(
        "from records import Decision\n\n\nmodel Holder[Kind]:\n    items: list[Kind]\n\n\ndef main() -> None:\n    d = Decision(ids=[], items=[], kinds=[])\n    println(len(d.ids))\n",
    )?;
    assert!(!unbound.contains_key("Kind"), "{unbound:?}");
    assert_eq!(
        unbound.get("EvidenceId").map(String::as_str),
        Some("crate::ids::EvidenceId")
    );
    Ok(())
}
