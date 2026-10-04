//! Proofs over the corpus as a whole: the provider paths keep stable non-green dispositions, the schema
//! surfaces malformed cases (red state), the seed corpus is structurally sound and confirms its documented
//! behavior (green state), the replacement rows carry receipt-bound execution evidence, and the CI-readable
//! summary lands at its stable path (#655).

use super::*;

/// The #1156 provider paths each carry a stable disposition and none of them claims a comparison it cannot support.
///
/// Stated as its own test rather than left to the aggregate green-count assertion: the "every path has a stable
/// #987 disposition" contract is about these five rows specifically, and an aggregate count would still pass if one
/// of them quietly disappeared.
#[test]
fn every_provider_path_carries_a_stable_non_green_disposition() -> Result<(), Box<dyn std::error::Error>> {
    let corpus = seed_corpus();
    let expected = [
        "parity-987-1156-provider-allowed",
        "parity-987-1156-provider-denied",
        "parity-987-1156-provider-failed",
        "parity-987-1156-provider-redacted",
        "parity-987-1156-provider-cleanup",
    ];
    for id in expected {
        let case = corpus
            .iter()
            .find(|case| case.id == id)
            .ok_or(format!("the #1156 corpus row `{id}` must remain in the corpus"))?;
        match &case.disposition {
            Disposition::IntentionalMigration { owning_issue, .. } if *owning_issue == 1156 => {}
            disposition => {
                return Err(
                    format!("`{id}` must be an intentional migration owned by #1156, got {disposition:?}").into(),
                );
            }
        }
    }

    let summary = parity_corpus::summarize(&seed_corpus());
    for id in expected {
        let report = summary
            .cases
            .iter()
            .find(|case| case.id == id)
            .ok_or(format!("the summary must report `{id}`"))?;
        assert_eq!(
            report.overall_state,
            OverallState::NonGreenShadowUnavailable,
            "`{id}` must stay non-green until #1146 supplies a receipt-bound paired comparison",
        );
    }
    Ok(())
}

/// Build cases that are individually malformed in a distinct way, to prove [`validate_corpus`] catches each
/// problem rather than letting a broken case pass silently. These are never added to [`seed_corpus`].
fn malformed_cases_for_red_state_proof() -> Vec<ParityCase> {
    vec![
        ParityCase {
            id: "parity-987-dup",
            title: "First of a duplicate pair",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "loaves/compiler/incan_driver/tests/parity_corpus_tests.rs (red-state fixture)",
            disposition: Disposition::Preserved,
            // Never reaches `evaluate_case` — `red_state_validate_corpus_...` calls
            // `validate_corpus` directly, so this placeholder source is never evaluated into an observation.
            source: "",
            evaluate: Some(|| ComparisonOutcome::Match),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-dup",
            title: "Second of a duplicate pair (same id — must be flagged)",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "loaves/compiler/incan_driver/tests/parity_corpus_tests.rs (red-state fixture)",
            disposition: Disposition::Preserved,
            // Never reaches `evaluate_case` — `red_state_validate_corpus_...` calls
            // `validate_corpus` directly, so this placeholder source is never evaluated into an observation.
            source: "",
            evaluate: Some(|| ComparisonOutcome::Match),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-empty-title",
            title: "",
            category: BehaviorCategory::DiagnosticBehavior,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "loaves/compiler/incan_driver/tests/parity_corpus_tests.rs (red-state fixture)",
            disposition: Disposition::Preserved,
            // Never reaches `evaluate_case` — `red_state_validate_corpus_...` calls
            // `validate_corpus` directly, so this placeholder source is never evaluated into an observation.
            source: "",
            evaluate: Some(|| ComparisonOutcome::Match),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-unsupported-no-issue",
            title: "Unsupported disposition missing an owning issue and note",
            category: BehaviorCategory::AccidentalAcceptedBehavior,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "loaves/compiler/incan_driver/tests/parity_corpus_tests.rs (red-state fixture)",
            disposition: Disposition::Unsupported {
                owning_issue: 0,
                migration_note: "",
            },
            // Never reaches `evaluate_case` — `red_state_validate_corpus_...` calls
            // `validate_corpus` directly, so this placeholder source is never evaluated into an observation.
            source: "",
            evaluate: Some(|| ComparisonOutcome::Match),
            identity_conformance: None,
            replacement_execution: None,
        },
    ]
}

