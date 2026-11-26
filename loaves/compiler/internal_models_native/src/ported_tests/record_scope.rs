//! Ported from internal_models/tests/test_record_scope.incn: scoped read evidence binds exact facts and fails closed
//! without publishing a partial result.

use crate::record_scope::{
    AbsentFact, MemberSelection, PayloadSelection, PreparationEvidence, ReadFault, ReadReceipt, ReadScope, ReadWitness,
    RecordKey, RecordRead, RecordReads, RecordRequest, RecordSelection, RecordSource, RevisionEvidence, SelectedFact,
};
use crate::record_service::{
    CompilerFaultKind, InputSnapshot, PreparedInputs, ProducerProfile, ProducerRank, ProducerStage, RankCertificate,
    ReadyWork, RecordRoot, prepared_reader,
};

/// A source returning one configured response, including deliberately invalid bindings for negative controls.
struct Source(RecordRead);

impl RecordSource for Source {
    fn read(&self, _scope: &ReadScope, _request: &RecordRequest) -> RecordRead {
        self.0.clone()
    }
}

fn key(origin: &str) -> RecordKey {
    RecordKey {
        family: "body".into(),
        identity_profile: "identity/1".into(),
        origin: origin.into(),
        identity: "main".into(),
        facet: "ir".into(),
        context: String::new(),
    }
}

fn request(selection: RecordSelection, byte_limit: i64) -> RecordRequest {
    RecordRequest {
        key: key("package"),
        selection,
        byte_limit,
    }
}

fn payload(reference: &str, offset: i64, length: i64) -> RecordSelection {
    RecordSelection::Payload(PayloadSelection {
        reference: reference.into(),
        offset,
        length,
    })
}

fn preparation() -> PreparationEvidence {
    PreparationEvidence {
        output: key("package"),
        rank_certificate: "certificate".into(),
        selector_witnesses: "selectors".into(),
    }
}

fn scope() -> ReadScope {
    ReadScope {
        snapshot: "snapshot".into(),
        profile: "profile".into(),
    }
}

fn fact(scope: &ReadScope, request: &RecordRequest, revision: &str, value: &[u8], digest: &str) -> SelectedFact {
    SelectedFact {
        evidence: RevisionEvidence {
            scope: scope.clone(),
            request: request.clone(),
            revision: revision.into(),
            semantic_digest: "semantic".into(),
        },
        value: value.to_vec(),
        selection_digest: digest.into(),
    }
}

fn read(reads: &mut RecordReads, reply: RecordRead, request: &RecordRequest) -> RecordRead {
    reads.read(&Source(reply), request)
}

fn refused(result: Result<ReadReceipt, ReadFault>, expected: ReadFault) -> bool {
    result.err() == Some(expected)
}

#[test]
fn scoped_record_evidence() {
    let scope = scope();
    let request = request(payload("payload", 3, 2), 65_536);
    let found = fact(&scope, &request, "revision", b"ok", "selection");
    let mut reads = RecordReads::start(&scope, &preparation(), std::slice::from_ref(&request));
    assert_eq!(
        read(&mut reads, RecordRead::Found(found.clone()), &request),
        RecordRead::Found(found.clone())
    );
    let absent = AbsentFact {
        scope: scope.clone(),
        request: request.clone(),
        exclusion_digest: "exclusion".into(),
    };
    assert_eq!(
        read(&mut reads, RecordRead::Absent(absent.clone()), &request),
        RecordRead::Absent(absent.clone())
    );
    assert_eq!((reads.counters().operations, reads.counters().response_bytes), (2, 2));
    let Ok(receipt) = reads.finish() else {
        panic!("complete reads refused")
    };
    assert_eq!(
        (receipt.witnesses.len(), receipt.operations, receipt.response_bytes),
        (2, 2, 2)
    );
    assert_eq!(receipt.scope, scope);
    assert_eq!(
        receipt.witnesses[0],
        ReadWitness::Found(found.evidence.clone(), found.selection_digest.clone())
    );
    assert_eq!(receipt.witnesses[1], ReadWitness::Absent(absent));
    assert_eq!(
        read(&mut reads, RecordRead::Found(found.clone()), &request),
        RecordRead::Failed(ReadFault::ClosedScope)
    );
    assert!(refused(reads.finish(), ReadFault::ClosedScope));
}

