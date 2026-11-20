//! The worker protocols' wire bytes and refusals, pinned to the reference worker host (release run 6 on f0fa3b405d).
//!
//! `wire_golden::FRAMES` holds the 46 frames the reference encoded for the inputs below (both wire generations: the
//! JSON documents and the typed Crumb packets); the refusal cases repeat `run1/golden/worker-refusals-1.incn` with the
//! reference's exact outcomes and texts.

use crate::protocol::{
    Admission, CompileRequest, Fault, MISMATCH, Reply, SCHEMA, WorkerCommand, admit, compile_frame, completion,
    encode_ready, encode_reply, handoff, parse_document, read_completion, read_ready, ready, ready_frame, reply_frame,
    request_document, retired,
};
use crate::record_protocol::{
    CompilerFault, Evaluate, NeedPrerequisite, Prepare, Prepared, Produced, RECORD_SCHEMA, RecordAdmission,
    RecordMessage, SemanticRefusal, accept_record_reply, admit_record, decode_record_message, encode_record_message,
    record_frame,
};
use crate::wire_golden::FRAMES;

const JSON_GEN: &str = "0123abcd";
const CW3_GEN: &str = "cw3-0123456789abcdef";

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn replies() -> Vec<Reply> {
    vec![
        Reply::Completed(0),
        Reply::Completed(255),
        Reply::Refused("why \"not\" é".to_owned()),
        Reply::CompilerFault(Fault::Panicked),
        Reply::CompilerFault(Fault::UnsafeCleanup("leaked state".to_owned())),
        Reply::HandedOff("/p/handoff.json".to_owned()),
    ]
}

fn messages() -> Vec<RecordMessage> {
    vec![
        RecordMessage::Prepare(Prepare {
            stage: "final".into(),
            snapshot: "s1".into(),
            demand_root: "d1".into(),
            producer_profile: "release".into(),
        }),
        RecordMessage::Prepared(Prepared {
            prepared_root: "prepared-root".into(),
        }),
        RecordMessage::NeedPrerequisite(NeedPrerequisite {
            keys_root: "keys".into(),
            earlier_rank: 4_294_967_295,
        }),
        RecordMessage::Evaluate(Evaluate {
            work_root: "w".into(),
            snapshot: "s2".into(),
            prepared_root: "p".into(),
            reservation: "r".into(),
            cancellation_generation: 7,
        }),
        RecordMessage::Produced(Produced {
            staged_root: "a".into(),
            witness_root: "b".into(),
            diagnostic_root: "c".into(),
            counter_root: "d".into(),
        }),
        RecordMessage::SemanticRefusal(SemanticRefusal {
            diagnostic_root: "diag".into(),
            witness_root: "wit".into(),
        }),
        RecordMessage::CompilerFault(CompilerFault {
            fault_root: "fault é".into(),
        }),
    ]
}

/// Every frame the capture program printed, encoded by this crate, in the capture's order and spelling.
fn encoded_frames() -> Result<Vec<(String, String)>, String> {
    let mut frames = Vec::new();
    for generation in [JSON_GEN, CW3_GEN] {
        let arguments = strings(&["--unit", "/p/u.json", "crate_name", "/p/out", "/p/sysroot", "--extra"]);
        let first = compile_frame(generation, &arguments, "/p", "/p/out.txt", "/p/é\"err", false)?;
        frames.push((format!("{generation}\tcompile\t0"), hex(&first)));
        let check = strings(&["--check", "/q/u", "/q/s"]);
        let second = compile_frame(generation, &check, "/q", "/q/o", "/q/e", true)?;
        frames.push((format!("{generation}\tcompile\t1"), hex(&second)));
        for (retirement, label) in [(false, "live"), (true, "retired")] {
            for (index, reply) in replies().iter().enumerate() {
                let frame = reply_frame(reply, retirement, generation)?;
                frames.push((format!("{generation}\treply\t{index}\t{label}"), hex(&frame)));
            }
        }
        frames.push((format!("{generation}\tready\t0"), hex(&ready_frame(generation, 42)?)));
        frames.push((format!("{generation}\tready\t1"), hex(&ready_frame(generation, 9_007_199_254_740_993)?)));
        for (index, message) in messages().iter().enumerate() {
            frames.push((format!("{generation}\trecord\t{index}"), hex(&record_frame(message, generation)?)));
        }
    }
    Ok(frames)
}

