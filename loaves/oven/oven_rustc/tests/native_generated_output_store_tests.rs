//! Native integration controls for the prepared public Oven APIs.
use oven_rustc as native_oven;
use oven_rustc::rustc as native_rustc;
#[path = "common/generated_output_store_cases.rs"]
mod generated_output_store_cases;

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
