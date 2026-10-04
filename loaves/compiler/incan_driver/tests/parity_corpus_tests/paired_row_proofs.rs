//! The paired-comparison rows: only rows with a real two-route comparison can be green, each selected row
//! carries its two route receipts, its Oven authority and its exact output, and an unavailable comparison keeps
//! the row's replacement evidence.

use super::*;

#[test]
fn only_rows_with_real_two_route_comparisons_can_be_green() -> Result<(), Box<dyn std::error::Error>> {
    // This is the corpus's core promise: direct replacement execution does not become green parity merely because
    // it has a receipt, and generated Rust never counts as proof. Only rows that declare the bounded #1146
    // comparison profile are green, and each is green only when that comparison actually ran through Oven and
    // agreed.
    //
    // The branch is taken on what the summary reports, not on whether a capability could be *resolved*: a staged
    // capability whose Oven build then fails has run no comparison, and must not be treated as if it had.
    let summary = parity_corpus::summarize(&seed_corpus());
    assert!(
        summary.execution_receipt_schema_available,
        "the summary must say the #986 execution-receipt schema is available now that PR #1120 landed it"
    );
    assert!(summary.cases_with_execution_receipts > 0);
    assert!(
        summary.cases_with_execution_receipts < summary.total_cases,
        "callback and artifact-only observations must not be counted as execution receipts"
    );
    assert_eq!(summary.non_green_shadow_diverged, 0);
    assert_eq!(summary.non_green_behavior, 0);

    let green: Vec<&str> = summary
        .cases
        .iter()
        .filter(|case| case.overall_state == OverallState::Green)
        .map(|case| case.id)
        .collect();

    if summary.source_observable_comparison_available {
        assert_eq!(
            green, SHADOW_COMPARED_CASE_IDS,
            "each selected row needs its own proven comparison; one matched row must not hide another unavailable row"
        );
        assert_eq!(summary.green, SHADOW_COMPARED_CASE_IDS.len());
        assert_eq!(
            summary.non_green_shadow_unavailable,
            summary.total_cases - SHADOW_COMPARED_CASE_IDS.len()
        );
    } else {
        // No comparison ran, so nothing may be green — including rows that declare one.
        require_staging_when_demanded(&summary)?;
        assert!(
            green.is_empty(),
            "no row may be green without a real comparison: {green:?}"
        );
        assert_eq!(summary.green, 0);
        assert_eq!(summary.non_green_shadow_unavailable, summary.total_cases);
    }
    Ok(())
}

/// Fail rather than report a skip when this environment declares that a comparison must have run.
///
/// Reads the reason straight off the compared row, so the failure says what actually stopped the comparison.
fn require_staging_when_demanded(summary: &parity_corpus::CorpusSummary) -> Result<(), Box<dyn std::error::Error>> {
    let reason = compared_row(summary)
        .map(|row| match &row.receipt {
            ReceiptRef::ReplacementExecuted { comparison_reason, .. } => comparison_reason.clone(),
            receipt => format!("{receipt:?}"),
        })
        .unwrap_or_else(|| "the compared row is missing from the corpus".to_string());
    assert!(
        !shadow_capability::legacy_route_is_required(),
        "{} is set but no source-observable comparison ran: {reason}",
        shadow_capability::REQUIRE_LEGACY_ROUTE_ENV
    );
    eprintln!("no source-observable comparison ran: {reason}");
    Ok(())
}

/// The original scalar row that declares the bounded #1146 comparison profile.
fn compared_row(summary: &parity_corpus::CorpusSummary) -> Option<&parity_corpus::CaseReport> {
    summary.cases.iter().find(|case| case.id == SHADOW_COMPARED_CASE_ID)
}

