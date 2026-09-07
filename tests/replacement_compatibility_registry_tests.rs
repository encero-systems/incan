//! Contract tests for the replacement compatibility control-plane registry.

use incan::replacement_compatibility::{
    ComparisonEvidence, IndependentComparisonState, LandingProvenanceState,
    REPLACEMENT_COMPATIBILITY_INVENTORY_SCHEMA_VERSION, checked_v0_5_public_capability_baseline,
    render_developer_projection, render_machine_readable_inventory, replacement_compatibility_registry,
    validate_replacement_compatibility_registry,
};

#[test]
fn release_pinned_baseline_is_checked_and_complete() -> Result<(), Box<dyn std::error::Error>> {
    let baseline = checked_v0_5_public_capability_baseline()?;

    assert_eq!(baseline.release.tag, "v0.5.0");
    assert_eq!(baseline.capabilities.len(), 67);
    assert_eq!(baseline.release.source_blob, "42f718a9c35f816a68bb3ff13578eaf6725e3d0b");
    assert_eq!(baseline.release.role.as_str(), "MigrationCompatibilityTarget");
    assert!(
        baseline
            .release
            .source_snapshot_path
            .contains("migration_baselines/v0.5.0")
    );
    assert!(
        baseline
            .capabilities
            .iter()
            .any(|capability| capability.id == "FirstClassFunctions")
    );
    assert!(baseline.capabilities.iter().any(|capability| capability.id == "StdWeb"));
    let mut unresolved = baseline
        .capabilities
        .iter()
        .filter(|capability| {
            matches!(
                capability.landing_provenance.state,
                LandingProvenanceState::HistoricalDiscrepancyUnresolved
            )
        })
        .map(|capability| capability.id.as_str())
        .collect::<Vec<_>>();
    unresolved.sort_unstable();
    assert_eq!(
        unresolved,
        [
            "AsyncAwait",
            "CodegraphInspection",
            "StdWeb",
            "TypeTokensReflection",
            "ValueEnums"
        ]
    );
    assert!(
        baseline
            .capabilities
            .iter()
            .filter(|capability| {
                matches!(
                    capability.landing_provenance.state,
                    LandingProvenanceState::HistoricalDiscrepancyUnresolved
                )
            })
            .all(|capability| capability.landing_provenance.owner_issue == Some(1153))
    );
    Ok(())
}

#[test]
fn registry_covers_the_baseline_without_claiming_parity() -> Result<(), Box<dyn std::error::Error>> {
    let baseline = checked_v0_5_public_capability_baseline()?;
    let registry = replacement_compatibility_registry();

    validate_replacement_compatibility_registry(&baseline, &registry)?;
    assert_eq!(registry.features.len(), 27);
    assert_eq!(registry.registration_sources.len(), 3);
    let body_ir = registry
        .registration_sources
        .iter()
        .find(|source| source.id == "frontend.body-ir.callable-values")
        .ok_or("missing Body-IR callable registration source")?;
    assert_eq!(body_ir.lifecycle.as_str(), "LocalImplementation");
    assert_eq!(
        body_ir.feature_ids,
        vec!["call.partial-binding".to_string(), "call.stored-callables".to_string()]
    );
    let executor = registry
        .registration_sources
        .iter()
        .find(|source| source.id == "backend.replacement.bounded-scalar-control")
        .ok_or("missing direct-executor registration source")?;
    assert_eq!(executor.lifecycle.as_str(), "LocalImplementation");
    assert_eq!(
        executor.feature_ids,
        vec![
            "language.control-flow".to_string(),
            "language.numeric-and-scalar".to_string(),
            "language.numeric-complete".to_string(),
            "async.tasks".to_string(),
        ]
    );
    let bootstrap = registry
        .registration_sources
        .iter()
        .find(|source| source.id == "replacement-compatibility.migration-bootstrap")
        .ok_or("missing migration bootstrap registration source")?;
    assert_eq!(bootstrap.lifecycle.as_str(), "MigrationBootstrap");
    assert_eq!(bootstrap.feature_ids.len(), 21);
    assert!(
        bootstrap
            .retirement_condition
            .as_deref()
            .is_some_and(|condition| condition.contains("every remaining feature and requirement"))
    );
    assert!(registry.features.iter().all(|feature| {
        !feature.evidence.is_parity_green()
            && matches!(
                feature.evidence.independent_comparison,
                IndependentComparisonState::NonGreenShadowUnavailable
            )
    }));
    let scalar = registry
        .features
        .iter()
        .find(|feature| feature.id == "language.numeric-and-scalar")
        .ok_or("missing scalar direct-profile feature")?;
    assert!(!scalar.evidence.is_parity_green());
    assert!(matches!(
        scalar.evidence.independent_comparison,
        IndependentComparisonState::NonGreenShadowUnavailable
    ));
    let compared_case_ids = scalar
        .evidence
        .surfaces
        .scoped_comparisons
        .iter()
        .map(|comparison| comparison.case_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        compared_case_ids,
        vec![
            "replacement-body-v0-001",
            "replacement-body-v0-022",
            "replacement-body-v0-025",
            "replacement-body-v0-027",
        ]
    );
    assert!(scalar.evidence.surfaces.scoped_comparisons.iter().all(|comparison| {
        matches!(comparison.state, IndependentComparisonState::ComparedMatch)
            && matches!(&comparison.evidence, ComparisonEvidence::Paired { .. })
    }));
    let iteration = registry
        .features
        .iter()
        .find(|feature| feature.id == "iteration.protocol-and-adapters")
        .ok_or("missing broad iterator feature")?;
    assert!(!iteration.evidence.is_parity_green());
    assert_eq!(iteration.evidence.direct_replacement.as_str(), "BlockedByRequirements");
    assert!(matches!(
        iteration.evidence.surfaces.scoped_comparisons.as_slice(),
        [comparison]
            if comparison.case_id == "replacement-body-v0-023"
                && matches!(comparison.state, IndependentComparisonState::ComparedMatch)
                && matches!(&comparison.evidence, ComparisonEvidence::Paired { .. })
    ));
    Ok(())
}

