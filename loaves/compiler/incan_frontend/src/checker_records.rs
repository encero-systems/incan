//! Internal canonical-checker test sink. The Incan contract owns schemas, validation, framing and semantic digests;
//! these bootstrap adapters expose existing checked values and current native position bindings as field primitives.

pub use crate::typechecker::canonical_nodes::NodeBindingError as RecordAdapterFault;

/// Return one fixture owner's produced field primitives and destroy its native checker before returning.
pub fn fixture_record(source: &str, name: &str, declaration: bool) -> Result<Vec<Vec<String>>, RecordAdapterFault> {
    crate::typechecker::body_check::records::fixture_record(source, name, declaration)
}

/// Rebind canonical primitives through a new fixture revision and compare every native field with a separate cold
/// baseline. Returns rendered current diagnostics. This test sink is neither a production source nor a reuse counter.
pub fn verify_declaration_fixture(
    source: &str,
    name: &str,
    fields: Vec<Vec<String>>,
) -> Result<Vec<String>, RecordAdapterFault> {
    crate::typechecker::body_check::records::verify_declaration_fixture(source, name, fields)
}

/// Return the actual pending local producer output before settlement, then discard the source checker and writer.
pub fn fixture_local_record(source: &str, name: &str) -> Result<Vec<Vec<String>>, RecordAdapterFault> {
    crate::typechecker::body_check::records::fixture_local_record(source, name)
}
