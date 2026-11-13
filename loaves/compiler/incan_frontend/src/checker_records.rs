//! Internal canonical-checker test sink. The Incan contract owns schemas, validation, framing and semantic digests;
//! these bootstrap adapters expose existing checked values and current native position bindings as field primitives.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use incan_semantics_core::stable_identity::{StableDeclarationContext, StableDeclarationId};

pub use crate::typechecker::canonical_nodes::NodeBindingError as RecordAdapterFault;

/// Exact canonical checker profile carried by graph body values and any cache capable of supplying their owners.
pub const GRAPH_BODY_RECORD_PROFILE: &str = "incan.checker-free-functions/16";

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

/// One graph-admitted editable namespace, already decoded through its checked-interface and kernel codecs.
#[derive(Debug, Clone)]
pub struct GraphNamespaceRecord {
    /// Project-relative logical source path used by the source-projection record.
    pub source: String,
    /// Immutable public checker tables installed for downstream editable namespaces.
    pub interface: Arc<crate::typechecker::ModuleDependencyInterface>,
    /// Complete checked Body IR contribution used instead of this module's transient
    /// [`crate::typechecker::TypeCheckInfo`].
    pub body_ir: incan_semantics_core::body_ir::BodyIrModule,
    /// Stable Body IR owner ids paired with the lowering witnesses authenticated by this namespace's cold check.
    pub lowering_inputs: Vec<(incan_semantics_core::CompilerNodeId, String)>,
}

/// Keyed source of independently bounded graph body pages.
pub trait GraphBodyRecordSource: std::fmt::Debug + Send + Sync {
    /// Read and authenticate the page for one exact stable owner identity, returning absence without scanning peers.
    fn read(&self, identity: &str) -> Result<Option<GraphBodyRecord>, RecordAdapterFault>;

    /// Read and authenticate one complete editable namespace page by its project-relative source projection.
    fn read_namespace(&self, source: &str) -> Result<Option<GraphNamespaceRecord>, RecordAdapterFault>;
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
    /// Exact project and package declaration records read by this owner, derived from checked semantic facts.
    pub dependency_reads: Vec<CheckedOwnerDependencyRead>,
}

