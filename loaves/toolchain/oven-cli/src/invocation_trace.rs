//! Opt-in parent-linked invocation records, shared by both compiler CLI entrypoints.
//!
//! `INCAN_TRACE_DIR` enables the existing tracing registry's diagnostic target. Other logs retain their normal
//! `RUST_LOG` filter. A disabled diagnostic target does not evaluate span fields or create an output sink. Records
//! describe instrumented boundaries, not every machine instruction or uninstrumented provider function.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id};
use tracing::{Event, Subscriber};
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// The single target reserved for invocation diagnostics rather than application logging.
const TARGET: &str = "incan::invocation";

/// Install ordinary logging and, only when requested, the invocation layer.
///
/// A requested sink must open successfully before execution. Existing trace files are never overwritten. Each
/// process writes its own JSONL file; supported launch boundaries pass `INCAN_TRACE_PARENT` to link child roots.
pub fn initialize() -> Result<(), Box<dyn std::error::Error>> {
    let diagnostic = std::env::var_os("INCAN_TRACE_DIR")
        .filter(|path| !path.is_empty())
        .map(|path| TraceLayer::open(Path::new(&path), std::env::var("INCAN_TRACE_PARENT").ok()))
        .transpose()?;
    subscriber(
        diagnostic,
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
    )
    .try_init()?;
    Ok(())
}

/// Compose one registry, keeping the diagnostic target disabled even if ordinary logging requests `trace`.
fn subscriber(diagnostic: Option<TraceLayer>, log_filter: EnvFilter) -> impl Subscriber + Send + Sync {
    let diagnostics_enabled = diagnostic.is_some();
    let logs = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(log_filter)
        .with_filter(filter_fn(|metadata| metadata.target() != TARGET));
    tracing_subscriber::registry()
        .with(filter_fn(move |metadata| {
            metadata.target() != TARGET || diagnostics_enabled
        }))
        .with(logs)
        .with(diagnostic.map(|layer| layer.with_filter(filter_fn(|metadata| metadata.target() == TARGET))))
}

