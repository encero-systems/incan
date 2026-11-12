//! Internal canonical-checker test sink. The Incan contract owns schemas, validation, framing and semantic digests;
//! these bootstrap adapters expose existing checked values and current native position bindings as field primitives.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub use crate::typechecker::canonical_nodes::NodeBindingError as RecordAdapterFault;

/// Exact canonical checker profile carried by graph body values and any cache capable of supplying their owners.
pub const GRAPH_BODY_RECORD_PROFILE: &str = "incan.checker-free-functions/11";

/// One graph-admitted body pair keyed by the compiler-owned stable declaration identity encoded in both records.
#[derive(Debug, Clone)]
pub struct GraphBodyRecord {
    /// Stable declaration identity authenticated by the Incan checker-record codec.
    pub identity: String,
    /// Pending local-family field primitives retained for publication through the graph authority.
    pub local: Vec<Vec<String>>,
    /// Settled final-family field primitives restored through the ordinary checked-body assembly.
    pub final_value: Vec<Vec<String>>,
    /// Kernel-decoded v55 executable owner frame, when the graph selected a complete Body IR value with the checker
    /// pair. The checker validates its stable owner identity and declaration name before exposing it to Body IR.
    pub body_ir: Option<incan_semantics_core::executable_representation::EncodedOwnerRepresentation>,
}

/// Keyed source of independently bounded graph body pages.
pub trait GraphBodyRecordSource: std::fmt::Debug + Send + Sync {
    /// Read and authenticate the page for one exact stable owner identity, returning absence without scanning peers.
    fn read(&self, identity: &str) -> Result<Option<GraphBodyRecord>, RecordAdapterFault>;
}

/// Immutable eager fixtures or a keyed page source shared by module checkers in one compilation session.
#[derive(Debug, Clone, Default)]
pub struct GraphBodyRecords {
    eager: Arc<BTreeMap<String, GraphBodyRecord>>,
    source: Option<Arc<dyn GraphBodyRecordSource>>,
}

/// Canonical field primitives emitted for one checked top-level free function.
///
/// This bootstrap publication view exposes no checker state and retains only values already emitted by their
/// declaration, local and final producer boundaries.
#[derive(Debug, Clone)]
pub struct CheckedOwnerRecordFields {
    /// Source spelling used to match the independently projected owner source record.
    pub name: String,
    /// Current physical owner span used only to pair this fresh record output with its native Body IR frame.
    pub span: crate::ast::Span,
    /// Stable declaration identity shared by the checker-record values and their eligibility decision.
    pub identity: String,
    /// Declaration-family field primitives.
    pub declaration: Vec<Vec<String>>,
    /// Pending local-family field primitives.
    pub local: Vec<Vec<String>>,
    /// Settled final-family field primitives.
    pub final_value: Vec<Vec<String>>,
    /// Exact package declaration source records read by this owner, derived from checked semantic facts.
    pub dependency_reads: Vec<CheckedOwnerDependencyRead>,
}

/// Package-owned declaration source read by one checked owner.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CheckedOwnerDependencyRead {
    /// Canonical package identity.
    pub package: String,
    /// Canonical logical module segments within the package source root.
    pub module_path: Vec<String>,
    /// Declaration spelling at the provider site.
    pub declaration_name: String,
}

/// One checked owner that cannot publish the complete record triple required for body reuse.
#[derive(Debug, Clone)]
pub struct CheckedOwnerRecordFallback {
    /// Source spelling used to match the independently projected owner source record.
    pub name: String,
    /// Stable declaration identity used by the explicit full-source eligibility value, when the identity producer
    /// itself succeeded. Its absence is an explicit fallback condition rather than a fabricated stable identity.
    pub identity: Option<String>,
    /// Exact producer-stage refusal or missing-output invariant that excluded this owner.
    pub reason: String,
}

