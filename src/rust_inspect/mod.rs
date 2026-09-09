//! Compatibility façade for Rust inspection.
//!
//! Implementation ownership lives in the `rust_inspect` crate. This module keeps `incan` imports stable.

#[cfg(test)]
mod test_fixtures;

#[cfg(test)]
pub(crate) use test_fixtures::{
    write_async_result_probe_crate, write_borrowed_param_probe_crate, write_hyphenated_function_probe_crate,
    write_rustix_as_fd_probe_crate, write_substrait_probe_crate,
};

pub use ::rust_inspect::{
    Fidelity, InspectError, InspectResult, Inspector, InspectorConfig, OVEN_DIRECT_INSPECTION_AUTHORITY_FILE,
    OVEN_DIRECT_INSPECTION_MARKER, OvenInspectionRegistrySource, RustMetadataCache, RustMetadataError, RustWorkspace,
    ValidatedInspectionProject, extract_rust_item, oven_inspection_registry_source_roots,
    write_oven_inspection_source_authority, write_sealed_oven_inspection_source_authority,
};
