//! JSON numbers outside the Incan int range keep the standard library's outcomes: an integer above the int range is
//! refused with its `ValueError` text wherever it appears in a document, the first in key order; integers beyond
//! the unsigned range and negative ones below the int range are floats, so they are no valid int.
//! Expected outcomes are the reference worker host's (`run1/golden/worker-integers-1.log`, `runtime-worker-ref-11`).

use crate::protocol::{Admission, admit, read_completion, read_ready};
use crate::record_protocol::decode_record_message;
use crate::record_snapshot::{digest, read_snapshot_record};

const GEN: &str = "0123abcd";
const SCHEMA: &str = "\"schema\":\"incan.compiler-worker/1\"";
const RECORD: &str = "\"generation\":\"0123abcd\",\"schema\":\"incan.compiler-worker/2\"";
const NO_CODE: &str = "compiler worker returned no valid completion code";

fn too_large(number: &str) -> String {
    format!("ValueError: JSON integer `{number}` does not fit in Incan int")
}

fn completion(fields: &str) -> Result<i64, String> {
    read_completion(format!("{{{fields},{SCHEMA}}}").as_bytes(), GEN).map(|found| found.status)
}

#[test]
fn completions_keep_the_standard_number_outcomes() {
    assert_eq!(
        completion("\"exit\":9223372036854775808"),
        Err(too_large("9223372036854775808"))
    );
    assert_eq!(
        completion("\"exit\":18446744073709551615"),
        Err(too_large("18446744073709551615"))
    );
    assert_eq!(completion("\"exit\":18446744073709551616"), Err(NO_CODE.to_owned()));
    assert_eq!(completion("\"exit\":-9223372036854775809"), Err(NO_CODE.to_owned()));
    assert_eq!(completion("\"exit\":1.5"), Err(NO_CODE.to_owned()));
    assert_eq!(
        completion("\"exit\":1e400"),
        Err("JSONDecodeError: number out of range at line 1 column 13".to_owned())
    );
    assert_eq!(
        completion("\"exit\":0,\"x\":[1,9223372036854775808]"),
        Err(too_large("9223372036854775808"))
    );
    assert_eq!(
        completion("\"a\":9223372036854775809,\"b\":9223372036854775808,\"exit\":0"),
        Err(too_large("9223372036854775809"))
    );
    assert_eq!(
        completion("\"exit\":2,\"z\":{\"y\":12345678901234567890}"),
        Err(too_large("12345678901234567890"))
    );
    assert_eq!(completion("\"exit\":-0"), Err(NO_CODE.to_owned()));
    assert_eq!(completion("\"exit\":3.0"), Err(NO_CODE.to_owned()));
}

#[test]
fn records_requests_readiness_and_descriptors_refuse_integers_beyond_int() {
    let need =
        format!("{{\"earlier_rank\":9223372036854775808,\"keys_root\":\"k\",\"kind\":\"need_prerequisite\",{RECORD}}}");
    assert_eq!(
        decode_record_message(need.as_bytes(), GEN).err(),
        Some(too_large("9223372036854775808"))
    );
    let evaluate = format!(
        "{{\"cancellation_generation\":18446744073709551615,\"kind\":\"evaluate\",\"prepared_root\":\"p\",\
         \"reservation\":\"r\",\"snapshot\":\"s\",\"work_root\":\"w\",{RECORD}}}"
    );
    assert_eq!(
        decode_record_message(evaluate.as_bytes(), GEN).err(),
        Some(too_large("18446744073709551615"))
    );
    let request = "{\"arguments\":[\"--check\",\"/u\",\"/s\"],\"cwd\":\"/p\",\"generation\":\"0123abcd\",\
                   \"n\":9223372036854775808,\"schema\":\"incan.compiler-worker/1\",\"stderr\":\"/e\",\"stdout\":\"/o\"}";
    match admit(request.as_bytes(), GEN) {
        Admission::Refused(reason) => assert_eq!(reason, too_large("9223372036854775808")),
        _ => panic!("a request with an integer beyond int was not refused"),
    }
    let ready = "{\"generation\":\"0123abcd\",\"pid\":9223372036854775808,\"schema\":\"incan.compiler-worker/1\",\
                 \"status\":\"ready\"}";
    assert_eq!(
        read_ready(ready.as_bytes(), GEN, 7).err(),
        Some(too_large("9223372036854775808"))
    );
    let descriptor = "{\"allowed_roots\":[],\"format_profile\":\"p\",\"generation\":9223372036854775808,\
                      \"lease\":\"l\",\"manifest\":\"m\",\"schema\":\"incan.compiler-snapshot/1\",\"store_id\":\"s\",\
                      \"view\":\"v\",\"view_revision\":\"r\"}";
    assert_eq!(
        read_snapshot_record(descriptor.as_bytes(), &digest(descriptor.as_bytes())).err(),
        Some(too_large("9223372036854775808"))
    );
}