/// Canonical Enumerate/Zip bind exact source output and an integer result to two independent route receipts.
#[test]
fn the_enumerate_zip_row_carries_two_route_receipts_and_exact_output() -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};

    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == ENUMERATE_ZIP_SHADOW_CASE_ID)
        .ok_or("missing Enumerate/Zip comparison row")?;
    assert_eq!(row.overall_state, OverallState::Green);
    let ReceiptRef::ShadowMatched {
        profile_kind,
        profile_identity,
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
    } = &row.receipt
    else {
        return Err(format!("Enumerate/Zip needs matched two-route evidence, got {:?}", row.receipt).into());
    };
    let stdout = b"left\nleft\nright\npair\npair\n";
    let stdout_digest = format!("sha256:{:x}", Sha256::digest(stdout));
    let stderr_digest = format!("sha256:{:x}", Sha256::digest(b""));
    assert_eq!(
        profile_kind,
        incan_driver::backend::shadow::SHADOW_COMPARISON_PROFILE_ID
    );
    assert!(profile_identity.starts_with("sha256:"));
    assert_eq!(
        observable,
        &format!(
            "completed(Int, \"49\"); stdout={} bytes ({stdout_digest}); stderr=0 bytes ({stderr_digest})",
            stdout.len()
        )
    );
    for identity in [
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        &legacy_authority.oven_receipt_identity,
        &legacy_authority.oven_build_unit_identity,
        &legacy_authority.direct_rustc_plan_identity,
    ] {
        assert!(identity.starts_with("sha256:"), "{identity}");
    }
    assert_ne!(legacy_receipt_identity, replacement_receipt_identity);
    assert_ne!(legacy_output_identity, replacement_output_identity);
    assert!(
        !legacy_authority.cargo_process_started,
        "the native observation must be attributable to Oven rather than a Cargo process"
    );
    Ok(())
}

/// The compared row's evidence must name both routes' receipts and the Oven authority behind the legacy one.
#[test]
fn the_compared_row_carries_two_route_receipts_and_its_oven_authority() -> Result<(), Box<dyn std::error::Error>> {
    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = compared_row(&summary).ok_or("the compared row must be present in the corpus")?;
    assert_eq!(row.overall_state, OverallState::Green);

    let ReceiptRef::ShadowMatched {
        profile_kind,
        profile_identity,
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
    } = &row.receipt
    else {
        return Err(format!(
            "{} must carry matched two-route evidence, got {:?}",
            row.id, row.receipt
        )
        .into());
    };
    // #1153 links on the stable kind and cites the instance identity; a receipt must carry both.
    assert_eq!(
        profile_kind,
        incan_driver::backend::shadow::SHADOW_COMPARISON_PROFILE_ID
    );
    assert!(profile_identity.starts_with("sha256:"));
    let empty_stream_digest = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    assert_eq!(
        observable,
        &format!(
            "completed(Int, \"42\"); stdout=0 bytes ({empty_stream_digest}); stderr=0 bytes ({empty_stream_digest})"
        )
    );
    assert!(legacy_receipt_identity.starts_with("sha256:"));
    assert!(replacement_receipt_identity.starts_with("sha256:"));
    assert_ne!(
        legacy_receipt_identity, replacement_receipt_identity,
        "the two routes' receipts differ by selected and executed backend and must not be conflated"
    );
    assert_ne!(
        legacy_output_identity, replacement_output_identity,
        "each route's output identity must cover what that route actually produced"
    );

    // The legacy answer is attributable to a real Oven build, not an ad-hoc compiler invocation.
    assert!(legacy_authority.oven_receipt_identity.starts_with("sha256:"));
    assert!(legacy_authority.oven_build_unit_identity.starts_with("sha256:"));
    assert!(legacy_authority.direct_rustc_plan_identity.starts_with("sha256:"));
    assert!(
        !legacy_authority.cargo_process_started,
        "Oven-owned legacy execution must not start a Cargo process"
    );
    Ok(())
}

/// Hash membership binds exact program output and its typed result to two independent route receipts.
#[test]
fn the_hashed_membership_row_carries_two_route_receipts_and_exact_output() -> Result<(), Box<dyn std::error::Error>> {
    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == HASHED_SHADOW_CASE_ID)
        .ok_or("missing hashed membership row")?;
    assert_eq!(row.overall_state, OverallState::Green);
    let ReceiptRef::ShadowMatched {
        profile_kind,
        profile_identity,
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
    } = &row.receipt
    else {
        return Err(format!(
            "hashed membership needs matched two-route evidence, got {:?}",
            row.receipt
        )
        .into());
    };
    assert_eq!(
        profile_kind,
        incan_driver::backend::shadow::SHADOW_COMPARISON_PROFILE_ID
    );
    assert_eq!(
        observable,
        "completed(Bool, \"true\"); stdout=18 bytes (sha256:25eebc99ccbd29d7f5bb03931768c3c19a466df57a8c3deddcd7a7e1830ab04a); stderr=0 bytes (sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855)"
    );
    for identity in [
        profile_identity,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        &legacy_authority.oven_receipt_identity,
        &legacy_authority.oven_build_unit_identity,
        &legacy_authority.direct_rustc_plan_identity,
    ] {
        assert!(identity.starts_with("sha256:"), "{identity}");
    }
    assert_ne!(legacy_receipt_identity, replacement_receipt_identity);
    assert_ne!(legacy_output_identity, replacement_output_identity);
    assert!(!legacy_authority.cargo_process_started);
    Ok(())
}

