//! Producer attempts on the engine trace channel, recorded as the platform's `enter_producer` and `finish_producer`
//! record them.
//!
//! With `INCAN_TRACE_PRODUCERS` set and `INCAN_ENGINE_TRACE` naming a directory, an attempt appends a
//! `producer_attempt` span to `engine-producers-<pid>.jsonl` there before the work runs, and its result a
//! `producer_result` span under the same span number once the work's disposition is known. Both carry the address
//! `<family>\t<identity>\t<detail>` and the shared raw host clock. Otherwise nothing is read, timed or written.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

const SCHEMA: &str = "incan.engine.trace/1";
/// The largest text field, in UTF-8 bytes, a trace record admits.
const MAX_TEXT: usize = 4096;
/// The largest encoded record, newline included.
const MAX_RECORD: usize = 65536;

/// This process's span sequence and lifetime identity.
struct Origin {
    sequence: i64,
    identity: String,
}

static ORIGIN: Mutex<Origin> = Mutex::new(Origin {
    sequence: 0,
    identity: String::new(),
});

impl Origin {
    /// Read host nanoseconds; a failed or out-of-range clock refuses an observation with -1.
    fn now(&mut self) -> i64 {
        let Some(started) = incan_platform::raw_uptime_ns()
            .ok()
            .and_then(|value| i64::try_from(value).ok())
        else {
            return -1;
        };
        if self.identity.is_empty() {
            self.identity = match SystemTime::now().duration_since(UNIX_EPOCH) {
                Ok(epoch) => format!("{}-{}", std::process::id(), epoch.as_nanos()),
                Err(_) => format!("{}-{started}", std::process::id()),
            };
        }
        started
    }
}

/// One observed execution: its span on this process's clock, its channel and its exact producer address.
pub struct ProducerAttempt {
    span: i64,
    start_ns: i64,
    origin: String,
    channel: PathBuf,
    producer: String,
    address: String,
}

/// The flat wire projection of a completed span, in the platform's field order.
#[derive(Serialize)]
struct TraceRecord<'a> {
    schema: &'a str,
    kind: &'a str,
    run: &'a str,
    request: &'a str,
    work: &'a str,
    producer: &'a str,
    operation: &'a str,
    key: &'a str,
    snapshot: &'a str,
    thread: &'a str,
    outcome: &'a str,
    clock: &'a str,
    clock_id: &'a str,
    span: i64,
    parent: i64,
    process: i64,
    start_ns: i64,
    end_ns: i64,
    bytes: i64,
    records: i64,
    calls: i64,
    cpu_ns: Option<i64>,
}

impl ProducerAttempt {
    /// Open an attempt on a trace directory's producer channel and record its entrance.
    fn entered(directory: &Path, actor: &str, family: &str, identity: &str, detail: &str) -> Option<Self> {
        let (span, start_ns, origin) = {
            let mut origin = ORIGIN.lock().ok()?;
            let started = origin.now();
            if started < 0 {
                return None;
            }
            origin.sequence += 1;
            (origin.sequence, started, origin.identity.clone())
        };
        let attempt = ProducerAttempt {
            span,
            start_ns,
            origin,
            channel: directory.join(format!("engine-producers-{}.jsonl", std::process::id())),
            producer: actor.to_owned(),
            address: format!("{family}\t{identity}\t{detail}"),
        };
        attempt.event("producer_attempt", "entered");
        Some(attempt)
    }

    /// Append one completed span; a refused or unwritable record loses the observation, never the work.
    fn event(&self, kind: &str, outcome: &str) {
        let Some(ended) = ORIGIN.lock().ok().map(|mut origin| origin.now()) else {
            return;
        };
        let process = std::process::id();
        let work = format!("{process}:{}", self.span);
        let thread = format!("{:?}", std::thread::current().id());
        let record = TraceRecord {
            schema: SCHEMA,
            kind: "span",
            run: &self.origin,
            request: "",
            work: &work,
            producer: &self.producer,
            operation: kind,
            key: &self.address,
            snapshot: "",
            thread: &thread,
            outcome,
            clock: "host-monotonic-raw",
            clock_id: &self.origin,
            span: self.span,
            parent: 0,
            process: i64::from(process),
            start_ns: self.start_ns,
            end_ns: ended,
            bytes: 0,
            records: 0,
            calls: 0,
            cpu_ns: None,
        };
        if let Some(line) = encoded(&record) {
            append(&self.channel, &line);
        }
    }
}

