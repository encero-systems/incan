//! Opt-in boundaries around fresh rustc setup, Incan injection, queries and teardown.

use std::io::Write;
use std::time::Instant;

/// Mark complete frontend work observation for one semantic request. Frontend-only routes execute no native
/// lowering; compile routes observe it only when the existing native diagnostic policy enables its Incan sink.
pub(crate) fn observe_frontend(lowers: bool) {
    tracing::trace!(target: "incan::invocation", schema = 1,
        lowering_observed = !lowers || std::env::var_os("INCAN_NATIVE_TIMING").is_some(),
        "compiler work observations");
}

/// Observe the pinned collector only when rustc requests it. Forcing this query in `after_analysis` changes
/// metadata construction order, so diagnostic collection must wrap the existing provider instead of querying early.
pub(crate) fn provide_codegen(providers: &mut rustc_middle::util::Providers) {
    if std::env::var_os("INCAN_NATIVE_TIMING").is_some() {
        providers.queries.collect_and_partition_mono_items = observed_partitions;
    }
}

/// Preserve the default collector's inputs, result and execution order while counting its actual partitions.
/// Nonincremental successful native compilation translates every partition. Incremental reuse remains unavailable,
/// and allocator/metadata shims are deliberately outside this mono-item count. A failed sink cannot fail compilation.
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
    let _ = writeln!(
        std::io::stderr(),
        "redo-codegen: {}",
        serde_json::json!({"partitions": partitions.codegen_units.len(), "execution": execution})
    );
    partitions
}

/// A disabled timer never reads a clock; enabled records use the same console as rustc's phase diagnostics.
pub(crate) struct Timing(Option<Clock>);

/// The invocation origin and previous boundary, retained only for a diagnostic invocation.
struct Clock {
    started: Instant,
    previous: Instant,
}

impl Timing {
    /// Start observing only when the existing native phase diagnostic has been requested.
    pub(crate) fn requested() -> Self {
        Self(std::env::var_os("INCAN_NATIVE_TIMING").map(|_| {
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
        let _ = writeln!(
            std::io::stderr(),
            "native-time: {}",
            serde_json::json!({
                "boundary": boundary,
                "elapsed": now.duration_since(clock.started).as_secs_f64(),
                "interval": now.duration_since(clock.previous).as_secs_f64(),
            })
        );
        clock.previous = now;
    }
}
