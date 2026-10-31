//! Internal canonical-checker test sink. The Incan contract owns schemas, validation, framing and semantic digests;
//! these bootstrap adapters expose existing checked values and current native position bindings as field primitives.

pub use crate::typechecker::canonical_nodes::NodeBindingError as RecordAdapterFault;

/// Return one fixture owner's field primitives and destroy its native checker before returning. `declaration`
/// selects the independent declaration/default writer; false selects the settled statement owner.
pub fn fixture_projection(source: &str, name: &str, declaration: bool) -> Result<Vec<Vec<String>>, RecordAdapterFault> {
    crate::typechecker::body_check::records::fixture_projection(source, name, declaration)
}

/// Rebind canonical primitives through a new fixture revision and compare every native field with a separate cold
/// oracle. Returns rendered current diagnostics. This test sink is neither a production source nor a reuse counter.
pub fn verify_fixture(source: &str, name: &str, fields: Vec<Vec<String>>) -> Result<Vec<String>, RecordAdapterFault> {
    crate::typechecker::body_check::records::verify_fixture(source, name, fields)
}

/// Return the actual pending local output before any settlement, then discard the source checker and writer.
pub fn fixture_local_projection(source: &str, name: &str) -> Result<Vec<Vec<String>>, RecordAdapterFault> {
    crate::typechecker::body_check::records::fixture_local_projection(source, name)
}

/// Compare a decoded pending local owner against an independent pre-settlement cold native oracle.
pub fn verify_local_fixture(source: &str, name: &str, fields: Vec<Vec<String>>) -> Result<(), RecordAdapterFault> {
    crate::typechecker::body_check::records::verify_local_fixture(source, name, fields)
}
