//! Static factory results retain checked owner arguments before Self is projected to the caller.

use super::{body_named, build, named_targets};
use incan_semantics_core::{IncanPrimitiveType, IncanType};

/// An annotated factory call and its propagated value retain the same closed OrdinalMap carrier as legacy.
#[test]
fn ordinal_map_bytes_factories_retain_checked_owner() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/emit_calls_and_builtins/ordinal_map_bytes_round_trip.incn"
        ),
        &["m", "ordinal_bytes"],
    )?;
    let body = body_named(&module, "run")?;
    assert!(
        body.locals.iter().all(|local| !open_ordinal_map(&local.ty)),
        "{}",
        body.render_snapshot()
    );
    let factories: Vec<_> = named_targets(&module, "run")
        .into_iter()
        .filter(|target| target.name.starts_with("OrdinalMap.from_"))
        .collect();
    assert_eq!(factories.len(), 2);
    assert!(
        factories
            .iter()
            .all(|target| target.type_args == [IncanType::Primitive(IncanPrimitiveType::Str)]),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

/// Keys and tuple-pair arguments close the original lookup fixture's static factory owner.
#[test]
fn ordinal_map_key_and_pair_factories_retain_checked_owner() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/snapshots_collections_and_strings/ordinal_map_str_lookup.incn"
        ),
        &["m", "ordinal_lookup"],
    )?;
    let body = body_named(&module, "direct_literal_construction")?;
    assert!(
        body.locals.iter().all(|local| !open_ordinal_map(&local.ty)),
        "{}",
        body.render_snapshot()
    );
    let factories: Vec<_> = named_targets(&module, "direct_literal_construction")
        .into_iter()
        .filter(|target| target.name.starts_with("OrdinalMap.from_"))
        .collect();
    assert_eq!(factories.len(), 2);
    assert!(
        factories
            .iter()
            .all(|target| target.type_args == [IncanType::Primitive(IncanPrimitiveType::Str)]),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

/// Serialized bytes alone do not determine the generic key type of an unannotated factory.
#[test]
fn ordinal_map_unconstrained_decode_keeps_owner_open() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.collections import OrdinalMap, OrdinalMapError\n\ndef decode(data: bytes) -> Result[None, OrdinalMapError]:\n    decoded = OrdinalMap.from_bytes(data)?\n    return Ok(None)\n",
        &["m", "ordinal_unconstrained"],
    )?;
    let body = body_named(&module, "decode")?;
    assert!(
        body.locals.iter().any(|local| open_ordinal_map(&local.ty)),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

/// A conflicting destination cannot authorize a native factory result even when legacy leaves the owner open.
#[test]
fn static_factory_conflicting_context_publishes_no_closed_result() -> Result<(), Box<dyn std::error::Error>> {
    let source = "class Holder[T]:\n    value: T\n\n    @staticmethod\n    def from_value(value: T) -> Self:\n        return Holder(value=value)\n\ndef choose() -> None:\n    holder: Holder[str] = Holder.from_value(3)\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["m".to_owned(), "factory_conflict".to_owned()];
    let mut checker = crate::typechecker::TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let start = source.find("Holder.from_value(3)").ok_or("factory call missing")?;
    let span = crate::ast::Span::new(start, start + "Holder.from_value(3)".len());
    let facts = checker.type_info();
    assert!(
        facts
            .calls
            .body_ir_factory_result_conflicts
            .contains(&(span.start, span.end))
    );
    assert!(
        !facts
            .calls
            .body_ir_call_result_types
            .contains_key(&(span.start, span.end))
    );
    assert!(
        !facts
            .expressions
            .body_ir_expr_types
            .contains_key(&(span.start, span.end))
    );
    let module = super::build_body_ir_module_v0(&program, &path, facts);
    let body = body_named(&module, "choose")?;
    assert!(
        body.locals.iter().any(|local| matches!(&local.ty,
            IncanType::Generic { base, args } if base == "Holder" && args == &[IncanType::Unknown]
        )),
        "legacy={:?}, native result={:?}, native expression={:?}, conflict={}\n{}",
        facts.expr_type(span),
        facts.calls.body_ir_call_result_types.get(&(span.start, span.end)),
        facts.expressions.body_ir_expr_types.get(&(span.start, span.end)),
        facts
            .calls
            .body_ir_factory_result_conflicts
            .contains(&(span.start, span.end)),
        body.render_snapshot(),
    );
    Ok(())
}

/// Inspect only the factory owner's unresolved argument, preserving ordinary open generic templates elsewhere.
fn open_ordinal_map(ty: &IncanType) -> bool {
    match ty {
        IncanType::Generic { base, args } => {
            (base == "OrdinalMap"
                && args
                    .iter()
                    .any(|ty| matches!(ty, IncanType::Unknown | IncanType::TypeVar(_))))
                || args.iter().any(open_ordinal_map)
        }
        _ => false,
    }
}