/// The selected string row binds the typed result and both exact streams to independent no-fallback receipts.
#[test]
fn the_string_helper_row_carries_two_route_receipts_and_exact_output() -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};

    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == STRING_HELPER_SHADOW_CASE_ID)
        .ok_or("missing selected string-helper row")?;
    assert_eq!(row.overall_state, OverallState::Green);
    let ReceiptRef::ShadowMatched {
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
        ..
    } = &row.receipt
    else {
        return Err(format!("string helpers need matched two-route evidence, got {:?}", row.receipt).into());
    };
    let stdout = b"string helper checks\n";
    let stdout_digest = format!("sha256:{:x}", Sha256::digest(stdout));
    let stderr_digest = format!("sha256:{:x}", Sha256::digest(b""));
    assert_eq!(
        observable,
        &format!(
            "completed(Bool, \"true\"); stdout={} bytes ({stdout_digest}); stderr=0 bytes ({stderr_digest})",
            stdout.len()
        )
    );
    for identity in [
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        &legacy_authority.oven_receipt_identity,
        &legacy_authority.oven_build_unit_identity,
        &legacy_authority.direct_rustc_plan_identity,
    ] {
        assert!(identity.starts_with("sha256:"), "{identity}");
    }
    assert_ne!(legacy_receipt_identity, replacement_receipt_identity);
    assert_ne!(legacy_output_identity, replacement_output_identity);
    assert!(!legacy_authority.cargo_process_started);
    Ok(())
}

/// Scalar JSON binds its exact returned bytes and empty program streams to two independently verified receipts.
#[test]
fn the_scalar_json_row_carries_two_route_receipts_and_exact_output() -> Result<(), Box<dyn std::error::Error>> {
    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == JSON_STRINGIFY_SHADOW_CASE_ID)
        .ok_or("missing scalar JSON row")?;
    assert_eq!(row.overall_state, OverallState::Green);
    let ReceiptRef::ShadowMatched {
        profile_kind,
        profile_identity,
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
    } = &row.receipt
    else {
        return Err(format!("scalar JSON needs matched two-route evidence, got {:?}", row.receipt).into());
    };
    let empty_stream_digest = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    assert_eq!(
        profile_kind,
        incan_driver::backend::shadow::SHADOW_COMPARISON_PROFILE_ID
    );
    assert_eq!(
        observable,
        &format!(
            "completed(Str, {:?}); stdout=0 bytes ({empty_stream_digest}); stderr=0 bytes ({empty_stream_digest})",
            JSON_STRINGIFY_SCALARS_EXPECTED
        )
    );
    for identity in [
        profile_identity,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        &legacy_authority.oven_receipt_identity,
        &legacy_authority.oven_build_unit_identity,
        &legacy_authority.direct_rustc_plan_identity,
    ] {
        assert!(identity.starts_with("sha256:"), "{identity}");
    }
    assert_ne!(legacy_receipt_identity, replacement_receipt_identity);
    assert_ne!(legacy_output_identity, replacement_output_identity);
    assert!(!legacy_authority.cargo_process_started);
    Ok(())
}

/// Hashed entry count binds duplicate normalization and exact streams to two independently verified receipts.
#[test]
fn the_collection_len_row_carries_two_route_receipts_and_exact_output() -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};

    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == COLLECTION_LEN_SHADOW_CASE_ID)
        .ok_or("missing collection-length row")?;
    assert_eq!(row.overall_state, OverallState::Green);
    let ReceiptRef::ShadowMatched {
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
        ..
    } = &row.receipt
    else {
        return Err(format!(
            "collection length needs matched two-route evidence, got {:?}",
            row.receipt
        )
        .into());
    };
    let stdout = b"collection len\n";
    let stdout_digest = format!("sha256:{:x}", Sha256::digest(stdout));
    let stderr_digest = format!("sha256:{:x}", Sha256::digest(b""));
    assert_eq!(
        observable,
        &format!(
            "completed(Int, \"2200\"); stdout={} bytes ({stdout_digest}); stderr=0 bytes ({stderr_digest})",
            stdout.len()
        )
    );
    for identity in [
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        &legacy_authority.oven_receipt_identity,
        &legacy_authority.oven_build_unit_identity,
        &legacy_authority.direct_rustc_plan_identity,
    ] {
        assert!(identity.starts_with("sha256:"), "{identity}");
    }
    assert_ne!(legacy_receipt_identity, replacement_receipt_identity);
    assert_ne!(legacy_output_identity, replacement_output_identity);
    assert!(!legacy_authority.cargo_process_started);
    Ok(())
}