/// A selected-helper corpus match must not invent a frozen capability relation or promote broad formatting parity.
#[test]
fn selected_string_helper_evidence_does_not_invent_a_frozen_capability() -> Result<(), Box<dyn std::error::Error>> {
    let registry = replacement_compatibility_registry();
    assert!(
        registry
            .feature_links
            .iter()
            .all(|link| link.feature_id != "language.string-helpers"),
        "the frozen public baseline has no string-helper capability; do not invent a numeric or formatter crosswalk"
    );
    let broad = registry
        .features
        .iter()
        .find(|feature| feature.id == "language.strings-and-format")
        .ok_or("missing wider string feature")?;
    assert!(!broad.evidence.is_parity_green());
    assert_eq!(broad.evidence.direct_replacement.as_str(), "BlockedByRequirements");
    assert_eq!(
        broad
            .evidence
            .surfaces
            .scoped_comparisons
            .iter()
            .map(|comparison| comparison.case_id.as_str())
            .collect::<Vec<_>>(),
        vec!["replacement-body-v0-021", "replacement-body-v0-024"]
    );
    assert!(broad.evidence.surfaces.scoped_comparisons.iter().all(|comparison| {
        matches!(comparison.state, IndependentComparisonState::ComparedMatch)
            && matches!(&comparison.evidence, ComparisonEvidence::Paired { .. })
    }));
    assert!(
        broad.migration_or_blocker.as_deref().is_some_and(|note| {
            note.contains("replacement-body-v0-021") && note.contains("replacement-body-v0-024")
        })
    );
    Ok(())
}

/// Bounded membership, entry-count, and integer-sort evidence must not claim the frozen collection capability.
#[test]
fn hashed_bounded_evidence_does_not_invent_a_frozen_capability() -> Result<(), Box<dyn std::error::Error>> {
    let registry = replacement_compatibility_registry();
    assert!(
        registry
            .feature_links
            .iter()
            .all(|link| link.feature_id != "language.hashed-membership"),
        "the frozen StdCollections capability describes imported specialized containers, not plain set/dict membership"
    );
    let aggregates = registry
        .features
        .iter()
        .find(|feature| feature.id == "language.aggregates-and-projections")
        .ok_or("missing broad aggregate feature")?;
    assert!(!aggregates.evidence.is_parity_green());
    assert_eq!(aggregates.evidence.direct_replacement.as_str(), "BlockedByRequirements");
    assert_eq!(
        aggregates
            .evidence
            .surfaces
            .scoped_comparisons
            .iter()
            .map(|comparison| comparison.case_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "replacement-body-v0-020",
            "replacement-body-v0-026",
            "replacement-body-v0-028",
        ]
    );
    assert!(
        aggregates
            .evidence
            .surfaces
            .scoped_comparisons
            .iter()
            .all(|comparison| {
                matches!(comparison.state, IndependentComparisonState::ComparedMatch)
                    && matches!(&comparison.evidence, ComparisonEvidence::Paired { .. })
            })
    );
    assert!(aggregates.migration_or_blocker.as_deref().is_some_and(|note| {
        note.contains("replacement-body-v0-020")
            && note.contains("replacement-body-v0-026")
            && note.contains("replacement-body-v0-028")
    }));
    Ok(())
}

