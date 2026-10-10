//! Native integration controls for the prepared public Oven APIs.
use oven_rustc as native_oven;
use oven_rustc::rustc as native_rustc;
#[path = "common/generated_output_store_cases.rs"]
mod generated_output_store_cases;

/// A missing inspection owner remains a typed cache miss; malformed coordinates remain hard failures.
#[test]
fn missing_project_inspection_owner_preserves_store_error() -> Result<(), Box<dyn std::error::Error>> {
    use native_rustc::{OvenProjectInspectionAuthorityRef, OvenRustcError, load_project_inspection_authority};
    use oven_store::store::{OvenStore, OvenStoreError, OvenStoreLimits};
    let root = tempfile::tempdir()?;
    let store = OvenStore::new(root.path(), OvenStoreLimits::new(1024, 1024, 1024));
    let identity = oven_store::digest_bytes(b"missing inspection owner");
    let reference = OvenProjectInspectionAuthorityRef {
        identity,
        receipt_identity: oven_store::digest_bytes(b"receipt"),
        build_unit_identity: oven_store::digest_bytes(b"build unit"),
    };
    let missing = load_project_inspection_authority(&store, &reference, "project", "source", "compiler");
    assert!(matches!(
        missing,
        Err(OvenRustcError::Store(OvenStoreError::Io { path, source }))
            if source.kind() == std::io::ErrorKind::NotFound
                && path.parent() == Some(root.path().join("entries").as_path())
    ));
    let malformed = OvenProjectInspectionAuthorityRef {
        identity: "../../outside".into(),
        ..reference
    };
    assert!(matches!(
        load_project_inspection_authority(&store, &malformed, "project", "source", "compiler"),
        Err(OvenRustcError::PlanSelection { .. })
    ));
    Ok(())
}

/// Direct rebuilds preserve immutable aliases and leave admitted outputs intact on compilation failure.
#[test]
fn direct_binary_rebuild_replaces_readonly_output_and_preserves_failed_build() -> Result<(), Box<dyn std::error::Error>>
{
    generated_output_store_cases::direct_binary_rebuild_replaces_readonly_output_and_preserves_failed_build()
}

/// Harnesses execute edited behavior and reuse the earlier admitted binary after source restoration.
#[test]
fn generated_test_store_keeps_harness_identity_and_restored_sources() -> Result<(), Box<dyn std::error::Error>> {
    generated_output_store_cases::generated_test_store_keeps_harness_identity_and_restored_sources()
}

/// Fresh outputs reuse the same admitted library and refuse stale source or environment.
#[test]
fn generated_library_store_reuses_across_outputs_and_refuses_stale_sources() -> Result<(), Box<dyn std::error::Error>> {
    generated_output_store_cases::generated_library_store_reuses_across_outputs_and_refuses_stale_sources()
}

/// Shared binaries execute with correct permissions and preserve source, environment, linker and lease authority.
#[test]
fn generated_binary_store_reuses_executable_bytes_across_outputs() -> Result<(), Box<dyn std::error::Error>> {
    generated_output_store_cases::generated_binary_store_reuses_executable_bytes_across_outputs()
}

/// Shared plans retain every native owner across stores and refuse substituted coordinates or member digests.
#[test]
fn shared_native_plan_retains_multi_store_owners_and_refuses_substitution() -> Result<(), Box<dyn std::error::Error>> {
    generated_output_store_cases::shared_native_plan_retains_multi_store_owners_and_refuses_substitution()
}
