//! Opt-in boundaries around fresh rustc setup, Incan injection, queries and teardown.

use std::time::Instant;

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
        eprintln!(
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
