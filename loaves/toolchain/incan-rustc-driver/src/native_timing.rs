//! Opt-in boundaries around fresh rustc setup, Incan injection, queries and teardown.
//!
//! Timing records never reach the compiled program's console: they go where `OVEN_TRACE` sends Oven's own reasoning
//! (appended to the file it names, or stderr for `-`), and nowhere when it is unset. rustc prints its phase records
//! with `eprintln!`, so a timed rustc run redirects stderr and routes only the phase records; every other
//! captured line still reaches stderr.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The prefix rustc gives its JSON phase records.
const RUSTC_RECORD: &[u8] = b"time: ";

/// Mark complete frontend work observation for one semantic request. Frontend-only routes execute no native
/// lowering; compile routes observe it only when the existing native diagnostic policy enables its Incan sink.
pub(crate) fn observe_frontend(lowers: bool) {
    tracing::trace!(target: "incan::invocation", schema = 1,
        lowering_observed = !lowers || requested(), "compiler work observations");
}

/// Observe the pinned collector only when rustc requests it. Forcing this query early changes metadata construction
/// order, so diagnostic collection wraps the existing provider instead.
pub(crate) fn provide_codegen(providers: &mut rustc_middle::util::Providers) {
    if requested() {
        providers.queries.collect_and_partition_mono_items = observed_partitions;
    }
}

/// Preserve the default collector's inputs, result, and execution order while counting its actual partitions.
fn observed_partitions<'tcx>(
    tcx: rustc_middle::ty::TyCtxt<'tcx>,
    input: (),
) -> rustc_middle::mono::MonoItemPartitions<'tcx> {
    let partitions = (rustc_interface::DEFAULT_QUERY_PROVIDERS
        .queries
        .collect_and_partition_mono_items)(tcx, input);
    let native = tcx.sess.opts.output_types.should_codegen() && !tcx.sess.opts.unstable_opts.no_codegen;
    let execution = if !native {
        "none"
    } else if tcx.sess.opts.incremental.is_some() {
        "unavailable_incremental"
    } else {
        "all_on_success"
    };
    for unit in partitions.codegen_units {
        let record = serde_json::json!({
            "phase": "codegen_unit",
            "unit": unit.name().to_string(),
            "items": unit.items().len(),
            "size_estimate": unit.size_estimate(),
            "execution": execution,
            "seconds": null,
            "timing": "reported only for aggregate LLVM passes",
            "reads": "this unit's partitioned monomorphized MIR and native layouts",
            "writes": "this unit's native object",
        });
        write_trace(format!("native-phase: {record}\n").as_bytes());
    }
    write_trace(
        format!(
            "redo-codegen: {}\n",
            serde_json::json!({"partitions": partitions.codegen_units.len(), "execution": execution})
        )
        .as_bytes(),
    );
    partitions
}

/// A disabled timer never reads a clock; enabled records go to the trace sink.
pub(crate) struct Timing(Option<Clock>);

/// The invocation origin and previous boundary, retained only for a diagnostic invocation.
struct Clock {
    started: Instant,
    previous: Instant,
}

impl Timing {
    /// Start observing only when the existing native phase diagnostic has been requested.
    pub(crate) fn requested() -> Self {
        Self(requested().then(|| {
            let now = Instant::now();
            Clock {
                started: now,
                previous: now,
            }
        }))
    }

    /// Preserve rustc's ordinary compilation policy while enabling its pinned JSON phase format.
    pub(crate) fn configure(&self, config: &mut rustc_interface::interface::Config) {
        if self.0.is_some() {
            config.opts.unstable_opts.time_passes = true;
            config.opts.unstable_opts.time_passes_format = rustc_data_structures::profiling::TimePassesFormat::Json;
        }
    }

    /// Record a named boundary in seconds; durations include preceding diagnostic writes and must not be summed
    /// with nested rustc phases. Fixed internal names keep the emitted JSON independent of source contents.
    pub(crate) fn mark(&mut self, boundary: &'static str) {
        let Some(clock) = &mut self.0 else {
            return;
        };
        let now = Instant::now();
        let ended_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|value| value.as_nanos());
        let record = serde_json::json!({
            "boundary": boundary,
            "elapsed": now.duration_since(clock.started).as_secs_f64(),
            "interval": now.duration_since(clock.previous).as_secs_f64(),
            "ended_ns": ended_ns,
        });
        write_trace(format!("native-time: {record}\n").as_bytes());
        clock.previous = now;
    }
}

/// Whether this invocation asked for native phase timing.
fn requested() -> bool {
    std::env::var_os("INCAN_NATIVE_TIMING").is_some()
}

/// Append timing records to the trace sink; a sink that cannot be written loses the records, never the compilation.
fn write_trace(records: &[u8]) {
    if records.is_empty() {
        return;
    }
    match std::env::var("OVEN_TRACE").as_deref() {
        Ok("-") => {
            let _ = std::io::stderr().write_all(records);
        }
        Ok(path) if !path.is_empty() => {
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                let _ = file.write_all(records);
            }
        }
        _ => {}
    }
}

/// Record the rustc front end after it has consumed the injected plan and native metadata and completed analysis.
pub(crate) fn record_front_end(elapsed: Duration) {
    write_phase(
        "front_end",
        elapsed.as_secs_f64(),
        "validated plan, source coordinates, native crate metadata",
        "analyzed MIR and monomorphization roots",
    );
}

