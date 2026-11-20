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
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use super::{
        CheckedOwnerDependencyRead, CheckedOwnerRecordOutput, checked_owner_record_outputs, stable_dependency_contexts,
    };
    use crate::library_exports::collect_checked_public_exports;
    use crate::library_manifest::LibraryManifest;
    use crate::loaf_index::LoafIndex;
    use crate::typechecker::TypeChecker;

    #[test]
    fn scalar_scope_requires_checked_types_and_a_closed_return() -> Result<(), Box<dyn std::error::Error>> {
        let parsed = program(
            "def leaf() -> str:\n    \"\"\"Keeps documentation out of the lookup scope.\"\"\"\n    return \"hello\"\n\ndef copy(value: int) -> int:\n    return value\n\ndef caller() -> str:\n    return leaf()\n\ndef printed() -> None:\n    println(leaf())\n\ndef add(value: int) -> int:\n    return value + 1\n",
        )?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["main".into()]));
        assert!(super::closed_scalar_scopes(&parsed, &checker).is_empty());
        checker.check_program(&parsed).map_err(|errors| format!("{errors:?}"))?;
        let scopes = super::closed_scalar_scopes(&parsed, &checker);
        let slots = parsed
            .declarations
            .iter()
            .map(|item| {
                scopes
                    .get(&item.span)
                    .map(|names| names.iter().map(String::as_str).collect::<Vec<_>>())
            })
            .collect::<Vec<_>>();
        assert_eq!(
            slots,
            vec![
                Some(vec!["leaf", "str"]),
                Some(vec!["copy", "i64", "int"]),
                Some(vec!["caller", "leaf", "str"]),
                Some(vec!["None", "leaf", "printed", "println"]),
                None,
            ]
        );
        Ok(())
    }

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

    /// Public-package fields with the same spelling retain the selected model in each dependency identity.
    #[test]
    fn public_package_member_reads_keep_their_selected_nominal() -> Result<(), Box<dyn std::error::Error>> {
        let provider = "pub model First:\n    pub pin: Option[int] = None\n\n    pub def message(self) -> str:\n        return \"first\"\n\npub model Second:\n    pub pin: Option[int] = None\n\n    pub def message(self) -> str:\n        return \"second\"\n";
        let (loaves, _directory) = provider_index(provider)?;
        let source = "from pub::provider import First, Second\n\ndef first(value: First) -> Option[int]:\n    return value.pin\n\ndef second(value: Second) -> Option[int]:\n    return value.pin\n\ndef first_message(value: First) -> str:\n    return value.message()\n\ndef second_message(value: Second) -> str:\n    return value.message()\n";
        let consumer = program(source)?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["main".into()]));
        checker.set_loaves(loaves);
        checker
            .check_program(&consumer)
            .map_err(|errors| format!("{errors:?}"))?;
        let facts = checker.type_info().semantic_fact_store(&["main".into()]);
        let contexts = stable_dependency_contexts(&facts);
        let mut reads = contexts
            .iter()
            .filter(|(identity, _)| identity.declaration_name == "pin" || identity.declaration_name == "message")
            .filter_map(|(identity, context)| {
                checker
                    .stable_dependency_identity_with_context(identity, Some(context))
                    .map(|identity| identity.render_compact())
            })
            .collect::<Vec<_>>();
        reads.sort();
        assert_eq!(
            reads,
            vec![
                "member/field:pub::provider::lib::pin#in(model:pub::provider::lib::First)[0]",
                "member/field:pub::provider::lib::pin#in(model:pub::provider::lib::Second)[0]",
                "member/method:pub::provider::lib::message#in(model:pub::provider::lib::First)[0]",
                "member/method:pub::provider::lib::message#in(model:pub::provider::lib::Second)[0]",
            ]
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
        let facts = checker.type_info().semantic_fact_store(&["main".into()]);
        let contexts = stable_dependency_contexts(&facts);
        let mut field_reads = contexts
            .iter()
            .filter(|(identity, _)| identity.declaration_name == "transaction")
            .filter_map(|(identity, context)| {
                checker
                    .stable_dependency_identity_with_context(identity, Some(context))
                    .map(|identity| identity.render_compact())
            })
            .collect::<Vec<_>>();
        field_reads.sort();
        assert_eq!(
            field_reads,
            vec![
                "member/field:engine_messages::transaction#in(model:engine_messages::First)[0]",
                "member/field:engine_messages::transaction#in(model:engine_messages::Second)[0]",
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

    /// A checker receipt owns only references in its body; a callee's dependencies belong to that callee's receipt.
    #[test]
    fn checked_owner_records_publish_only_direct_dependencies() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def leaf() -> int:\n    return 1\n\ndef middle() -> int:\n    return leaf()\n\ndef main() -> None:\n    println(middle())\n";
        let parsed = program(source)?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["main".into()]));
        checker.check_program(&parsed).map_err(|errors| format!("{errors:?}"))?;
        let outputs = checked_owner_record_outputs(&parsed, &checker, &["main".into()])?;
        let mut reads = BTreeMap::new();
        for output in outputs {
            let CheckedOwnerRecordOutput::Value(value) = output else {
                return Err("direct dependency fixture fell back".into());
            };
            reads.insert(
                value.name,
                value
                    .dependency_reads
                    .into_iter()
                    .map(|read| read.declaration_name)
                    .collect::<Vec<_>>(),
            );
        }
        assert_eq!(reads.get("leaf"), Some(&Vec::new()));
        assert_eq!(reads.get("middle"), Some(&vec!["leaf".into()]));
        assert_eq!(reads.get("main"), Some(&vec!["middle".into()]));
        Ok(())
    }

    /// Adding a third specialization changes the exact receiver membership while preserving the two prior members.
    #[test]
    fn checked_overload_reader_names_the_candidates_it_consulted() -> Result<(), Box<dyn std::error::Error>> {
        let first = "trait Describe[T]:\n    def describe(self, value: T) -> str\n\nmodel Describable with Describe[str], Describe[int]:\n    def describe(self, value: str) -> str:\n        return \"item\"\n    def describe(self, value: int) -> str:\n        return \"integer\"\n";
        let third = "    def describe(self, value: FrozenStr) -> str:\n        return \"frozen\"\n";
        let reader =
            "\ndef read_description(item: Describable, message: str) -> str:\n    return item.describe(message)\n";
        let mut prior = Vec::new();
        for (source, count) in [
            (format!("{first}{reader}"), 2),
            (
                format!(
                    "{}{third}{reader}",
                    first.replace("Describe[int]:", "Describe[int], Describe[FrozenStr]:")
                ),
                3,
            ),
        ] {
            let parsed = program(&source)?;
            let mut checker = TypeChecker::new();
            checker.set_current_module_path(Some(vec!["main".to_owned()]));
            checker.check_program(&parsed).map_err(|errors| format!("{errors:?}"))?;
            let span = parsed.declarations.last().ok_or("missing reader")?.span;
            let reads = checker
                .checked_method_lookups(span)
                .ok_or("incomplete method-membership evidence")?;
            assert_eq!(reads.len(), 1, "missing read for {count} declarations");
            let read = reads.first().ok_or("missing overload set")?;
            assert_eq!(read.receiver, "Describable");
            assert_eq!(read.method, "describe");
            assert_eq!(read.module, ["main"]);
            assert_eq!(read.candidates.len(), count);
            assert!(prior.iter().all(|member| read.candidates.contains(member)));
            prior = read.candidates.clone();
        }
        Ok(())
    }

    /// Frame raw producer headers the same way as the Incan codec's native-fields projection.
    fn selected_graph_record(
        mut owner: super::CheckedOwnerRecordFields,
    ) -> Result<super::GraphBodyRecord, Box<dyn std::error::Error>> {
        for (fields, family) in [(&mut owner.local, "local"), (&mut owner.final_value, "final")] {
            fields
                .first_mut()
                .ok_or("missing checked record header")?
                .insert(3, family.into());
        }
        Ok(super::GraphBodyRecord {
            identity: owner.identity,
            local: owner.local.into(),
            final_value: owner.final_value.into(),
            body_ir: None,
        })
    }

    /// A ranked check enters the demanded statements once and declines an absent peer before its statement producer.
    #[test]
    fn ranked_reader_counts_actual_statement_entries() -> Result<(), Box<dyn std::error::Error>> {
        let source = program(concat!(
            "model Peer:\n",
            "    value: int\n",
            "    def read(self) -> int:\n        return self.value\n",
            "    property doubled -> int:\n        return self.value * 2\n\n",
            "def left(value: int) -> int:\n    return value + 1\n\n",
            "def right(value: int) -> int:\n    return value * 2\n",
        ))?;
        let module = vec!["main".to_owned()];
        let mut cold = TypeChecker::new();
        cold.set_current_module_path(Some(module.clone()));
        cold.check_program(&source).map_err(|errors| format!("{errors:?}"))?;
        assert_eq!(cold.check_work().method_bodies, 1);
        assert_eq!(cold.check_work().property_bodies, 1);
        let mut prior = Vec::new();
        let mut target = None;
        for output in checked_owner_record_outputs(&source, &cold, &module)? {
            let CheckedOwnerRecordOutput::Value(owner) = output else {
                return Err("ranked fixture fell back".into());
            };
            if owner.name == "left" {
                target = Some(owner.identity.clone());
            }
            prior.push(selected_graph_record(owner)?);
        }
        let target = target.ok_or("missing ranked fixture owner")?;
        for (records, missing) in [(prior, false), (Vec::new(), true)] {
            let scope = super::GraphBodyRecords::new(records)?.for_ranked_owner(&target);
            let mut checker = TypeChecker::new();
            checker.set_current_module_path(Some(module.clone()));
            checker.set_graph_body_records(scope.clone());
            let result = checker.check_program(&source);
            assert_eq!(result.is_err(), missing, "{result:?}");
            assert_eq!(checker.check_work().body_local_checks, 1);
            assert_eq!(checker.check_work().method_bodies, 0);
            assert_eq!(checker.check_work().property_bodies, 0);
            assert_eq!(scope.ranked_admission(), Some((1, missing, false)));
        }
        Ok(())
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
            let CheckedOwnerRecordOutput::Value(owner) = owner else {
                let CheckedOwnerRecordOutput::Fallback(fallback) = owner else {
                    return Err("missing checker output".into());
                };
                return Err(format!("ordinary owner {} fell back: {}", fallback.name, fallback.reason).into());
            };
            records.push(selected_graph_record(owner)?);
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

    /// Same-name closure parameters bind to distinct structural ordinals when restoring a selected body.
    #[test]
    fn restores_each_closure_parameter_in_its_current_source_position() -> Result<(), Box<dyn std::error::Error>> {
        reusable_ordinary_calls(concat!(
            "def normalize(value: Result[int, str]) -> Result[int, str]:\n",
            "    number = value.map_err((error) => error)?\n",
            "    return value.map_err((error) => error)\n",
        ))
    }

    /// An aliased Rust item is bound to its registered source import. A different module's item with the
    /// same leaf and local spelling must cause a new check before its reference is published.
    #[test]
    fn registered_rust_item_reads_require_the_current_import() -> Result<(), Box<dyn std::error::Error>> {
        let source =
            "from rust::std::fmt import Error as NativeError\n\ndef read_item() -> None:\n    _ = NativeError\n";
        let module = vec!["main".to_owned()];
        let check = |program: &crate::ast::Program, records| -> Result<TypeChecker, String> {
            let mut checker = TypeChecker::new();
            checker.set_current_module_path(Some(module.clone()));
            checker.set_graph_body_records(super::GraphBodyRecords::new(records).map_err(|error| error.to_string())?);
            checker.check_program(program).map_err(|errors| format!("{errors:?}"))?;
            Ok(checker)
        };
        let original = program(source)?;
        let prior = check(&original, Vec::new())?;
        let outputs = checked_owner_record_outputs(&original, &prior, &module)?;
        let [CheckedOwnerRecordOutput::Value(fields)] = outputs.as_slice() else {
            return Err("missing checked record for a registered Rust item read".into());
        };
        let record = selected_graph_record(fields.clone())?;
        let reused = check(&original, vec![record.clone()])?;
        assert_eq!(reused.check_work().function_bodies, 0);
        assert_eq!(
            reused.type_info().references.resolved_identities,
            prior.type_info().references.resolved_identities
        );
        let changed = program(&source.replace("std::fmt", "std::io"))?;
        let checked = check(&changed, vec![record])?;
        let fresh = check(&changed, Vec::new())?;
        assert_eq!(checked.check_work().function_bodies, 1);
        assert_eq!(
            checked.type_info().references.resolved_identities,
            fresh.type_info().references.resolved_identities
        );
        assert_ne!(
            checked.type_info().references.resolved_identities,
            prior.type_info().references.resolved_identities
        );
        Ok(())
    }

    /// The interpolation's outer range includes its braces while the callee's range starts at its identifier.
    #[test]
    fn preserves_nested_call_provenance_inside_an_interpolation() -> Result<(), Box<dyn std::error::Error>> {
        reusable_ordinary_calls(concat!(
            "def inner(value: int) -> int:\n    return value + 1\n\n",
            "def outer(value: int) -> int:\n    return value + 2\n\n",
            "def rendered(value: int) -> str:\n    return f\"result: {outer(inner(value))}\"\n",
        ))
    }

    /// A generic function, its neighbor and the caller that instantiates it at two types all publish reusable values,
    /// so a shifted program re-checks none of their bodies.
    #[test]
    fn generic_owner_and_its_caller_publish_reusable_values() -> Result<(), Box<dyn std::error::Error>> {
        reusable_ordinary_calls(concat!(
            "def pick[T](items: list[T]) -> T:\n    return items[0]\n\n",
            "def unrelated(value: int) -> int:\n    return value + 1\n\n",
            "def main() -> None:\n    println(pick([1, 2, 3]))\n    println(pick([\"a\", \"b\", \"c\"]))\n",
            "    println(unrelated(41))\n",
        ))
    }

    /// A registry-only builtin has no physical root-scope symbol to supply its restored callee identity.
    #[test]
    fn registry_builtin_callee_restores_after_source_moves() -> Result<(), Box<dyn std::error::Error>> {
        reusable_ordinary_calls("def main() -> None:\n    println(json_stringify(1))\n")
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

    /// Both isolated producers derive the qualified owner and match the independently checked module byte for byte.
    #[test]
    fn scoped_producers_recompute_a_qualified_project_owner() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def supported_schema(version: str) -> bool:\n    return version == \"1\"\n";
        let module = vec!["project".to_owned(), "importer".to_owned()];
        let parsed = program(source)?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module.clone()));
        checker.check_program(&parsed).map_err(|errors| format!("{errors:?}"))?;
        let outputs = checked_owner_record_outputs(&parsed, &checker, &module)?;
        let [CheckedOwnerRecordOutput::Value(owner)] = outputs.as_slice() else {
            return Err("expected one reusable project owner".into());
        };
        let mut local = super::produce_local_record(source, "supported_schema", owner.declaration.clone(), vec![])?;
        assert_eq!(local, owner.local);
        local
            .first_mut()
            .ok_or("missing local header")?
            .insert(3, "local".into());
        let settled = super::produce_final_record(source, "supported_schema", local, vec![])?;
        assert_eq!(settled, owner.final_value);
        let before = "def supported_schema(version: int) -> bool:\n    return version >= 1 and version <= 9\n";
        let after = before.replace("version", "schema_version");
        let checked = |source: &str| -> Result<super::CheckedOwnerRecordFields, Box<dyn std::error::Error>> {
            let parsed = program(source)?;
            let mut checker = TypeChecker::new();
            checker.set_current_module_path(Some(module.clone()));
            checker.check_program(&parsed).map_err(|errors| format!("{errors:?}"))?;
            let outputs = checked_owner_record_outputs(&parsed, &checker, &module)?;
            let [CheckedOwnerRecordOutput::Value(output)] = outputs.as_slice() else {
                return Err("missing closed owner".into());
            };
            Ok(output.clone())
        };
        let previous = checked(before)?;
        let changed = checked(&after)?;
        assert_eq!(previous.identity, changed.identity);
        let fresh = super::produce_closed_declaration(&after, "supported_schema", previous.declaration.clone())?
            .ok_or("missing independently checked declaration")?;
        assert_eq!(fresh, changed.declaration);
        assert_eq!(
            super::produce_local_record(&after, "supported_schema", fresh, vec![])?,
            changed.local
        );
        let nonlocal = after.replace("schema_version >= 1 and schema_version <= 9", "check(schema_version)");
        assert!(super::produce_closed_declaration(&nonlocal, "supported_schema", previous.declaration)?.is_none());
        Ok(())
    }

    /// After a provider edit shifts its variant, restoring the consumer must bind to the variant's current span.
    #[test]
    fn restored_enum_member_uses_the_current_provider_anchor() -> Result<(), Box<dyn std::error::Error>> {
        let provider = "pub enum Failure:\n    Missing\n    Type\n";
        let (original, _first) = provider_index(provider)?;
        let source = "from pub::provider import Failure\n\ndef kind() -> Failure:\n    return Failure.Type\n";
        let consumer = program(source)?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["main".into()]));
        checker.set_loaves(original);
        checker
            .check_program(&consumer)
            .map_err(|errors| format!("{errors:?}"))?;
        let outputs = checked_owner_record_outputs(&consumer, &checker, &["main".into()])?;
        let [CheckedOwnerRecordOutput::Value(owner)] = outputs.as_slice() else {
            return Err("expected one reusable consumer of the package variant".into());
        };
        assert!(
            owner
                .final_value
                .iter()
                .any(|row| row.first().is_some_and(|tag| tag == "identity-context")
                    && row.last().is_some_and(|name| name == "Type"))
        );
        let (current, _second) = provider_index(&provider.replace("    Type", "    # a moved declaration\n    Type"))?;
        assert!(
            super::verify_fixture_with_loaves(source, "kind", owner.final_value.clone(), current, vec!["main".into()])?
                .is_empty()
        );
        Ok(())
    }

    /// Imported method calls must resolve to the current executable declaration after a provider edit.
    #[test]
    fn restored_method_uses_its_declaration_inside_the_current_nominal() -> Result<(), Box<dyn std::error::Error>> {
        let provider =
            "pub model Failure:\n    code: int\n\n    pub def message(self) -> int:\n        return self.code\n";
        let source =
            "from pub::provider import Failure\n\ndef result(error: Failure) -> int:\n    return error.message()\n";
        let (original, _original_files) = provider_index(provider)?;
        let consumer = program(source)?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["main".into()]));
        checker.set_loaves(original);
        checker
            .check_program(&consumer)
            .map_err(|errors| format!("{errors:?}"))?;
        let outputs = checked_owner_record_outputs(&consumer, &checker, &["main".into()])?;
        let [CheckedOwnerRecordOutput::Value(owner)] = outputs.as_slice() else {
            return Err("expected one reusable consumer of the package method".into());
        };
        let changed = provider.replace(
            "    pub def message",
            "    # change only the method locator\n    pub def message",
        );
        let (current, _current_files) = provider_index(&changed)?;
        let differences = super::verify_fixture_with_loaves(
            source,
            "result",
            owner.final_value.clone(),
            current,
            vec!["main".into()],
        )?;
        assert!(differences.is_empty(), "{differences:?}");
        Ok(())
    }

    /// Repeated references to an indirectly returned nominal retain each call site and one current provider method.
    #[test]
    fn restored_repeated_method_on_an_imported_return_type_skips_statements() -> Result<(), Box<dyn std::error::Error>>
    {
        let provider = "pub model Failure:\n    pub code: int\n\n    pub def message(self) -> int:\n        return self.code\n\npub def failure() -> Failure:\n    return Failure(code=3)\n";
        let source = "from pub::provider import failure\n\ndef result() -> int:\n    error = failure()\n    return error.message() + error.message()\n";
        let consumer = program(source)?;
        let (original, _original_files) = provider_index(provider)?;
        let mut prior = TypeChecker::new();
        prior.set_current_module_path(Some(vec!["main".into()]));
        prior.set_loaves(original);
        prior.check_program(&consumer).map_err(|errors| format!("{errors:?}"))?;
        let outputs = checked_owner_record_outputs(&consumer, &prior, &["main".into()])?;
        let [CheckedOwnerRecordOutput::Value(owner)] = outputs.as_slice() else {
            return Err("expected the indirect package method caller to publish records".into());
        };
        let (current, _current_files) =
            provider_index(&provider.replace("    pub def message", "    # new coordinates\n    pub def message"))?;
        let mut fresh = TypeChecker::new();
        fresh.set_current_module_path(Some(vec!["main".into()]));
        fresh.set_loaves(current.clone());
        fresh.check_program(&consumer).map_err(|errors| format!("{errors:?}"))?;
        let mut warm = TypeChecker::new();
        warm.set_current_module_path(Some(vec!["main".into()]));
        warm.set_loaves(current);
        warm.set_graph_body_records(super::GraphBodyRecords::new(vec![selected_graph_record(
            owner.clone(),
        )?])?);
        warm.check_program(&consumer).map_err(|errors| format!("{errors:?}"))?;
        assert_eq!(warm.check_work().function_bodies, 0);
        assert_eq!(
            warm.type_info().references.resolved_identities,
            fresh.type_info().references.resolved_identities
        );
        Ok(())
    }

    fn lower_graph_test_module(
        parsed: &crate::ast::Program,
        checker: &TypeChecker,
    ) -> incan_semantics_core::body_ir::BodyIrModule {
        let providers = crate::body_ir::ProviderOperationCatalog::new();
        crate::body_ir::build_body_ir_module(
            parsed,
            &["main".into()],
            checker.type_info(),
            &crate::body_ir::BodyIrContext {
                modules: crate::body_ir::ContextModules::default(),
                provider_operations: &providers,
                registry_package: None,
                library_package: None,
                include_inline_tests: false,
            },
        )
    }

    /// Method records come from the ordinary checked nominal and keep owners with the same member name distinct.
    #[test]
    fn ordinary_nominal_methods_publish_their_own_checked_records() -> Result<(), Box<dyn std::error::Error>> {
        let source = concat!(
            "model Left:\n    value: int\n",
            "    def read(self) -> int:\n        return self.value\n\n",
            "model Right:\n    value: int\n",
            "    def read(self) -> int:\n        return self.value + 1\n"
        );
        let parsed = program(source)?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["main".into()]));
        checker.check_program(&parsed).map_err(|errors| format!("{errors:?}"))?;
        assert_eq!(checker.check_work().method_bodies, 2);
        let lowered = lower_graph_test_module(&parsed, &checker);
        let outputs = super::checked_graph_record_outputs(&parsed, &checker, &["main".into()])?;
        assert_eq!(outputs.len(), 2);
        let mut selected = Vec::new();
        for (declaration, output) in parsed.declarations.iter().zip(outputs) {
            let crate::ast::Declaration::Model(model) = &declaration.node else {
                return Err("missing model".into());
            };
            let stable = format!("member/method:main::read#in(model:main::{})[0]", model.name);
            let CheckedOwnerRecordOutput::Value(owner) = output else {
                return Err(format!("{}: {output:?}", model.name).into());
            };
            assert_eq!(owner.identity, stable);
            for fields in [&owner.declaration, &owner.local, &owner.final_value] {
                assert_eq!(fields[0][1], stable);
            }
            assert!(
                owner
                    .dependency_reads
                    .iter()
                    .any(|read| read.declaration_name == "value"),
                "{}: {:?}",
                model.name,
                owner.dependency_reads
            );
            let body = lowered
                .bodies
                .iter()
                .find(|body| body.span == owner.span)
                .ok_or("missing nominal frame")?;
            let witness = checker
                .graph_body_physical_inputs(&parsed, owner.span)
                .ok_or("missing nominal witness")?;
            let wire = incan_semantics_core::executable_representation::encode_graph_owner(
                &lowered.module_id,
                "src/main.incn",
                &owner.identity,
                body,
                Some(witness),
            )?;
            let mut record = selected_graph_record(owner)?;
            record.body_ir = Some(incan_semantics_core::executable_representation::decode_owner(&wire)?);
            selected.push(record);
        }
        for (changed, expected_checks) in [
            (source.to_owned(), 0),
            (source.replace("self.value + 1", "self.value + 2"), 1),
        ] {
            let current = program(&changed)?;
            let mut warm = TypeChecker::new();
            warm.set_current_module_path(Some(vec!["main".into()]));
            warm.set_graph_body_records(super::GraphBodyRecords::new(selected.clone())?);
            warm.check_program(&current).map_err(|errors| format!("{errors:?}"))?;
            assert_eq!(warm.check_work().method_bodies, expected_checks);
            let mut fresh = TypeChecker::new();
            fresh.set_current_module_path(Some(vec!["main".into()]));
            fresh.check_program(&current).map_err(|errors| format!("{errors:?}"))?;
            let restored_frames = current
                .declarations
                .iter()
                .flat_map(|declaration| crate::typechecker::canonical_nodes::nominal_methods(&declaration.node))
                .filter(|method| warm.type_info().selected_callable_body(method.span).is_some())
                .count();
            assert_eq!(restored_frames, 2 - expected_checks);
            assert_eq!(
                lower_graph_test_module(&current, &warm).bodies,
                lower_graph_test_module(&current, &fresh).bodies
            );
            assert_eq!(
                warm.type_info().references.resolved_identities,
                fresh.type_info().references.resolved_identities
            );
        }
        Ok(())
    }

    #[test]
    fn per_owner_call_reads_survive_without_a_namespace_and_refuse_another_reader()
    -> Result<(), Box<dyn std::error::Error>> {
        let module = vec!["main".to_owned()];
        let parsed = program("def callee() -> int:\n    return 1\n\ndef caller() -> int:\n    return callee()\n")?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module.clone()));
        checker.check_program(&parsed).map_err(|errors| format!("{errors:?}"))?;
        let mut lowered = lower_graph_test_module(&parsed, &checker);
        let access = crate::body_ir::SettledCallInputs::from_modules(std::slice::from_ref(&lowered))?;
        let reads = access.reads(&mut lowered);
        let [read] = reads.as_slice() else {
            return Err("expected one project call".into());
        };
        assert!(access.matches(read));
        let mut forged = read.clone();
        forged.digest = format!("sha256:{}", "0".repeat(64));
        assert!(!access.matches(&forged));
        let owner = checked_owner_record_outputs(&parsed, &checker, &module)?
            .into_iter()
            .find_map(|output| match output {
                CheckedOwnerRecordOutput::Value(owner) if owner.name == "caller" => Some(owner),
                _ => None,
            })
            .ok_or("missing caller")?;
        let body = lowered
            .bodies
            .iter()
            .find(|body| body.name == "caller")
            .ok_or("missing kernel caller")?;
        let source = "src/main.incn".to_owned();
        let wire = incan_semantics_core::executable_representation::encode_graph_owner(
            &lowered.module_id,
            &source,
            &owner.identity,
            body,
            Some([1; 32]),
        )?;
        let mut record = selected_graph_record(owner.clone())?;
        let select = |record: super::GraphBodyRecord| super::GraphBodyRecords::new(vec![record]);
        record.body_ir = Some(incan_semantics_core::executable_representation::decode_owner(&wire)?);
        assert!(
            select(record.clone())?
                .get(&owner.identity)?
                .ok_or("missing old record")?
                .body_ir
                .is_none()
        );
        let encoded_reads = vec![internal_models::SettledCallRead {
            reader: read.reader.clone(),
            target: read.target.clone(),
            digest: read.digest.clone(),
        }];
        for (identity, allowed) in [(&owner.identity, true), (&"other".to_owned(), false)] {
            let witness = internal_models::encode_owner_call_reads(
                identity.clone(),
                read.reader.clone(),
                source.clone(),
                &encoded_reads,
            )?;
            let mut guarded = wire.clone();
            guarded.extend_from_slice(&witness);
            record.body_ir = Some(incan_semantics_core::executable_representation::decode_owner(&guarded)?);
            let mut selected = select(record.clone())?;
            if allowed {
                let mut retry = super::GraphBodyRecords::default();
                retry.retain_current(vec![record.clone()]);
                assert!(retry.checked_here(&owner.identity));
                assert!(
                    retry
                        .get(&owner.identity)?
                        .ok_or("missing current provider")?
                        .body_ir
                        .is_some()
                );
                assert_eq!(retry.selected_call_reads(&source)?, reads);
                assert!(
                    selected
                        .get(&owner.identity)?
                        .ok_or("missing current frame")?
                        .body_ir
                        .is_some()
                );
                assert_eq!(selected.selected_call_reads(&source)?, reads);
                selected.reopen_callers(
                    [(source.clone(), [read.reader.clone()].into_iter().collect())]
                        .into_iter()
                        .collect(),
                )?;
                assert!(
                    selected
                        .get(&owner.identity)?
                        .ok_or("missing checked pair")?
                        .body_ir
                        .is_none()
                );
                assert!(selected.selected_call_reads(&source)?.is_empty());
            } else {
                assert!(selected.get(&owner.identity).is_err());
            }
        }
        Ok(())
    }

    /// A graph peer restores all body-internal scopes in source order, including empty branches, before a later owner.
    #[test]
    fn restored_nested_scopes_match_fresh_bindings_in_later_owners() -> Result<(), Box<dyn std::error::Error>> {
        let source = program(concat!(
            "def count(items: list[int]) -> int:\n",
            "    mut total = 0\n",
            "    for item in items:\n",
            "        if item > 0:\n",
            "            kept = item\n",
            "            total += kept\n",
            "        else:\n",
            "            pass\n",
            "    return total\n\n",
            "def next_value(number: int) -> int:\n    return number + 1\n",
        ))?;
        let module = vec!["main".to_owned()];
        let mut cold = TypeChecker::new();
        cold.set_current_module_path(Some(module.clone()));
        cold.check_program(&source).map_err(|errors| format!("{errors:?}"))?;
        let checked = checked_owner_record_outputs(&source, &cold, &module)?;
        let mut warm = TypeChecker::new();
        warm.set_current_module_path(Some(module));
        let Some(CheckedOwnerRecordOutput::Value(owner)) = checked
            .iter()
            .find(|v| matches!(v,CheckedOwnerRecordOutput::Value(o) if o.name=="count"))
        else {
            return Err(format!("scope owner produced no checked rows: {checked:?}").into());
        };
        let selected = selected_graph_record(owner.clone())?;
        let allocation = selected
            .local
            .iter()
            .find(|row| row[0] == "scope")
            .ok_or("no lexical scope tree")?;
        assert!(allocation[3].parse::<usize>()? >= 3);
        warm.set_graph_body_records(super::GraphBodyRecords::new(vec![selected.clone()])?);
        warm.check_program(&source).map_err(|errors| format!("{errors:?}"))?;
        assert_eq!(warm.check_work().function_bodies, 1);
        assert_eq!(warm.symbols.next_scope_index(), cold.symbols.next_scope_index());
        assert_eq!(
            warm.type_info().references.resolved_identities,
            cold.type_info().references.resolved_identities
        );
        assert_eq!(
            warm.type_info().references.resolved_write_identities,
            cold.type_info().references.resolved_write_identities
        );

        let mut bad = selected;
        let layout = std::sync::Arc::make_mut(&mut bad.local)
            .iter_mut()
            .find(|row| row[0] == "scope")
            .ok_or("missing scope tree")?;
        layout[4] = "99".into();
        let mut refused = TypeChecker::new();
        refused.set_current_module_path(Some(vec!["main".into()]));
        refused.set_graph_body_records(super::GraphBodyRecords::new(vec![bad])?);
        refused.check_program(&source).map_err(|errors| format!("{errors:?}"))?;
        assert_eq!(refused.check_work().function_bodies, 2);
        assert_eq!(refused.symbols.next_scope_index(), cold.symbols.next_scope_index());
        Ok(())
    }

    /// A retained native body needs its current positions as well as the reusable position-independent check result.
    #[test]
    fn selected_graph_body_requires_current_physical_inputs() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def twice(value: int) -> int:\n    return  value + value\n";
        let module = vec!["main".to_string()];
        let original = program(source)?;
        let check = |parsed: &crate::ast::Program,
                     records: Vec<super::GraphBodyRecord>|
         -> Result<TypeChecker, String> {
            let mut checker = TypeChecker::new();
            checker.set_current_module_path(Some(module.clone()));
            checker.set_graph_body_records(super::GraphBodyRecords::new(records).map_err(|error| error.to_string())?);
            checker.check_program(parsed).map_err(|errors| format!("{errors:?}"))?;
            Ok(checker)
        };
        let cold = check(&original, Vec::new())?;
        let checked = checked_owner_record_outputs(&original, &cold, &module)?;
        let [CheckedOwnerRecordOutput::Value(owner)] = checked.as_slice() else {
            return Err("missing checked native owner".into());
        };
        let lowered = lower_graph_test_module(&original, &cold);
        let body = lowered.bodies.first().ok_or("missing lowered body")?;
        let witness = cold
            .graph_body_physical_inputs(&original, owner.span)
            .ok_or("missing positioned producer witness")?;
        for (input, guarded, expected) in [
            (source.to_string(), true, true),
            (source.to_string(), false, false),
            (
                source.replace("return  value + value", "return value  + value"),
                true,
                false,
            ),
            (format!("# moved\n{source}"), true, false),
        ] {
            let encoded = incan_semantics_core::executable_representation::encode_graph_owner(
                &lowered.module_id,
                "src/main.incn",
                &owner.identity,
                body,
                guarded.then_some(witness),
            )?;
            let frame = incan_semantics_core::executable_representation::decode_owner(&encoded)?;
            let mut record = selected_graph_record(owner.clone())?;
            record.body_ir = Some(frame);
            let parsed = program(&input)?;
            let warm = check(&parsed, vec![record])?;
            let fresh = check(&parsed, vec![])?;
            let span = parsed.declarations[0].span;
            assert_eq!(warm.check_work().function_bodies, 0);
            assert_eq!(
                warm.type_info().selected_callable_body(span).is_some(),
                expected,
                "{input:?}, guarded={guarded}"
            );
            assert_eq!(
                lower_graph_test_module(&parsed, &warm).bodies,
                lower_graph_test_module(&parsed, &fresh).bodies
            );
        }
        Ok(())
    }

    /// Rebinding source positions must reproduce the exact fresh native body for both the edited owner and its caller.
    #[test]
    fn selected_scalar_frames_rebind_comments_and_documentation() -> Result<(), Box<dyn std::error::Error>> {
        use incan_semantics_core::executable_representation::{
            append_owner_coordinates, decode_owner, encode_graph_owner,
        };
        let src = "def greeting() -> str:\n    return \"hello\"\n\n\ndef main() -> None:\n    println(greeting())\n";
        let module = vec!["main".to_owned()];
        let check = |parsed: &crate::ast::Program, records| -> Result<TypeChecker, String> {
            let mut checker = TypeChecker::new();
            checker.set_current_module_path(Some(module.clone()));
            checker.set_graph_body_records(super::GraphBodyRecords::new(records).map_err(|error| error.to_string())?);
            checker.check_program(parsed).map_err(|errors| format!("{errors:?}"))?;
            Ok(checker)
        };
        let parsed = program(src)?;
        let first = check(&parsed, Vec::new())?;
        let mut native = lower_graph_test_module(&parsed, &first);
        let access = crate::body_ir::SettledCallInputs::from_modules(std::slice::from_ref(&native))?;
        let reads = access.reads(&mut native);
        let mut records = Vec::new();
        for output in checked_owner_record_outputs(&parsed, &first, &module)? {
            let CheckedOwnerRecordOutput::Value(owner) = output else {
                return Err("required scalar owner missing".into());
            };
            let body = native
                .bodies
                .iter()
                .find(|body| body.name == owner.name)
                .ok_or("missing native body")?;
            let (proof, coordinates) = first
                .graph_body_inputs(&parsed, owner.span, true)
                .ok_or("missing checked placement")?;
            assert!(!coordinates.is_empty());
            let mut bytes = encode_graph_owner(&native.module_id, "main.incn", &owner.identity, body, Some(proof))?;
            append_owner_coordinates(&mut bytes, &coordinates);
            let reader = body.direct_call_id.path();
            let calls = reads
                .iter()
                .filter(|read| read.reader == reader)
                .map(|read| internal_models::SettledCallRead {
                    reader: read.reader.clone(),
                    target: read.target.clone(),
                    digest: read.digest.clone(),
                })
                .collect::<Vec<_>>();
            bytes.extend_from_slice(&internal_models::encode_owner_call_reads(
                owner.identity.clone(),
                reader.to_owned(),
                "main.incn".to_owned(),
                &calls,
            )?);
            let frame = decode_owner(&bytes)?;
            assert_eq!(frame.coordinates, coordinates);
            let mut record = selected_graph_record(owner)?;
            record.body_ir = Some(frame);
            records.push(record);
        }
        assert_eq!(records.len(), 2);
        for changed in [
            src.replace("def main()", "# a comment before main\ndef main()"),
            src.replace("    return", "    \"\"\"New documentation.\"\"\"\n    return"),
        ] {
            let after = program(&changed)?;
            let warm = check(&after, records.clone())?;
            let fresh = check(&after, Vec::new())?;
            for decl in &after.declarations {
                assert!(
                    warm.type_info().selected_callable_body(decl.span).is_some(),
                    "decl {:?} in {changed}",
                    decl.span
                );
                assert_eq!(
                    warm.completed_graph_body_digest(decl.span),
                    warm.graph_body_physical_inputs(&after, decl.span)
                );
            }
            assert_eq!(
                lower_graph_test_module(&after, &warm).bodies,
                lower_graph_test_module(&after, &fresh).bodies
            );
            let mut corrupted = records.clone();
            for record in &mut corrupted {
                let frame = record.body_ir.as_mut().ok_or("no selected body")?;
                frame.physical_inputs = Some([0x42; 32]);
            }
            let refused = check(&after, corrupted)?;
            assert!(
                after
                    .declarations
                    .iter()
                    .all(|decl| refused.type_info().selected_callable_body(decl.span).is_none())
            );
        }
        Ok(())
    }

    /// Scalar products and graph pages can select the same untouched callable during an ordinary edit.
    /// Only its current checker bindings can authenticate the graph frame that will be published again.
    #[test]
    fn graph_body_republication_with_a_competing_scalar_product() -> Result<(), Box<dyn std::error::Error>> {
        use crate::declaration_interface::{CallableBodyInputs, ValidatedCallableBodies};
        use incan_semantics_core::executable_representation::{decode_owner, encode_graph_owner};

        let source =
            "def greeting() -> int:\n    return 1\n\ndef twice(value: int) -> int:\n    return  value + value\n";
        let module = vec!["main".to_owned()];
        let check = |parsed: &crate::ast::Program, records, scalar: Option<ValidatedCallableBodies>| {
            let mut checker = TypeChecker::new();
            checker.set_current_module_path(Some(module.clone()));
            checker.set_graph_body_records(super::GraphBodyRecords::new(records).map_err(|error| error.to_string())?);
            if let Some(scalar) = scalar {
                checker.set_callable_body_reuse(scalar);
            }
            checker.check_program(parsed).map_err(|errors| format!("{errors:?}"))?;
            Ok::<_, String>(checker)
        };
        let original = program(source)?;
        let baseline = check(&original, vec![], None)?;
        let lowered = lower_graph_test_module(&original, &baseline);
        let products =
            CallableBodyInputs::from_checked_for_unit(&original, source, &baseline, "fixture")?.finish(&lowered)?;
        let owner = checked_owner_record_outputs(&original, &baseline, &module)?
            .into_iter()
            .find_map(|output| match output {
                CheckedOwnerRecordOutput::Value(owner) if owner.name == "twice" => Some(owner),
                _ => None,
            })
            .ok_or("missing checked peer")?;
        let publish =
            |parsed: &crate::ast::Program, checker: &TypeChecker| -> Result<Vec<u8>, Box<dyn std::error::Error>> {
                let lowered = lower_graph_test_module(parsed, checker);
                let body = lowered
                    .bodies
                    .iter()
                    .find(|body| body.name == "twice")
                    .ok_or("missing lowered peer")?;
                let physical = checker
                    .graph_body_physical_inputs(parsed, body.span)
                    .ok_or("missing current physical inputs")?;
                Ok(encode_graph_owner(
                    &lowered.module_id,
                    "main.incn",
                    &owner.identity,
                    body,
                    Some(physical),
                )?)
            };
        let frame = decode_owner(&publish(&original, &baseline)?)?;
        let valid = frame.physical_inputs.ok_or("missing baseline proof")?;
        let mut corrupt = valid;
        corrupt[0] ^= 1;
        let edited = source.replace("return 1", "return 2");
        let scalar_only_program = program(&edited)?;
        let scalar_only = check(
            &scalar_only_program,
            vec![],
            Some(products.select(&scalar_only_program, &edited, "fixture", &module)?),
        )?;
        let peer = scalar_only_program.declarations[1].span;
        assert!(scalar_only.type_info().selected_callable_body(peer).is_some());
        assert_eq!(scalar_only.check_work().function_bodies, 1);

        for (input, proof, corrupt_rows, expected_scalar, expected_frame) in [
            (edited.clone(), Some(valid), false, 1, true),
            (edited.clone(), None, false, 1, false),
            (edited.clone(), Some(corrupt), false, 1, false),
            (edited.clone(), Some(valid), true, 1, false),
            (
                edited.replace("return  value + value", "return value  + value"),
                Some(valid),
                false,
                0,
                false,
            ),
        ] {
            let parsed = program(&input)?;
            let selection = products.select(&parsed, &input, "fixture", &module)?;
            assert_eq!(selection.len(), expected_scalar);
            let mut record = selected_graph_record(owner.clone())?;
            let mut candidate = frame.clone();
            candidate.physical_inputs = proof;
            record.body_ir = Some(candidate);
            if corrupt_rows {
                Arc::make_mut(&mut record.final_value).retain(|row| !row.first().is_some_and(|field| field == "scope"));
            }
            let warm = check(&parsed, vec![record], Some(selection))?;
            let fresh = check(&parsed, vec![], None)?;
            assert_eq!(warm.check_work().function_bodies, if corrupt_rows { 2 } else { 1 });
            assert_eq!(
                warm.type_info()
                    .selected_callable_body(parsed.declarations[1].span)
                    .is_some(),
                expected_frame
            );
            assert_eq!(publish(&parsed, &warm)?, publish(&parsed, &fresh)?);
        }
        Ok(())
    }

    /// Moving a callee invalidates its current physical call target. An earlier owner with no read of that
    /// declaration keeps its already lowered frame, even when both owners received valid checker records.
    #[test]
    fn physical_owner_frames_follow_selected_callable_handles() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def unrelated() -> int:\n    return 1\n\ndef caller() -> int:\n    return provider()\n\ndef caller_through_value() -> int:\n    callback = provider\n    return callback()\n\ndef provider() -> int:\n    return 20\n";
        let module = vec!["main".to_owned()];
        let check = |program: &crate::ast::Program, records| -> Result<TypeChecker, String> {
            let mut checker = TypeChecker::new();
            checker.set_current_module_path(Some(module.clone()));
            checker.set_graph_body_records(super::GraphBodyRecords::new(records).map_err(|err| err.to_string())?);
            checker.check_program(program).map_err(|errors| format!("{errors:?}"))?;
            Ok(checker)
        };
        let original = program(source)?;
        let cold = check(&original, vec![])?;
        let lowered = lower_graph_test_module(&original, &cold);
        let mut inputs = Vec::new();
        for output in checked_owner_record_outputs(&original, &cold, &module)? {
            let CheckedOwnerRecordOutput::Value(owner) = output else {
                return Err("missing checked callable in handle fixture".into());
            };
            if owner.name == "provider" {
                continue;
            }
            let body = lowered
                .bodies
                .iter()
                .find(|body| body.name == owner.name)
                .ok_or("missing lowered owner")?;
            let witness = cold
                .graph_body_physical_inputs(&original, owner.span)
                .ok_or("missing physical inputs")?;
            let bytes = incan_semantics_core::executable_representation::encode_graph_owner(
                &lowered.module_id,
                "src/main.incn",
                &owner.identity,
                body,
                Some(witness),
            )?;
            let mut input = selected_graph_record(owner)?;
            input.body_ir = Some(incan_semantics_core::executable_representation::decode_owner(&bytes)?);
            inputs.push(input);
        }
        let edited = program(&source.replace("return 20", "return 2000"))?;
        let warm = check(&edited, inputs)?;
        let fresh = check(&edited, vec![])?;
        assert!(
            warm.type_info()
                .selected_callable_body(edited.declarations[0].span)
                .is_some()
        );
        assert!(
            warm.type_info()
                .selected_callable_body(edited.declarations[1].span)
                .is_none()
        );
        assert!(
            warm.type_info()
                .selected_callable_body(edited.declarations[2].span)
                .is_none()
        );
        assert_eq!(
            lower_graph_test_module(&edited, &warm).bodies,
            lower_graph_test_module(&edited, &fresh).bodies
        );
        Ok(())
    }

    /// Frame admission tracks a named type the owner uses and ignores a physically moved ambient type.
    #[test]
    fn physical_owner_frames_follow_selected_nominal_reads() -> Result<(), Box<dyn std::error::Error>> {
        let parsed = program(concat!(
            "model Selected:\n    value: int\n\n",
            "model Unused:\n    value: int\n\n",
            "def take(item: Selected) -> int:\n    return item.value\n",
        ))?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["main".to_owned()]));
        checker.check_program(&parsed).map_err(|errors| format!("{errors:?}"))?;
        let owner = parsed.declarations[2].span;
        let before = checker
            .graph_body_physical_inputs(&parsed, owner)
            .ok_or("missing initial proof")?;
        for (name, same) in [("Unused", true), ("Selected", false)] {
            let binding = checker
                .type_info
                .declarations
                .named_type_identities
                .get_mut(name)
                .ok_or("missing declared type binding")?;
            binding.declaration_span.start += 1;
            let current = checker
                .graph_body_physical_inputs(&parsed, owner)
                .ok_or("missing updated proof")?;
            assert_eq!(current == before, same, "{name}");
        }
        Ok(())
    }

    /// Moving an imported function changes the executable handles for its direct and first-class users.
    /// A separate function in the consumer keeps the same authenticated executable inputs.
    #[test]
    fn physical_owner_frames_follow_foreign_callable_reads() -> Result<(), Box<dyn std::error::Error>> {
        let consumer = program(concat!(
            "from provider import value\n\n",
            "def unrelated() -> int:\n    return 1\n\n",
            "def direct() -> int:\n    return value()\n\n",
            "def indirect() -> int:\n    callback = value\n    return callback()\n",
        ))?;
        let fingerprints = |provider: &crate::ast::Program| -> Result<BTreeMap<String, [u8; 32]>, String> {
            let path = vec!["provider".to_owned()];
            let mut declarations = TypeChecker::new();
            declarations.set_current_module_path(Some(path.clone()));
            declarations.register_dependency_module_path_segments("provider", path.clone());
            let interface = declarations
                .build_dependency_interface_component(
                    &[(
                        "provider",
                        provider,
                        Arc::from(crate::module::exported_symbols(provider)),
                    )],
                    &[],
                    true,
                )
                .pop()
                .ok_or("provider interface missing")?;
            let mut checker = TypeChecker::new();
            checker.set_current_module_path(Some(vec!["main".to_owned()]));
            checker.register_dependency_module_path_segments("provider", path);
            checker.select_dependency_interfaces(vec![interface]);
            checker
                .check_with_imports(&consumer, &[("provider", provider)])
                .map_err(|errors| format!("{errors:?}"))?;
            consumer
                .declarations
                .iter()
                .filter_map(|decl| match &decl.node {
                    crate::ast::Declaration::Function(function) => Some((function.name.clone(), decl.span)),
                    _ => None,
                })
                .map(|(name, span)| {
                    checker
                        .graph_body_physical_inputs(&consumer, span)
                        .map(|fingerprint| (name.clone(), fingerprint))
                        .ok_or_else(|| format!("missing physical proof for {name}"))
                })
                .collect()
        };
        let provider = "pub def value() -> int:\n    return 20\n";
        let original = fingerprints(&program(provider)?)?;
        let moved = fingerprints(&program(&format!("# moved\n{provider}"))?)?;
        assert_eq!(original.len(), 3);
        assert_ne!(original["direct"], moved["direct"]);
        assert_ne!(original["indirect"], moved["indirect"]);
        assert_eq!(original["unrelated"], moved["unrelated"]);
        Ok(())
    }

    /// A loan-free match scrutinee is retained as a reusable owner fact instead of forcing a source fallback.
    #[test]
    fn loan_free_match_owner_publishes_reusable_value() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def main() -> int:\n    match 1:\n        1 => return 2\n        _ => return 3\n";
        let program = program(source)?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["main".into()]));
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;

        let outputs = checked_owner_record_outputs(&program, &checker, &["main".into()])?;
        assert_eq!(outputs.len(), 1, "a checked owner was silently omitted");
        let CheckedOwnerRecordOutput::Value(owner) = &outputs[0] else {
            return Err("the match owner fell back".into());
        };
        assert_eq!(owner.name, "main");
        assert!(
            owner
                .final_value
                .iter()
                .any(|row| row[0] == "artifact-protocol" && row[2] == "loan_free_scrutinees" && row[3] == "true")
        );
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