#[test]
fn every_frame_is_byte_identical_to_the_reference() -> Result<(), String> {
    let frames = encoded_frames()?;
    assert_eq!(frames.len(), FRAMES.len());
    for ((label, encoded), (expected_label, expected)) in frames.iter().zip(FRAMES) {
        assert_eq!(label, expected_label);
        assert_eq!(encoded, expected, "{label}");
    }
    Ok(())
}

#[test]
fn every_reference_frame_decodes_back_to_its_input() -> Result<(), String> {
    for generation in [JSON_GEN, CW3_GEN] {
        for message in messages() {
            assert_eq!(decode_record_message(&record_frame(&message, generation)?, generation)?, message);
        }
        for (retirement, reply) in replies().iter().enumerate().map(|(index, reply)| (index % 2 == 0, reply)) {
            let completion = read_completion(&reply_frame(reply, retirement, generation)?, generation);
            match reply {
                Reply::Refused(reason) if !generation.starts_with("cw3-") => {
                    assert_eq!(completion.err().as_deref(), Some(reason.as_str()))
                }
                Reply::CompilerFault(Fault::UnsafeCleanup(reason)) if !generation.starts_with("cw3-") => {
                    assert_eq!(completion.err().as_deref(), Some(reason.as_str()))
                }
                _ => assert_eq!(completion?.retired, retirement),
            }
        }
        read_ready(&ready_frame(generation, 42)?, generation, 42)?;
        assert!(read_ready(&ready_frame(generation, 42)?, generation, 43).is_err());
    }
    Ok(())
}

fn described(outcome: Admission) -> String {
    match outcome {
        Admission::Admitted(request) => {
            let CompileRequest {
                command,
                arguments,
                stdout,
                stderr,
                cwd,
                retire_after,
            } = request;
            format!("admitted\t{}\t{arguments:?}\t{stdout}\t{stderr}\t{cwd}\t{retire_after}", command.light())
        }
        Admission::Refused(reason) => format!("refused\t{reason}"),
        Admission::Incompatible => "incompatible".to_owned(),
    }
}

fn head(generation: &str) -> String {
    format!("\"schema\":\"incan.compiler-worker/1\",\"generation\":\"{generation}\"")
}