#[test]
fn red_state_validate_corpus_catches_duplicate_ids_missing_titles_and_unowned_dispositions() {
    let violations = validate_corpus(&malformed_cases_for_red_state_proof());

    let has_violation_matching = |case_id: &str, needle: &str| {
        violations
            .iter()
            .any(|v| v.case_id == case_id && v.problem.contains(needle))
    };

    assert!(
        has_violation_matching("parity-987-dup", "duplicate"),
        "expected a duplicate-id violation, got: {violations:?}"
    );
    assert!(
        has_violation_matching("parity-987-empty-title", "title"),
        "expected an empty-title violation, got: {violations:?}"
    );
    assert!(
        has_violation_matching("parity-987-unsupported-no-issue", "owning issue"),
        "expected a missing-owning-issue violation, got: {violations:?}"
    );
    assert!(
        has_violation_matching("parity-987-unsupported-no-issue", "migration note"),
        "expected a missing-migration-note violation, got: {violations:?}"
    );
    // Four distinct cases, at least one violation each (duplicate id reports on the second occurrence only).
    assert!(
        violations.len() >= 4,
        "expected at least 4 violations across the malformed fixtures, got {}: {violations:?}",
        violations.len()
    );
}

#[test]
fn seed_corpus_has_no_structural_violations() {
    let violations = validate_corpus(&seed_corpus());
    assert!(
        violations.is_empty(),
        "seed corpus has structural violations: {violations:?}"
    );
}

#[test]
fn seed_corpus_ids_are_stable_and_globally_unique() {
    let ids: Vec<&str> = seed_corpus().iter().map(|c| c.id).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len(), "seed corpus case ids must be globally unique");
    for id in &ids {
        assert!(
            id.starts_with("parity-987-") || id.starts_with("replacement-body-v0-"),
            "case id {id} must carry a stable #987 or #988 replacement-body namespace prefix"
        );
    }
}

#[test]
fn seed_corpus_every_case_confirms_its_documented_current_behavior() -> Result<(), Box<dyn std::error::Error>> {
    let corpus = seed_corpus();
    let summary = parity_corpus::summarize(&corpus);
    let regressions: Vec<&parity_corpus::CaseReport> = summary
        .cases
        .iter()
        .filter(|c| !c.behavior_outcome.is_green())
        .collect();
    assert!(
        regressions.is_empty(),
        "seed corpus cases whose evaluate() no longer confirms the documented current behavior (this means the \
         compiler's actual behavior drifted from what this corpus recorded — update the case or investigate the \
         regression, do not silently accept it): {regressions:#?}"
    );
    for case in corpus.iter().filter(|case| case.evaluate.is_some()) {
        let report = summary
            .cases
            .iter()
            .find(|report| report.id == case.id)
            .ok_or_else(|| format!("summary omitted callback row `{}`", case.id))?;
        let ReceiptRef::BehaviorObserved {
            evidence_identity,
            comparison_reason,
        } = &report.receipt
        else {
            return Err(format!(
                "callback row `{}` fabricated or borrowed an execution receipt: {:?}",
                case.id, report.receipt
            )
            .into());
        };
        assert_eq!(
            behavior_observation_identity(case.id, case.evidence, case.source, &report.behavior_outcome),
            *evidence_identity,
            "callback row `{}` did not bind its actual outcome into evidence",
            case.id
        );
        assert_ne!(
            behavior_observation_identity(
                case.id,
                case.evidence,
                case.source,
                &ComparisonOutcome::Mismatch {
                    detail: "tampered callback outcome".to_string(),
                },
            ),
            *evidence_identity,
            "callback row `{}` evidence ignored its observed outcome",
            case.id
        );
        assert!(!comparison_reason.is_empty());
    }
    Ok(())
}