#[cfg(test)]
mod tests {
    use super::{EnvFilter, TraceLayer, subscriber};
    use serde_json::Value;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Read every complete JSONL record after a scoped registry has released its sink.
    fn records(directory: &Path) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
        let mut result = Vec::new();
        for entry in std::fs::read_dir(directory)? {
            let text = std::fs::read_to_string(entry?.path())?;
            for line in text.lines() {
                result.push(serde_json::from_str(line)?);
            }
        }
        Ok(result)
    }

    #[test]
    fn disabled_trace_does_not_construct_fields_or_create_output() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let fields = AtomicUsize::new(0);
        tracing::subscriber::with_default(subscriber(None, EnvFilter::new("trace")), || {
            let _span = tracing::trace_span!(target: "incan::invocation", "disabled_probe", input = fields.fetch_add(1, Ordering::Relaxed)).entered();
            tracing::trace!(target: "incan::invocation", work = fields.fetch_add(1, Ordering::Relaxed));
        });
        assert_eq!(fields.load(Ordering::Relaxed), 0);
        assert!(records(directory.path())?.is_empty());
        Ok(())
    }

    #[test]
    fn enabled_trace_preserves_nested_calls_fields_and_decisions() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let layer = TraceLayer::open(directory.path(), None)?;
        tracing::subscriber::with_default(subscriber(Some(layer), EnvFilter::new("warn")), || {
            let _root = tracing::trace_span!(target: "incan::invocation", "root", input = "same-input").entered();
            for _ in 0..2 {
                let _child = tracing::trace_span!(target: "incan::invocation", "child", input = "same-input", caller_file = file!(), caller_line = u64::from(line!())).entered();
                tracing::trace!(target: "incan::invocation", decision = "reuse", compiled = false);
            }
        });
        let records = records(directory.path())?;
        let root = records
            .iter()
            .find(|record| record["function"] == "root")
            .ok_or("missing root")?;
        let children: Vec<_> = records.iter().filter(|record| record["function"] == "child").collect();
        assert_eq!(children.len(), 2);
        assert_ne!(children[0]["id"], children[1]["id"]);
        for child in children {
            assert_eq!(child["parent"], root["id"]);
            assert_eq!(child["fields"]["input"], "same-input");
            assert!(
                child["file"]
                    .as_str()
                    .is_some_and(|value| value.ends_with("invocation_trace.rs"))
            );
            assert!(child["fields"]["caller_line"].as_u64().is_some_and(|line| line > 0));
            assert!(records.iter().any(|end| end["kind"] == "end"
                && end["id"] == child["id"]
                && end["time_ns"].as_u64() >= child["time_ns"].as_u64()));
            assert!(records.iter().any(|event| event["kind"] == "event"
                && event["parent"] == child["id"]
                && event["fields"]["decision"] == "reuse"
                && event["fields"]["compiled"] == false));
        }
        Ok(())
    }

    #[test]
    fn inherited_parent_links_process_roots_without_overwriting_traces() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let original = directory.path().join("retained.jsonl");
        std::fs::write(&original, "{\"kind\":\"retained\"}\n")?;
        let layer = TraceLayer::open(directory.path(), Some("123:456".to_string()))?;
        tracing::subscriber::with_default(subscriber(Some(layer), EnvFilter::new("warn")), || {
            let _root = tracing::trace_span!(target: "incan::invocation", "child_process").entered();
        });
        let records = records(directory.path())?;
        assert_eq!(std::fs::read_to_string(original)?, "{\"kind\":\"retained\"}\n");
        assert_eq!(
            records
                .iter()
                .find(|record| record["function"] == "child_process")
                .ok_or("missing child root")?["parent"],
            "123:456"
        );
        Ok(())
    }

    /// Spawned only by the process-linkage control; other test runs leave global logging untouched.
    #[test]
    fn subprocess_trace_child() -> Result<(), Box<dyn std::error::Error>> {
        if std::env::var_os("INCAN_INVOCATION_TEST_CHILD").is_none() {
            return Ok(());
        }
        super::initialize()?;
        let _invocation = tracing::trace_span!(target: "incan::invocation", "trace_child_entrypoint").entered();
        Ok(())
    }

    #[test]
    fn bounded_subprocess_links_actual_child_trace_root() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let layer = TraceLayer::open(directory.path(), None)?;
        tracing::subscriber::with_default(
            subscriber(Some(layer), EnvFilter::new("warn")),
            || -> Result<(), Box<dyn std::error::Error>> {
                let _root = tracing::trace_span!(target: "incan::invocation", "trace_parent_entrypoint").entered();
                let mut command = std::process::Command::new(std::env::current_exe()?);
                command
                    .args([
                        "--exact",
                        "invocation_trace::tests::subprocess_trace_child",
                        "--nocapture",
                    ])
                    .env("INCAN_INVOCATION_TEST_CHILD", "1")
                    .env("INCAN_TRACE_DIR", directory.path());
                let output = oven_store::process::run_bounded_process(
                    &mut command,
                    oven_store::process::BoundedProcessLimits {
                        stdout_bytes: 64 * 1024,
                        stderr_bytes: 64 * 1024,
                        timeout: Some(std::time::Duration::from_secs(15)),
                    },
                    None,
                )?;
                assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
                Ok(())
            },
        )?;
        let records = records(directory.path())?;
        let launch = records
            .iter()
            .find(|r| r["function"] == "oven_store::run_bounded_process")
            .ok_or("missing launch span")?;
        let child = records
            .iter()
            .find(|r| r["function"] == "trace_child_entrypoint")
            .ok_or("missing child root")?;
        assert_ne!(launch["pid"], child["pid"]);
        assert_eq!(child["parent"], launch["id"]);
        assert!(
            records.iter().any(|r| r["kind"] == "event"
                && r["parent"] == launch["id"]
                && r["fields"]["child_pid"] == child["pid"])
        );
        Ok(())
    }

    #[test]
    fn concurrent_threads_keep_complete_records_and_distinct_span_ids() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let layer = TraceLayer::open(directory.path(), None)?;
        let dispatch = Arc::new(tracing::Dispatch::new(subscriber(Some(layer), EnvFilter::new("warn"))));
        let mut workers = Vec::new();
        for worker in 0..4 {
            let dispatch = Arc::clone(&dispatch);
            workers.push(std::thread::spawn(move || {
                tracing::dispatcher::with_default(&dispatch, || {
                    for input in 0..16 {
                        let _span =
                            tracing::trace_span!(target: "incan::invocation", "parallel", worker, input).entered();
                    }
                });
            }));
        }
        for worker in workers {
            worker.join().map_err(|_| "trace worker panicked")?;
        }
        tracing::dispatcher::with_default(&dispatch, || {
            for input in 0..8192 {
                let _span = tracing::trace_span!(target: "incan::invocation", "registry_slot_reuse", input).entered();
            }
        });
        drop(dispatch);
        let records = records(directory.path())?;
        let ids: std::collections::BTreeSet<_> = records
            .iter()
            .filter(|record| record["kind"] == "begin")
            .filter_map(|record| record["id"].as_str())
            .collect();
        assert_eq!(ids.len(), 64 + 8192);
        assert_eq!(
            records.iter().filter(|record| record["kind"] == "end").count(),
            64 + 8192
        );
        Ok(())
    }
}

/// A lifetime identity independent of reusable tracing registry slots.
struct InvocationId(String);

