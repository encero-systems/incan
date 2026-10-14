//! Request-local test output shared by the scheduler and its workers.
//!
//! The client supplies the destination. An owner request can stream each event to its own connection without
//! redirecting process-wide descriptors or allowing one worker to write into another request's report.

use super::{TestOutputFormat, TestProgress};
use std::sync::Arc;

/// One ordered output fragment, preserving the command's stdout/stderr distinction.
pub(crate) enum TestOutput {
    /// Console text, JSON records or explicitly requested uncaptured test output.
    Stdout(String),
    /// Diagnostics and notices historically written to stderr.
    Stderr(String),
    /// Semantic progress facts, formatted only by the invoking client.
    Progress(TestProgress),
}

/// A cloneable request-local destination; workers share only this request's output callback.
#[derive(Clone)]
pub(crate) struct TestOutputSink(Arc<dyn Fn(TestOutput) + Send + Sync>);

impl TestOutputSink {
    /// Route output through an explicit destination owned by one command request.
    pub(crate) fn new(emit: impl Fn(TestOutput) + Send + Sync + 'static) -> Self {
        Self(Arc::new(emit))
    }

    /// Emit one semantic event immediately, retaining order with this request's raw output.
    pub(crate) fn progress(&self, event: TestProgress) {
        (self.0)(TestOutput::Progress(event));
    }

    /// Create the client renderer used by both local execution and the owner transport.
    pub(crate) fn rendered(format: TestOutputFormat, verbose: bool, use_color: bool) -> Self {
        let output = Self::default();
        Self::new(move |event| match event {
            TestOutput::Progress(event) => render_progress(&event, format, verbose, use_color, &output),
            raw => (output.0)(raw),
        })
    }

    /// Emit exact stdout bytes already represented as UTF-8 text, without adding a newline.
    pub(crate) fn stdout(&self, message: String) {
        self.emit(message, false);
    }

    /// Emit one stdout line while retaining the same framing for empty and multiline text.
    pub(crate) fn line(&self, message: impl std::fmt::Display) {
        self.stdout(format!("{message}\n"));
    }

    /// Emit one stderr line for this request without changing the process's stderr destination.
    pub(crate) fn error_line(&self, message: impl std::fmt::Display) {
        self.emit(format!("{message}\n"), true);
    }
    /// Bound each transport fragment without splitting UTF-8 or changing the concatenated output bytes.
    fn emit(&self, message: String, stderr: bool) {
        const MAX_FRAGMENT: usize = 64 * 1024;
        let mut remaining = message.as_str();
        while !remaining.is_empty() {
            let mut end = remaining.len().min(MAX_FRAGMENT);
            while !remaining.is_char_boundary(end) {
                end -= 1;
            }
            let fragment = remaining[..end].to_string();
            (self.0)(if stderr {
                TestOutput::Stderr(fragment)
            } else {
                TestOutput::Stdout(fragment)
            });
            remaining = &remaining[end..];
        }
    }
}

impl Default for TestOutputSink {
    /// Ordinary in-process calls render their own output directly in the invoking CLI.
    fn default() -> Self {
        Self::new(print_console_output)
    }
}

/// Print raw fragments directly; semantic events use the same console renderer as configured clients.
/// Construction stores this callback without invoking it, and rendering emits only raw fragments.
fn print_console_output(event: TestOutput) {
    match event {
        TestOutput::Stdout(message) => print!("{message}"),
        TestOutput::Stderr(message) => eprint!("{message}"),
        TestOutput::Progress(event) => render_progress(
            &event,
            TestOutputFormat::Console,
            false,
            false,
            &TestOutputSink::default(),
        ),
    }
}

