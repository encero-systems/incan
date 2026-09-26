//! RFC 120 conformance proofs: a consistent wrong-owner or wrong-target selection fails coverage, an emitted
//! projection needs an exact Rust identifier token, typed coverage rejects invalid scopes and missing carriers,
//! and the RFC 120 rows publish real evidence without fabricating legacy execution.

use super::*;

#[test]
fn rfc_120_member_coverage_rejects_a_consistent_wrong_owner_selection() {
    let case = ParityCase {
        id: "parity-987-120-negative-wrong-owner",
        title: "Wrong-owner member identity must fail conformance",
        category: BehaviorCategory::SupportedLanguageContract,
        lane: EvidenceLane::PackageImportBoundary,
        evidence: "loaves/compiler/incan_driver/tests/parity_corpus_tests.rs::verify_wrong_owner_member_selection",
        disposition: Disposition::Preserved,
        source: IDENTITY_MATRIX_SRC,
        evaluate: None,
        identity_conformance: Some(IdentityConformancePlan::SourceGraph(SourceIdentityConformancePlan {
            modules: IDENTITY_MATRIX_MODULES,
            root_module: "identity_matrix",
            verify: verify_wrong_owner_member_selection,
            replacement: IdentityReplacementPlan::Unavailable {
                owning_issue: 1332,
                reason: "a negative fixture is rejected during conformance, so no route runs it; #1332 owns the paired reference route these rows would need if they ever did",
            },
            comparison_reason: "negative conformance fixture must fail before execution",
        })),
        replacement_execution: None,
    };
    let report = parity_corpus::evaluate_case(&case);
    assert_eq!(report.overall_state, OverallState::NonGreenBehavior);
    assert!(
        matches!(
            report.behavior_outcome,
            ComparisonOutcome::Mismatch { ref detail } if detail.contains("outside owner")
        ),
        "wrong-owner member selection must fail on exact declaration spans: {:?}",
        report.behavior_outcome
    );
}

#[test]
fn rfc_120_module_path_coverage_rejects_a_consistent_wrong_target_selection() {
    let case = ParityCase {
        id: "parity-987-120-negative-wrong-path-target",
        title: "Wrong-target module-path identity must fail conformance",
        category: BehaviorCategory::SupportedLanguageContract,
        lane: EvidenceLane::PackageImportBoundary,
        evidence: "loaves/compiler/incan_driver/tests/parity_corpus_tests.rs::verify_wrong_path_target_selection",
        disposition: Disposition::Preserved,
        source: IDENTITY_MATRIX_SRC,
        evaluate: None,
        identity_conformance: Some(IdentityConformancePlan::SourceGraph(SourceIdentityConformancePlan {
            modules: IDENTITY_MATRIX_MODULES,
            root_module: "identity_matrix",
            verify: verify_wrong_path_target_selection,
            replacement: IdentityReplacementPlan::Unavailable {
                owning_issue: 1332,
                reason: "a negative fixture is rejected during conformance, so no route runs it; #1332 owns the paired reference route these rows would need if they ever did",
            },
            comparison_reason: "negative conformance fixture must fail before execution",
        })),
        replacement_execution: None,
    };
    let report = parity_corpus::evaluate_case(&case);
    assert_eq!(report.overall_state, OverallState::NonGreenBehavior);
    assert!(
        matches!(
            report.behavior_outcome,
            ComparisonOutcome::Mismatch { ref detail }
                if detail.contains("path/module reconstructed or selected the wrong identity")
        ),
        "module-path selection must fail on the exact expected origin and name: {:?}",
        report.behavior_outcome
    );
}

#[test]
fn rfc_120_emitted_projection_requires_an_exact_rust_identifier_token() -> Result<(), Box<dyn std::error::Error>> {
    let projection = "__incan_v1_001122";
    let lookalikes =
        format!("fn {projection}_suffix() {{}}\n// fn {projection}() {{}}\nconst TEXT: &str = \"{projection}\";");
    assert!(
        exact_rust_identifier(&lookalikes, projection).is_err(),
        "prefixes, comments, and string literals must not prove an emitted projection"
    );
    let emitted = format!("fn {projection}() {{}}");
    assert_eq!(exact_rust_identifier(&emitted, projection)?, projection);
    Ok(())
}