/// A bounded checked type-test match must not promote the broad nominal/union feature family.
#[test]
fn checked_isinstance_evidence_remains_case_scoped_and_non_green() -> Result<(), Box<dyn std::error::Error>> {
    let registry = replacement_compatibility_registry();
    let feature = registry
        .features
        .iter()
        .find(|feature| feature.id == "nominal.models-unions-enums")
        .ok_or("missing broad nominal/union feature")?;

    assert!(!feature.evidence.is_parity_green());
    assert_eq!(feature.evidence.body_ir.as_str(), "Partial");
    assert_eq!(feature.evidence.direct_replacement.as_str(), "BlockedByRequirements");
    assert_eq!(
        feature.evidence.independent_comparison.as_str(),
        "NonGreenShadowUnavailable"
    );
    assert_eq!(feature.disposition.as_str(), "Planned");
    assert_eq!(feature.owner_issue, Some(988));
    assert_eq!(
        feature
            .evidence
            .surfaces
            .scoped_comparisons
            .iter()
            .map(|comparison| comparison.case_id.as_str())
            .collect::<Vec<_>>(),
        vec!["replacement-body-v0-030"]
    );
    assert!(feature.migration_or_blocker.as_deref().is_some_and(|note| {
        note.contains("#1281")
            && note.contains("replacement-body-v0-030")
            && note.contains("Closed #1154")
            && note.contains("open #988")
    }));
    Ok(())
}

#[test]
fn joined_projection_is_deterministic_and_exposes_the_callable_boundary() -> Result<(), Box<dyn std::error::Error>> {
    let baseline = checked_v0_5_public_capability_baseline()?;
    let registry = replacement_compatibility_registry();

    let projection = render_developer_projection(&baseline, &registry)?;
    assert!(projection.contains("# Replacement compatibility inventory"));
    assert!(projection.contains("not a permanent second language-feature catalogue"));
    assert!(projection.contains("## Collector assembly and bootstrap retirement"));
    assert!(projection.contains("`frontend.body-ir.callable-values` | LocalImplementation"));
    assert!(projection.contains("`replacement-compatibility.migration-bootstrap` | MigrationBootstrap"));
    assert!(projection.contains("`call.stored-callables`"));
    assert!(projection.contains("NonGreenShadowUnavailable"));
    assert!(projection.contains("#1152"));
    assert!(projection.contains("HistoricalDiscrepancyUnresolved; owner #1153"));
    assert!(projection.contains("replacement-body-v0-001: ComparedMatch"));
    assert!(projection.contains("replacement-body-v0-025: ComparedMatch"));
    assert!(projection.contains("replacement-body-v0-027: ComparedMatch"));
    for case_id in [
        "replacement-body-v0-020",
        "replacement-body-v0-021",
        "replacement-body-v0-023",
        "replacement-body-v0-024",
        "replacement-body-v0-026",
        "replacement-body-v0-028",
        "replacement-body-v0-029",
        "replacement-body-v0-030",
    ] {
        assert!(projection.contains(&format!("Case `{case_id}` (ComparedMatch)")));
    }
    assert!(projection.contains("replacement-body-v0-020` through `replacement-body-v0-030"));
    assert!(projection.contains("legacy_receipt_identity"));
    assert!(projection.contains("replacement_receipt_identity"));
    assert!(projection.contains("completed comparison infrastructure #1146"));
    assert!(projection.contains("Closed #1152"));
    assert!(projection.contains("outstanding evidence owner #988"));
    assert!(projection.contains("unscheduled evidence debt"));
    assert!(!projection.contains("unavailable via #1146"));
    assert!(
        projection
            .contains("Case `replacement-body-v0-022` (ComparedMatch) using completed comparison infrastructure #1146")
    );
    assert!(
        projection
            .contains("Case `replacement-body-v0-025` (ComparedMatch) using completed comparison infrastructure #1146")
    );
    assert!(
        projection
            .contains("Case `replacement-body-v0-027` (ComparedMatch) using completed comparison infrastructure #1146")
    );
    assert!(!projection.contains("Completed #1146 case"));

    let machine: serde_json::Value = serde_json::from_str(&render_machine_readable_inventory(&baseline, &registry)?)?;
    assert!(machine.is_object());
    assert_eq!(
        machine.get("schema_version").and_then(serde_json::Value::as_u64),
        Some(u64::from(REPLACEMENT_COMPATIBILITY_INVENTORY_SCHEMA_VERSION))
    );
    assert!(machine.get("baseline").is_some());
    assert!(machine.get("registry").is_some());
    assert!(machine["registry"].get("registration_sources").is_some());
    Ok(())
}