/// Render semantic progress at the client boundary; machine-readable mode never receives human banners.
/// Raw output and diagnostics are handled separately so their contents and stream remain unchanged.
pub(crate) fn render_progress(
    event: &TestProgress,
    format: TestOutputFormat,
    verbose: bool,
    use_color: bool,
    output: &TestOutputSink,
) {
    use super::{centered_eq_banner, style};
    if matches!(event, TestProgress::NoTestsCollected) {
        output.error_line("No tests collected");
        return;
    }
    if format != TestOutputFormat::Console {
        return;
    }
    match event {
        TestProgress::SessionStarted {
            collected,
            shuffle_seed,
            jobs,
        } => {
            output.line(style(centered_eq_banner("test session starts"), "1", use_color));
            output.line(format_args!("collected {collected} item(s)"));
            if let Some(seed) = shuffle_seed {
                output.line(format_args!("shuffle seed: {seed}"));
            }
            if *jobs > 1 {
                output.line(format_args!("jobs: {jobs}"));
            }
            output.line("");
        }
        TestProgress::UnitStarting { files, tests } => {
            let files = files
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            output.line(style(format!("running {files} ({tests} item(s))"), "2", use_color));
        }
        TestProgress::FixturesDiscovered { fixtures } if verbose => {
            output.line(format_args!("Discovered {} fixture(s):", fixtures.len()));
            for fixture in fixtures {
                output.line(format_args!(
                    "  - {}: scope={}{}{}{}",
                    fixture.name,
                    fixture.scope.as_str(),
                    if fixture.autouse { " (autouse)" } else { "" },
                    if fixture.is_async { " async" } else { "" },
                    if fixture.has_teardown { " (with teardown)" } else { "" }
                ));
            }
            output.line("");
        }
        TestProgress::UnitsPlanned { units, elapsed } if verbose => {
            output.line(format_args!(
                "planned {units} generated harness unit(s) in {:.2}s",
                elapsed.as_secs_f64()
            ));
        }
        TestProgress::ExecutionCompleted { elapsed } if verbose => {
            output.line(format_args!(
                "generated harness execution phase completed in {:.2}s",
                elapsed.as_secs_f64()
            ));
        }
        TestProgress::HarnessCompleted {
            file,
            frontend,
            generation,
            receipt,
            selection,
            native,
            execution,
            native_reused,
        } if verbose => {
            output.line(format_args!("Oven test phases for {}: front-end {:.2}s, generation {:.2}s, receipt {:.2}s, selection {:.2}s, native {} {:.2}s, execution {:.2}s",
                file.display(), frontend.as_secs_f64(), generation.as_secs_f64(), receipt.as_secs_f64(), selection.as_secs_f64(),
                if *native_reused { "reuse" } else { "bake" }, native.as_secs_f64(), execution.as_secs_f64()));
        }
        _ => {}
    }
}

impl std::fmt::Debug for TestOutputSink {
    /// Describe the request boundary without exposing the callback or captured output.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("TestOutputSink(request-local)")
    }
}

#[cfg(test)]
mod tests {
    use super::{TestOutput, TestOutputSink};
    use std::sync::{Arc, Mutex};

    /// Semantic events preserve their position between raw stdout and stderr without worker-side formatting.
    #[test]
    fn typed_progress_keeps_raw_stream_order() -> Result<(), Box<dyn std::error::Error>> {
        let received = Arc::new(Mutex::new(Vec::new()));
        let capture = Arc::clone(&received);
        let sink = TestOutputSink::new(move |event| {
            if let Ok(mut received) = capture.lock() {
                received.push(event);
            }
        });
        sink.stdout("raw λ\n".to_string());
        sink.progress(super::TestProgress::SessionStarted {
            collected: 7,
            shuffle_seed: Some(42),
            jobs: 4,
        });
        sink.error_line("stderr");
        let received = received.lock().map_err(|_| "poisoned output capture")?;
        assert_eq!(received.len(), 3);
        assert!(matches!(&received[0], TestOutput::Stdout(text) if text == "raw λ\n"));
        assert!(matches!(
            &received[1],
            TestOutput::Progress(super::TestProgress::SessionStarted {
                collected: 7,
                shuffle_seed: Some(42),
                jobs: 4
            })
        ));
        assert!(matches!(&received[2], TestOutput::Stderr(text) if text == "stderr\n"));
        Ok(())
    }

