//! Same-source native/direct proof for Unicode-scalar string length.

use incan::backend::replacement::ReplacementValue;
use incan::backend::shadow::ShadowComparisonProfile;
use incan::cli::commands::compare_source_observable;

#[path = "support/shadow_capability.rs"]
mod shadow_capability;

const STRING_LEN_SOURCE: &str = include_str!("fixtures/replacement/string_len.incn");

/// Global and method string length agree on result and ordinary program streams under receipt-backed native authority.
#[test]
fn string_len_matches_the_receipt_backed_native_route() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(reason) = shadow_capability::unstaged_legacy_route_reason()? {
        eprintln!("skipping: {reason}");
        return Ok(());
    }
    let capability = shadow_capability::legacy_capability()?;
    let workspace = tempfile::tempdir()?;
    let profile = ShadowComparisonProfile::new(STRING_LEN_SOURCE, "string_len", Vec::new());
    let comparison = compare_source_observable(&profile, &capability, workspace.path());
    assert!(comparison.matched(), "{:?}", comparison.state);

    let execution = comparison
        .replacement_execution
        .as_ref()
        .ok_or("missing direct execution")?;
    assert_eq!(execution.value, ReplacementValue::Bool(true));
    assert_eq!(execution.output.stdout(), "string len\n".as_bytes());
    assert!(execution.output.stderr().is_empty());
    assert!(execution.body_snapshot.contains("call helper:str_len"));

    let legacy = comparison.legacy.as_ref().ok_or("missing native evidence")?;
    let replacement = comparison.replacement.as_ref().ok_or("missing replacement evidence")?;
    assert_eq!(legacy.observation.stdout, "string len\n".as_bytes());
    assert_eq!(legacy.observation.stdout, replacement.observation.stdout);
    assert!(legacy.observation.stderr.is_empty());
    assert!(replacement.observation.stderr.is_empty());
    assert!(
        comparison
            .legacy_authority
            .as_ref()
            .is_some_and(|authority| !authority.cargo_process_started)
    );
    legacy.receipt()?.verify_identity()?;
    replacement.receipt()?.verify_identity()?;
    Ok(())
}
