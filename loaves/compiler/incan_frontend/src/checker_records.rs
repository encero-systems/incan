//! Internal canonical-checker test sink. The Incan contract owns schemas, validation, framing and semantic digests;
//! these bootstrap adapters expose existing checked values and current native position bindings as field primitives.

use std::collections::BTreeMap;
use std::sync::Arc;

pub use crate::typechecker::canonical_nodes::NodeBindingError as RecordAdapterFault;

/// One graph-admitted body pair keyed by the compiler-owned stable declaration identity encoded in both records.
#[derive(Debug, Clone)]
pub struct GraphBodyRecord {
    /// Stable declaration identity authenticated by the Incan checker-record codec.
    pub identity: String,
    /// Pending local-family field primitives retained for publication through the graph authority.
    pub local: Vec<Vec<String>>,
    /// Settled final-family field primitives restored through the ordinary checked-body assembly.
    pub final_value: Vec<Vec<String>>,
}

/// Immutable graph-selected body inventory shared by module checkers in one compilation session.
#[derive(Debug, Clone, Default)]
pub struct GraphBodyRecords(Arc<BTreeMap<String, GraphBodyRecord>>);

/// Canonical field primitives emitted for one checked top-level free function.
///
/// This bootstrap publication view exposes no checker state and retains only values already emitted by their
/// declaration, local and final producer boundaries.
#[derive(Debug, Clone)]
pub struct CheckedOwnerRecordFields {
    /// Source spelling used to match the independently projected owner source record.
    pub name: String,
    /// Declaration-family field primitives.
    pub declaration: Vec<Vec<String>>,
    /// Pending local-family field primitives.
    pub local: Vec<Vec<String>>,
    /// Settled final-family field primitives.
    pub final_value: Vec<Vec<String>>,
}

impl GraphBodyRecords {
    /// Build one identity-keyed inventory, refusing duplicate identities rather than selecting by input order.
    pub fn new(records: Vec<GraphBodyRecord>) -> Result<Self, RecordAdapterFault> {
        let mut selected = BTreeMap::new();
        for record in records {
            let identity = record.identity.clone();
            if selected.insert(identity, record).is_some() {
                return Err(RecordAdapterFault::InvalidField("duplicate graph body identity".into()));
            }
        }
        Ok(Self(Arc::new(selected)))
    }

    /// Borrow one exact declaration's already codec-validated pair.
    pub(crate) fn get(&self, identity: &str) -> Option<&GraphBodyRecord> {
        self.0.get(identity)
    }

    /// Report whether this transport carries any graph-selected bodies.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Return one fixture owner's field primitives and destroy its native checker before returning. `declaration`
/// selects the independent declaration/default writer; false selects the settled statement owner.
pub fn fixture_record(source: &str, name: &str, declaration: bool) -> Result<Vec<Vec<String>>, RecordAdapterFault> {
    crate::typechecker::body_check::records::fixture_record(source, name, declaration)
}

/// Rebind canonical primitives through a new fixture revision and compare every native field with a separate cold
/// baseline. Returns rendered current diagnostics. This test sink is neither a production source nor a reuse counter.
pub fn verify_fixture(source: &str, name: &str, fields: Vec<Vec<String>>) -> Result<Vec<String>, RecordAdapterFault> {
    crate::typechecker::body_check::records::verify_fixture(source, name, fields)
}

/// Enter one local producer from decoded declaration and environment values, then discard its private checker.
///
/// The adapter accepts no ambient [`crate::typechecker::TypeChecker`]. It collects only the supplied revision's
/// declaration headers, installs the decoded records, enters the selected declaration's existing
/// `BodyCheckScope`, and returns that one producer output before settlement.
pub fn produce_local_record(
    source: &str,
    name: &str,
    declaration: Vec<Vec<String>>,
    environment: Vec<(String, String, Vec<Vec<String>>)>,
) -> Result<Vec<Vec<String>>, RecordAdapterFault> {
    crate::typechecker::body_check::records::produce_local_record(source, name, declaration, environment)
}

/// Enter one final producer from decoded local fields, settle that owner, and discard its private checker.
///
/// This adapter accepts no ambient [`crate::typechecker::TypeChecker`] and performs no body checking. It collects
/// only current source coordinates and headers, restores one authenticated local value, enters the existing
/// settlement operation and returns that owner's final fields.
pub fn produce_final_record(
    source: &str,
    name: &str,
    local: Vec<Vec<String>>,
    environment: Vec<(String, String, Vec<Vec<String>>)>,
) -> Result<Vec<Vec<String>>, RecordAdapterFault> {
    crate::typechecker::body_check::records::produce_final_record(source, name, local, environment)
}

/// Collect already-produced records for top-level free functions after one successful program check.
///
/// A function without all three producer outputs is omitted rather than reconstructed from whole-check state. Graph
/// publication therefore cannot manufacture a value that did not cross its owning semantic boundary.
pub fn checked_owner_record_fields(
    program: &crate::ast::Program,
    checker: &crate::typechecker::TypeChecker,
) -> Vec<CheckedOwnerRecordFields> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let crate::ast::Declaration::Function(function) = &declaration.node else {
                return None;
            };
            Some(CheckedOwnerRecordFields {
                name: function.name.clone(),
                declaration: checker.produced_declaration_record(declaration.span)?.clone(),
                local: checker.available_local_record(declaration.span)?.clone(),
                final_value: checker.available_final_record(declaration.span)?.clone(),
            })
        })
        .collect()
}
