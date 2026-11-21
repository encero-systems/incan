//! Ported from `test_record_activations_use_the_worker_version_two_protocol` in the Incan host's tests: both the
//! JSON generation and its typed successor carry all seven record variants, and an engine protocol body is refused.

use crate::record_protocol::{
    CompilerFault, Evaluate, NeedPrerequisite, Prepare, Prepared, Produced, RECORD_SCHEMA, RecordMessage,
    SemanticRefusal, decode_record_message, record_frame,
};

fn record_variants(generation: &str) -> Result<(), String> {
    let round_trip = |message: RecordMessage| -> Result<RecordMessage, String> {
        decode_record_message(&record_frame(&message, generation)?, generation)
    };
    let prepare = RecordMessage::Prepare(Prepare {
        stage: "declaration".into(),
        snapshot: "s".into(),
        demand_root: "d".into(),
        producer_profile: "p".into(),
    });
    match round_trip(prepare)? {
        RecordMessage::Prepare(value) => assert!(value.stage == "declaration" && value.snapshot == "s"),
        _ => return Err("worker record Prepare changed family".into()),
    }
    let prerequisite = RecordMessage::NeedPrerequisite(NeedPrerequisite {
        keys_root: "keys".into(),
        earlier_rank: 7,
    });
    match round_trip(prerequisite)? {
        RecordMessage::NeedPrerequisite(value) => assert!(value.keys_root == "keys" && value.earlier_rank == 7),
        _ => return Err("worker record NeedPrerequisite changed family".into()),
    }
    match round_trip(RecordMessage::Prepared(Prepared {
        prepared_root: "prepared".into(),
    }))? {
        RecordMessage::Prepared(value) => assert_eq!(value.prepared_root, "prepared"),
        _ => return Err("worker record Prepared changed family".into()),
    }
    let evaluate = RecordMessage::Evaluate(Evaluate {
        work_root: "work".into(),
        snapshot: "s".into(),
        prepared_root: "prepared".into(),
        reservation: "reservation".into(),
        cancellation_generation: 9,
    });
    match round_trip(evaluate)? {
        RecordMessage::Evaluate(value) => assert!(value.work_root == "work" && value.cancellation_generation == 9),
        _ => return Err("worker record Evaluate changed family".into()),
    }
    let produced = RecordMessage::Produced(Produced {
        staged_root: "staged".into(),
        witness_root: "w".into(),
        diagnostic_root: "d".into(),
        counter_root: "c".into(),
    });
    match round_trip(produced)? {
        RecordMessage::Produced(value) => assert!(value.staged_root == "staged" && value.witness_root == "w"),
        _ => return Err("worker record Produced changed family".into()),
    }
    let refusal = RecordMessage::SemanticRefusal(SemanticRefusal {
        diagnostic_root: "diagnostics".into(),
        witness_root: "witnesses".into(),
    });
    match round_trip(refusal)? {
        RecordMessage::SemanticRefusal(value) => {
            assert!(value.diagnostic_root == "diagnostics" && value.witness_root == "witnesses")
        }
        _ => return Err("worker record SemanticRefusal changed family".into()),
    }
    match round_trip(RecordMessage::CompilerFault(CompilerFault {
        fault_root: "fault".into(),
    }))? {
        RecordMessage::CompilerFault(value) => assert_eq!(value.fault_root, "fault"),
        _ => return Err("worker record CompilerFault changed family".into()),
    }
    let wrong = format!("{{\"generation\":\"{generation}\",\"kind\":\"prepare\",\"schema\":\"incan.engine/1\"}}");
    assert!(
        decode_record_message(wrong.as_bytes(), generation).is_err(),
        "the worker admitted an engine protocol record body"
    );
    assert_eq!(RECORD_SCHEMA, "incan.compiler-worker/2");
    Ok(())
}

#[test]
fn record_activations_use_the_worker_version_two_protocol() -> Result<(), String> {
    record_variants("0123abcd")?;
    record_variants("cw3-0123456789abcdef")
}
