//! Internal canonical-checker test sink. The Incan contract owns schemas, validation, framing and semantic digests;
//! these bootstrap adapters expose existing checked values and current native position bindings as field primitives.

use crate::body_ir::SettledCallRead;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use incan_semantics_core::stable_identity::{StableDeclarationContext, StableDeclarationId};

pub use crate::typechecker::canonical_nodes::NodeBindingError as RecordAdapterFault;

/// Exact canonical checker profile carried by graph body values and any cache capable of supplying their owners.
pub const GRAPH_BODY_RECORD_PROFILE: &str = "incan.checker-free-functions/21";

/// One graph-admitted body pair keyed by the compiler-owned stable declaration identity encoded in both records.
#[derive(Debug, Clone)]
pub struct GraphBodyRecord {
    /// Stable declaration identity authenticated by the Incan checker-record codec.
    pub identity: String,
    /// Pending local-family field primitives retained for publication through the graph authority.
    pub local: Arc<Vec<Vec<String>>>,
    /// Settled final-family field primitives restored through the ordinary checked-body assembly.
    pub final_value: Arc<Vec<Vec<String>>>,
    /// Kernel-decoded v55 executable owner frame, when the graph selected a complete Body IR value with the checker
    /// pair. The checker validates its stable owner identity and declaration name before exposing it to Body IR.
    pub body_ir: Option<incan_semantics_core::executable_representation::EncodedOwnerRepresentation>,
}

/// The module identity is needed to select a namespace. Its checked declaration tables are only needed
/// when a source module imports it. The compiler adapter owns the authenticated encoding in between.
pub trait GraphNamespaceInterface: std::fmt::Debug + Send + Sync {
    /// True when the checked root belongs to this collected module, including synthetic module paths.
    fn belongs_to(&self, name: &str, path: &[String]) -> bool;

    /// Materialize and validate all declarations before another checker observes their lookup tables.
    fn installable(&self) -> Result<Arc<crate::typechecker::ModuleDependencyInterface>, String>;
}

impl GraphNamespaceInterface for crate::typechecker::CheckedNamespaceInterface {
    fn belongs_to(&self, name: &str, path: &[String]) -> bool {
        self.module_name() == name && self.module_path() == path
    }

    fn installable(&self) -> Result<Arc<crate::typechecker::ModuleDependencyInterface>, String> {
        self.installable()
    }
}

/// Immutable metadata shared by checking and a resident compiler's verified namespace body.
#[derive(Debug, Clone)]
pub struct GraphNamespaceHeader {
    /// Project-relative logical source path used by the source-projection record.
    pub source: String,
    /// Settled argument accesses imported by this namespace at publication.
    pub call_reads: Arc<[SettledCallRead]>,
    /// Authenticated declaration surface. A downstream source check materializes its lookup tables on demand.
    pub interface: Arc<dyn GraphNamespaceInterface>,
    /// Exact external declarations the ordinary checker selected for executable dependency resolution.
    pub package_roots: Arc<[incan_semantics_core::CanonicalSymbolId]>,
    /// Stable Body IR owner ids paired with the lowering witnesses authenticated by this namespace's cold check.
    pub lowering_inputs: Arc<[(incan_semantics_core::CompilerNodeId, String)]>,
}

/// One unchanged namespace admitted by its header: what checking reads of it, with no interface or Body IR decoded.
#[derive(Debug, Clone)]
pub struct GraphNamespaceAdmission {
    /// Settled argument accesses imported by this namespace at publication.
    pub call_reads: Arc<[SettledCallRead]>,
    /// Authenticated declaration surface, materialized from the namespace frame only when a checker installs it.
    pub interface: Arc<dyn GraphNamespaceInterface>,
}

/// A verified header with one movable, decoded Body IR contribution.
#[derive(Debug)]
pub struct GraphNamespaceRecord {
    pub header: Arc<GraphNamespaceHeader>,
    pub body_ir: incan_semantics_core::body_ir::BodyIrModule,
}

/// Keyed source of independently bounded graph body pages.
pub trait GraphBodyRecordSource: std::fmt::Debug + Send + Sync {
    /// Recognize a staged owner without admitting its contents. An unreadable page still needs ordinary validation.
    fn has_owner(&self, identity: &str) -> bool {
        !matches!(self.read(identity), Ok(None))
    }

    /// Read and authenticate the page for one exact stable owner identity, returning absence without scanning peers.
    fn read(&self, identity: &str) -> Result<Option<GraphBodyRecord>, RecordAdapterFault>;