/// Write one stable native phase record beside the pinned rustc's version-specific phase diagnostics.
fn write_phase(phase: &str, seconds: f64, reads: &str, writes: &str) {
    let record = serde_json::json!({
        "phase": phase,
        "seconds": seconds,
        "reads": reads,
        "writes": writes,
    });
    write_trace(format!("native-phase: {record}\n").as_bytes());
}

/// The stderr of one timed rustc run, held in an anonymous file while it runs and routed when it ends. rustc's copy
/// of the standard library prints with its own console state, so only the descriptor itself can be redirected.
pub(crate) struct Capture(Option<Held>);

/// The original stderr descriptor and the anonymous file standing in for it.
struct Held {
    original: OwnedFd,
    file: File,
}

impl Capture {
    /// Point stderr at an anonymous file when timing was requested; any failure leaves stderr as it is.
    pub(crate) fn begin() -> Self {
        if !requested() {
            return Self(None);
        }
        Self(hold().ok())
    }

    /// Restore stderr and summarize code generation and linking after rustc returns. The elapsed interval starts
    /// after analysis; the pinned rustc's outer `link` record separates the linker from the preceding codegen wall.
    pub(crate) fn finish(mut self, codegen_link: Option<Duration>) {
        self.route(codegen_link);
    }

    /// Route captured diagnostics once, preserving non-timing stderr and adding stable native phase records.
    fn route(&mut self, codegen_link: Option<Duration>) {
        let Some(mut held) = self.0.take() else {
            return;
        };
        let _ = std::io::stderr().flush();
        // SAFETY: both descriptors are open: the saved original stderr and the process's stderr slot.
        unsafe { libc::dup2(held.original.as_raw_fd(), libc::STDERR_FILENO) };
        let mut captured = Vec::new();
        if held.file.seek(SeekFrom::Start(0)).is_err() || held.file.read_to_end(&mut captured).is_err() {
            return;
        }
        let mut records = Vec::new();
        let mut console = Vec::new();
        let mut link_seconds = None;
        let mut llvm_pass_seconds = None;
        for line in captured.split_inclusive(|byte| *byte == b'\n') {
            if line.starts_with(RUSTC_RECORD) {
                records.extend_from_slice(line);
                observe_rustc_phase(line, &mut link_seconds, &mut llvm_pass_seconds);
            } else {
                console.extend_from_slice(line);
            }
        }
        write_trace(&records);
        record_codegen(codegen_link, link_seconds, llvm_pass_seconds);
        let mut stderr = std::io::stderr();
        let _ = stderr.write_all(&console);
        let _ = stderr.flush();
    }
}

/// Retain only the pinned phase values needed for the stable E6 split; malformed diagnostics remain raw records.
fn observe_rustc_phase(line: &[u8], link_seconds: &mut Option<f64>, llvm_pass_seconds: &mut Option<f64>) {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&line[RUSTC_RECORD.len()..]) else {
        return;
    };
    let (Some(pass), Some(seconds)) = (
        value.get("pass").and_then(serde_json::Value::as_str),
        value.get("time").and_then(serde_json::Value::as_f64),
    ) else {
        return;
    };
    match pass {
        "LLVM_passes" => *llvm_pass_seconds = Some(seconds),
        "link" => *link_seconds = Some(seconds),
        _ => {}
    }
}

/// Separate the post-analysis wall into aggregate codegen and link without misattributing rustc's aggregate LLVM
/// pass duration to an individual CGU. The collector records each actual CGU independently.
fn record_codegen(codegen_link: Option<Duration>, link_seconds: Option<f64>, llvm_pass_seconds: Option<f64>) {
    let Some(link) = link_seconds else {
        return;
    };
    if let Some(total) = codegen_link {
        write_phase(
            "codegen",
            (total.as_secs_f64() - link).max(0.0),
            "partitioned monomorphized MIR and native layouts",
            "native objects",
        );
    }
    if let Some(seconds) = llvm_pass_seconds {
        write_phase(
            "llvm_passes",
            seconds,
            "all generated LLVM modules",
            "optimized native object contents",
        );
    }
    write_phase(
        "link",
        link,
        "native objects, external libraries, and linker arguments",
        "final native artifact and profile-selected debug companion",
    );
}

/// Redirect stderr into a new anonymous file, keeping the original descriptor.
fn hold() -> std::io::Result<Held> {
    let path = std::env::temp_dir().join(format!("incan-timing-{}-{:?}", std::process::id(), Instant::now()));
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(&path)?;
    std::fs::remove_file(&path)?;
    std::io::stderr().flush()?;
    // SAFETY: dup borrows the live stderr descriptor and returns a new descriptor that OwnedFd closes once.
    let copied = unsafe { libc::dup(libc::STDERR_FILENO) };
    if copied < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the successful dup above returned this unique descriptor.
    let original = unsafe { OwnedFd::from_raw_fd(copied) };
    // SAFETY: both descriptors are open: the anonymous file and the process's stderr.
    if unsafe { libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(Held { original, file })
}

impl Drop for Capture {
    /// Restore stderr; rustc's phase records go to the trace and every other line, in order, to stderr.
    fn drop(&mut self) {
        self.route(None);
    }
}