/// Bind each selected direct-replacement source case to its own receipt and complete Body-IR proof evidence.
#[test]
fn replacement_body_v0_cases_have_receipt_bound_non_green_execution_evidence() -> Result<(), Box<dyn std::error::Error>>
{
    let summary = parity_corpus::summarize(&seed_corpus());
    let replacement_rows: Vec<&parity_corpus::CaseReport> = summary
        .cases
        .iter()
        .filter(|case| case.id.starts_with("replacement-body-v0-"))
        .collect();
    assert_eq!(
        replacement_rows.len(),
        30,
        "the nineteen original direct cases plus hashed membership, selected string helpers, scalar conversions, canonical Enumerate/Zip, string length, scalar JSON, hashed collection length, bounded bool truthiness, nonempty integer-list sorting, typed numeric carriers and checked isinstance targets must stay stable in #987"
    );
    let nominal_row = replacement_rows
        .iter()
        .find(|row| row.id == "replacement-body-v0-013")
        .ok_or("the #1154 nominal Body-IR row must remain in the corpus")?;
    let ReceiptRef::ReplacementExecuted { body_snapshot, .. } = &nominal_row.receipt else {
        return Err("the #1154 nominal Body-IR row must retain a direct execution receipt".into());
    };
    assert!(
        body_snapshot.contains("executed nominal constructor name=Pair id=decl:")
            && body_snapshot.contains("fields=[left, right]"),
        "the #1154 nominal row must bind its receipt evidence to the retained declaration identity and canonical layout: {body_snapshot}"
    );
    let value_enum_row = replacement_rows
        .iter()
        .find(|row| row.id == "replacement-body-v0-014")
        .ok_or("the #1154 value-enum Body-IR row must remain in the corpus")?;
    let ReceiptRef::ReplacementExecuted { body_snapshot, .. } = &value_enum_row.receipt else {
        return Err("the #1154 value-enum Body-IR row must retain a direct execution receipt".into());
    };
    assert!(
        body_snapshot.contains("executed value-enum variant name=HttpStatus::NotFound enum_id=decl:")
            && body_snapshot.contains("raw=404")
            && body_snapshot.contains("extracted value-enum scalar name=HttpStatus::NotFound"),
        "the #1154 value-enum row must bind receipt evidence to retained enum/member identities and scalar extraction: {body_snapshot}"
    );
    let fieldless_enum_row = replacement_rows
        .iter()
        .find(|row| row.id == "replacement-body-v0-015")
        .ok_or("the #1154 fieldless-enum Body-IR row must remain in the corpus")?;
    let ReceiptRef::ReplacementExecuted { body_snapshot, .. } = &fieldless_enum_row.receipt else {
        return Err("the #1154 fieldless-enum Body-IR row must retain a direct execution receipt".into());
    };
    assert!(
        body_snapshot.contains("executed fieldless-enum variant name=Signal::Ready enum_id=decl:")
            && body_snapshot.contains("executed fieldless-enum variant name=Signal::Stop enum_id=decl:"),
        "the #1154 fieldless-enum row must bind receipt evidence to retained enum/member identities: {body_snapshot}"
    );
    let pattern_row = replacement_rows
        .iter()
        .find(|row| row.id == "replacement-body-v0-016")
        .ok_or("the #1154 direct-pattern Body-IR row must remain in the corpus")?;
    let ReceiptRef::ReplacementExecuted { body_snapshot, .. } = &pattern_row.receipt else {
        return Err("the #1154 direct-pattern Body-IR row must retain a direct execution receipt".into());
    };
    assert!(
        body_snapshot.contains("nominal Pair id=decl:")
            && body_snapshot.contains("fieldless fieldless_enum_variant(Signal::Ready")
            && body_snapshot.contains("executed direct match arm"),
        "the #1154 pattern row must bind receipt evidence to retained targets and a selected direct arm: {body_snapshot}"
    );
    let result_row = replacement_rows
        .iter()
        .find(|row| row.id == "replacement-body-v0-017")
        .ok_or("the #1154 direct-Result Body-IR row must remain in the corpus")?;
    let ReceiptRef::ReplacementExecuted { body_snapshot, .. } = &result_row.receipt else {
        return Err("the #1154 direct-Result Body-IR row must retain a direct execution receipt".into());
    };
    assert!(
        body_snapshot.contains("result_ok(")
            && body_snapshot.contains("same_error_type=Failure")
            && body_snapshot.contains("executed Result::ok construction")
            && body_snapshot.contains("executed Result try route=ok"),
        "the #1154 Result row must bind receipt evidence to explicit construction and same-error routing: {body_snapshot}"
    );

    for row in replacement_rows {
        assert_eq!(row.lane, EvidenceLane::DirectReplacementBodyIr);
        if SHADOW_COMPARED_CASE_IDS.contains(&row.id) && summary.source_observable_comparison_available {
            // When the comparison ran, this row's evidence is the comparison itself. The dedicated receipt tests
            // above verify each compared row's typed result, exact streams, and independent route authority.
            continue;
        }
        assert_eq!(row.overall_state, OverallState::NonGreenShadowUnavailable);
        match &row.receipt {
            ReceiptRef::ReplacementExecuted {
                selection_identity,
                receipt_identity,
                output_identity,
                body_snapshot,
                ownership_reads,
                runtime_requirements,
                task_lifecycle,
                comparison_reason,
            } => {
                assert!(selection_identity.starts_with("sha256:"));
                assert!(receipt_identity.starts_with("sha256:"));
                assert!(output_identity.starts_with("sha256:"));
                assert!(body_snapshot.contains("body "));
                assert!(
                    ownership_reads
                        .iter()
                        .all(|read| read.span_end >= read.span_start && !read.fact.is_empty()),
                    "{} lost canonical ownership evidence: {ownership_reads:?}",
                    row.id
                );
                assert!(
                    runtime_requirements
                        .iter()
                        .all(|requirement| !requirement.requirement.is_empty()),
                    "{} emitted an invalid runtime-requirement projection: {runtime_requirements:?}",
                    row.id
                );
                if matches!(row.id, "replacement-body-v0-018" | "replacement-body-v0-019") {
                    assert!(
                        task_lifecycle.iter().any(|event| event.event == "constructed")
                            && task_lifecycle.iter().any(|event| event.event == "completed"),
                        "{} needs receipt-bound task construction/completion evidence: {task_lifecycle:?}",
                        row.id
                    );
                }
                // Rows that never declared a comparison say so; the declaring row, when its comparison could
                // not run, names the boundary that stopped it instead. Neither may imply generated Rust proved
                // anything.
                let expected_reason = if SHADOW_COMPARED_CASE_IDS.contains(&row.id) {
                    "the legacy route did not execute"
                } else {
                    "does not declare the bounded #1146 source-observable"
                };
                assert!(
                    comparison_reason.contains(expected_reason) || comparison_reason.contains("not staged"),
                    "{} must state why no comparison was made rather than implying generated-Rust evidence: \
                     {comparison_reason}",
                    row.id
                );
            }
            receipt => {
                return Err(format!(
                    "{} needs its own replacement execution receipt, got {receipt:?}",
                    row.id
                )
                .into());
            }
        }
    }
    Ok(())
}

