//! Root-owned libtest results, isolated from subprocess stdout and stderr.
//!
//! The pinned libtest harness writes `--logfile` records itself, before formatting a terminal console event. Nested
//! commands inherit neither the argument nor this file handle, so diagnostic JSON cannot become case evidence. The
//! option is deprecated upstream; real-process regressions pin its required behavior, and absent/incomplete records
//! refuse complete-suite success instead of falling back to stdout. Times have libtest's millisecond precision.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::oven::loaf::LoafTemporaryDirectory;

use super::{OvenNativeTestCaseCounts, OvenNativeTestCaseTiming, seconds_to_rounded_millis};

/// One terminal record emitted by the selected harness, never inferred from diagnostic output.
#[derive(Debug, Clone)]
pub(super) struct CaseResult {
    /// Exact name from the selected root inventory.
    pub name: String,
    /// Known libtest terminal spelling: `ok`, `failed`, or `ignored`.
    pub outcome: &'static str,
    /// Millisecond duration, absent for ignored cases.
    pub elapsed_ms: Option<u64>,
}

/// Result evidence from one native process, including useful times left by an incomplete run.
#[derive(Debug, Default)]
pub(super) struct NativeTestEvidence {
    /// Complete coverage counts, absent for missing, malformed, or duplicate results.
    pub counts: Option<OvenNativeTestCaseCounts>,
    /// Observed case times, retained even when another selected case never completed.
    pub timings: Vec<OvenNativeTestCaseTiming>,
}

/// Temporary result channel whose path is supplied only to the root harness invocation.
pub(super) struct NativeTestLog {
    // Close the handle before the directory guard removes its file, including on Windows.
    reader: File,
    _directory: LoafTemporaryDirectory,
    path: PathBuf,
    expected: BTreeSet<String>,
    results: BTreeMap<String, CaseResult>,
    unread: Vec<u8>,
    pending_record: String,
    invalid: bool,
    #[cfg(test)]
    read_failure_marker: Option<std::path::PathBuf>,
}

