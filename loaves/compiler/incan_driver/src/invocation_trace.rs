//! Opt-in parent-linked invocation records, shared by the compiler's executables.
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