    /// Ask the graph owner for the checked peers of an admitted namespace reopened by a settled call change.
    fn reopen_namespace_peers(&self, _source: &str, _changed: &BTreeSet<String>) -> Result<(), String> {
        Ok(())
    }

    /// Read and authenticate one complete editable namespace page by its project-relative source projection.
    fn read_namespace(&self, source: &str) -> Result<Option<GraphNamespaceRecord>, RecordAdapterFault>;

    /// Admit one unchanged namespace by its header alone: the settled call reads it consumed and its declaration
    /// surface, whose frame is opened only when a checker installs it. `None` when the request offers no header for
    /// `source`; the caller then reads the complete namespace.
    fn read_namespace_header(&self, _source: &str) -> Result<Option<GraphNamespaceAdmission>, RecordAdapterFault> {
        Ok(None)
    }

    /// Register a freshly published namespace's exact durable value and owned checked metadata.
    fn publish_namespace(&self, _header: Arc<GraphNamespaceHeader>, _value: String, _body_ir: Vec<u8>) {}

    /// Whether this activation already owns the exact header needed to keep a decoded body.
    fn retains_namespace(&self, _source: &str) -> bool {
        false
    }

    /// Keep an unchanged body at the end of this activation if this source owns its verified header.
    fn retain_namespace(&self, _source: &str, _body: &mut incan_semantics_core::body_ir::BodyIrModule) {}

    /// Retain exact authored bytes for namespaces whose decoded state was consumed by native preparation.
    fn settle_namespace_retention(&self) {}
}

/// Immutable eager fixtures or a keyed page source shared by module checkers in one compilation session.
#[derive(Debug, Clone, Default)]
pub struct GraphBodyRecords {
    eager: Arc<BTreeMap<String, GraphBodyRecord>>,
    current: Arc<BTreeMap<String, GraphBodyRecord>>,
    source: Option<Arc<dyn GraphBodyRecordSource>>,
    ranked_owner: Option<Arc<RankedOwner>>,
    call_reads: Arc<Mutex<BTreeMap<String, Arc<[SettledCallRead]>>>>,
    changed_callers: BTreeMap<String, BTreeSet<String>>,
    selected_calls: Arc<Mutex<BTreeMap<String, BTreeMap<String, Vec<SettledCallRead>>>>>,
}

/// One compiler activation shares its exact body-entry count across all of the unit's module checkers.
#[derive(Debug)]
struct RankedOwner {
    identity: String,
    contextual: Option<ContextualRank>,
    attempts: AtomicUsize,
    missing_peer: AtomicBool,
    damaged_input: AtomicBool,
}

/// A body-only edit scheduled from the graph's certified import order.
#[derive(Debug)]
struct ContextualRank {
    module: String,
    rank: i64,
    certificate: String,
    statement_entries: AtomicUsize,
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
    /// Exact overload families the checked owner enumerated before selecting its dispatch.
    pub method_lookups: Option<Vec<CheckedMethodLookup>>,
    /// Exact project and package declaration records read by this owner, derived from checked semantic facts.
    pub dependency_reads: Vec<CheckedOwnerDependencyRead>,
}

/// One same-project instance-method lookup and its signature-addressed candidate members.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CheckedMethodLookup {
    pub module: Vec<String>,
    pub receiver: String,
    pub method: String,
    pub candidates: Vec<String>,
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
    /// Every unsupported checked artifact grouped by its canonical family and native field.
    pub artifact_refusals: Vec<String>,
}

/// Complete publication decision for one checked top-level free function.
#[derive(Debug, Clone)]
pub enum CheckedOwnerRecordOutput {
    /// All three checker-record producer values are present.
    Value(CheckedOwnerRecordFields),
    /// The owner remains on full-source checking for the stated reason.
    Fallback(CheckedOwnerRecordFallback),
}

