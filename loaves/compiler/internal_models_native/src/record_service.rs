//! Preparation and evaluation contracts over revision-bound compiler reads.

use crate::record_scope::{
    PreparationEvidence, ReadScope, RecordKey, RecordReads, RecordRequest, valid_key, valid_request,
};

pub const RECORD_SERVICE_SCHEMA: &str = "incan.compiler-record-service/1";

/// Compiler-owned phases, ordered within the namespace import rank.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProducerStage {
    Discover,
    Signature,
    Layout,
    Default,
    CheckBody,
    BodyIr,
    Instance,
    LoweringDemand,
    PlanFunction,
    NativeInterface,
}

/// An immutable store root's canonical digest, never a locator, table position or process pointer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordRoot {
    pub digest: String,
}
/// A captured input root protected by the caller's snapshot lease for the entire operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputSnapshot {
    pub root: RecordRoot,
}
/// Exact compiler generation, producer schema and semantic options, admitted before preparation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProducerProfile {
    pub identity: RecordRoot,
}
/// A compiler-certified namespace order, phase and phase-local dependency order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProducerRank {
    pub namespace: i64,
    pub stage: ProducerStage,
    pub dependency: i64,
}
/// One bounded page of logical output keys; continuation is handled before evaluation, never by a retry loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemandPage {
    pub keys: Vec<RecordKey>,
}
/// The immutable evidence that every possible supplier is final before this producer's certified rank.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RankCertificate {
    pub rank: ProducerRank,
    pub evidence: RecordRoot,
}
/// Final input envelope for one output, bound to the captured snapshot and exact producer profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedInputs {
    pub output: RecordKey,
    pub snapshot: InputSnapshot,
    pub profile: ProducerProfile,
    pub required_reads: Vec<RecordRequest>,
    pub rank_certificate: RankCertificate,
    pub selector_witnesses: RecordRoot,
}
/// Preparation needs these earlier-rank suppliers; this is scheduling information, never a source diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeedPrerequisite {
    pub keys: Vec<RecordKey>,
    pub earlier_rank: ProducerRank,
}
/// Compiler-owned diagnostics and the exact reads supporting them, including failed speculative reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticRefusal {
    pub diagnostics: RecordRoot,
    pub read_witness_root: RecordRoot,
}
/// Typed contract or infrastructure failures; no variant is a language-level refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompilerFaultKind {
    UnfinishedSupplier(RecordKey),
    SnapshotMismatch,
    ProfileMismatch,
    InvalidPreparation,
    UnsupportedProducer(ProducerStage),
    RecordReadFailed(RecordKey),
    InvalidRecord(RecordKey),
}
/// A failed producer operation; details refer to structured evidence rather than a refusal string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompilerFault {
    pub kind: CompilerFaultKind,
    pub details: RecordRoot,
}
/// Preparation either certifies final inputs, requests earlier work, refuses source or reports a fault.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Preparation {
    Prepared(PreparedInputs),
    NeedPrerequisite(NeedPrerequisite),
    SemanticRefusal(SemanticRefusal),
    CompilerFault(CompilerFault),
}
/// One logical output selected for value evaluation after all preparation has settled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadyWork {
    pub output: RecordKey,
    pub stage: ProducerStage,
    pub profile: ProducerProfile,
}
/// The produced logical record and immutable value root, valid under the caller's staging ownership.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValueReference {
    pub key: RecordKey,
    pub root: RecordRoot,
}
/// Work performed by this call, excluding reuse and native compilation owned by other components.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProducerCounters {
    pub evaluations: i64,
    pub records_read: i64,
    pub bytes_read: i64,
}
/// A staged answer with semantic identity, exact read evidence, diagnostics and measured work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Produced {
    pub value_ref: ValueReference,
    pub semantic_digest: RecordRoot,
    pub read_witness_root: RecordRoot,
    pub diagnostic_root: RecordRoot,
    pub counters: ProducerCounters,
}
/// A final value, semantic refusal or compiler fault; unfinished suppliers cannot masquerade as diagnostics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Evaluation {
    Produced(Produced),
    SemanticRefusal(SemanticRefusal),
    CompilerFault(CompilerFault),
}

/// Prepare and evaluate compiler-owned records without running the whole-program check or lowering path.  The reader
/// binds every response to its full key, selection, immutable snapshot and producer profile. A failed activation cannot
/// publish a partial witness set. Native value adaptation and store admission remain separate boundaries; this contract
/// supplies no whole-module implementation.
pub trait CompilerRecordService {
    /// Discover certified earlier suppliers before producing the requested record.
    fn prepare(
        &mut self,
        stage: &ProducerStage,
        input_snapshot: &InputSnapshot,
        demand_page: &DemandPage,
        producer_profile: &ProducerProfile,
    ) -> Preparation;
    /// Evaluate final inputs and retain all observed read evidence.
    fn evaluate(
        &mut self,
        ready_work: &ReadyWork,
        input_snapshot: &InputSnapshot,
        prepared_inputs: &PreparedInputs,
        scoped_reader: &mut RecordReads,
    ) -> Evaluation;
}

/// Bind an evaluation to its certified output, stage, snapshot, profile and request envelope.
pub fn prepared_reader(
    ready: &ReadyWork,
    snapshot: &InputSnapshot,
    prepared: &PreparedInputs,
) -> Result<RecordReads, CompilerFaultKind> {
    if snapshot.root.digest.is_empty() || snapshot.root.digest != prepared.snapshot.root.digest {
        return Err(CompilerFaultKind::SnapshotMismatch);
    }
    let profile = &ready.profile.identity.digest;
    if profile.is_empty() || profile != &prepared.profile.identity.digest {
        return Err(CompilerFaultKind::ProfileMismatch);
    }
    let certificate = &prepared.rank_certificate;
    let rank = &certificate.rank;
    if !valid_key(&ready.output)
        || ready.output != prepared.output
        || ready.stage != rank.stage
        || rank.namespace < 0
        || rank.dependency < 0
        || certificate.evidence.digest.is_empty()
        || prepared.selector_witnesses.digest.is_empty()
        || !prepared.required_reads.iter().all(valid_request)
    {
        return Err(CompilerFaultKind::InvalidPreparation);
    }
    // The reader owns these bindings and can outlive the borrowed preparation.
    let scope = ReadScope {
        snapshot: snapshot.root.digest.clone(),
        profile: profile.clone(),
    };
    let evidence = PreparationEvidence {
        output: prepared.output.clone(),
        rank_certificate: certificate.evidence.digest.clone(),
        selector_witnesses: prepared.selector_witnesses.digest.clone(),
    };
    Ok(RecordReads::from_parts(scope, evidence, &prepared.required_reads))
}
