//! Revision-bound reads. A single failed read prevents publication of a partial receipt.

use std::collections::HashSet;

pub const READ_SCHEMA: &str = "incan.compiler-fact-reads/1";

/// Full logical identity; locations, revisions and invocation-local table slots are excluded. `context` defaults to
/// empty.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RecordKey {
    pub family: String,
    pub identity: String,
    pub identity_profile: String,
    pub origin: String,
    pub facet: String,
    pub context: String,
}
/// Exact leased snapshot and producer profile, retained by the source for the activation's lifetime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadScope {
    pub snapshot: String,
    pub profile: String,
}
/// A snapshot-bound payload reference and exact byte range, not a physical location.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PayloadSelection {
    pub reference: String,
    pub offset: i64,
    pub length: i64,
}
/// A schema-owned selector, ordering and bounded continuation, including empty-result evidence.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MemberSelection {
    pub selector: String,
    pub order: String,
    pub lower: String,
    pub upper: String,
    pub continuation: String,
}
/// A field, exact payload range, or bounded ordered membership selector, including its continuation boundary.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RecordSelection {
    Field(String),
    Payload(PayloadSelection),
    Members(MemberSelection),
}
/// A native checker graph page and the compiled output carrying it each admit at most 64 MiB.
pub const MAX_CHECKER_ROW_BYTES: i64 = 67_108_864;
/// Other selected responses and individual range reads keep the record protocol's 64 KiB page bound.
pub const DEFAULT_READ_BYTES: i64 = 65_536;

/// One bounded selection; the source must return evidence for precisely this key and selection. `byte_limit`
/// defaults to [`DEFAULT_READ_BYTES`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RecordRequest {
    pub key: RecordKey,
    pub selection: RecordSelection,
    pub byte_limit: i64,
}
/// Exact immutable binding and semantic value; payload and selector evidence remain separate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevisionEvidence {
    pub scope: ReadScope,
    pub request: RecordRequest,
    pub revision: String,
    pub semantic_digest: String,
}
/// One selected encoded value and its authenticated source evidence, never a decoded dependency closure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedFact {
    pub evidence: RevisionEvidence,
    pub value: Vec<u8>,
    pub selection_digest: String,
}
/// A proven semantic exclusion for the exact request, never an unloaded or unfinished supplier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbsentFact {
    pub scope: ReadScope,
    pub request: RecordRequest,
    pub exclusion_digest: String,
}
/// A witnessed tombstone for the exact request, retaining the immutable tombstone revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeletedFact {
    pub scope: ReadScope,
    pub request: RecordRequest,
    pub revision: String,
}
/// One bounded ordered member page and the authenticated digest and continuation of that exact selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberFact {
    pub scope: ReadScope,
    pub request: RecordRequest,
    pub value: Vec<u8>,
    pub membership_digest: String,
    pub continuation: String,
}
/// Infrastructure faults terminate the activation and cannot become source diagnostics or semantic absence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadFault {
    SnapshotMismatch,
    ProfileMismatch,
    SelectionMismatch,
    InvalidEvidence,
    UnfinishedSupplier,
    DamagedRecord,
    WitnessUnavailable,
    ClosedScope,
    OutsidePreparation,
}
/// Final selected value, witnessed semantic absence, or a typed infrastructure fault.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordRead {
    Found(SelectedFact),
    Deleted(DeletedFact),
    Absent(AbsentFact),
    Members(MemberFact),
    Failed(ReadFault),
}
/// Successful reads only: found and exclusion evidence preserve their different invalidation contracts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadWitness {
    Found(RevisionEvidence, String),
    Deleted(DeletedFact),
    Absent(AbsentFact),
    Members(ReadScope, RecordRequest, String, String),
}
/// Source calls and encoded response bytes, including invalid replies; source I/O and decode costs are separate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadCounters {
    pub operations: i64,
    pub response_bytes: i64,
}
/// The certified output and earlier selector evidence remain dependencies even when no value is read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparationEvidence {
    pub output: RecordKey,
    pub rank_certificate: String,
    pub selector_witnesses: String,
}
/// Complete activation evidence, including empty reads, exact profile and measured response bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadReceipt {
    pub schema: String,
    pub scope: ReadScope,
    pub preparation: PreparationEvidence,
    pub witnesses: Vec<ReadWitness>,
    pub operations: i64,
    pub response_bytes: i64,
}