/// Complete publication decision for one checked top-level free function.
#[derive(Debug, Clone)]
pub enum CheckedOwnerRecordOutput {
    /// All three checker-record producer values are present.
    Value(CheckedOwnerRecordFields),
    /// The owner remains on full-source checking for the stated reason.
    Fallback(CheckedOwnerRecordFallback),
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
        Ok(Self {
            eager: Arc::new(selected),
            source: None,
        })
    }

    /// Bind a lazy source whose implementation reads only the exact stable owner requested by the checker.
    pub fn from_source(source: Arc<dyn GraphBodyRecordSource>) -> Self {
        Self {
            eager: Arc::new(BTreeMap::new()),
            source: Some(source),
        }
    }

    /// Read one exact declaration's already codec-validated pair without scanning unrelated owner pages.
    pub(crate) fn get(&self, identity: &str) -> Result<Option<GraphBodyRecord>, RecordAdapterFault> {
        if let Some(record) = self.eager.get(identity) {
            return Ok(Some(record.clone()));
        }
        match &self.source {
            Some(source) => source.read(identity),
            None => Ok(None),
        }
    }

    /// Report whether this transport carries any graph-selected bodies.
    pub fn is_empty(&self) -> bool {
        self.eager.is_empty() && self.source.is_none()
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

/// Collect one explicit publication decision per top-level free function after a successful program check.
///
/// Complete triples expose only values that crossed their owning producer boundaries. Any missing producer output
/// becomes a fallback with its exact refusal; a checked owner is never silently omitted.
pub fn checked_owner_record_outputs(
    program: &crate::ast::Program,
    checker: &crate::typechecker::TypeChecker,
    module_path: &[String],
) -> Result<Vec<CheckedOwnerRecordOutput>, String> {
    let facts = checker.type_info().semantic_fact_store(module_path);
    let dependencies = incan_semantics_core::dependencies::CheckedDependencyGraph::from_fact_stores([&facts]);
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let crate::ast::Declaration::Function(function) = &declaration.node else {
                return None;
            };
            let Some(binding) = checker
                .type_info()
                .declarations
                .function_bindings_by_span
                .get(&declaration.span)
            else {
                return Some(Ok(CheckedOwnerRecordOutput::Fallback(CheckedOwnerRecordFallback {
                    name: function.name.clone(),
                    identity: None,
                    reason: "checked owner has no function binding".into(),
                })));
            };
            let canonical = match binding.identity.as_ref() {
                Some(identity) => identity,
                None => {
                    return Some(Ok(CheckedOwnerRecordOutput::Fallback(CheckedOwnerRecordFallback {
                        name: function.name.clone(),
                        identity: None,
                        reason: "checked owner has no canonical declaration identity".into(),
                    })));
                }
            };
            let identity = match checker.type_info().declarations.source_identities.get(canonical) {
                Some(identity) => identity.render_compact(),
                None => {
                    return Some(Ok(CheckedOwnerRecordOutput::Fallback(CheckedOwnerRecordFallback {
                        name: function.name.clone(),
                        identity: None,
                        reason: "checked owner has no stable declaration identity".into(),
                    })));
                }
            };
            let semantic_owners = facts
                .facts_by_kind(incan_semantics_core::SemanticFactKind::DeclarationIdentity)
                .filter_map(|fact| match &fact.value {
                    incan_semantics_core::SemanticFactValue::CanonicalIdentity(candidate)
                        if candidate.declaration_name == canonical.declaration_name =>
                    {
                        Some(candidate.clone())
                    }
                    _ => None,
                })
                .collect::<BTreeSet<_>>();
            let semantic_owner = if semantic_owners.len() == 1 {
                semantic_owners.iter().next()
            } else {
                semantic_owners
                    .iter()
                    .find(|candidate| candidate.declaration_span == canonical.declaration_span)
            };
            let mut dependency_reads = semantic_owner
                .into_iter()
                .flat_map(|owner| dependencies.reachable_from([owner.clone()]))
                .filter_map(|dependency| match &dependency.origin {
                    incan_semantics_core::SymbolOrigin::Package { library, module_path } => {
                        Some(CheckedOwnerDependencyRead {
                            package: library.clone(),
                            module_path: module_path.clone(),
                            declaration_name: dependency.declaration_name.clone(),
                        })
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            dependency_reads.sort();
            dependency_reads.dedup();
            let refusal = missing_record_reason(checker, declaration.span);
            if let Some(reason) = refusal {
                return Some(Ok(CheckedOwnerRecordOutput::Fallback(CheckedOwnerRecordFallback {
                    name: function.name.clone(),
                    identity: Some(identity),
                    reason,
                })));
            }
            let Some(declaration_record) = checker.available_declaration_record(declaration.span).cloned() else {
                return Some(Err("declaration record disappeared after eligibility selection".into()));
            };
            let Some(local_record) = checker.available_local_record(declaration.span).cloned() else {
                return Some(Err("local record disappeared after eligibility selection".into()));
            };
            let Some(final_record) = checker.available_final_record(declaration.span).cloned() else {
                return Some(Err("final record disappeared after eligibility selection".into()));
            };
            Some(Ok(CheckedOwnerRecordOutput::Value(CheckedOwnerRecordFields {
                name: function.name.clone(),
                span: declaration.span,
                identity,
                declaration: declaration_record,
                local: local_record,
                final_value: final_record,
                dependency_reads,
            })))
        })
        .collect()
}

/// Name the first missing producer stage using its retained refusal when one was emitted.
fn missing_record_reason(checker: &crate::typechecker::TypeChecker, owner: crate::ast::Span) -> Option<String> {
    if checker.available_declaration_record(owner).is_none() {
        return Some(match checker.declaration_record_refusal(owner) {
            Some(error) => format!("declaration record output refused: {error:?}"),
            None => "declaration record output missing".into(),
        });
    }
    if checker.available_local_record(owner).is_none() {
        return Some(match checker.local_record_refusal(owner) {
            Some(error) => format!("local record output refused: {error:?}"),
            None => "local record output missing".into(),
        });
    }
    if checker.available_final_record(owner).is_none() {
        return Some(match checker.final_record_refusal(owner) {
            Some(error) => format!("final record output refused: {error:?}"),
            None => "final record output missing".into(),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{CheckedOwnerDependencyRead, CheckedOwnerRecordOutput, checked_owner_record_outputs};
    use crate::library_exports::collect_checked_public_exports;
    use crate::library_manifest::LibraryManifest;
    use crate::loaf_index::LoafIndex;
    use crate::typechecker::TypeChecker;

    /// Parse one focused checker-record fixture through the ordinary source frontend.
    fn program(source: &str) -> Result<crate::ast::Program, Box<dyn std::error::Error>> {
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        Ok(crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?)
    }

    /// Build one checked provider artifact so the consumer reads a real package declaration identity.
    fn provider_index() -> Result<(Arc<LoafIndex>, tempfile::TempDir), Box<dyn std::error::Error>> {
        let provider = program("pub def value() -> int:\n    return 41\n")?;
        let mut checker = TypeChecker::new();
        checker.set_current_package_identity(Some("provider".into()));
        checker.set_current_module_path(Some(vec!["lib".into()]));
        checker
            .check_program(&provider)
            .map_err(|errors| format!("{errors:?}"))?;
        let exports = collect_checked_public_exports(&provider, &checker);
        let manifest = LibraryManifest::from_checked_exports("provider", "0.1.0", &exports);
        let directory = tempfile::tempdir()?;
        std::fs::write(directory.path().join("provider.incnlib"), manifest.to_json_string()?)?;
        let entry = LoafIndex::dependency("provider", "provider", directory.path())?;
        Ok((Arc::new(LoafIndex::new([entry])?), directory))
    }

    /// A program owner that calls an editable dependency publishes itself and the exact declaration record it read.
    #[test]
    fn program_owner_publishes_exact_package_declaration_read() -> Result<(), Box<dyn std::error::Error>> {
        let (loaves, _directory) = provider_index()?;
        let source = "from pub::provider import value\n\ndef main() -> int:\n    return value()\n";
        let consumer = program(source)?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["main".into()]));
        checker.set_loaves(loaves);
        checker
            .check_program(&consumer)
            .map_err(|errors| format!("{errors:?}"))?;

        let owner = consumer
            .declarations
            .iter()
            .find(|declaration| matches!(declaration.node, crate::ast::Declaration::Function(_)))
            .ok_or("missing consumer owner")?
            .span;
        assert!(
            checker
                .type_info()
                .declarations
                .function_bindings_by_span
                .get(&owner)
                .and_then(|binding| binding.identity.as_ref())
                .is_some(),
            "the checked owner has no canonical binding identity"
        );
        assert!(
            checker.available_declaration_record(owner).is_some(),
            "the checked owner has no declaration record"
        );
        assert!(
            checker.available_local_record(owner).is_some(),
            "the checked owner has no local record: {:?}",
            checker.local_record_refusal(owner)
        );
        assert!(
            checker.available_final_record(owner).is_some(),
            "the checked owner has no final record"
        );
        let owners = checked_owner_record_outputs(&consumer, &checker, &["main".into()])?;
        assert_eq!(owners.len(), 1, "a successful checked owner was silently omitted");
        let CheckedOwnerRecordOutput::Value(owner) = &owners[0] else {
            return Err("the dependency-consuming owner fell back".into());
        };
        assert_eq!(owner.name, "main");
        assert_eq!(
            owner.dependency_reads,
            vec![CheckedOwnerDependencyRead {
                package: "provider".into(),
                module_path: vec!["lib".into()],
                declaration_name: "value".into(),
            }]
        );
        Ok(())
    }

    /// A checked owner whose canonical record projection is unsupported publishes an explicit fallback reason.
    #[test]
    fn unsupported_checked_owner_publishes_fallback_reason() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def main() -> int:\n    match 1:\n        1 => return 2\n        _ => return 3\n";
        let program = program(source)?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["main".into()]));
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;

        let outputs = checked_owner_record_outputs(&program, &checker, &["main".into()])?;
        assert_eq!(outputs.len(), 1, "a checked owner was silently omitted");
        let CheckedOwnerRecordOutput::Fallback(fallback) = &outputs[0] else {
            return Err("the unsupported owner unexpectedly published a reusable value".into());
        };
        assert_eq!(fallback.name, "main");
        assert!(fallback.identity.is_some());
        assert!(fallback.reason.contains("ProtocolArtifacts.loan_free_scrutinees"));
        Ok(())
    }

    /// A checked owner whose signature cannot produce a stable identity remains an explicit source fallback. The Rust
    /// carrier `Rc` has no source identity, so the signature publishes no portable key.
    #[test]
    fn identityless_checked_owner_publishes_fallback_reason() -> Result<(), Box<dyn std::error::Error>> {
        let source = "from rust::std::rc import Rc\n\nmodel Guard:\n    held: int\n\ndef required_pin(pin: Option[Rc[Guard]]) -> Result[Rc[Guard], str]:\n    return Err(\"missing\")\n";
        let program = program(source)?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["main".into()]));
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;

        let outputs = checked_owner_record_outputs(&program, &checker, &["main".into()])?;
        assert_eq!(outputs.len(), 1, "a checked owner was silently omitted");
        let CheckedOwnerRecordOutput::Fallback(fallback) = &outputs[0] else {
            return Err("the identity-less owner unexpectedly published a reusable value".into());
        };
        assert_eq!(fallback.name, "required_pin");
        assert_eq!(fallback.identity, None);
        assert_eq!(fallback.reason, "checked owner has no stable declaration identity");
        Ok(())
    }
}
