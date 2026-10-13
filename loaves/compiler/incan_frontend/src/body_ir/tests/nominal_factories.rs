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
