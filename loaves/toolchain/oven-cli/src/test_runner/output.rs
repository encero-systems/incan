//! Request-local test output shared by the scheduler and its workers.
//!
//! The client supplies the destination. An owner request can stream each event to its own connection without
//! redirecting process-wide descriptors or allowing one worker to write into another request's report.

use std::sync::Arc;

/// One ordered output fragment, preserving the command's stdout/stderr distinction.
pub(crate) enum TestOutput {
    /// Console text, JSON records or explicitly requested uncaptured test output.
    Stdout(String),
    /// Diagnostics and notices historically written to stderr.
    Stderr(String),
}

/// A cloneable request-local destination; workers share only this request's output callback.
#[derive(Clone)]
pub(crate) struct TestOutputSink(Arc<dyn Fn(TestOutput) + Send + Sync>);

impl TestOutputSink {
    /// Route output through an explicit destination owned by one command request.
    pub(crate) fn new(emit: impl Fn(TestOutput) + Send + Sync + 'static) -> Self {
        Self(Arc::new(emit))
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
        Self::new(|event| match event {
            TestOutput::Stdout(message) => print!("{message}"),
            TestOutput::Stderr(message) => eprint!("{message}"),
        })
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