/// Name the checked artifact field one record row carries, as the checker reuse report groups refused fields.
///
/// A generated artifact row leads with its family's row tag and names its native field third. Any other row carries
/// no artifact field.
pub fn record_row_artifact_field(row: &[String]) -> Option<String> {
    let family = match row.first()?.as_str() {
        "artifact-call" => "CallArtifacts",
        "artifact-protocol" => "ProtocolArtifacts",
        "artifact-rust" => "RustInteropArtifacts",
        _ => return None,
    };
    Some(format!("{family}.{}", row.get(2)?))
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
            ..Self::default()
        })
    }

    /// Bind a lazy source whose implementation reads only the exact stable owner requested by the checker.
    pub fn from_source(source: Arc<dyn GraphBodyRecordSource>) -> Self {
        Self {
            eager: Arc::new(BTreeMap::new()),
            source: Some(source),
            ..Self::default()
        }
    }

    /// Require the ordinary compiler to evaluate one exact stable owner in its complete unit environment.
    pub fn for_ranked_owner(mut self, identity: &str) -> Self {
        self.ranked_owner = Some(Arc::new(RankedOwner {
            identity: identity.to_owned(),
            contextual: None,
            attempts: AtomicUsize::new(0),
            missing_peer: AtomicBool::new(false),
            damaged_input: AtomicBool::new(false),
        }));
        self
    }

    /// Schedule a graph-selected source edit inside the ordinary native compilation and its imported interfaces.
    pub fn for_contextual_ranked_owner(
        mut self,
        identity: String,
        module: String,
        rank: i64,
        certificate: String,
    ) -> Self {
        self.ranked_owner = Some(Arc::new(RankedOwner {
            identity,
            contextual: Some(ContextualRank {
                module,
                rank,
                certificate,
                statement_entries: AtomicUsize::new(0),
            }),
            attempts: AtomicUsize::new(0),
            missing_peer: AtomicBool::new(false),
            damaged_input: AtomicBool::new(false),
        }));
        self
    }

    /// The only body allowed to run during a standalone ranked reader activation.
    pub(crate) fn ranked_owner(&self) -> Option<&str> {
        self.ranked_owner
            .as_ref()
            .filter(|owner| owner.contextual.is_none())
            .map(|owner| owner.identity.as_str())
    }

    /// The exact body requested by either ranked entry point.
    pub(crate) fn demanded_owner(&self) -> Option<&str> {
        self.ranked_owner.as_ref().map(|owner| owner.identity.as_str())
    }

    /// Whether this native compilation carries a graph-authorized contextual edit.
    pub fn has_contextual_rank(&self) -> bool {
        self.ranked_owner
            .as_ref()
            .is_some_and(|owner| owner.contextual.is_some())
    }

    /// Add this checked module's actual statement entries, including methods and properties.
    pub fn count_contextual_statements(&self, count: usize) {
        if let Some(context) = self.ranked_owner.as_ref().and_then(|owner| owner.contextual.as_ref()) {
            context.statement_entries.fetch_add(count, Ordering::Relaxed);
        }
    }

    /// Account for the physical body entries of this certified edit after the native checker has settled.
    /// A peer miss keeps ordinary correctness and is visible as a declined ranked activation.
    pub fn report_contextual_ranked_owner(&self) {
        let Some(owner) = &self.ranked_owner else { return };
        let Some(context) = &owner.contextual else { return };
        let Some((entries, missing_peer, damaged_input)) = self.ranked_admission() else {
            return;
        };
        let statement_entries = context.statement_entries.load(Ordering::Relaxed);
        tracing::debug!(target: "incan::invocation", owner = %owner.identity, module = %context.module,
            rank = context.rank, certificate = %context.certificate, entries, statement_entries, missing_peer, damaged_input,
            admitted = entries == 1 && statement_entries == 1 && !missing_peer && !damaged_input,
            "settled contextual ranked body demand");
    }

    /// Permit one actual statement entry and identify a missing settled peer independently of source errors.
    pub(crate) fn enter_ranked_owner(&self, identity: Option<&str>) -> bool {
        let Some(owner) = &self.ranked_owner else { return true };
        if identity != Some(owner.identity.as_str()) {
            owner.missing_peer.store(true, Ordering::Relaxed);
            return owner.contextual.is_some();
        }
        let prior = owner.attempts.fetch_add(1, Ordering::Relaxed);
        prior == 0 || owner.contextual.is_some()
    }

    /// The observed statement-entry attempts and whether another owner lacked a reusable value.
    pub fn ranked_admission(&self) -> Option<(usize, bool, bool)> {
        self.ranked_owner.as_ref().map(|owner| {
            (
                owner.attempts.load(Ordering::Relaxed),
                owner.missing_peer.load(Ordering::Relaxed),
                owner.damaged_input.load(Ordering::Relaxed),
            )
        })
    }

    /// An invalid graph record must not be reported as a peer for which there was simply no prior value.
    pub(crate) fn refuse_ranked_input(&self) {
        if let Some(owner) = &self.ranked_owner {
            owner.damaged_input.store(true, Ordering::Relaxed);
        }
    }

    /// Give the active graph the first opportunity to check its demanded owner or authenticate a staged peer.
    pub(crate) fn selects_owner(&self, owner: &StableDeclarationId) -> bool {
        if self.ranked_owner.is_none() && self.current.is_empty() && self.is_empty() {
            return false;
        }
        let identity = owner.render_compact();
        self.demanded_owner() == Some(identity.as_str())
            || self.current.contains_key(&identity)
            || self.eager.contains_key(&identity)
            || self.source.as_ref().is_some_and(|source| source.has_owner(&identity))
    }

    /// Read one exact declaration's already codec-validated pair without scanning unrelated owner pages.
    pub(crate) fn get(&self, identity: &str) -> Result<Option<GraphBodyRecord>, RecordAdapterFault> {
        if self.demanded_owner() == Some(identity) {
            return Ok(None);
        }
        let mut record = match self.current.get(identity).or_else(|| self.eager.get(identity)) {
            Some(record) => Some(record.clone()),
            None => match &self.source {
                Some(source) => source.read(identity)?,
                None => None,
            },
        };
        if let Some(selected) = &mut record
            && let Some(frame) = &mut selected.body_ir
        {
            let source = &frame.source;
            let reader = frame.body.direct_call_id.path();
            if self
                .changed_callers
                .get(source)
                .is_some_and(|owners| owners.contains(reader))
            {
                selected.body_ir = None;
                return Ok(record);
            }
            let reads = if let Some(witness) = &frame.call_witness {
                let reads =
                    internal_models::decode_owner_call_reads(&frame.identity, &reader.to_owned(), source, witness)
                        .map_err(RecordAdapterFault::InvalidField)?;
                Some(
                    reads
                        .into_iter()
                        .map(|read| SettledCallRead {
                            reader: read.reader,
                            target: read.target,
                            digest: read.digest,
                        })
                        .collect(),
                )
            } else {
                self.call_reads
                    .lock()
                    .map_err(|_| RecordAdapterFault::InvalidField("settled call witness lock poisoned".into()))?
                    .get(source)
                    .map(|reads| reads.iter().filter(|r| r.reader == reader).cloned().collect())
            };
            if let Some(reads) = reads {
                self.selected_calls
                    .lock()
                    .map_err(|_| RecordAdapterFault::InvalidField("settled call reader lock poisoned".into()))?
                    .entry(source.clone())
                    .or_default()
                    .insert(reader.to_owned(), reads);
            } else if crate::body_ir::needs_call_witness(&mut frame.body) {
                selected.body_ir = None;
            }
        }
        Ok(record)
    }

    /// Read one complete namespace and remember only the settled ownership contracts it consumed.
    pub fn namespace(&self, source: &str) -> Result<Option<GraphNamespaceRecord>, RecordAdapterFault> {
        if self.changed_callers.contains_key(source) {
            return Ok(None);
        }
        let Some(records) = &self.source else {
            return Ok(None);
        };
        let record = records.read_namespace(source)?;
        if let Some(row) = &record {
            self.call_reads
                .lock()
                .map_err(|_| RecordAdapterFault::InvalidField("settled call witness lock poisoned".into()))?
                .insert(source.to_owned(), Arc::clone(&row.header.call_reads));
        }
        Ok(record)
    }

    /// Admit one namespace by its header, remembering the settled call contracts it consumed, as [`Self::namespace`]
    /// does; `None` when no header is offered for it, or when a changed call contract reopened its callers.
    pub fn namespace_admission(&self, source: &str) -> Result<Option<GraphNamespaceAdmission>, RecordAdapterFault> {
        if self.changed_callers.contains_key(source) {
            return Ok(None);
        }
        let Some(records) = &self.source else {
            return Ok(None);
        };
        let admission = records.read_namespace_header(source)?;
        if let Some(admitted) = &admission {
            self.call_reads
                .lock()
                .map_err(|_| RecordAdapterFault::InvalidField("settled call witness lock poisoned".into()))?
                .insert(source.to_owned(), Arc::clone(&admitted.call_reads));
        }
        Ok(admission)
    }

    /// Return only the call contracts of a namespace actually restored by this attempt.
    pub fn settled_call_reads(&self, source: &str) -> Result<Vec<SettledCallRead>, String> {
        self.call_reads
            .lock()
            .map_err(|_| "settled call witness lock poisoned".to_owned())?
            .get(source)
            .map(|reads| reads.to_vec())
            .ok_or_else(|| format!("restored namespace has no settled call witness: {source}"))
    }

    /// Register new complete output so the next saved activation can recognize it by its selected value digest.
    pub fn publish_namespace(&self, header: Arc<GraphNamespaceHeader>, value: String, body_ir: Vec<u8>) {
        if let Some(reader) = &self.source {
            reader.publish_namespace(header, value, body_ir);
        }
    }

    /// Settle the exact encoded products left over once native preparation returns each available decoded body.
    pub fn settle_namespace_retention(&self) {
        if let Some(reader) = &self.source {
            reader.settle_namespace_retention();
        }
    }

    /// Whether this request can put the checked body back beside a selected namespace.
    pub fn retains_namespace(&self, source: &str) -> bool {
        self.source
            .as_ref()
            .is_some_and(|reader| reader.retains_namespace(source))
    }

    /// Return one unchanged namespace body after the native consumer finishes using it.
    pub fn retain_namespace(&self, source: &str, body: &mut incan_semantics_core::body_ir::BodyIrModule) {
        if let Some(reader) = &self.source {
            reader.retain_namespace(source, body);
        }
    }

    /// Per-owner admissions inside a module that was rebuilt. The complete checked namespace did not supply these
    /// bodies.
    pub fn selected_call_reads(&self, source: &str) -> Result<Vec<SettledCallRead>, String> {
        let selected = self
            .selected_calls
            .lock()
            .map_err(|_| "settled call reader lock poisoned".to_owned())?;
        let Some(owners) = selected.get(source) else {
            return Ok(Vec::new());
        };
        Ok(owners
            .iter()
            .filter(|(reader, _)| {
                !self
                    .changed_callers
                    .get(source)
                    .is_some_and(|dirty| dirty.contains(*reader))
            })
            .flat_map(|(_, reads)| reads.iter().cloned())
            .collect())
    }

    /// Reuse completed source work when a changed call contract requires another attempt in this same activation.
    pub fn retain_current(&mut self, values: Vec<GraphBodyRecord>) {
        let current = Arc::make_mut(&mut self.current);
        for value in values {
            current.insert(value.identity.clone(), value);
        }
    }

    /// The current compile checked this owner itself, even if a subsequent attempt recovered its result.
    pub fn checked_here(&self, identity: &str) -> bool {
        self.current.contains_key(identity)
    }

    /// Reopen exactly these readers' executable bodies. Their checked local and final products remain admissible.
    pub fn reopen_callers(&mut self, changes: BTreeMap<String, BTreeSet<String>>) -> Result<(), String> {
        let mut added = false;
        for (source, owners) in changes {
            let current = self.changed_callers.entry(source.clone()).or_default();
            let before = current.len();
            current.extend(owners);
            if current.len() != before {
                added = true;
                let admitted = self
                    .call_reads
                    .lock()
                    .map_err(|_| "settled call witness lock poisoned".to_owned())?
                    .contains_key(&source);
                if let Some(records) = &self.source
                    && admitted
                {
                    records.reopen_namespace_peers(&source, current)?;
                }
            }
        }
        if !added {
            return Err("settled call ABI frontier made no progress".into());
        }
        self.selected_calls
            .lock()
            .map_err(|_| "settled call reader lock poisoned".to_owned())?
            .clear();
        Ok(())
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

/// Project the lookup names of a straight-line scalar owner from its checked bindings.
/// An unsupported expression or declaration keeps the broader namespace witness.
pub fn closed_scalar_scopes(
    program: &crate::ast::Program,
    checker: &crate::typechecker::TypeChecker,
) -> BTreeMap<crate::ast::Span, Vec<String>> {
    use crate::ast::{Declaration, ParamKind, Statement, Type};
    use crate::symbols::ResolvedType;
    use incan_lang::lang::types::{BuiltinTypeName, builtin_type_name};
    let mut scopes = BTreeMap::new();
    for item in &program.declarations {
        let Declaration::Function(function) = &item.node else {
            continue;
        };
        let documentation = crate::documentation::leading_documentation(&function.body);
        let [statement] = &function.body[documentation..] else {
            continue;
        };
        let expression = match &statement.node {
            Statement::Return(Some(value)) | Statement::Expr(value) => value,
            _ => continue,
        };
        if !function.decorators.is_empty()
            || !function.type_params.is_empty()
            || !function.surface_modifiers.is_empty()
            || function.result_source.is_some()
            || function
                .params
                .iter()
                .any(|p| p.node.kind != ParamKind::Normal || p.node.default.is_some() || p.node.is_mut)
        {
            continue;
        }
        let Some(binding) = checker.declaration_view().function(item.span) else {
            continue;
        };
        let Some(identity) = &binding.identity else { continue };
        if binding.is_async || binding.params.len() != function.params.len() || !function.name.is_ascii() {
            continue;
        }
        let actual = binding
            .params
            .iter()
            .map(|p| &p.ty)
            .chain(std::iter::once(&binding.return_type));
        let mut names = BTreeSet::from([function.name.clone()]);
        let types = function
            .params
            .iter()
            .map(|p| &p.node.ty.node)
            .chain(std::iter::once(&function.return_type.node));
        let scalar = types.zip(actual).all(|(ty, resolved)| {
            if matches!(ty, Type::Unit) {
                return *resolved == ResolvedType::Unit;
            }
            let Type::Simple(name) = ty else { return false };
            if ResolvedType::builtin_simple(name).as_ref() != Some(resolved) {
                return false;
            }
            if *resolved == ResolvedType::Unit {
                names.insert(name.clone());
                return true;
            }
            let Some(kind @ (BuiltinTypeName::Numeric(_) | BuiltinTypeName::StringLike(_))) = builtin_type_name(name)
            else {
                return false;
            };
            names.insert(name.clone());
            names.insert(kind.canonical().to_owned());
            true
        });
        if scalar
            && collect_scalar_lookup_names(expression, function, &identity.origin, checker.type_info(), &mut names)
        {
            scopes.insert(item.span, names.into_iter().collect());
        }
    }
    scopes
}

/// Record plain scalar calls through checked declarations. Primitive printing needs no display implementation lookup.
fn collect_scalar_lookup_names(
    value: &crate::ast::Spanned<crate::ast::Expr>,
    function: &crate::ast::FunctionDecl,
    origin: &incan_semantics_core::SymbolOrigin,
    facts: &crate::typechecker::TypeCheckInfo,
    names: &mut BTreeSet<String>,
) -> bool {
    use crate::ast::{CallArg, Expr};
    use crate::symbols::ResolvedType;
    use incan_lang::lang::builtins::BuiltinFnId;
    use incan_semantics_core::SemanticSourceTargetKind;
    if !matches!(
        facts.expr_type(value.span),
        Some(
            ResolvedType::Int
                | ResolvedType::Float
                | ResolvedType::Bool
                | ResolvedType::Numeric(_)
                | ResolvedType::Str
                | ResolvedType::FrozenStr
                | ResolvedType::Unit
        )
    ) {
        return false;
    }
    match &value.node {
        Expr::Literal(_) => true,
        Expr::Ident(name) => function.params.iter().any(|p| p.node.name == *name),
        Expr::Call(callee, type_args, args) if type_args.is_empty() => {
            let Expr::Ident(name) = &callee.node else { return false };
            let print = facts.resolved_builtin_call(value.span) == Some(BuiltinFnId::Print);
            let local = facts.resolved_identity(callee.span).is_some_and(|target| {
                target.kind == SemanticSourceTargetKind::Function
                    && &target.origin == origin
                    && target.declaration_name == *name
                    && facts.declarations.source_identities.contains_key(target)
            });
            if !print && !local {
                return false;
            }
            names.insert(name.clone());
            args.iter().all(|arg| {
                matches!(arg, CallArg::Positional(value) if collect_scalar_lookup_names(value, function, origin, facts, names))
            })
        }
        _ => false,
    }
}

/// Obtain a new declaration for a closed scalar owner using only its current source and previous stable identity.
/// `None` requests ordinary compiler discovery for syntax or bindings outside this independently checked family.
pub fn produce_closed_declaration(
    source: &str,
    name: &str,
    previous: Vec<Vec<String>>,
) -> Result<Option<Vec<Vec<String>>>, RecordAdapterFault> {
    crate::typechecker::body_check::records::produce_closed_declaration(source, name, previous)
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
    checked_record_outputs(program, checker, module_path, None)
}

/// Read the completed checker products of the single function named by an ordinary ranked demand.
pub fn checked_record_outputs(
    program: &crate::ast::Program,
    checker: &crate::typechecker::TypeChecker,
    module_path: &[String],
    name: Option<&str>,
) -> Result<Vec<CheckedOwnerRecordOutput>, String> {
    let owners = program.declarations.iter().filter_map(|declaration| {
        let crate::ast::Declaration::Function(function) = &declaration.node else {
            return None;
        };
        name.is_none_or(|selected| selected == function.name).then_some((
            function.name.as_str(),
            declaration.span,
            false,
        ))
    });
    checked_callable_record_outputs(program, checker, module_path, owners)
}

/// Publish graph products for ordinary functions and concrete nominal methods under their checked source identity.
/// Ranked function demands continue to select a single free function through the narrower producer above.
pub fn checked_graph_record_outputs(
    program: &crate::ast::Program,
    checker: &crate::typechecker::TypeChecker,
    module_path: &[String],
) -> Result<Vec<CheckedOwnerRecordOutput>, String> {
    let mut owners = Vec::new();
    for declaration in &program.declarations {
        if let crate::ast::Declaration::Function(function) = &declaration.node {
            owners.push((function.name.as_str(), declaration.span, false));
        }
        for method in crate::typechecker::canonical_nodes::nominal_methods(&declaration.node) {
            if method.node.body.is_some() && checker.available_declaration_record(method.span).is_some() {
                owners.push((method.node.name.as_str(), method.span, true));
            }
        }
    }
    checked_callable_record_outputs(program, checker, module_path, owners)
}

fn checked_callable_record_outputs<'a>(
    program: &crate::ast::Program,
    checker: &crate::typechecker::TypeChecker,
    module_path: &[String],
    owners: impl IntoIterator<Item = (&'a str, crate::ast::Span, bool)>,
) -> Result<Vec<CheckedOwnerRecordOutput>, String> {
    let facts = checker.type_info().semantic_fact_store(module_path);
    let dependencies = incan_semantics_core::dependencies::CheckedDependencyGraph::from_fact_stores([&facts]);
    let stable_contexts = stable_dependency_contexts(&facts);
    let mut identities_by_name = BTreeMap::<_, BTreeSet<_>>::new();
    for fact in facts.facts_by_kind(incan_semantics_core::SemanticFactKind::DeclarationIdentity) {
        if let incan_semantics_core::SemanticFactValue::CanonicalIdentity(identity) = &fact.value {
            identities_by_name
                .entry(identity.declaration_name.as_str())
                .or_default()
                .insert(identity);
        }
    }
    owners
        .into_iter()
        .map(|(name, span, method)| {
            let Some(binding) = checker.declaration_view().callable(span) else {
                checker.record_body_check_outcome(span, "fallback");
                return Ok(CheckedOwnerRecordOutput::Fallback(CheckedOwnerRecordFallback {
                    name: name.to_owned(),
                    identity: None,
                    reason: "checked owner has no function binding".into(),
                    artifact_refusals: Vec::new(),
                }));
            };
            let canonical = match binding.identity.as_ref() {
                Some(identity) => identity,
                None => {
                    checker.record_body_check_outcome(span, "fallback");
                    return Ok(CheckedOwnerRecordOutput::Fallback(CheckedOwnerRecordFallback {
                        name: name.to_owned(),
                        identity: None,
                        reason: "checked owner has no canonical declaration identity".into(),
                        artifact_refusals: Vec::new(),
                    }));
                }
            };
            let stable = if method {
                crate::typechecker::canonical_nodes::SemanticNodeBindings::declaration(checker, program, span)
                    .ok()
                    .map(|nodes| nodes.owner().clone())
            } else {
                checker
                    .type_info()
                    .declarations
                    .source_identities
                    .get(canonical)
                    .cloned()
            };
            let identity = match stable {
                Some(identity) => identity.render_compact(),
                None => {
                    checker.record_body_check_outcome(span, "fallback");
                    return Ok(CheckedOwnerRecordOutput::Fallback(CheckedOwnerRecordFallback {
                        name: name.to_owned(),
                        identity: None,
                        reason: "checked owner has no stable declaration identity".into(),
                        artifact_refusals: Vec::new(),
                    }));
                }
            };
            let semantic_owner = identities_by_name
                .get(canonical.declaration_name.as_str())
                .and_then(|candidates| {
                    if method {
                        candidates.get(&canonical).copied()
                    } else if candidates.len() == 1 {
                        candidates.iter().next().copied()
                    } else {
                        candidates
                            .iter()
                            .copied()
                            .find(|candidate| candidate.declaration_span == canonical.declaration_span)
                    }
                });
            if method && semantic_owner.is_none() {
                checker.record_body_check_outcome(span, "fallback");
                return Ok(CheckedOwnerRecordOutput::Fallback(CheckedOwnerRecordFallback {
                    name: name.to_owned(),
                    identity: Some(identity),
                    reason: "method has no exact checked dependency owner".into(),
                    artifact_refusals: Vec::new(),
                }));
            }
            let mut dependency_reads = Vec::new();
            for dependency in semantic_owner
                .into_iter()
                .flat_map(|owner| dependencies.direct_dependencies_from([owner.clone()]))
            {
                if &dependency.target == canonical || !projectable_source_dependency(&dependency.target) {
                    continue;
                }
                let (package, module_path) = match &dependency.target.origin {
                    incan_semantics_core::SymbolOrigin::Package { library, module_path } => {
                        (Some(library.clone()), module_path)
                    }
                    incan_semantics_core::SymbolOrigin::Module(module_path) => (None, module_path),
                    _ => continue,
                };
                let Some(stable) = checker.stable_dependency_identity_with_context(
                    &dependency.target,
                    stable_contexts.get(&dependency.target),
                ) else {
                    if method {
                        checker.record_body_check_outcome(span, "fallback");
                        return Ok(CheckedOwnerRecordOutput::Fallback(CheckedOwnerRecordFallback {
                            name: name.to_owned(),
                            identity: Some(identity),
                            reason: format!(
                                "method dependency has no stable declaration identity: {}",
                                dependency.target.render_compact()
                            ),
                            artifact_refusals: Vec::new(),
                        }));
                    }
                    continue;
                };
                dependency_reads.push(CheckedOwnerDependencyRead {
                    package,
                    module_path: module_path.clone(),
                    declaration_name: dependency.target.declaration_name.clone(),
                    declaration_identity: stable.render_compact(),
                    reference_subject: dependency.subject.to_string(),
                });
            }
            dependency_reads.sort();
            dependency_reads.dedup();
            let refusal = missing_record_reason(checker, span);
            if let Some(reason) = refusal {
                checker.record_body_check_outcome(span, "fallback");
                return Ok(CheckedOwnerRecordOutput::Fallback(CheckedOwnerRecordFallback {
                    name: name.to_owned(),
                    identity: Some(identity),
                    reason,
                    artifact_refusals: checker
                        .body_artifact_refusals(span)
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                }));
            }
            let Some(declaration_record) = checker.available_declaration_record(span).cloned() else {
                return Err("declaration record disappeared after eligibility selection".into());
            };
            let Some(local_record) = checker.available_local_record(span).cloned() else {
                return Err("local record disappeared after eligibility selection".into());
            };
            let Some(final_record) = checker.available_final_record(span).cloned() else {
                return Err("final record disappeared after eligibility selection".into());
            };
            if checker.restored_body_record(span) {
                checker.record_body_check_outcome(span, "restored");
            } else {
                checker.record_body_check_outcome(span, "fresh");
            }
            Ok(CheckedOwnerRecordOutput::Value(CheckedOwnerRecordFields {
                name: name.to_owned(),
                span,
                identity,
                declaration: declaration_record,
                local: local_record,
                final_value: final_record,
                dependency_reads,
                method_lookups: checker.checked_method_lookups(span),
            }))
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
    /// Attribute enumerations to the selected body view, after speculative method choices have been discarded.
    fn checked_method_lookups(&self, owner: crate::ast::Span) -> Option<Vec<CheckedMethodLookup>> {
        let body = self.checked_body(owner)?;
        let mut result = BTreeSet::new();
        for call in body.calls().resolved_method_calls.values() {
            let lookup = call.lookup.as_ref()?;
            let incan_semantics_core::SymbolOrigin::Module(module) = &lookup.receiver.origin else {
                return None;
            };
            let mut members = Vec::with_capacity(lookup.candidates.len());
            for candidate in &lookup.candidates {
                let stable = self.type_info().declarations.source_identities.get(candidate)?;
                let incan_semantics_core::stable_identity::StableDeclarationLocation::Nested { owner, .. } =
                    &stable.location
                else {
                    return None;
                };
                // This graph format witnesses one declared receiver. Inherited implementations require the
                // source scopes of all their declaring receivers before they can use the same preparation.
                if owner.origin != lookup.receiver.origin
                    || owner.declaration_name != lookup.receiver.declaration_name
                    || owner.kind != lookup.receiver.kind
                    || stable.signature.is_none()
                {
                    return None;
                }
                members.push(stable.render_compact());
            }
            members.sort();
            members.dedup();
            if members.len() != lookup.candidates.len() {
                return None;
            }
            result.insert(CheckedMethodLookup {
                module: module.clone(),
                receiver: lookup.receiver.declaration_name.clone(),
                method: call.method.clone(),
                candidates: members,
            });
        }
        Some(result.into_iter().collect())
    }

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
mod tests;