/// A source owns its value bytes and authenticates the evidence under the leased snapshot.
pub trait RecordSource {
    /// Read exactly the selected request.
    fn read(&self, scope: &ReadScope, request: &RecordRequest) -> RecordRead;
}

struct Activation {
    scope: ReadScope,
    preparation: PreparationEvidence,
    allowed: HashSet<RecordRequest>,
    witnesses: Vec<ReadWitness>,
    fault: Option<ReadFault>,
}

/// One activation's actual reads; a fault is sticky and prevents publication of a partial witness set.
pub struct RecordReads {
    activation: Option<Activation>,
    counters: ReadCounters,
}

impl RecordReads {
    /// Retain the immutable request envelope for the life of the returned activation.
    pub fn start(scope: &ReadScope, preparation: &PreparationEvidence, allowed: &[RecordRequest]) -> Self {
        // The returned activation owns its scope and request permissions after the caller releases the inputs.
        Self::from_parts(scope.clone(), preparation.clone(), allowed)
    }

    /// Take freshly prepared bindings directly when the reader already owns them.
    pub(crate) fn from_parts(scope: ReadScope, preparation: PreparationEvidence, allowed: &[RecordRequest]) -> Self {
        let valid = !scope.snapshot.is_empty()
            && !scope.profile.is_empty()
            && valid_key(&preparation.output)
            && !preparation.rank_certificate.is_empty()
            && !preparation.selector_witnesses.is_empty();
        let activation = Activation {
            scope,
            preparation,
            allowed: allowed.iter().cloned().collect(),
            witnesses: Vec::new(),
            fault: (!valid).then_some(ReadFault::InvalidEvidence),
        };
        Self {
            activation: Some(activation),
            counters: ReadCounters::default(),
        }
    }

    /// Charge each source call and its returned bytes before accepting evidence. The first fault is sticky.
    pub fn read<S: RecordSource>(&mut self, source: &S, request: &RecordRequest) -> RecordRead {
        let Some(active) = self.activation.as_mut() else {
            return RecordRead::Failed(ReadFault::ClosedScope);
        };
        if let Some(fault) = active.fault {
            return RecordRead::Failed(fault);
        }
        let refusal = if !valid_request(request) {
            Some(ReadFault::InvalidEvidence)
        } else if !active.allowed.contains(request) {
            Some(ReadFault::OutsidePreparation)
        } else {
            None
        };
        if let Some(fault) = refusal {
            active.fault = Some(fault);
            return RecordRead::Failed(fault);
        }
        self.counters.operations += 1;
        let reply = source.read(&active.scope, request);
        let bytes = match &reply {
            RecordRead::Found(fact) => fact.value.len(),
            RecordRead::Members(fact) => fact.value.len(),
            _ => 0,
        };
        // A single Rust byte allocation always fits isize and therefore i64 on supported hosts.
        self.counters.response_bytes += bytes as i64;
        match witness(&active.scope, request, &reply) {
            Ok(evidence) => {
                active.witnesses.push(evidence);
                reply
            }
            Err(fault) => {
                active.fault = Some(fault);
                RecordRead::Failed(fault)
            }
        }
    }

    /// Counters include invalid replies, and remain available after finish.
    pub fn counters(&self) -> ReadCounters {
        self.counters
    }

    /// Close once, moving the complete witness vector into the receipt.
    pub fn finish(&mut self) -> Result<ReadReceipt, ReadFault> {
        let active = self.activation.take().ok_or(ReadFault::ClosedScope)?;
        if let Some(fault) = active.fault {
            return Err(fault);
        }
        Ok(ReadReceipt {
            schema: READ_SCHEMA.to_owned(),
            scope: active.scope,
            preparation: active.preparation,
            witnesses: active.witnesses,
            operations: self.counters.operations,
            response_bytes: self.counters.response_bytes,
        })
    }
}

/// Check the common identity fields. Context may be empty.
pub fn valid_key(key: &RecordKey) -> bool {
    !key.family.is_empty()
        && !key.identity_profile.is_empty()
        && !key.origin.is_empty()
        && !key.identity.is_empty()
        && !key.facet.is_empty()
}

