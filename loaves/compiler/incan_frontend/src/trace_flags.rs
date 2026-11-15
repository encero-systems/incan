//! The compiler's side of the one tracing channel.
//!
//! `INCAN_ENGINE_TRACE` names a trace directory. A compiler process appends every record it traces, one JSON object
//! per line with an `operation` naming its category, to its one file there, `engine-<producer>-<pid>.jsonl`, the file
//! its engine spans also use. Tracing never writes the console: a record that cannot be written is lost, never the
//! compilation. Each category's former variable still selects its former destination for one release; Oven names the
//! replacement when it sees one.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// The variable that selects the channel's directory.
pub const CHANNEL: &str = "INCAN_ENGINE_TRACE";

/// What a compiler process traces, each a category of the channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    /// Instrumented compiler boundaries, formerly `INCAN_TRACE_DIR`.
    Invocation,
    /// Rust ABI fact extraction and reads, formerly `INCAN_TRACE_RUST_FACTS`.
    RustFacts,
    /// Checker memory at module boundaries, formerly `INCAN_TRACE_TYPECHECK_MEMORY`.
    TypecheckMemory,
    /// Lowering decisions, formerly `INCAN_TRACE_LOWERING`.
    Lowering,
    /// Memory at lowering boundaries, formerly `INCAN_TRACE_LOWERING_FOOTPRINT`.
    LoweringFootprint,
    /// Native phase timings, formerly sent where `OVEN_TRACE` pointed.
    NativeTiming,
    /// Vocabulary desugarer responses, formerly sent where `OVEN_TRACE` pointed.
    VocabRuntime,
    /// Repeated lowering requests, formerly sent where `OVEN_TRACE` pointed.
    Redo,
    /// Editor graph analysis phases, formerly `INCAN_TRACE_GRAPH`.
    LspGraph,
}

impl Category {
    /// The category's name in the channel's `operation` field.
    pub fn name(self) -> &'static str {
        match self {
            Self::Invocation => "invocation",
            Self::RustFacts => "rust_facts",
            Self::TypecheckMemory => "typecheck_memory",
            Self::Lowering => "lowering",
            Self::LoweringFootprint => "lowering_footprint",
            Self::NativeTiming => "native_timing",
            Self::VocabRuntime => "vocab_runtime",
            Self::Redo => "redo",
            Self::LspGraph => "lsp_graph",
        }
    }

    /// The variable that selected the category before the channel, read for one more release.
    pub fn former(self) -> &'static str {
        match self {
            Self::Invocation => "INCAN_TRACE_DIR",
            Self::RustFacts => "INCAN_TRACE_RUST_FACTS",
            Self::TypecheckMemory => "INCAN_TRACE_TYPECHECK_MEMORY",
            Self::Lowering => "INCAN_TRACE_LOWERING",
            Self::LoweringFootprint => "INCAN_TRACE_LOWERING_FOOTPRINT",
            Self::NativeTiming | Self::VocabRuntime | Self::Redo => "OVEN_TRACE",
            Self::LspGraph => "INCAN_TRACE_GRAPH",
        }
    }
}

/// The channel's directory, read once per process; `None` when tracing is off.
pub fn channel() -> Option<&'static Path> {
    static DIRECTORY: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIRECTORY
        .get_or_init(|| {
            std::env::var_os(CHANNEL)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .as_deref()
}

/// The name a process's records are filed under; the compiler unless a front end such as the editor sets another.
static PRODUCER: OnceLock<&'static str> = OnceLock::new();

/// Name this process's producer before its first record: `lsp` for the editor server.
pub fn set_producer(name: &'static str) {
    let _ = PRODUCER.set(name);
}

/// This process's one file in the channel.
pub fn channel_file() -> Option<PathBuf> {
    let producer = PRODUCER.get().copied().unwrap_or("compiler");
    channel().map(|directory| directory.join(format!("engine-{producer}-{}.jsonl", std::process::id())))
}

/// Whether `category` is traced: the channel is on, or the category's former variable is set.
pub fn enabled(category: Category) -> bool {
    channel().is_some() || former_destination(category).is_some()
}

/// Whether the channel or `INCAN_TRACE_RUST_FACTS` asks for Rust-facts records, at every Rust-facts trace site.
pub fn trace_rust_facts() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| enabled(Category::RustFacts))
}

/// Trace one line of `category`: into the channel when it is on, otherwise where the former variable sent it.
pub fn emit(category: Category, text: &str) {
    if let Some(path) = channel_file() {
        let end_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|time| time.as_nanos())
            .unwrap_or(0);
        let record = serde_json::json!({
            "schema": "incan.engine.trace/1", "kind": "line",
            "producer": PRODUCER.get().copied().unwrap_or("compiler"), "operation": category.name(),
            "key": text, "process": std::process::id(), "end_ns": end_ns,
        });
        if let Ok(mut line) = serde_json::to_vec(&record) {
            line.push(b'\n');
            append(&path, &line);
        }
        return;
    }
    match former_destination(category) {
        Some(Destination::Stderr) => eprintln!("{text}"),
        Some(Destination::Stdout) => println!("{text}"),
        Some(Destination::File(path)) => append(Path::new(&path), format!("{text}\n").as_bytes()),
        None => {}
    }
}

/// Append one whole line; a sink that cannot be written loses the line.
fn append(path: &Path, bytes: &[u8]) {
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(bytes);
    }
}

/// Where a category went before the channel.
enum Destination {
    Stderr,
    Stdout,
    File(String),
}

/// The former destination its variable selects, when it is set.
fn former_destination(category: Category) -> Option<Destination> {
    let value = std::env::var_os(category.former())?;
    match category {
        Category::NativeTiming | Category::VocabRuntime | Category::Redo => match value.to_str() {
            Some("-") => Some(Destination::Stderr),
            Some(path) if !path.is_empty() => Some(Destination::File(path.to_string())),
            _ => None,
        },
        Category::LoweringFootprint | Category::TypecheckMemory => Some(Destination::Stdout),
        Category::Invocation => None,
        _ => Some(Destination::Stderr),
    }
}

/// Trace one formatted line of a category, formatting it only when the category is traced.
#[macro_export]
macro_rules! trace_line {
    ($category:expr, $($format:tt)+) => {
        if $crate::trace_flags::enabled($category) {
            $crate::trace_flags::emit($category, &format!($($format)+));
        }
    };
}
