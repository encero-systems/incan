//! Tests ported from the reference lowering's Incan test files, one module per file.
//!
//! `test_parallel_ranges` and `test_runtime_layouts` live beside their modules (`parallel_ranges`, `runtime_calls`).

mod bodies;
mod body_positions;
mod carrier_identities;
mod cleanup_chains;
mod compiler_record_adapters;
mod explicit_arguments;
mod explicit_input_storage;
mod fallible_factories;
mod indexed_field_reads;
mod input_access;
mod instance_products;
mod instance_requests;
mod model_types;
mod record_positions;
mod rust_runtime_calls;
mod scalar_reads;
