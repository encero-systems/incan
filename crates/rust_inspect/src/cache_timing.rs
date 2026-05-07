//! Timing helpers for `RustMetadataCache`.
//!
//! This module keeps debug timing plumbing out of the core cache/query logic so lookup flow stays readable.

use std::path::Path;
use std::time::{Duration, Instant};

const TIMING_ENV: &str = "INCAN_RUST_INSPECT_TIMING";

fn env_flag_enabled(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| {
        let value = value.to_string_lossy();
        matches!(value.as_ref(), "1" | "true" | "TRUE" | "on" | "ON")
    })
}

/// Whether rust-inspect timing logs should be emitted.
pub(crate) fn rust_inspect_timing_enabled() -> bool {
    env_flag_enabled(TIMING_ENV)
}

/// Emit one timing line for a named stage when timing is enabled.
pub(crate) fn log_timing_stage(
    enabled: bool,
    root: &Path,
    query_path: &str,
    stage: &str,
    elapsed: Duration,
    detail: &str,
) {
    if !enabled {
        return;
    }
    let root_label = root.file_name().and_then(|name| name.to_str()).unwrap_or("workspace");
    if detail.is_empty() {
        eprintln!(
            "[rust-inspect-timing] root={} query={} stage={} ms={:.2}",
            root_label,
            query_path,
            stage,
            elapsed.as_secs_f64() * 1000.0
        );
    } else {
        eprintln!(
            "[rust-inspect-timing] root={} query={} stage={} ms={:.2} {}",
            root_label,
            query_path,
            stage,
            elapsed.as_secs_f64() * 1000.0,
            detail
        );
    }
}

/// Tracks one cache call and records total elapsed time on drop.
pub(crate) struct CallTrace<'a> {
    enabled: bool,
    root: &'a Path,
    query_path: &'a str,
    started: Instant,
    outcome: &'static str,
}

impl<'a> CallTrace<'a> {
    pub(crate) fn new(enabled: bool, root: &'a Path, query_path: &'a str) -> Self {
        Self {
            enabled,
            root,
            query_path,
            started: Instant::now(),
            outcome: "error",
        }
    }

    /// Set final outcome label for the `call.total` timing line.
    pub(crate) fn set_outcome(&mut self, outcome: &'static str) {
        self.outcome = outcome;
    }
}

impl Drop for CallTrace<'_> {
    fn drop(&mut self) {
        if !self.enabled {
            return;
        }
        log_timing_stage(
            true,
            self.root,
            self.query_path,
            "call.total",
            self.started.elapsed(),
            self.outcome,
        );
    }
}
