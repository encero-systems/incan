//! Caller-owned program streams and observations for direct replacement execution.
//!
//! Writes go to the supplied host writers during execution. Observation records only bytes accepted by those
//! writers; it does not gate delivery on a later return value or receipt. Each stream preserves its own byte order,
//! with no promised ordering between stdout and stderr and no claim that a terminal displayed accepted bytes.

use std::io::{self, Write};

/// One of the two ordinary program-output streams, independent of compiler reports and receipts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramStream {
    /// Bytes addressed to the program's standard output.
    Stdout,
    /// Bytes addressed to the program's standard error.
    Stderr,
}

impl std::fmt::Display for ProgramStream {
    /// Use the ordinary stream name in source-located write diagnostics.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        })
    }
}

/// The host operation that failed while delivering program output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramIoOperation {
    /// The writer did not accept all supplied bytes.
    Write,
    /// The writer accepted bytes but could not complete its flush.
    Flush,
}

impl std::fmt::Display for ProgramIoOperation {
    /// Distinguish acceptance failures from flushing failures without interpreting host error text.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Write => "write",
            Self::Flush => "flush",
        })
    }
}

/// A failed host write or flush; already accepted bytes remain in the caller's observation.
#[derive(Debug, thiserror::Error)]
#[error("program {stream} {operation} failed: {source}")]
pub struct ProgramIoError {
    /// Stream on which the operation failed.
    pub stream: ProgramStream,
    /// Whether failure occurred while writing or flushing.
    pub operation: ProgramIoOperation,
    /// Original host error, including its typed error kind.
    #[source]
    pub source: io::Error,
}

/// Bytes accepted by each supplied writer, even if execution or receipt persistence subsequently fails.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProgramOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    /// Completed builtin-print calls, retained for the existing line-oriented successful report projection.
    pub(super) printed_lines: Vec<String>,
}

impl ProgramOutput {
    /// Return stdout exactly as accepted, without trimming or requiring valid UTF-8.
    #[must_use]
    pub fn stdout(&self) -> &[u8] {
        &self.stdout
    }

    /// Return stderr exactly as accepted, independently of stdout and compiler diagnostic rendering.
    #[must_use]
    pub fn stderr(&self) -> &[u8] {
        &self.stderr
    }
}

/// Observation offsets separating one execution from earlier writes through a reused caller-owned adapter.
#[derive(Clone, Copy)]
pub(super) struct OutputCheckpoint {
    stdout: usize,
    stderr: usize,
    printed_lines: usize,
}

/// Ordinary program writers borrowed from the caller for one or more sequential executions.
///
/// No files or descriptors are opened here: embedding code supplies the writers, and the CLI supplies its normal
/// stdout/stderr handles. A capture harness can explicitly supply in-memory writers instead. This adapter adds no
/// whole-program delivery buffer. Builtin print writes and flushes each rendered line before execution continues;
/// partial writes and flush failures stop execution without discarding bytes that the writer already accepted.
pub struct ProgramIo<'writer> {
    stdout: &'writer mut dyn Write,
    stderr: &'writer mut dyn Write,
    output: ProgramOutput,
}

impl<'writer> ProgramIo<'writer> {
    /// Borrow the caller's stdout and stderr writers without taking over their lifetime or flushing on drop.
    pub fn new(stdout: &'writer mut dyn Write, stderr: &'writer mut dyn Write) -> Self {
        Self {
            stdout,
            stderr,
            output: ProgramOutput::default(),
        }
    }

    /// Inspect accepted bytes after success or failure, independently of a successful execution result.
    #[must_use]
    pub fn output(&self) -> &ProgramOutput {
        &self.output
    }

    /// Retain the start of an execution so its receipt identity excludes prior uses of this adapter.
    pub(super) fn checkpoint(&self) -> OutputCheckpoint {
        OutputCheckpoint {
            stdout: self.output.stdout.len(),
            stderr: self.output.stderr.len(),
            printed_lines: self.output.printed_lines.len(),
        }
    }

    /// Snapshot only one execution's observations, leaving the caller's full history available after return.
    pub(super) fn output_since(&self, checkpoint: OutputCheckpoint) -> ProgramOutput {
        ProgramOutput {
            stdout: self.output.stdout[checkpoint.stdout..].to_vec(),
            stderr: self.output.stderr[checkpoint.stderr..].to_vec(),
            printed_lines: self.output.printed_lines[checkpoint.printed_lines..].to_vec(),
        }
    }

    /// Deliver a builtin print line now and flush before the next Incan operation executes.
    pub(super) fn print_line(&mut self, line: String) -> Result<(), ProgramIoError> {
        self.write(ProgramStream::Stdout, line.as_bytes())?;
        self.write(ProgramStream::Stdout, b"\n")?;
        self.flush(ProgramStream::Stdout)?;
        self.output.printed_lines.push(line);
        Ok(())
    }

    /// Write all bytes to one supplied program stream, retaining each accepted prefix before a later failure.
    ///
    /// Interrupted writes retry as with `std::io::Write::write_all`; zero progress is a write failure. The adapter
    /// deliberately does not flush arbitrary chunks: the calling operation determines its flush boundary.
    pub fn write(&mut self, stream: ProgramStream, mut bytes: &[u8]) -> Result<(), ProgramIoError> {
        let (writer, observed) = match stream {
            ProgramStream::Stdout => (&mut self.stdout, &mut self.output.stdout),
            ProgramStream::Stderr => (&mut self.stderr, &mut self.output.stderr),
        };
        while !bytes.is_empty() {
            let result = match writer.write(bytes) {
                Ok(0) => Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "program writer made no progress",
                )),
                Ok(length) if length > bytes.len() => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "program writer returned an invalid byte count",
                )),
                Ok(length) => {
                    observed.extend_from_slice(&bytes[..length]);
                    bytes = &bytes[length..];
                    continue;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => Err(error),
            };
            result.map_err(|source| ProgramIoError {
                stream,
                operation: ProgramIoOperation::Write,
                source,
            })?;
        }
        Ok(())
    }

    /// Flush one supplied writer; accepted bytes remain observed even when the flush fails.
    pub fn flush(&mut self, stream: ProgramStream) -> Result<(), ProgramIoError> {
        let writer = match stream {
            ProgramStream::Stdout => &mut self.stdout,
            ProgramStream::Stderr => &mut self.stderr,
        };
        writer.flush().map_err(|source| ProgramIoError {
            stream,
            operation: ProgramIoOperation::Flush,
            source,
        })
    }
}