impl NativeTestLog {
    /// Create a private result file for precisely the names this invocation selects.
    pub fn new(expected: BTreeSet<String>) -> io::Result<Self> {
        let directory = LoafTemporaryDirectory::create(&std::env::temp_dir(), "incan-libtest-")?;
        let path = directory.path().join("results.log");
        let mut options = OpenOptions::new();
        options.create_new(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let reader = options.open(&path)?;
        Ok(Self {
            reader,
            _directory: directory,
            path,
            expected,
            results: BTreeMap::new(),
            unread: Vec::new(),
            pending_record: String::new(),
            invalid: false,
            #[cfg(test)]
            read_failure_marker: None,
        })
    }

    /// Path passed as a root command argument, never placed in an inherited environment variable.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Drain newly complete records while preserving a writer's partial line for the next poll.
    pub fn poll(&mut self) -> io::Result<Vec<CaseResult>> {
        #[cfg(test)]
        if self.read_failure_marker.as_ref().is_some_and(|path| path.is_file()) {
            return Err(io::Error::other("injected result-channel read failure"));
        }
        self.reader.read_to_end(&mut self.unread)?;
        let Some(last_newline) = self.unread.iter().rposition(|byte| *byte == b'\n') else {
            return Ok(Vec::new());
        };
        let complete = self.unread.drain(..=last_newline).collect::<Vec<_>>();
        let mut observed = Vec::new();
        for line in complete.split_inclusive(|byte| *byte == b'\n') {
            let Ok(line) = std::str::from_utf8(line) else {
                self.invalid = true;
                continue;
            };
            self.pending_record.push_str(line);
            let Some(result) = self.parse_record(self.pending_record.trim_end_matches('\n')) else {
                continue;
            };
            self.pending_record.clear();
            if self.results.contains_key(&result.name) {
                self.invalid = true;
                continue;
            }
            self.results.insert(result.name.clone(), result.clone());
            observed.push(result);
        }
        Ok(observed)
    }

    /// Exercise supervisor cleanup after the child has started and the result channel becomes unreadable.
    #[cfg(test)]
    pub fn fail_when_marker_exists(&mut self, marker: std::path::PathBuf) {
        self.read_failure_marker = Some(marker);
    }

    /// Match the verified name at the end; ignored/failed reasons may contain spaces and newlines.
    fn parse_record(&self, record: &str) -> Option<CaseResult> {
        let (record, elapsed_ms) = if let Some((record, duration)) = record.rsplit_once(" <") {
            let seconds: f64 = duration.strip_suffix("s>")?.parse().ok()?;
            (record, Some(seconds_to_rounded_millis(seconds)?))
        } else {
            (record, None)
        };
        let (outcome, name) = record.rsplit_once(' ')?;
        if !self.expected.contains(name) {
            return None;
        }
        let outcome = match outcome {
            "ok" if elapsed_ms.is_some() => "ok",
            "failed" | "failed (time limit exceeded)" if elapsed_ms.is_some() => "failed",
            text if text.starts_with("failed:") && elapsed_ms.is_some() => "failed",
            "ignored" => "ignored",
            text if text.starts_with("ignored:") => "ignored",
            _ => return None,
        };
        Some(CaseResult {
            name: name.to_string(),
            outcome,
            elapsed_ms,
        })
    }

    /// Validate complete coverage independently of process exit, keeping observed timings on crashes/timeouts.
    pub fn finish(self) -> NativeTestEvidence {
        let complete = !self.invalid
            && self.unread.is_empty()
            && self.pending_record.is_empty()
            && self.results.len() == self.expected.len();
        let mut counts = OvenNativeTestCaseCounts::default();
        let mut timings = Vec::new();
        for result in self.results.into_values() {
            match result.outcome {
                "ok" => counts.passed += 1,
                "failed" => counts.failed += 1,
                "ignored" => counts.ignored += 1,
                _ => unreachable!("log parser produces only known terminal outcomes"),
            }
            if let Some(elapsed_ms) = result.elapsed_ms {
                timings.push(OvenNativeTestCaseTiming {
                    name: result.name,
                    elapsed_ms,
                });
            }
        }
        timings.sort_by(|left, right| {
            right
                .elapsed_ms
                .cmp(&left.elapsed_ms)
                .then_with(|| left.name.cmp(&right.name))
        });
        NativeTestEvidence {
            counts: complete.then_some(counts),
            timings,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::NativeTestLog;
    use std::collections::BTreeSet;
    use std::fs::OpenOptions;
    use std::io::Write;

    #[test]
    fn simultaneous_invocations_own_distinct_result_files() -> Result<(), Box<dyn std::error::Error>> {
        let first = NativeTestLog::new(BTreeSet::from(["selected".into()]))?;
        let second = NativeTestLog::new(BTreeSet::from(["selected".into()]))?;
        let first_path = first.path().to_path_buf();
        let second_path = second.path().to_path_buf();
        assert_ne!(first_path, second_path);
        drop(first);
        assert!(!first_path.exists());
        assert!(second_path.is_file(), "another invocation removed this result channel");
        drop(second);
        assert!(!second_path.exists());
        Ok(())
    }

    #[test]
    fn partial_records_and_multiline_ignore_reasons_wait_for_the_complete_record()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut log = NativeTestLog::new(BTreeSet::from(["ignored_case".into(), "selected".into()]))?;
        let mut writer = OpenOptions::new().append(true).open(log.path())?;
        writer.write_all(b"ignored: reason\nwith another line ignored_case\nok selected <0.")?;
        let first = log.poll()?;
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].outcome, "ignored");
        writer.write_all(b"123s>\n")?;
        let second = log.poll()?;
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].elapsed_ms, Some(123));
        let evidence = log.finish();
        let counts = evidence.counts.ok_or("complete records were not accounted for")?;
        assert_eq!((counts.passed, counts.failed, counts.ignored), (1, 0, 1));
        Ok(())
    }

    #[test]
    fn malformed_duplicate_and_missing_records_never_become_complete_evidence() -> Result<(), Box<dyn std::error::Error>>
    {
        for bytes in [
            "ok selected <0.123s>\nok selected <0.001s>\n",
            "ok selected <0.123s>\nmalformed tail\n",
            "ok selected <0.123s>",
            "ok unknown <0.123s>\n",
            "ok selected <NaNs>\n",
            "",
        ] {
            let mut log = NativeTestLog::new(BTreeSet::from(["selected".into()]))?;
            std::fs::write(log.path(), bytes)?;
            log.poll()?;
            assert!(log.finish().counts.is_none(), "invalid evidence accepted: {bytes:?}");
        }
        Ok(())
    }

    #[test]
    fn unfinished_runs_keep_only_observed_times_and_remove_the_owned_file() -> Result<(), Box<dyn std::error::Error>> {
        let mut log = NativeTestLog::new(BTreeSet::from(["finished".into(), "unfinished".into()]))?;
        let path = log.path().to_path_buf();
        std::fs::write(&path, "ok finished <1.250s>\n")?;
        log.poll()?;
        let evidence = log.finish();
        assert!(evidence.counts.is_none());
        assert_eq!(evidence.timings.len(), 1);
        assert_eq!(evidence.timings[0].elapsed_ms, 1250);
        assert!(!path.exists(), "the invocation left its result file behind");
        Ok(())
    }
}