/// Canonical truthiness binds its bounded carrier result and exact streams to independently verified receipts.
#[test]
fn the_bool_truthiness_row_carries_two_route_receipts_and_exact_output() -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};

    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == BOOL_TRUTHINESS_SHADOW_CASE_ID)
        .ok_or("missing bool-truthiness row")?;
    assert_eq!(row.overall_state, OverallState::Green);
    let ReceiptRef::ShadowMatched {
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
        ..
    } = &row.receipt
    else {
        return Err(format!(
            "bool truthiness needs matched two-route evidence, got {:?}",
            row.receipt
        )
        .into());
    };
    let stdout = b"bool truthiness\n";
    let stdout_digest = format!("sha256:{:x}", Sha256::digest(stdout));
    let stderr_digest = format!("sha256:{:x}", Sha256::digest(b""));
    assert_eq!(
        observable,
        &format!(
            "completed(Bool, \"true\"); stdout={} bytes ({stdout_digest}); stderr=0 bytes ({stderr_digest})",
            stdout.len()
        )
    );
    for identity in [
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        &legacy_authority.oven_receipt_identity,
        &legacy_authority.oven_build_unit_identity,
        &legacy_authority.direct_rustc_plan_identity,
    ] {
        assert!(identity.starts_with("sha256:"), "{identity}");
    }
    assert_ne!(legacy_receipt_identity, replacement_receipt_identity);
    assert_ne!(legacy_output_identity, replacement_output_identity);
    assert!(!legacy_authority.cargo_process_started);
    Ok(())
}

/// Integer-list sorting binds order, source preservation, and exact streams to independently verified receipts.
#[test]
fn the_sorted_int_list_row_carries_two_route_receipts_and_exact_output() -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};

    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == SORTED_INT_LIST_SHADOW_CASE_ID)
        .ok_or("missing sorted-integer-list row")?;
    assert_eq!(row.overall_state, OverallState::Green);
    let ReceiptRef::ShadowMatched {
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
        ..
    } = &row.receipt
    else {
        return Err(format!(
            "sorted integer list needs matched two-route evidence, got {:?}",
            row.receipt
        )
        .into());
    };
    let stdout = b"sorted int list\n";
    let stdout_digest = format!("sha256:{:x}", Sha256::digest(stdout));
    let stderr_digest = format!("sha256:{:x}", Sha256::digest(b""));
    assert_eq!(
        observable,
        &format!(
            "completed(Int, \"29320233\"); stdout={} bytes ({stdout_digest}); stderr=0 bytes ({stderr_digest})",
            stdout.len()
        )
    );
    for identity in [
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        &legacy_authority.oven_receipt_identity,
        &legacy_authority.oven_build_unit_identity,
        &legacy_authority.direct_rustc_plan_identity,
    ] {
        assert!(identity.starts_with("sha256:"), "{identity}");
    }
    assert_ne!(legacy_receipt_identity, replacement_receipt_identity);
    assert_ne!(legacy_output_identity, replacement_output_identity);
    assert!(!legacy_authority.cargo_process_started);
    Ok(())
}