/// Where the CI-readable summary is written, honoring a harness-selected `CARGO_TARGET_DIR` when set (matching
/// `incan_test_support`'s convention for other generated test artifacts) and falling back to the repository-local
/// `target/` directory otherwise.
fn summary_output_path() -> PathBuf {
    support::selected_harness_path("CARGO_TARGET_DIR", "target")
        .join("parity-corpus")
        .join("summary.json")
}

#[test]
fn ci_summary_serializes_with_the_fields_655_needs_and_is_written_to_a_stable_path()
-> Result<(), Box<dyn std::error::Error>> {
    let summary = parity_corpus::summarize(&seed_corpus());
    let json = serde_json::to_string_pretty(&summary)?;
    let value: serde_json::Value = serde_json::from_str(&json)?;

    for field in [
        "schema_version",
        "total_cases",
        "green",
        "non_green_shadow_unavailable",
        "non_green_shadow_diverged",
        "non_green_behavior",
        "execution_receipt_schema_available",
        "cases_with_execution_receipts",
        "source_observable_comparison_available",
        "cases",
    ] {
        assert!(
            value.get(field).is_some(),
            "CI summary is missing required top-level field `{field}`: {value}"
        );
    }
    assert!(
        value.get("receipt_schema_available").is_none(),
        "schema v7 must not retain the ambiguous field that implied every row had a receipt"
    );

    let cases = value
        .get("cases")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("CI summary `cases` must be a JSON array: {value}"))?;
    assert_eq!(cases.len(), seed_corpus().len());
    for case in cases {
        for field in [
            "id",
            "title",
            "category",
            "lane",
            "evidence",
            "disposition_kind",
            "behavior_outcome",
            "receipt",
            "identity_conformance",
            "overall_state",
        ] {
            assert!(
                case.get(field).is_some(),
                "CI summary case row is missing required field `{field}`: {case}"
            );
        }
    }

    let output_path = summary_output_path();
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output_path, &json)?;
    Ok(())
}