#[test]
fn json_compile_requests_keep_their_admission_and_refusals() {
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        ("a01", vec![0xff, 0x00], "refused\tthe frame is not UTF-8"),
        ("a02", b"not json".to_vec(), "refused\tJSONDecodeError: expected ident at line 1 column 2"),
        ("a03", b"{}".to_vec(), "incompatible"),
        ("a04", b"{\"schema\":\"incan.compiler-worker/1\",\"generation\":\"other\"}".to_vec(), "incompatible"),
        ("a05", format!("{{{}}}", head(JSON_GEN)).into_bytes(), "refused\tmissing `arguments`"),
    ];
    let more: Vec<(&str, &str, &str)> = vec![
        ("a06", ",\"arguments\":\"x\"", "refused\t`arguments` is not an array of strings"),
        ("a07", ",\"arguments\":[1]", "refused\t`arguments` is not an array of strings"),
        (
            "a08",
            ",\"arguments\":[]",
            "refused\ta compiler worker accepts only --unit, --check, --fmt, --source-files and --rust-imports requests",
        ),
        (
            "a09",
            ",\"arguments\":[\"--run\"]",
            "refused\ta compiler worker accepts only --unit, --check, --fmt, --source-files and --rust-imports requests",
        ),
        ("a10", ",\"arguments\":[\"--unit\",\"/u\"]", "refused\ta compiler worker --unit request is incomplete"),
        (
            "a11",
            ",\"arguments\":[\"--unit\",\"u\",\"c\",\"/o\",\"/s\"]",
            "refused\tcompiler worker unit, output and sysroot paths must be absolute",
        ),
        ("a12", ",\"arguments\":[\"--check\",\"/u\",\"/s\"]", "refused\tmissing `stdout`"),
        (
            "a13",
            ",\"arguments\":[\"--check\",\"/u\",\"/s\"],\"cwd\":\"rel\",\"stdout\":\"/o\",\"stderr\":\"/e\"",
            "refused\tcompiler worker `cwd` path must be absolute",
        ),
        (
            "a14",
            ",\"arguments\":[\"--check\",\"/u\",\"/s\"],\"cwd\":\"/c\",\"stdout\":\"/o\",\"stderr\":\"/e\",\"retire_after\":1",
            "refused\tretire_after must be Boolean",
        ),
        (
            "a15",
            ",\"arguments\":[\"--fmt\",\"/u\",\"a\",\"b\"],\"cwd\":\"/c\",\"stdout\":\"/o\",\"stderr\":\"/e\",\"retire_after\":true",
            "admitted\ttrue\t[\"/u\", \"a\", \"b\"]\t/o\t/e\t/c\ttrue",
        ),
        (
            "a16",
            ",\"arguments\":[\"--source-files\",\"x\"],\"cwd\":\"/c\",\"stdout\":\"/o\",\"stderr\":\"/e\"",
            "admitted\ttrue\t[\"x\"]\t/o\t/e\t/c\tfalse",
        ),
        (
            "a17",
            ",\"arguments\":[\"--rust-imports\",\"/u\"],\"cwd\":\"/c\",\"stdout\":\"/o\",\"stderr\":\"/e\"",
            "admitted\ttrue\t[\"/u\"]\t/o\t/e\t/c\tfalse",
        ),
        (
            "a18",
            ",\"arguments\":[\"--unit\",\"/u\",\"c\",\"/o\",\"/s\"],\"cwd\":\"/c\",\"stdout\":\"o\",\"stderr\":\"/e\"",
            "refused\tcompiler worker `stdout` path must be absolute",
        ),
        (
            "a19",
            ",\"arguments\":[\"--unit\",\"/u\",\"c\",\"/o\",\"/s\"],\"cwd\":\"/c\",\"stderr\":\"/e\"",
            "refused\tmissing `stdout`",
        ),
    ];
    for (label, payload, expected) in cases {
        assert_eq!(described(admit(&payload, JSON_GEN)), expected, "{label}");
    }
    for (label, tail, expected) in more {
        let payload = format!("{{{}{tail}}}", head(JSON_GEN)).into_bytes();
        assert_eq!(described(admit(&payload, JSON_GEN)), expected, "{label}");
    }
}

#[test]
fn typed_compile_frames_keep_their_admission_and_refusals() -> Result<(), String> {
    assert_eq!(described(admit(&[0xff, 0x00], CW3_GEN)), "refused\tCrumb binary truncated at byte 2");
    assert_eq!(described(admit(b"{}", CW3_GEN)), "refused\tCrumb binary truncated at byte 2");
    let check = strings(&["--check", "/u", "/s"]);
    let frame = compile_frame(CW3_GEN, &check, "/c", "/o", "/e", false)?;
    assert_eq!(described(admit(&frame, CW3_GEN)), "admitted\tfalse\t[\"/u\", \"/s\"]\t/o\t/e\t/c\tfalse");
    assert_eq!(described(admit(&frame, "cw3-ffffffffffffffff")), "incompatible");
    assert!(matches!(admit_record(&frame, CW3_GEN), RecordAdmission::NotRecordProtocol));
    assert_eq!(
        decode_record_message(&frame, CW3_GEN).err().as_deref(),
        Some("unexpected compiler worker record packet")
    );
    assert_eq!(
        read_completion(&frame, CW3_GEN).err().as_deref(),
        Some("compiler worker completion protocol mismatch")
    );
    let relative = compile_frame(CW3_GEN, &strings(&["--check", "u", "/s"]), "c", "/o", "/e", false)?;
    assert_eq!(
        described(admit(&relative, CW3_GEN)),
        "refused\tcompiler worker unit, output and sysroot paths must be absolute"
    );
    let directory = compile_frame(CW3_GEN, &check, "c", "/o", "/e", false)?;
    assert_eq!(
        described(admit(&directory, CW3_GEN)),
        "refused\tcompiler worker console paths and working directory must be absolute"
    );
    let reply = reply_frame(&Reply::Completed(3), false, CW3_GEN)?;
    let completed = read_completion(&reply, CW3_GEN)?;
    assert_eq!((completed.status, completed.reason.as_str(), completed.handoff.as_str()), (3, "", ""));
    assert!(!completed.retired);
    assert_eq!(described(admit(&reply, CW3_GEN)), "incompatible");
    assert!(matches!(admit_record(&reply, CW3_GEN), RecordAdmission::NotRecordProtocol));
    assert_eq!(
        read_ready(&reply, CW3_GEN, 42).err().as_deref(),
        Some("compiler worker readiness protocol, generation or PID mismatch")
    );
    Ok(())
}