#[test]
fn rfc_120_typed_coverage_rejects_invalid_scope_and_missing_carriers() {
    let invalid_scope = IdentityCoverageCell {
        binding: IdentityBindingForm::Local,
        namespace: IdentityNamespace::Member,
        scope: IdentityScope::Module,
        checked_identity: "member".to_string(),
        hir_identity: None,
        body_ir_identity: None,
        emitted_projection: Some("projection".to_string()),
    };
    assert!(
        matches!(
            validate_identity_coverage(&[invalid_scope]),
            Err(detail) if detail.contains("not a semantically valid namespace/scope combination")
        ),
        "member declarations must use owner scope"
    );

    let missing_hir = IdentityCoverageCell {
        binding: IdentityBindingForm::Import,
        namespace: IdentityNamespace::Lexical,
        scope: IdentityScope::Module,
        checked_identity: "callable".to_string(),
        hir_identity: None,
        body_ir_identity: None,
        emitted_projection: Some("projection".to_string()),
    };
    assert!(
        matches!(
            validate_identity_coverage(&[missing_hir]),
            Err(detail) if detail.contains("invalid HIR carrier presence")
        ),
        "a module-scope lexical cell must prove its HIR carrier"
    );

    let missing_projection = IdentityCoverageCell {
        binding: IdentityBindingForm::ReExport,
        namespace: IdentityNamespace::Member,
        scope: IdentityScope::Owner,
        checked_identity: "member".to_string(),
        hir_identity: None,
        body_ir_identity: None,
        emitted_projection: None,
    };
    assert!(
        matches!(
            validate_identity_coverage(&[missing_projection]),
            Err(detail) if detail.contains("invalid emitted projection carrier presence")
        ),
        "an owner-scope member cell must prove the linker's source-declaration projection"
    );
}

#[test]
fn rfc_120_rows_publish_real_conformance_evidence_without_fabricating_legacy_execution()
-> Result<(), Box<dyn std::error::Error>> {
    let summary = parity_corpus::summarize(&seed_corpus());
    let rows = summary
        .cases
        .iter()
        .filter(|row| row.id.starts_with("parity-987-120-"))
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 6, "the stable RFC 120 corpus rows must remain complete");
    for row in &rows {
        assert_eq!(
            row.overall_state,
            OverallState::NonGreenShadowUnavailable,
            "{} produced the wrong state from {:?}",
            row.id,
            row.behavior_outcome
        );
        let ReceiptRef::IdentityConformanceObserved {
            replacement_receipt_identity,
            evidence_identity,
            comparison_reason,
        } = &row.receipt
        else {
            return Err(format!(
                "{} lost its identity-conformance observation: {:?}",
                row.id, row.receipt
            )
            .into());
        };
        assert!(!comparison_reason.is_empty());
        let evidence = row
            .identity_conformance
            .as_ref()
            .ok_or_else(|| format!("{} omitted its conformance evidence", row.id))?;
        match (&evidence.subject, row.id) {
            (IdentityConformanceSubject::SourceGraph { graph_identity }, id)
                if id != "parity-987-120-06-release-artifact" =>
            {
                assert!(!graph_identity.is_empty());
            }
            (
                IdentityConformanceSubject::ReleaseArtifact {
                    fixture_input_identity,
                    artifact_content_identity,
                    recovered_observation_identity,
                },
                "parity-987-120-06-release-artifact",
            ) => {
                for identity in [
                    fixture_input_identity,
                    artifact_content_identity,
                    recovered_observation_identity,
                ] {
                    assert!(identity.starts_with("sha256:"));
                }
            }
            (subject, id) => return Err(format!("{id} reported the wrong conformance subject: {subject:?}").into()),
        }
        assert_eq!(
            identity_conformance_evidence_identity(evidence),
            *evidence_identity,
            "{} published an evidence identity that cannot be recomputed from its report",
            row.id
        );
        assert_eq!(
            evidence.evidence_identity, *evidence_identity,
            "{} split the receipt and report evidence identities",
            row.id
        );
        let mut tampered = evidence.clone();
        tampered.checked_relations.push("tampered checked relation".to_string());
        assert_ne!(
            identity_conformance_evidence_identity(&tampered),
            *evidence_identity,
            "{} evidence digest ignored a serialized checked relation",
            row.id
        );
        let mut tampered_subject = evidence.clone();
        match &mut tampered_subject.subject {
            IdentityConformanceSubject::SourceGraph { graph_identity } => graph_identity.push_str("-tampered"),
            IdentityConformanceSubject::ReleaseArtifact {
                artifact_content_identity,
                ..
            } => artifact_content_identity.push_str("-tampered"),
        }
        assert_ne!(
            identity_conformance_evidence_identity(&tampered_subject),
            *evidence_identity,
            "{} evidence digest ignored its typed conformance subject",
            row.id
        );
        if matches!(
            row.id,
            "parity-987-120-01-identity-matrix"
                | "parity-987-120-02-let-shadow"
                | "parity-987-120-03-mut-shadow"
                | "parity-987-120-04-generic-binder"
                | "parity-987-120-05-builtin-rebinding"
        ) {
            assert!(
                replacement_receipt_identity
                    .as_deref()
                    .is_some_and(|identity| identity.starts_with("sha256:"))
            );
            assert!(evidence.replacement_output_identity.is_some());
        } else {
            assert_eq!(replacement_receipt_identity, &None);
            assert_eq!(evidence.replacement_output_identity, None);
        }
    }

    let matrix = rows
        .iter()
        .find(|row| row.id == "parity-987-120-01-identity-matrix")
        .and_then(|row| row.identity_conformance.as_ref())
        .ok_or("RFC 120 identity matrix evidence is missing")?;
    assert_eq!(matrix.coverage_cells.len(), 26);
    // The matrix used to record #989 as owning an unavailable replacement route. #1260 and #1261 made cross-module
    // execution real, so the row now carries an executed output identity instead of an owner for its absence.
    assert_eq!(matrix.replacement_unavailable_issue, None);

    let artifact = rows
        .iter()
        .find(|row| row.id == "parity-987-120-06-release-artifact")
        .and_then(|row| row.identity_conformance.as_ref())
        .ok_or("RFC 120 release-artifact evidence is missing")?;
    assert_eq!(artifact.legacy_projections.len(), 4);
    assert!(
        artifact
            .artifact_observations
            .iter()
            .any(|item| item.contains("non-Incan"))
    );
    Ok(())
}