/// The typed-numeric row binds exact carrier identity, decimal scale, f32 rounding, and streams to both receipts.
#[test]
fn the_typed_numeric_row_carries_exact_type_and_two_route_receipts() -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};

    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == TYPED_NUMERIC_SHADOW_CASE_ID)
        .ok_or("missing typed-numeric row")?;
    assert_eq!(row.overall_state, OverallState::Green);
    let ReceiptRef::ShadowMatched {
        profile_kind,
        profile_identity,
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
    } = &row.receipt
    else {
        return Err(format!("typed numerics need matched two-route evidence, got {:?}", row.receipt).into());
    };
    let stdout = b"0 255 -170141183460469231731687303715884105728 340282366920938463463374607431768211455 19.90\n";
    let stdout_digest = format!("sha256:{:x}", Sha256::digest(stdout));
    let stderr_digest = format!("sha256:{:x}", Sha256::digest(b""));
    assert_eq!(
        profile_kind,
        incan_driver::backend::shadow::SHADOW_COMPARISON_PROFILE_ID
    );
    assert!(profile_identity.starts_with("sha256:"));
    assert_eq!(
        observable,
        &format!(
            "completed(Numeric(F32), \"1.2345679\"); stdout={} bytes ({stdout_digest}); stderr=0 bytes ({stderr_digest})",
            stdout.len()
        )
    );
    for identity in [
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        &legacy_authority.oven_receipt_identity,
        &legacy_authority.oven_build_unit_identity,
        &legacy_authority.direct_rustc_plan_identity,
    ] {
        assert!(identity.starts_with("sha256:"), "{identity}");
    }
    assert_ne!(legacy_receipt_identity, replacement_receipt_identity);
    assert_ne!(legacy_output_identity, replacement_output_identity);
    assert!(!legacy_authority.cargo_process_started);
    Ok(())
}

/// Checked `isinstance` targets bind their exact type-test output to independently verified route receipts.
#[test]
fn the_isinstance_targets_row_carries_two_route_receipts_and_exact_output() -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};

    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == ISINSTANCE_TARGETS_SHADOW_CASE_ID)
        .ok_or("missing checked-isinstance-target row")?;
    assert_eq!(row.overall_state, OverallState::Green);
    let ReceiptRef::ShadowMatched {
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
        ..
    } = &row.receipt
    else {
        return Err(format!(
            "checked isinstance targets need matched two-route evidence, got {:?}",
            row.receipt
        )
        .into());
    };
    let stdout = b"isinstance targets\n";
    let stdout_digest = format!("sha256:{:x}", Sha256::digest(stdout));
    let stderr_digest = format!("sha256:{:x}", Sha256::digest(b""));
    assert_eq!(
        observable,
        &format!(
            "completed(Bool, \"true\"); stdout={} bytes ({stdout_digest}); stderr=0 bytes ({stderr_digest})",
            stdout.len()
        )
    );
    for identity in [
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        &legacy_authority.oven_receipt_identity,
        &legacy_authority.oven_build_unit_identity,
        &legacy_authority.direct_rustc_plan_identity,
    ] {
        assert!(identity.starts_with("sha256:"), "{identity}");
    }
    assert_ne!(legacy_receipt_identity, replacement_receipt_identity);
    assert_ne!(legacy_output_identity, replacement_output_identity);
    assert!(!legacy_authority.cargo_process_started);
    Ok(())
}

/// The string-length row binds Unicode behavior and both exact streams to independent no-fallback receipts.
#[test]
fn the_string_len_row_carries_two_route_receipts_and_exact_output() -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};

    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == STRING_LEN_SHADOW_CASE_ID)
        .ok_or("missing string-length row")?;
    assert_eq!(row.overall_state, OverallState::Green);
    let ReceiptRef::ShadowMatched {
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
        ..
    } = &row.receipt
    else {
        return Err(format!("string length needs matched two-route evidence, got {:?}", row.receipt).into());
    };
    let stdout = b"string len\n";
    let stdout_digest = format!("sha256:{:x}", Sha256::digest(stdout));
    let stderr_digest = format!("sha256:{:x}", Sha256::digest(b""));
    assert_eq!(
        observable,
        &format!(
            "completed(Bool, \"true\"); stdout={} bytes ({stdout_digest}); stderr=0 bytes ({stderr_digest})",
            stdout.len()
        )
    );
    for identity in [
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        &legacy_authority.oven_receipt_identity,
        &legacy_authority.oven_build_unit_identity,
        &legacy_authority.direct_rustc_plan_identity,
    ] {
        assert!(identity.starts_with("sha256:"), "{identity}");
    }
    assert_ne!(legacy_receipt_identity, replacement_receipt_identity);
    assert_ne!(legacy_output_identity, replacement_output_identity);
    assert!(!legacy_authority.cargo_process_started);
    Ok(())
}