/// One process's append-only diagnostic sink and clock origin.
struct TraceLayer {
    output: Mutex<File>,
    started: Instant,
    inherited_parent: Option<String>,
    write_failed: AtomicBool,
    next_id: AtomicU64,
}

impl TraceLayer {
    /// Create a unique process sink and record its cross-process clock and linkage facts.
    fn open(directory: &Path, inherited_parent: Option<String>) -> io::Result<Self> {
        fs::create_dir_all(directory)?;
        let started = Instant::now();
        let epoch = SystemTime::now().duration_since(UNIX_EPOCH).map_err(io::Error::other)?;
        let path = directory.join(format!("{}-{}.jsonl", std::process::id(), epoch.as_nanos()));
        let output = OpenOptions::new().write(true).create_new(true).open(path)?;
        let layer = Self {
            output: Mutex::new(output),
            started,
            inherited_parent,
            write_failed: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
        };
        layer.write(json!({
            "schema": "incan.invocations/1", "kind": "process", "pid": std::process::id(),
            "epoch_ns": epoch.as_nanos(), "parent": layer.inherited_parent,
            "executable": std::env::current_exe().ok(),
        }));
        Ok(layer)
    }

    /// Persist one complete line immediately, including before an existing command calls `process::exit`.
    /// A failed diagnostic write is reported once; it cannot change compilation or hide behind a successful trace.
    fn write(&self, mut record: Value) {
        record["pid"] = json!(std::process::id());
        record["thread"] = json!(format!("{:?}", std::thread::current().id()));
        record["time_ns"] = json!(self.started.elapsed().as_nanos());
        let result = (|| -> io::Result<()> {
            let mut bytes = serde_json::to_vec(&record)?;
            bytes.push(b'\n');
            self.output
                .lock()
                .map_err(|_| io::Error::other("invocation sink lock poisoned"))?
                .write_all(&bytes)
        })();
        if let Err(error) = result
            && !self.write_failed.swap(true, Ordering::Relaxed)
        {
            eprintln!("invocation trace incomplete: {error}");
        }
    }
}

/// Preserve typed diagnostic fields without formatting the whole event into an ambiguous log line.
#[derive(Default)]
struct Fields(BTreeMap<String, Value>);

impl Visit for Fields {
    /// Retain string identity and decision fields without additional debug quoting.
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), json!(value));
    }

    /// Retain actual work counts and source line numbers as numbers.
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_string(), json!(value));
    }

    /// Retain signed values without converting them into strings.
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_string(), json!(value));
    }

    /// Retain boolean decisions independently of their display representation.
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_string(), json!(value));
    }

    /// Render other instrumented values only after the callsite has been enabled.
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_string(), json!(format!("{value:?}")));
    }
}

impl<S> Layer<S> for TraceLayer
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    /// Record each invocation's actual registry parent, instrumented source and lazily evaluated input fields.
    fn on_new_span(&self, attributes: &Attributes<'_>, id: &Id, context: Context<'_, S>) {
        let Some(span) = context.span(id) else { return };
        let mut fields = Fields::default();
        attributes.record(&mut fields);
        let invocation = fields
            .0
            .get("invocation_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "{}:span:{}",
                    std::process::id(),
                    self.next_id.fetch_add(1, Ordering::Relaxed)
                )
            });
        let parent = span
            .parent()
            .and_then(|parent| {
                parent
                    .scope()
                    .find_map(|parent| parent.extensions().get::<InvocationId>().map(|id| id.0.clone()))
            })
            .or_else(|| self.inherited_parent.clone());
        span.extensions_mut().insert(InvocationId(invocation.clone()));
        let metadata = attributes.metadata();
        self.write(json!({
            "kind": "begin", "id": invocation, "parent": parent,
            "function": metadata.name(), "file": metadata.file(), "line": metadata.line(), "fields": fields.0,
        }));
    }

    /// Close an invocation by its original ID; unfinished invocations remain detectable in the retained stream.
    fn on_close(&self, id: Id, context: Context<'_, S>) {
        if let Some(span) = context.span(&id)
            && let Some(invocation) = span.extensions().get::<InvocationId>()
        {
            self.write(json!({"kind": "end", "id": invocation.0}));
        }
    }

    /// Attach actual reuse, miss and compilation decisions to the currently executing invocation.
    fn on_event(&self, event: &Event<'_>, context: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let parent = context.event_span(event).and_then(|span| {
            span.scope()
                .find_map(|span| span.extensions().get::<InvocationId>().map(|id| id.0.clone()))
        });
        self.write(json!({"kind": "event", "parent": parent, "file": event.metadata().file(), "line": event.metadata().line(), "fields": fields.0}));
    }
}