#[test]
fn fault_and_binding_controls() {
    let scope = scope();
    let request = request(payload("payload", 3, 2), 65_536);
    let found = fact(&scope, &request, "revision", b"ok", "selection");
    for fault in [
        ReadFault::UnfinishedSupplier,
        ReadFault::DamagedRecord,
        ReadFault::WitnessUnavailable,
    ] {
        let mut reads = RecordReads::start(&scope, &preparation(), std::slice::from_ref(&request));
        read(&mut reads, RecordRead::Found(found.clone()), &request);
        assert_eq!(
            read(&mut reads, RecordRead::Failed(fault), &request),
            RecordRead::Failed(fault)
        );
        assert_eq!(
            read(&mut reads, RecordRead::Found(found.clone()), &request),
            RecordRead::Failed(fault)
        );
        assert_eq!(reads.counters().operations, 2);
        assert!(refused(reads.finish(), fault));
    }
    let stale = ReadScope {
        snapshot: "stale".into(),
        profile: scope.profile.clone(),
    };
    let wrong = fact(&stale, &request, "revision", b"ok", "selection");
    let mut reads = RecordReads::start(&scope, &preparation(), std::slice::from_ref(&request));
    assert_eq!(
        read(&mut reads, RecordRead::Found(wrong), &request),
        RecordRead::Failed(ReadFault::SnapshotMismatch)
    );
    assert!(refused(reads.finish(), ReadFault::SnapshotMismatch));
    let wrong_profile = ReadScope {
        snapshot: scope.snapshot.clone(),
        profile: "other".into(),
    };
    let wrong_request = RecordRequest {
        key: key("other"),
        selection: request.selection.clone(),
        byte_limit: 65_536,
    };
    for (reply, expected) in [
        (
            fact(&wrong_profile, &request, "revision", b"ok", "selection"),
            ReadFault::ProfileMismatch,
        ),
        (
            fact(&scope, &wrong_request, "revision", b"ok", "selection"),
            ReadFault::SelectionMismatch,
        ),
        (
            fact(&scope, &request, "", b"ok", "selection"),
            ReadFault::InvalidEvidence,
        ),
    ] {
        let mut reads = RecordReads::start(&scope, &preparation(), std::slice::from_ref(&request));
        assert_eq!(
            read(&mut reads, RecordRead::Found(reply), &request),
            RecordRead::Failed(expected)
        );
        assert_eq!(reads.counters().response_bytes, 2);
        assert!(refused(reads.finish(), expected));
    }
    let mut reads = RecordReads::start(&scope, &preparation(), &[]);
    assert_eq!(
        read(&mut reads, RecordRead::Found(found), &request),
        RecordRead::Failed(ReadFault::OutsidePreparation)
    );
    assert_eq!(reads.counters().operations, 0);
    assert!(refused(reads.finish(), ReadFault::OutsidePreparation));
}

#[test]
fn empty_invalid_and_selector_controls() {
    let scope = scope();
    let request = request(payload("payload", 3, 2), 65_536);
    let found = fact(&scope, &request, "revision", b"ok", "selection");
    let mut empty = RecordReads::start(&scope, &preparation(), &[]);
    let Ok(receipt) = empty.finish() else {
        panic!("empty activation refused")
    };
    assert_eq!(receipt.schema, "incan.compiler-fact-reads/1");
    assert_eq!(
        (
            receipt.preparation,
            receipt.scope.clone(),
            receipt.witnesses.len(),
            receipt.operations
        ),
        (preparation(), scope.clone(), 0, 0)
    );
    let members = RecordSelection::Members(MemberSelection {
        selector: "candidates".into(),
        order: "canonical".into(),
        lower: String::new(),
        upper: String::new(),
        continuation: "end".into(),
    });
    let member_request = RecordRequest {
        key: key("package"),
        selection: members,
        byte_limit: 65_536,
    };
    let empty_members = fact(&scope, &member_request, "revision", b"", "empty-members");
    let mut reads = RecordReads::start(&scope, &preparation(), std::slice::from_ref(&member_request));
    assert_eq!(
        read(&mut reads, RecordRead::Found(empty_members.clone()), &member_request),
        RecordRead::Found(empty_members)
    );
    let Ok(receipt) = reads.finish() else {
        panic!("member read refused")
    };
    assert_eq!(
        (receipt.witnesses.len(), receipt.response_bytes, receipt.operations),
        (1, 0, 1)
    );
    let invalid_scope = ReadScope {
        snapshot: String::new(),
        profile: scope.profile.clone(),
    };
    assert!(refused(
        RecordReads::start(&invalid_scope, &preparation(), &[]).finish(),
        ReadFault::InvalidEvidence
    ));
    let missing = PreparationEvidence {
        output: key("package"),
        rank_certificate: "certificate".into(),
        selector_witnesses: String::new(),
    };
    assert!(refused(
        RecordReads::start(&scope, &missing, &[]).finish(),
        ReadFault::InvalidEvidence
    ));
    let absent = AbsentFact {
        scope: scope.clone(),
        request: request.clone(),
        exclusion_digest: String::new(),
    };
    let mut reads = RecordReads::start(&scope, &preparation(), std::slice::from_ref(&request));
    assert_eq!(
        read(&mut reads, RecordRead::Absent(absent), &request),
        RecordRead::Failed(ReadFault::InvalidEvidence)
    );
    let short = fact(&scope, &request, "revision", b"x", "selection");
    let mut reads = RecordReads::start(&scope, &preparation(), std::slice::from_ref(&request));
    assert_eq!(
        read(&mut reads, RecordRead::Found(short), &request),
        RecordRead::Failed(ReadFault::InvalidEvidence)
    );
    assert_eq!(reads.counters().response_bytes, 1);
    for selection in [
        payload("other-payload", 3, 2),
        payload("payload", 4, 2),
        RecordSelection::Field("return".into()),
        RecordSelection::Members(MemberSelection {
            selector: "candidates".into(),
            order: "canonical".into(),
            lower: String::new(),
            upper: String::new(),
            continuation: "next".into(),
        }),
    ] {
        let changed = RecordRequest {
            key: key("package"),
            selection,
            byte_limit: 65_536,
        };
        let reply = fact(&scope, &changed, "revision", &found.value, &found.selection_digest);
        let mut reads = RecordReads::start(&scope, &preparation(), std::slice::from_ref(&request));
        assert_eq!(
            read(&mut reads, RecordRead::Found(reply), &request),
            RecordRead::Failed(ReadFault::SelectionMismatch)
        );
        assert!(refused(reads.finish(), ReadFault::SelectionMismatch));
    }
}