/// Validate and encode a span as the platform's bounded wire record, newline included.
fn encoded(record: &TraceRecord<'_>) -> Option<Vec<u8>> {
    if record.span <= 0 || record.start_ns < 0 || record.end_ns < record.start_ns {
        return None;
    }
    let texts = [
        record.run,
        record.request,
        record.work,
        record.producer,
        record.operation,
        record.key,
        record.snapshot,
        record.thread,
        record.outcome,
        record.clock_id,
    ];
    if texts.iter().any(|text| text.len() > MAX_TEXT) {
        return None;
    }
    let mut line = serde_json::to_vec(record).ok()?;
    line.push(b'\n');
    (line.len() <= MAX_RECORD).then_some(line)
}

/// Append one whole record to its channel.
fn append(channel: &Path, line: &[u8]) {
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(channel) {
        let _ = file.write_all(line);
    }
}

/// Whether a switch names a nonempty value; absence creates no clock, event or sink.
fn enabled(variable: &str) -> bool {
    std::env::var_os(variable).is_some_and(|value| !value.is_empty())
}

/// Record the physical call before executing it, when the request explicitly asks for a producer trace.
pub fn enter_producer(actor: &str, family: &str, identity: &str, detail: &str) -> Option<ProducerAttempt> {
    if !enabled("INCAN_TRACE_PRODUCERS") {
        return None;
    }
    let directory = std::env::var("INCAN_ENGINE_TRACE").ok()?;
    if directory.is_empty() {
        return None;
    }
    ProducerAttempt::entered(Path::new(&directory), actor, family, identity, detail)
}

/// Close the same measured operation only once its actual disposition is known.
pub fn finish_producer(attempt: Option<ProducerAttempt>, outcome: &str) {
    if let Some(started) = attempt {
        started.event("producer_result", outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::{ProducerAttempt, finish_producer};

    /// Every field of one written span after the text before it, so the platform's field order is checked too.
    fn fields_in_order(line: &str, fields: &[&str]) -> bool {
        let mut rest = line;
        for field in fields {
            match rest.find(field) {
                Some(at) => rest = &rest[at + field.len()..],
                None => return false,
            }
        }
        true
    }

    #[test]
    fn an_attempt_and_its_result_share_one_span_on_the_producer_channel() {
        let directory = std::env::temp_dir().join(format!("incan-producer-trace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("trace directory");
        let attempt = ProducerAttempt::entered(&directory, "compiler", "plan-function", "pub::app::main", "body:main");
        let span = attempt.as_ref().map(|started| started.span).expect("a clock");
        finish_producer(attempt, "produced");
        let channel = directory.join(format!("engine-producers-{}.jsonl", std::process::id()));
        let text = std::fs::read_to_string(&channel).expect("producer channel");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let process = std::process::id();
        for (line, operation, outcome) in [
            (lines[0], "producer_attempt", "entered"),
            (lines[1], "producer_result", "produced"),
        ] {
            let work = format!("\"work\":\"{process}:{span}\"");
            let operation = format!("\"operation\":\"{operation}\"");
            let outcome = format!("\"outcome\":\"{outcome}\"");
            let numbers = format!("\"span\":{span},\"parent\":0,\"process\":{process},\"start_ns\":");
            assert!(
                fields_in_order(
                    line,
                    &[
                        "{\"schema\":\"incan.engine.trace/1\",\"kind\":\"span\",\"run\":\"",
                        "\"request\":\"\",",
                        &work,
                        ",\"producer\":\"compiler\",",
                        &operation,
                        ",\"key\":\"plan-function\\tpub::app::main\\tbody:main\",\"snapshot\":\"\",\"thread\":\"ThreadId(",
                        &outcome,
                        ",\"clock\":\"host-monotonic-raw\",\"clock_id\":\"",
                        &numbers,
                        ",\"bytes\":0,\"records\":0,\"calls\":0,\"cpu_ns\":null}",
                    ],
                ),
                "{line}"
            );
        }
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn an_unobserved_request_records_nothing() {
        finish_producer(None, "produced");
        assert!(
            std::env::var_os("INCAN_TRACE_PRODUCERS").is_some()
                || super::enter_producer("compiler", "x", "y", "z").is_none()
        );
    }
}