    /// The same typed event has client-selected presentation and never contaminates JSON result records.
    #[test]
    fn typed_progress_renderer_respects_format_and_verbose() -> Result<(), Box<dyn std::error::Error>> {
        use super::{TestOutputFormat, TestProgress, render_progress};
        let received = Arc::new(Mutex::new(Vec::new()));
        let capture = Arc::clone(&received);
        let sink = TestOutputSink::new(move |event| {
            if let Ok(mut received) = capture.lock() {
                received.push(event);
            }
        });
        let events = [
            TestProgress::SessionStarted {
                collected: 3,
                shuffle_seed: Some(17),
                jobs: 4,
            },
            TestProgress::UnitStarting {
                files: vec!["tests/test_one.incn".into()],
                tests: 3,
            },
            TestProgress::UnitsPlanned {
                units: 1,
                elapsed: std::time::Duration::from_millis(125),
            },
        ];
        for event in &events {
            let encoded = serde_json::to_vec(event)?;
            let decoded: TestProgress = serde_json::from_slice(&encoded)?;
            render_progress(&decoded, TestOutputFormat::Json, true, true, &sink);
        }
        assert!(received.lock().map_err(|_| "poisoned output capture")?.is_empty());
        for event in &events {
            render_progress(event, TestOutputFormat::Console, false, false, &sink);
        }
        let plain = received
            .lock()
            .map_err(|_| "poisoned output capture")?
            .iter()
            .filter_map(|event| match event {
                TestOutput::Stdout(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert!(plain.contains("collected 3 item(s)\nshuffle seed: 17\njobs: 4\n"));
        assert!(plain.contains("running tests/test_one.incn (3 item(s))"));
        assert!(!plain.contains("planned"));
        assert!(!plain.contains('\u{1b}'));
        render_progress(&events[2], TestOutputFormat::Console, true, false, &sink);
        render_progress(
            &TestProgress::NoTestsCollected,
            TestOutputFormat::Json,
            false,
            false,
            &sink,
        );
        let received = received.lock().map_err(|_| "poisoned output capture")?;
        assert!(received.iter().any(|event| matches!(event, TestOutput::Stdout(text) if text.contains("planned 1 generated harness unit(s) in 0.12s"))));
        assert!(matches!(received.last(), Some(TestOutput::Stderr(text)) if text == "No tests collected\n"));
        Ok(())
    }

    /// Concurrent workers share only their request's destination and preserve each output stream.
    #[test]
    fn workers_keep_output_request_local() -> Result<(), Box<dyn std::error::Error>> {
        let left = Arc::new(Mutex::new(Vec::new()));
        let right = Arc::new(Mutex::new(Vec::new()));
        let mut workers = Vec::new();
        for (label, received) in [("left", Arc::clone(&left)), ("right", Arc::clone(&right))] {
            let sink = TestOutputSink::new(move |event| {
                if let Ok(mut received) = received.lock() {
                    received.push(event);
                }
            });
            workers.push(std::thread::spawn(move || {
                sink.line(label);
                sink.error_line(label);
            }));
        }
        for worker in workers {
            worker.join().map_err(|_| "output worker panicked")?;
        }
        for (label, received) in [("left", left), ("right", right)] {
            let received = received.lock().map_err(|_| "poisoned output capture")?;
            assert!(matches!(received.first(), Some(TestOutput::Stdout(text)) if text == &format!("{label}\n")));
            assert!(matches!(received.get(1), Some(TestOutput::Stderr(text)) if text == &format!("{label}\n")));
            assert_eq!(received.len(), 2);
        }
        Ok(())
    }

    /// Large uncaptured Unicode transcripts remain byte-identical across bounded output frames.
    #[test]
    fn bounded_fragments_preserve_unicode_transcript() -> Result<(), Box<dyn std::error::Error>> {
        let received = Arc::new(Mutex::new(Vec::new()));
        let capture = Arc::clone(&received);
        let sink = TestOutputSink::new(move |event| {
            if let Ok(mut received) = capture.lock() {
                received.push(event);
            }
        });
        let transcript = "λ🦀\n".repeat(20_000);
        sink.stdout(transcript.clone());
        let received = received.lock().map_err(|_| "poisoned output capture")?;
        assert!(received.len() > 1);
        let mut actual = String::new();
        for event in received.iter() {
            let TestOutput::Stdout(fragment) = event else {
                return Err("wrong output stream".into());
            };
            assert!(fragment.len() <= 64 * 1024);
            actual.push_str(fragment);
        }
        assert_eq!(actual, transcript);
        Ok(())
    }
}