fn record_outcome(outcome: RecordAdmission) -> String {
    match outcome {
        RecordAdmission::Admitted(RecordMessage::Prepare(value)) => {
            format!("admitted prepare\t{}\t{}", value.stage, value.snapshot)
        }
        RecordAdmission::Admitted(RecordMessage::Evaluate(value)) => {
            format!("admitted evaluate\t{}\t{}", value.work_root, value.cancellation_generation)
        }
        RecordAdmission::Admitted(_) => "admitted other".to_owned(),
        RecordAdmission::NotRecordProtocol => "not-record".to_owned(),
        RecordAdmission::Incompatible => "incompatible".to_owned(),
        RecordAdmission::ProtocolFault => "protocol-fault".to_owned(),
    }
}

#[test]
fn record_requests_keep_their_admission() -> Result<(), String> {
    let base = "{\"schema\":\"incan.compiler-worker/2\",\"generation\":\"0123abcd\"";
    let cases: Vec<(&str, String, &str)> = vec![
        ("c01", "x".into(), "not-record"),
        ("c02", "{\"schema\":\"other\"}".into(), "not-record"),
        ("c03", "{\"schema\":\"incan.compiler-worker/2\"}".into(), "incompatible"),
        ("c04", "{\"schema\":\"incan.compiler-worker/2\",\"generation\":\"x\"}".into(), "incompatible"),
        ("c05", format!("{base},\"kind\":\"prepared\",\"prepared_root\":\"p\"}}"), "protocol-fault"),
        (
            "c06",
            format!(
                "{base},\"kind\":\"prepare\",\"stage\":\"a\",\"snapshot\":\"b\",\"demand_root\":\"c\",\"producer_profile\":\"d\"}}"
            ),
            "admitted prepare\ta\tb",
        ),
        (
            "c07",
            format!(
                "{base},\"kind\":\"evaluate\",\"work_root\":\"w\",\"snapshot\":\"s\",\"prepared_root\":\"p\",\"reservation\":\"r\",\"cancellation_generation\":4294967296}}"
            ),
            "protocol-fault",
        ),
        ("c08", format!("{base},\"kind\":\"bogus\"}}"), "protocol-fault"),
    ];
    for (label, payload, expected) in cases {
        assert_eq!(record_outcome(admit_record(payload.as_bytes(), JSON_GEN)), expected, "{label}");
    }
    let prepare = RecordMessage::Prepare(Prepare {
        stage: "s".into(),
        snapshot: "n".into(),
        demand_root: "d".into(),
        producer_profile: "p".into(),
    });
    let frame = record_frame(&prepare, CW3_GEN)?;
    assert_eq!(record_outcome(admit_record(&frame, CW3_GEN)), "admitted prepare\ts\tn");
    assert_eq!(record_outcome(admit_record(&frame, "cw3-ffffffffffffffff")), "incompatible");
    assert_eq!(described(admit(&frame, CW3_GEN)), "incompatible");
    assert_eq!(
        decode_record_message(&frame, "cw3-ffffffffffffffff").err().as_deref(),
        Some("compiler worker generation or record protocol mismatch")
    );
    assert_eq!(
        decode_record_message(&frame[..20], CW3_GEN).err().as_deref(),
        Some("Crumb binary truncated at byte 20")
    );
    Ok(())
}

