//! Console progress for the long-running Oven commands.
//!
//! Two mechanisms, and the distinction between them is the point. [`announce`] reports a state change the moment it
//! happens, which needs something to have happened. [`Heartbeat`] reports what is *still* happening on a fixed
//! interval. A heartbeat proves supervisor liveness and names outstanding work; it cannot prove that a test or bake
//! phase is making useful progress. An increasing elapsed time without case completions identifies what needs
//! inspection.
//!
//! Both write to stderr. Stdout carries the caller's machine-readable report, and a `--format json` run must still be
//! able to say what it is doing without interleaving prose into the document a caller is parsing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// How often an idle command says it is still working.
///
/// Short enough that a person does not conclude the run has died, long enough that a healthy multi-minute phase does
/// not bury its own real output.
pub const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Where progress lines go.
///
/// Progress is normally stderr, and a test that wants to assert on it has no business rebinding this process's
/// descriptors to find out — this repository forbids the `unsafe` that would take. A collecting sink gives the same
/// observation in-process, which is what makes *ordering* claims testable: that a failure's explanation reaches the
/// console with its result rather than after the next case.
#[derive(Clone)]
pub struct ProgressSink {
    collected: Option<Arc<Mutex<String>>>,
}

impl ProgressSink {
    /// The ordinary sink: stderr, because stdout carries the caller's machine-readable report.
    pub fn stderr() -> Self {
        Self { collected: None }
    }

    /// A sink that accumulates instead of printing, for tests that need to see what was written and in what order.
    pub fn collecting() -> Self {
        Self {
            collected: Some(Arc::new(Mutex::new(String::new()))),
        }
    }

    /// Write one line, terminated.
    ///
    /// A poisoned collecting sink drops the line rather than propagating: progress reporting must never be able to
    /// fail the run it is describing.
    pub fn line(&self, line: &str) {
        match &self.collected {
            None => eprintln!("{line}"),
            Some(collected) => {
                if let Ok(mut collected) = collected.lock() {
                    collected.push_str(line);
                    collected.push('\n');
                }
            }
        }
    }

    /// Everything written to a collecting sink so far, or `None` for the stderr sink.
    pub fn collected(&self) -> Option<String> {
        self.collected
            .as_ref()
            .and_then(|collected| collected.lock().ok().map(|collected| collected.clone()))
    }
}

impl std::fmt::Debug for ProgressSink {
    /// Name the destination rather than dumping whatever a collecting sink has accumulated.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProgressSink")
            .field(
                "destination",
                &if self.collected.is_some() {
                    "collecting"
                } else {
                    "stderr"
                },
            )
            .finish()
    }
}

impl Default for ProgressSink {
    /// Progress goes to stderr unless a caller says otherwise.
    fn default() -> Self {
        Self::stderr()
    }
}

/// Report one unit of Oven progress on a single line, in the shape every long-running Oven command uses.
///
/// A run that goes quiet for forty minutes cannot be told from one that has hung. One line per state change fixes
/// that, and keeping every command on the same shape — a fixed-width state, the subject, then an optional detail —
/// means a reader learns to scan it once rather than per command.
pub fn announce(state: &str, subject: &str, detail: Option<&str>) {
    ProgressSink::stderr().line(&match detail {
        Some(detail) => format!("{state:<10} {subject} ({detail})"),
        None => format!("{state:<10} {subject}"),
    });
}

/// Format a measured duration for a progress line.
///
/// Seconds with one decimal, because these phases run from seconds to tens of minutes and a reader comparing two of
/// them cares about the magnitude rather than the millisecond.
pub fn elapsed_detail(started: Instant) -> String {
    format!("{:.1}s", started.elapsed().as_secs_f64())
}

/// Say what is still in flight on a fixed interval, whether or not anything is producing output.
///
/// This exists because every output-driven signal has the same blind spot. libtest's own slow-case warning is emitted
/// only by its concurrent runner, so a root given a single thread never produces one however long it takes; a bake
/// phase that is one long call produces nothing between its start and its end; and a wedged process produces nothing
/// at all. In each case the console goes silent, and silence is exactly what a caller cannot interpret.
///
/// The reporting closure returns `None` when there is nothing worth saying, so a heartbeat can be started
/// unconditionally and stay quiet while the thing it watches is idle or already finished.
pub struct Heartbeat {
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Heartbeat {
    /// Start reporting on `interval` until the returned handle is dropped.
    ///
    /// Dropping the handle unparks the worker immediately; short roots never pay a polling interval to stop reporting.
    pub fn start(
        sink: ProgressSink,
        interval: Duration,
        describe: impl Fn() -> Option<String> + Send + 'static,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            let mut next_report = Instant::now() + interval;
            while !worker_stop.load(Ordering::Relaxed) {
                thread::park_timeout(next_report.saturating_duration_since(Instant::now()));
                if worker_stop.load(Ordering::Relaxed) {
                    break;
                }
                if Instant::now() < next_report {
                    continue;
                }
                if let Some(line) = describe() {
                    sink.line(&line);
                }
                next_report = Instant::now() + interval;
            }
        });
        Self {
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for Heartbeat {
    /// Stop the reporting thread and wait for it, so no line can arrive after the work it describes has finished.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            // A reporting thread that panicked has already lost its own output; it must not also fail the command
            // whose progress it was describing.
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Heartbeat, ProgressSink, announce, elapsed_detail};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    #[test]
    fn a_heartbeat_reports_without_being_fed_anything() {
        // The whole point: no input arrives, and it still speaks.
        let reports = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&reports);
        let heartbeat = Heartbeat::start(ProgressSink::stderr(), Duration::from_millis(150), move || {
            counted.fetch_add(1, Ordering::Relaxed);
            None
        });
        std::thread::sleep(Duration::from_millis(500));
        drop(heartbeat);

        assert!(
            reports.load(Ordering::Relaxed) >= 2,
            "a 150ms heartbeat held for 500ms must have reported at least twice, got {}",
            reports.load(Ordering::Relaxed)
        );
    }

    #[test]
    fn a_dropped_heartbeat_stops_reporting() {
        let reports = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&reports);
        let heartbeat = Heartbeat::start(ProgressSink::stderr(), Duration::from_millis(100), move || {
            counted.fetch_add(1, Ordering::Relaxed);
            None
        });
        std::thread::sleep(Duration::from_millis(250));
        drop(heartbeat);
        let after_drop = reports.load(Ordering::Relaxed);

        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            reports.load(Ordering::Relaxed),
            after_drop,
            "dropping the handle must join the thread, not merely ask it to stop"
        );
    }

    #[test]
    fn a_heartbeat_that_finishes_before_its_first_interval_never_reports() {
        let reports = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&reports);
        let heartbeat = Heartbeat::start(ProgressSink::stderr(), Duration::from_secs(30), move || {
            counted.fetch_add(1, Ordering::Relaxed);
            None
        });
        drop(heartbeat);

        assert_eq!(
            reports.load(Ordering::Relaxed),
            0,
            "quick work must stay quiet rather than announce itself on the way out"
        );
    }

    #[test]
    fn announcing_and_formatting_do_not_depend_on_a_terminal() {
        // Guards the signatures the callers rely on; the streams themselves are covered by the subprocess tests.
        announce("STATE", "subject", Some("detail"));
        announce("STATE", "subject", None);
        assert!(elapsed_detail(Instant::now()).ends_with('s'));
    }
}