#[test]
fn field_and_preparation_controls() {
    let scope = scope();
    let field = request(RecordSelection::Field("return".into()), 2);
    let found = fact(&scope, &field, "revision", b"ok", "field");
    let mut reads = RecordReads::start(&scope, &preparation(), std::slice::from_ref(&field));
    assert_eq!(
        read(&mut reads, RecordRead::Found(found.clone()), &field),
        RecordRead::Found(found.clone())
    );
    let Ok(receipt) = reads.finish() else {
        panic!("field read refused")
    };
    assert_eq!(
        (receipt.operations, receipt.response_bytes, receipt.witnesses.len()),
        (1, 2, 1)
    );
    let small = request(RecordSelection::Field("return".into()), 1);
    let oversized = fact(&scope, &small, "revision", b"ok", "field");
    let mut reads = RecordReads::start(&scope, &preparation(), std::slice::from_ref(&small));
    assert_eq!(
        read(&mut reads, RecordRead::Found(oversized), &small),
        RecordRead::Failed(ReadFault::InvalidEvidence)
    );
    assert_eq!((reads.counters().operations, reads.counters().response_bytes), (1, 2));
    let payload_request = request(payload("payload", 3, 2), 65_536);
    let payload_found = fact(&scope, &payload_request, "revision", b"ok", "selection");
    let snapshot = InputSnapshot {
        root: RecordRoot {
            digest: scope.snapshot.clone(),
        },
    };
    let profile = ProducerProfile {
        identity: RecordRoot {
            digest: scope.profile.clone(),
        },
    };
    let prepared = PreparedInputs {
        output: payload_request.key.clone(),
        snapshot: snapshot.clone(),
        profile: profile.clone(),
        required_reads: vec![payload_request.clone()],
        rank_certificate: RankCertificate {
            rank: ProducerRank {
                namespace: 0,
                stage: ProducerStage::BodyIr,
                dependency: 0,
            },
            evidence: RecordRoot {
                digest: "certificate".into(),
            },
        },
        selector_witnesses: RecordRoot {
            digest: "selectors".into(),
        },
    };
    let ready = ReadyWork {
        output: payload_request.key.clone(),
        stage: ProducerStage::BodyIr,
        profile: profile.clone(),
    };
    let Ok(mut reader) = prepared_reader(&ready, &snapshot, &prepared) else {
        panic!("prepared reader refused")
    };
    assert_eq!(
        read(&mut reader, RecordRead::Found(payload_found.clone()), &payload_request),
        RecordRead::Found(payload_found)
    );
    assert_eq!(reader.finish().map(|receipt| receipt.scope), Ok(scope.clone()));
    let stale = InputSnapshot {
        root: RecordRoot { digest: "stale".into() },
    };
    assert_eq!(
        prepared_reader(&ready, &stale, &prepared).err(),
        Some(CompilerFaultKind::SnapshotMismatch)
    );
    let other = ProducerProfile {
        identity: RecordRoot { digest: "other".into() },
    };
    let wrong = ReadyWork {
        output: payload_request.key.clone(),
        stage: ProducerStage::BodyIr,
        profile: other,
    };
    assert_eq!(
        prepared_reader(&wrong, &snapshot, &prepared).err(),
        Some(CompilerFaultKind::ProfileMismatch)
    );
    let wrong_stage = ReadyWork {
        output: payload_request.key.clone(),
        stage: ProducerStage::CheckBody,
        profile,
    };
    assert_eq!(
        prepared_reader(&wrong_stage, &snapshot, &prepared).err(),
        Some(CompilerFaultKind::InvalidPreparation)
    );
}