/// Project or package declaration source read by one checked owner.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CheckedOwnerDependencyRead {
    /// Canonical package identity, or `None` for another module in the current project.
    pub package: Option<String>,
    /// Canonical logical module segments within the package source root.
    pub module_path: Vec<String>,
    /// Declaration spelling at the provider site.
    pub declaration_name: String,
    /// Stable checked identity of the declaration whose declaration record authenticates this read.
    pub declaration_identity: String,
    /// Compiler-owned semantic subject that introduced this exact dependency edge.
    pub reference_subject: String,
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

    /// Read one complete namespace selected by the engine for this exact source projection.
    pub fn namespace(&self, source: &str) -> Result<Option<GraphNamespaceRecord>, RecordAdapterFault> {
        match &self.source {
            Some(source_records) => source_records.read_namespace(source),
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

/// Test-only cold comparison for a fixture whose nominal facts come from one admitted Loaf dependency.
#[cfg(test)]
fn verify_fixture_with_loaves(
    source: &str,
    name: &str,
    fields: Vec<Vec<String>>,
    loaves: Arc<crate::loaf_index::LoafIndex>,
    module_path: Vec<String>,
) -> Result<Vec<String>, RecordAdapterFault> {
    crate::typechecker::body_check::records::verify_fixture_with_loaves(source, name, fields, loaves, module_path)
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
    let stable_contexts = stable_dependency_contexts(&facts);
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
                checker.record_body_check_outcome(declaration.span, "fallback");
                return Some(Ok(CheckedOwnerRecordOutput::Fallback(CheckedOwnerRecordFallback {
                    name: function.name.clone(),
                    identity: None,
                    reason: "checked owner has no function binding".into(),
                })));
            };
            let canonical = match binding.identity.as_ref() {
                Some(identity) => identity,
                None => {
                    checker.record_body_check_outcome(declaration.span, "fallback");
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
                    checker.record_body_check_outcome(declaration.span, "fallback");
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
                .flat_map(|owner| dependencies.reachable_dependencies_from([owner.clone()]))
                .filter(|dependency| &dependency.target != canonical)
                .filter(|dependency| projectable_source_dependency(&dependency.target))
                .filter_map(|dependency| match &dependency.target.origin {
                    incan_semantics_core::SymbolOrigin::Package { library, module_path } => {
                        StableDeclarationId::from_canonical(&dependency.target, None, None).map(|stable| {
                            CheckedOwnerDependencyRead {
                                package: Some(library.clone()),
                                module_path: module_path.clone(),
                                declaration_name: dependency.target.declaration_name.clone(),
                                declaration_identity: stable.render_compact(),
                                reference_subject: dependency.subject.to_string(),
                            }
                        })
                    }
                    incan_semantics_core::SymbolOrigin::Module(dependency_module) => checker
                        .stable_dependency_identity_with_context(
                            &dependency.target,
                            stable_contexts.get(&dependency.target),
                        )
                        .map(|stable| CheckedOwnerDependencyRead {
                            package: None,
                            module_path: dependency_module.clone(),
                            declaration_name: dependency.target.declaration_name.clone(),
                            declaration_identity: stable.render_compact(),
                            reference_subject: dependency.subject.to_string(),
                        }),
                    _ => None,
                })
                .collect::<Vec<_>>();
            dependency_reads.sort();
            dependency_reads.dedup();
            let refusal = missing_record_reason(checker, declaration.span);
            if let Some(reason) = refusal {
                checker.record_body_check_outcome(declaration.span, "fallback");
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
            if checker.restored_body_record(declaration.span) {
                checker.record_body_check_outcome(declaration.span, "restored");
            } else {
                checker.record_body_check_outcome(declaration.span, "fresh");
            }
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

/// Only declarations with independent source projections cross a checked owner's publication boundary.
fn projectable_source_dependency(dependency: &incan_semantics_core::CanonicalSymbolId) -> bool {
    !matches!(
        dependency.kind,
        incan_semantics_core::SemanticSourceTargetKind::Local
            | incan_semantics_core::SemanticSourceTargetKind::Parameter
            | incan_semantics_core::SemanticSourceTargetKind::Receiver
            | incan_semantics_core::SemanticSourceTargetKind::GenericBinder
            | incan_semantics_core::SemanticSourceTargetKind::Module
            | incan_semantics_core::SemanticSourceTargetKind::RustItem
            | incan_semantics_core::SemanticSourceTargetKind::Builtin
            | incan_semantics_core::SemanticSourceTargetKind::Other(_)
    )
}

impl crate::typechecker::TypeChecker {
    /// Resolve the checked stable identity used by one editable dependency read.
    fn stable_dependency_identity(
        &self,
        dependency: &incan_semantics_core::CanonicalSymbolId,
    ) -> Option<StableDeclarationId> {
        self.stable_dependency_identity_with_context(dependency, None)
    }

    /// Resolve one dependency with the exact enclosing declaration retained at its checked reference site.
    fn stable_dependency_identity_with_context(
        &self,
        dependency: &incan_semantics_core::CanonicalSymbolId,
        context: Option<&incan_semantics_core::CanonicalStableDeclarationContext>,
    ) -> Option<StableDeclarationId> {
        if let Some(identity) = self.type_info().declarations.source_identities.get(dependency).cloned() {
            return Some(identity);
        }
        if let Some(context) = context {
            return stable_member_identity(dependency, &context.owner, context.binding_ordinal);
        }
        if let Some(owner) = self
            .type_info()
            .calls
            .compiler_generated_member_identities
            .get(dependency)
        {
            return stable_member_identity(dependency, owner, 0);
        }
        let owner = self
            .symbols
            .all_symbols()
            .iter()
            .enumerate()
            .find_map(|(index, symbol)| {
                let crate::symbols::SymbolKind::Type(info) = &symbol.kind else {
                    return None;
                };
                nominal_type_owns_member(info, dependency)
                    .then(|| self.symbols.identity_of(index))
                    .flatten()
            });
        if let Some(owner) = owner {
            return stable_member_identity(dependency, owner, 0);
        }
        StableDeclarationId::from_canonical(dependency, None, None)
    }
}

/// Bind a checked member to the canonical declaration retained for its enclosing nominal.
fn stable_member_identity(
    member: &incan_semantics_core::CanonicalSymbolId,
    owner: &incan_semantics_core::CanonicalSymbolId,
    binding_ordinal: u32,
) -> Option<StableDeclarationId> {
    let owner = StableDeclarationId::from_canonical(owner, None, None)?;
    let context = StableDeclarationContext { owner, binding_ordinal };
    StableDeclarationId::from_canonical(member, None, Some(context))
}

/// Join each canonical reference target to its unique checked stable context; disagreement stays absent.
fn stable_dependency_contexts(
    facts: &incan_semantics_core::SemanticFactStore,
) -> BTreeMap<incan_semantics_core::CanonicalSymbolId, incan_semantics_core::CanonicalStableDeclarationContext> {
    let mut grouped = BTreeMap::<_, BTreeSet<_>>::new();
    for subject in facts.subjects() {
        let identities = facts.symbol_identities_for(subject).collect::<BTreeSet<_>>();
        if identities.len() != 1 {
            continue;
        }
        let Some(identity) = identities.into_iter().next() else {
            continue;
        };
        grouped
            .entry(identity.clone())
            .or_default()
            .extend(facts.stable_declaration_contexts_for(subject).cloned());
    }
    grouped
        .into_iter()
        .filter_map(|(identity, contexts)| {
            (contexts.len() == 1)
                .then(|| contexts.into_iter().next().map(|context| (identity, context)))
                .flatten()
        })
        .collect()
}

/// Test canonical member provenance carried by one checked nominal type; spelling is never consulted.
fn nominal_type_owns_member(
    info: &crate::symbols::TypeInfo,
    dependency: &incan_semantics_core::CanonicalSymbolId,
) -> bool {
    match info {
        crate::symbols::TypeInfo::Class(info) => info
            .fields
            .values()
            .any(|field| field.identity.as_ref() == Some(dependency)),
        crate::symbols::TypeInfo::Model(info) => info
            .fields
            .values()
            .any(|field| field.identity.as_ref() == Some(dependency)),
        crate::symbols::TypeInfo::Enum(info) => info.variant_identities.values().any(|identity| identity == dependency),
        crate::symbols::TypeInfo::Builtin
        | crate::symbols::TypeInfo::TypeAlias
        | crate::symbols::TypeInfo::Newtype(_) => false,
    }
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
    use std::collections::BTreeMap;
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
    fn provider_index(source: &str) -> Result<(Arc<LoafIndex>, tempfile::TempDir), Box<dyn std::error::Error>> {
        let provider = program(source)?;
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
        let (loaves, _directory) = provider_index("pub def value() -> int:\n    return 41\n")?;
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
        let read = owner
            .dependency_reads
            .first()
            .ok_or("missing provider dependency read")?;
        assert_eq!(read.package, Some("provider".into()));
        assert_eq!(read.module_path, vec!["lib".to_string()]);
        assert_eq!(read.declaration_name, "value");
        assert_eq!(read.declaration_identity, "function:pub::provider::lib::value");
        assert!(read.reference_subject.starts_with("expr:main#"));
        for source in [
            "def main() -> None:\n    println(\"hello\")\n",
            "def greeting() -> str:\n    return \"hello\"\n\ndef main() -> None:\n    println(greeting())\n",
            "def increment(value: int) -> int:\n    return value + 1\n\npub def main() -> None:\n    println(increment(increment(1)))\n",
        ] {
            reusable_ordinary_calls(source)?;
        }
        let base = "def value() -> int:\n    return 1\n\ndef main() -> None:\n    println(value())\n";
        let signature = local_callee_identity(base)?;
        assert_eq!(signature, local_callee_identity(&base.replace("return 1", "return 2"))?);
        assert_ne!(
            signature,
            local_callee_identity(&base.replace("-> int", "-> str").replace("return 1", "return \"one\""))?
        );
        Ok(())
    }

    /// An editable sibling enum variant keeps the provider enum as its stable owner, even when another top-level
    /// declaration has the variant's spelling.
    #[test]
    fn sibling_nominal_member_stable_dependency_identity_keeps_owner() -> Result<(), Box<dyn std::error::Error>> {
        let provider = program(
            "pub model Cancelled:\n    reason: str\n\npub model First:\n    pub transaction: str\n\npub model Second:\n    pub transaction: str\n\npub enum CompletionStatus:\n    Success\n    Failed\n    Cancelled\n",
        )?;
        let module_path = vec!["engine_messages".into()];
        let mut provider_checker = TypeChecker::new();
        provider_checker.set_current_module_path(Some(module_path.clone()));
        provider_checker.register_dependency_module_path_segments("engine_messages", module_path.clone());
        let interface = provider_checker
            .build_dependency_interface_component(
                &[(
                    "engine_messages",
                    &provider,
                    Arc::from(crate::module::exported_symbols(&provider)),
                )],
                &[],
                true,
            )
            .pop()
            .ok_or("provider namespace was not publishable")?;

        let consumer = program(
            "from engine_messages import CompletionStatus, First, Second\n\ndef selected() -> CompletionStatus:\n    return CompletionStatus.Failed\n\ndef cancelled() -> CompletionStatus:\n    return CompletionStatus.Cancelled\n\ndef first(value: First) -> str:\n    return value.transaction\n\ndef second(value: Second) -> str:\n    return value.transaction\n",
        )?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["main".into()]));
        checker.register_dependency_module_path_segments("engine_messages", module_path);
        checker.select_dependency_interfaces(vec![interface]);
        checker
            .check_with_imports(&consumer, &[("engine_messages", &provider)])
            .map_err(|errors| format!("{errors:?}"))?;

        let mut reads = checker
            .symbols
            .all_symbols()
            .iter()
            .flat_map(|symbol| match &symbol.kind {
                crate::symbols::SymbolKind::Type(crate::symbols::TypeInfo::Enum(info)) => {
                    info.variant_identities.values().collect::<Vec<_>>()
                }
                crate::symbols::SymbolKind::Type(crate::symbols::TypeInfo::Model(info)) => info
                    .fields
                    .values()
                    .filter_map(|field| field.identity.as_ref())
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .filter(|identity| {
                identity.declaration_name == "Failed"
                    || identity.declaration_name == "Cancelled"
                    || identity.declaration_name == "transaction"
            })
            .map(|identity| {
                checker
                    .stable_dependency_identity(identity)
                    .map(|stable| stable.render_compact())
                    .ok_or_else(|| "missing variant dependency identity".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        reads.sort();
        reads.dedup();
        assert_eq!(
            reads,
            vec![
                "member/field:engine_messages::transaction#in(model:engine_messages::First)[0]",
                "member/field:engine_messages::transaction#in(model:engine_messages::Second)[0]",
                "member/variant:engine_messages::Cancelled#in(enum:engine_messages::CompletionStatus)[0]",
                "member/variant:engine_messages::Failed#in(enum:engine_messages::CompletionStatus)[0]",
            ]
        );
        Ok(())
    }

    /// A generated field identity retains its checked enclosing declaration when same-spelled defaults coexist.
    #[test]
    fn local_field_reads_keep_their_stable_declaration_owner() -> Result<(), Box<dyn std::error::Error>> {
        let program = program(
            "pub model ProgramOutcome:\n    pub transaction: Option[int] = None\n\npub model NativeSelection:\n    pub transaction: Option[int] = None\n",
        )?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["engine_programs".into()]));
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        let owner = checker
            .symbols
            .lookup("ProgramOutcome")
            .and_then(|symbol| checker.symbols.identity_of(symbol))
            .cloned()
            .ok_or("missing ProgramOutcome identity")?;
        let dependency = owner.generated_member("transaction", incan_semantics_core::SemanticSourceTargetKind::Field);
        let context = incan_semantics_core::CanonicalStableDeclarationContext {
            owner: owner.clone(),
            binding_ordinal: 0,
        };
        assert_eq!(
            checker
                .stable_dependency_identity_with_context(&dependency, Some(&context))
                .map(|identity| identity.render_compact()),
            Some("member/field:engine_programs::transaction#in(model:engine_programs::ProgramOutcome)[0]".into())
        );
        checker
            .type_info
            .record_compiler_generated_member_identity(dependency.clone(), owner);
        assert_eq!(
            checker
                .stable_dependency_identity(&dependency)
                .map(|identity| identity.render_compact()),
            Some("member/field:engine_programs::transaction#in(model:engine_programs::ProgramOutcome)[0]".into())
        );
        Ok(())
    }

    /// A body-local parameter read, as in Oven's `apple_sdk.retained_sdk`, is not an external source dependency.
    #[test]
    fn body_parameter_reads_stay_inside_their_owner() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def retained_sdk(basis: str) -> Result[str, str]:\n    if basis != \"selected\":\n        return Err(\"the developer selection changed\")\n    return Ok(basis)\n";
        let program = program(source)?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["apple_sdk".into()]));
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        let outputs = checked_owner_record_outputs(&program, &checker, &["apple_sdk".into()])?;
        let CheckedOwnerRecordOutput::Value(owner) = &outputs[0] else {
            return Err("retained_sdk did not publish a checked owner".into());
        };
        assert!(
            owner.dependency_reads.is_empty(),
            "body-local parameter escaped as a source dependency: {:?}",
            owner.dependency_reads
        );
        Ok(())
    }

    /// A local call publishes exactly its callee's checked signature identity, independently of the callee's body.
    fn local_callee_identity(source: &str) -> Result<String, Box<dyn std::error::Error>> {
        let parsed = program(source)?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["main".into()]));
        checker.check_program(&parsed).map_err(|errors| format!("{errors:?}"))?;
        let outputs = checked_owner_record_outputs(&parsed, &checker, &["main".into()])?;
        let mut values = BTreeMap::new();
        for output in outputs {
            let CheckedOwnerRecordOutput::Value(value) = output else {
                return Err("local callee signature fixture fell back".into());
            };
            values.insert(value.name.clone(), value);
        }
        let callee = values.get("value").ok_or("missing callee")?;
        let caller = values.get("main").ok_or("missing caller")?;
        assert_eq!(caller.dependency_reads.len(), 1);
        let read = caller.dependency_reads.first().ok_or("missing callee read")?;
        assert_eq!(read.package, None);
        assert_eq!(read.declaration_identity, callee.identity);
        Ok(read.declaration_identity.clone())
    }

    /// Builtins and nested ordinary calls retain their complete tooling provenance when a shifted owner reuses
    /// graph records. The independent cold checker supplies expected facts, never reconstruction inputs.
    fn reusable_ordinary_calls(source: &str) -> Result<(), Box<dyn std::error::Error>> {
        let consumer = program(source)?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["main".into()]));
        checker
            .check_program(&consumer)
            .map_err(|errors| format!("{errors:?}"))?;
        let mut records = Vec::new();
        for owner in checked_owner_record_outputs(&consumer, &checker, &["main".into()])? {
            let CheckedOwnerRecordOutput::Value(mut owner) = owner else {
                let CheckedOwnerRecordOutput::Fallback(fallback) = owner else {
                    return Err("missing checker output".into());
                };
                return Err(format!("ordinary owner {} fell back: {}", fallback.name, fallback.reason).into());
            };
            owner
                .local
                .first_mut()
                .ok_or("missing local header")?
                .insert(3, "local".into());
            owner
                .final_value
                .first_mut()
                .ok_or("missing final header")?
                .insert(3, "final".into());
            records.push(super::GraphBodyRecord {
                identity: owner.identity,
                local: owner.local,
                final_value: owner.final_value,
                body_ir: None,
            });
        }
        let moved = program(&format!("# shifted declaration\n{source}"))?;
        let mut selected = TypeChecker::new();
        selected.set_current_module_path(Some(vec!["main".into()]));
        selected.set_graph_body_records(super::GraphBodyRecords::new(records)?);
        selected.check_program(&moved).map_err(|errors| format!("{errors:?}"))?;
        assert_eq!(selected.check_work().function_bodies, 0);
        assert_eq!(selected.check_work().local_record_evaluations, 0);
        let mut cold = TypeChecker::new();
        cold.set_current_module_path(Some(vec!["main".into()]));
        cold.check_program(&moved).map_err(|errors| format!("{errors:?}"))?;
        assert_eq!(
            selected.type_info().c_abi.function_calls,
            cold.type_info().c_abi.function_calls
        );
        Ok(())
    }

    /// A nominal expression retains its canonical type identity without excluding an otherwise reusable owner.
    #[test]
    fn nominal_expression_type_identities_publish_reusable_owner() -> Result<(), Box<dyn std::error::Error>> {
        let (loaves, _directory) = provider_index(
            "pub model Choice:\n    pub value: int\n\npub def value() -> Choice:\n    return Choice(value=41)\n",
        )?;
        let source = "from pub::provider import value\n\ndef keep() -> int:\n    choice = value()\n    return 1\n";
        let program = program(source)?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["main".into()]));
        checker.set_loaves(loaves.clone());
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;

        let outputs = checked_owner_record_outputs(&program, &checker, &["main".into()])?;
        assert_eq!(outputs.len(), 1, "the nominal owner was silently omitted");
        let CheckedOwnerRecordOutput::Value(owner) = &outputs[0] else {
            let CheckedOwnerRecordOutput::Fallback(fallback) = &outputs[0] else {
                return Err("the nominal expression owner had no publication decision".into());
            };
            return Err(format!("the nominal expression owner fell back: {}", fallback.reason).into());
        };
        assert_eq!(owner.name, "keep");
        assert!(
            super::verify_fixture_with_loaves(source, "keep", owner.final_value.clone(), loaves, vec!["main".into()],)?
                .is_empty()
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