/// Scalar conversions bind a typed `str` result and their visible output to two independent route receipts.
#[test]
fn the_scalar_conversions_row_carries_two_route_receipts_and_exact_output() -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};

    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == SCALAR_CONVERSIONS_SHADOW_CASE_ID)
        .ok_or("missing scalar-conversions comparison row")?;
    assert_eq!(row.overall_state, OverallState::Green);
    let ReceiptRef::ShadowMatched {
        profile_kind,
        profile_identity,
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
    } = &row.receipt
    else {
        return Err(format!(
            "scalar conversions need matched two-route evidence, got {:?}",
            row.receipt
        )
        .into());
    };
    let stdout = b"converted: 42 3.14 10.0\n";
    let stdout_digest = format!("sha256:{:x}", Sha256::digest(stdout));
    let stderr_digest = format!("sha256:{:x}", Sha256::digest(b""));
    assert_eq!(
        profile_kind,
        incan_driver::backend::shadow::SHADOW_COMPARISON_PROFILE_ID
    );
    assert!(profile_identity.starts_with("sha256:"));
    assert_eq!(
        observable,
        &format!(
            "completed(Str, \"42 3.14 10.0\"); stdout={} bytes ({stdout_digest}); stderr=0 bytes ({stderr_digest})",
            stdout.len()
        )
    );
    for identity in [
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        &legacy_authority.oven_receipt_identity,
        &legacy_authority.oven_build_unit_identity,
        &legacy_authority.direct_rustc_plan_identity,
    ] {
        assert!(identity.starts_with("sha256:"), "{identity}");
    }
    assert_ne!(legacy_receipt_identity, replacement_receipt_identity);
    assert_ne!(legacy_output_identity, replacement_output_identity);
    assert!(!legacy_authority.cargo_process_started);
    Ok(())
}

/// A comparison that could not run still leaves the row the replacement execution it really performed.
#[test]
fn an_unavailable_comparison_keeps_the_rows_replacement_evidence() -> Result<(), Box<dyn std::error::Error>> {
    let summary = parity_corpus::summarize(&seed_corpus());
    if summary.source_observable_comparison_available {
        eprintln!("skipping: the comparison ran, so this row reports agreement rather than degraded evidence");
        return Ok(());
    }
    let row = compared_row(&summary).ok_or("the compared row must be present in the corpus")?;

    assert_eq!(row.overall_state, OverallState::NonGreenShadowUnavailable);
    let ReceiptRef::ReplacementExecuted {
        receipt_identity,
        body_snapshot,
        comparison_reason,
        ..
    } = &row.receipt
    else {
        return Err(format!(
            "an unavailable comparison must still report the replacement execution that ran, got {:?}",
            row.receipt
        )
        .into());
    };
    assert!(receipt_identity.starts_with("sha256:"));
    assert!(
        body_snapshot.contains("body add"),
        "the retained evidence must be the real Body-IR execution: {body_snapshot}"
    );
    assert!(!comparison_reason.is_empty(), "the row must say why no comparison ran");
    Ok(())
}

/// An unstaged Enumerate/Zip comparison remains explicitly non-green while retaining its direct receipt evidence.
#[test]
fn an_unavailable_enumerate_zip_comparison_keeps_its_replacement_evidence() -> Result<(), Box<dyn std::error::Error>> {
    let summary = parity_corpus::summarize(&seed_corpus());
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == ENUMERATE_ZIP_SHADOW_CASE_ID)
        .ok_or("the Enumerate/Zip comparison row must be present in the corpus")?;
    if matches!(&row.receipt, ReceiptRef::ShadowMatched { .. }) {
        eprintln!(
            "skipping: the Enumerate/Zip comparison ran, so this row reports agreement rather than degraded evidence"
        );
        return Ok(());
    }
    assert_eq!(row.overall_state, OverallState::NonGreenShadowUnavailable);
    let ReceiptRef::ReplacementExecuted {
        receipt_identity,
        body_snapshot,
        comparison_reason,
        ..
    } = &row.receipt
    else {
        return Err(format!(
            "an unavailable Enumerate/Zip comparison must retain direct replacement evidence, got {:?}",
            row.receipt
        )
        .into());
    };
    assert!(receipt_identity.starts_with("sha256:"));
    assert!(
        body_snapshot.contains("body enumerate_zip_profile"),
        "the retained evidence must be the real Enumerate/Zip Body-IR execution: {body_snapshot}"
    );
    assert!(
        !comparison_reason.is_empty(),
        "the row must name why its requested native comparison did not run"
    );
    Ok(())
}