/// The ranked worker's prior-declaration is a snapshot copy of the same typed declaration row.
pub fn checked_row_family(family: &str) -> bool {
    matches!(family, "declaration" | "prior-declaration" | "local" | "final")
}

/// Read one typed checker owner's complete authenticated row. An oversized reply needs its known encoded extent.
pub fn checked_row_request(key: &RecordKey, encoded_bytes: i64) -> Result<RecordRequest, String> {
    if !checked_row_family(&key.family) || encoded_bytes <= 0 || encoded_bytes > MAX_CHECKER_ROW_BYTES {
        return Err("checked field lies outside its bounded compiler graph page".to_owned());
    }
    Ok(RecordRequest {
        key: key.clone(),
        selection: RecordSelection::Field("value".to_owned()),
        byte_limit: DEFAULT_READ_BYTES.max(encoded_bytes),
    })
}

/// Reject incomplete full keys and invalid range requests before the source can observe them.
pub fn valid_request(request: &RecordRequest) -> bool {
    if request.byte_limit <= 0 || request.byte_limit > MAX_CHECKER_ROW_BYTES {
        return false;
    }
    if request.byte_limit > DEFAULT_READ_BYTES
        && (!checked_row_family(&request.key.family)
            || !matches!(&request.selection, RecordSelection::Field(name) if name == "value"))
    {
        return false;
    }
    if !valid_key(&request.key) {
        return false;
    }
    match &request.selection {
        RecordSelection::Field(name) => !name.is_empty(),
        RecordSelection::Payload(p) => {
            !p.reference.is_empty() && p.offset >= 0 && p.length >= 0 && p.length <= request.byte_limit
        }
        RecordSelection::Members(m) => !m.selector.is_empty() && !m.order.is_empty(),
    }
}

/// Snapshot, profile and selector failures are reported in that order.
fn bind(
    scope: &ReadScope,
    request: &RecordRequest,
    actual: &ReadScope,
    selected: &RecordRequest,
) -> Result<(), ReadFault> {
    if scope.snapshot != actual.snapshot {
        Err(ReadFault::SnapshotMismatch)
    } else if scope.profile != actual.profile {
        Err(ReadFault::ProfileMismatch)
    } else if request != selected {
        Err(ReadFault::SelectionMismatch)
    } else {
        Ok(())
    }
}

/// Witnesses and replies have independent owners. Duplicate only accepted metadata, never the returned payload.
fn witness(scope: &ReadScope, request: &RecordRequest, reply: &RecordRead) -> Result<ReadWitness, ReadFault> {
    match reply {
        RecordRead::Failed(fault) => Err(*fault),
        RecordRead::Deleted(fact) => {
            bind(scope, request, &fact.scope, &fact.request)?;
            if fact.revision.is_empty() {
                return Err(ReadFault::InvalidEvidence);
            }
            Ok(ReadWitness::Deleted(fact.clone()))
        }
        RecordRead::Absent(fact) => {
            bind(scope, request, &fact.scope, &fact.request)?;
            if fact.exclusion_digest.is_empty() {
                return Err(ReadFault::InvalidEvidence);
            }
            Ok(ReadWitness::Absent(fact.clone()))
        }
        RecordRead::Found(fact) => {
            bind(scope, request, &fact.evidence.scope, &fact.evidence.request)?;
            if fact.value.len() as i64 > request.byte_limit
                || fact.evidence.revision.is_empty()
                || fact.evidence.semantic_digest.is_empty()
                || fact.selection_digest.is_empty()
                || matches!(&request.selection, RecordSelection::Payload(p) if p.length != fact.value.len() as i64)
            {
                return Err(ReadFault::InvalidEvidence);
            }
            Ok(ReadWitness::Found(fact.evidence.clone(), fact.selection_digest.clone()))
        }
        RecordRead::Members(fact) => {
            bind(scope, request, &fact.scope, &fact.request)?;
            if !matches!(request.selection, RecordSelection::Members(_)) {
                return Err(ReadFault::SelectionMismatch);
            }
            if fact.value.len() as i64 > request.byte_limit
                || fact.membership_digest.is_empty()
                || fact.continuation.is_empty()
            {
                return Err(ReadFault::InvalidEvidence);
            }
            Ok(ReadWitness::Members(
                fact.scope.clone(),
                fact.request.clone(),
                fact.membership_digest.clone(),
                fact.continuation.clone(),
            ))
        }
    }
}