#[test]
fn rfc_120_expected_value_mismatch_retains_the_observed_replacement_receipt_and_evidence() {
    let case = ParityCase {
        id: "parity-987-120-negative-expected-value",
        title: "A completed replacement mismatch retains its execution evidence",
        category: BehaviorCategory::SupportedLanguageContract,
        lane: EvidenceLane::DirectReplacementBodyIr,
        evidence: "loaves/compiler/incan_driver/tests/parity_corpus_tests.rs::rfc_120_expected_value_mismatch_retains_the_observed_replacement_receipt_and_evidence",
        disposition: Disposition::Preserved,
        source: GENERIC_BINDER_SRC,
        evaluate: None,
        identity_conformance: Some(IdentityConformancePlan::SourceGraph(SourceIdentityConformancePlan {
            modules: GENERIC_BINDER_MODULES,
            root_module: "identity_generic",
            verify: verify_generic_binder,
            replacement: IdentityReplacementPlan::Direct {
                module: "identity_generic",
                function: "generic_entry",
                arguments: no_replacement_arguments,
                expected: expected_three,
            },
            comparison_reason: "negative fixture executed one replacement route only",
        })),
        replacement_execution: None,
    };
    let report = parity_corpus::evaluate_case(&case);
    assert_eq!(report.overall_state, OverallState::NonGreenBehavior);
    assert!(
        matches!(
            report.behavior_outcome,
            ComparisonOutcome::Mismatch { ref detail }
                if detail.contains("returned Int(42), expected Int(3)")
        ),
        "unexpected mismatch outcome: {:?}",
        report.behavior_outcome
    );
    assert!(matches!(
        report.receipt,
        ReceiptRef::IdentityConformanceObserved {
            replacement_receipt_identity: Some(ref identity),
            ..
        } if identity.starts_with("sha256:")
    ));
    assert!(
        report
            .identity_conformance
            .as_ref()
            .is_some_and(|evidence| evidence.replacement_output_identity.is_some()),
        "completed mismatch must retain the output identity that its receipt finalized"
    );
}