#[test]
fn record_decoding_keeps_its_refusal_texts() {
    let base = "{\"schema\":\"incan.compiler-worker/2\",\"generation\":\"0123abcd\"";
    let need = format!("{base},\"kind\":\"need-prerequisite\",\"keys_root\":\"k\"");
    let cases: Vec<(&str, Vec<u8>, &str, &str)> = vec![
        ("d01", b"x".to_vec(), JSON_GEN, "JSONDecodeError: expected value at line 1 column 1"),
        ("d02", b"{}".to_vec(), JSON_GEN, "missing `schema`"),
        ("d03", b"{\"schema\":1}".to_vec(), JSON_GEN, "`schema` must be text"),
        ("d04", format!("{base}}}").into_bytes(), JSON_GEN, "missing `kind`"),
        ("d05", format!("{need},\"earlier_rank\":-1}}").into_bytes(), JSON_GEN, "`earlier_rank` exceeds u32"),
        ("d06", format!("{need},\"earlier_rank\":1.5}}").into_bytes(), JSON_GEN, "`earlier_rank` must be an integer"),
        (
            "d07",
            format!("{base},\"kind\":\"need-prerequisite\",\"keys_root\":2}}").into_bytes(),
            JSON_GEN,
            "`keys_root` must be text",
        ),
        ("d08", vec![0xff], JSON_GEN, "the record frame is not UTF-8"),
        ("d09", b"CRMB".to_vec(), CW3_GEN, "Crumb binary truncated at byte 4"),
    ];
    for (label, payload, generation, expected) in cases {
        assert_eq!(decode_record_message(&payload, generation).err().as_deref(), Some(expected), "{label}");
    }
}

#[test]
fn json_completions_and_readiness_keep_their_texts() -> Result<(), String> {
    let cases: Vec<(&str, &str, Result<i64, &str>, Result<(), &str>)> = vec![
        (
            "e01",
            "{}",
            Err("KeyError: missing JSON object key `schema`"),
            Err("KeyError: missing JSON object key `schema`"),
        ),
        ("e02", "{\"schema\":2}", Err("TypeError: expected JSON string"), Err("TypeError: expected JSON string")),
        (
            "e03",
            "{\"schema\":\"incan.compiler-worker/1\",\"reason\":\"why\"}",
            Err("why"),
            Err("KeyError: missing JSON object key `generation`"),
        ),
        (
            "e04",
            "{\"schema\":\"incan.compiler-worker/1\",\"exit\":256}",
            Err("compiler worker returned no valid completion code"),
            Err("KeyError: missing JSON object key `generation`"),
        ),
        (
            "e05",
            "{\"schema\":\"incan.compiler-worker/1\",\"exit\":7,\"generation\":\"0123abcd\",\"pid\":42,\"status\":\"ready\"}",
            Ok(7),
            Ok(()),
        ),
        (
            "e06",
            "{\"schema\":\"incan.compiler-worker/1\",\"generation\":\"0123abcd\",\"pid\":\"42\"}",
            Err("compiler worker returned no valid completion code"),
            Err("compiler worker reply has no integer `pid`"),
        ),
        (
            "e07",
            "{\"schema\":\"incan.compiler-worker/1\",\"generation\":\"0123abcd\",\"pid\":41}",
            Err("compiler worker returned no valid completion code"),
            Err("compiler worker readiness PID mismatch"),
        ),
        (
            "e08",
            "{\"schema\":\"incan.compiler-worker/1\",\"generation\":\"0123abcd\",\"pid\":42}",
            Err("compiler worker returned no valid completion code"),
            Err("KeyError: missing JSON object key `status`"),
        ),
        (
            "e09",
            "{\"schema\":\"incan.compiler-worker/1\",\"generation\":\"0123abcd\",\"pid\":42,\"status\":\"busy\"}",
            Err("compiler worker returned no valid completion code"),
            Err("compiler worker did not acknowledge readiness"),
        ),
    ];
    for (label, text, code, readiness) in cases {
        let document = parse_document(text)?;
        assert_eq!(completion(&document).map_err(|error| error.to_string()), code.map_err(str::to_owned), "{label}");
        assert_eq!(
            ready(&document, JSON_GEN, 42).map_err(|error| error.to_string()),
            readiness.map_err(str::to_owned),
            "{label}"
        );
    }
    assert_eq!(
        parse_document("[1").err().as_deref(),
        Some("JSONDecodeError: EOF while parsing a list at line 1 column 2")
    );
    Ok(())
}

#[test]
fn replies_keep_the_protocol_bytes() -> Result<(), String> {
    let generation = "0123456789abcdef";
    let completed = format!("{{\"exit\":0,\"reason\":\"\",\"schema\":\"{SCHEMA}\"}}");
    assert_eq!(encode_reply(&Reply::Completed(0), false), completed.as_bytes());
    let refused = format!("{{\"exit\":1,\"reason\":\"missing `cwd`\",\"retired\":true,\"schema\":\"{SCHEMA}\"}}");
    assert_eq!(encode_reply(&Reply::Refused("missing `cwd`".into()), true), refused.as_bytes());
    let ready_text = format!("{{\"generation\":\"{generation}\",\"pid\":42,\"schema\":\"{SCHEMA}\",\"status\":\"ready\"}}");
    assert_eq!(encode_ready(generation, 42), ready_text.as_bytes());
    assert_eq!(completion(&parse_document(&completed)?)?, 0);
    let refused_document = parse_document(&refused)?;
    assert!(retired(&refused_document));
    assert_eq!(completion(&refused_document).err().as_deref(), Some("missing `cwd`"));
    for code in [0, 101, 255] {
        let document = parse_document(&String::from_utf8_lossy(&encode_reply(&Reply::Completed(code), false)))?;
        assert_eq!(completion(&document)?, code);
        assert!(!retired(&document));
        assert_eq!(handoff(&document), None);
    }
    for invalid in [
        "{}".to_owned(),
        "{\"schema\":\"other\",\"exit\":0}".to_owned(),
        format!("{{\"schema\":\"{SCHEMA}\",\"exit\":256}}"),
        format!("{{\"schema\":\"{SCHEMA}\",\"exit\":-1}}"),
    ] {
        assert!(completion(&parse_document(&invalid)?).is_err(), "{invalid}");
    }
    assert_eq!(
        encode_reply(&Reply::Refused(MISMATCH.into()), false),
        format!("{{\"exit\":1,\"reason\":\"{MISMATCH}\",\"schema\":\"{SCHEMA}\"}}").as_bytes()
    );
    let arguments = strings(&["--fmt", "/p/unit.json", "src/main.incn", "--check"]);
    let expected = format!(
        "{{\"arguments\":[\"--fmt\",\"/p/unit.json\",\"src/main.incn\",\"--check\"],\"cwd\":\"/p\",\"generation\":\"{generation}\",\"schema\":\"{SCHEMA}\",\"stderr\":\"/p/r.err\",\"stdout\":\"/p/r.out\"}}"
    );
    assert_eq!(request_document(generation, &arguments, "/p", "/p/r.out", "/p/r.err", false), expected);
    let retiring = expected.replace(",\"schema\"", ",\"retire_after\":true,\"schema\"");
    assert_eq!(request_document(generation, &arguments, "/p", "/p/r.out", "/p/r.err", true), retiring);
    assert!(!WorkerCommand::Unit.light() && !WorkerCommand::Check.light() && WorkerCommand::Format.light());
    Ok(())
}

#[test]
fn record_request_reply_sets_are_closed() -> Result<(), String> {
    let all = messages();
    let prepare = all[0].clone();
    let evaluate = all[3].clone();
    for index in [1, 2, 5, 6] {
        accept_record_reply(&prepare, all[index].clone())?;
    }
    for index in [4, 5, 6] {
        accept_record_reply(&evaluate, all[index].clone())?;
    }
    let mismatch = "compiler worker record reply does not match its request";
    assert_eq!(accept_record_reply(&prepare, all[4].clone()).err().as_deref(), Some(mismatch));
    assert_eq!(accept_record_reply(&evaluate, all[1].clone()).err().as_deref(), Some(mismatch));
    assert_eq!(accept_record_reply(&all[1], all[1].clone()).err().as_deref(), Some(mismatch));
    let wrong = format!("{{\"generation\":\"{JSON_GEN}\",\"kind\":\"prepare\",\"schema\":\"incan.engine/1\"}}");
    assert!(decode_record_message(wrong.as_bytes(), JSON_GEN).is_err());
    assert_eq!(RECORD_SCHEMA, "incan.compiler-worker/2");
    let encoded = encode_record_message(&all[1], JSON_GEN);
    assert_eq!(decode_record_message(&encoded, JSON_GEN)?, all[1]);
    Ok(())
}
