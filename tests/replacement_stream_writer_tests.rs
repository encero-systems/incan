//! Direct writer regressions for replacement execution, including partial observations on failure.

use std::io::{self, Write};

use incan::backend::replacement::program_io::{ProgramIoOperation, ProgramStream};
use incan::backend::replacement::{ProgramIo, ReplacementExecutionError, execute_free_function_with_io};
use incan::frontend::body_ir::build_body_ir_module_v0;
use incan::frontend::typechecker::TypeChecker;
use incan::frontend::{lexer, parser};
use incan_semantics_core::body_ir::BodyIrModule;

/// Construct checked Body IR without a legacy compilation or generated-source handoff.
fn lower(source: &str) -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| io::Error::other(format!("{errors:?}")))?;
    let module_path = ["program_streams".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.to_vec()));
    checker
        .check_program(&program)
        .map_err(|errors| io::Error::other(format!("{errors:?}")))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Fail a chosen stream operation after retaining bytes the writer actually accepted.
#[derive(Default)]
struct TestWriter {
    bytes: Vec<u8>,
    max_chunk: Option<usize>,
    fail_after: Option<usize>,
    fail_flush: bool,
    interruptions_remaining: usize,
    flush_count: usize,
}

impl Write for TestWriter {
    /// Model short writes and a deterministic broken pipe without hiding accepted prefixes.
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.interruptions_remaining > 0 {
            self.interruptions_remaining -= 1;
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        let remaining = self
            .fail_after
            .map(|limit| limit.saturating_sub(self.bytes.len()))
            .unwrap_or(bytes.len());
        if remaining == 0 {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "injected stream failure"));
        }
        let length = bytes.len().min(remaining).min(self.max_chunk.unwrap_or(bytes.len()));
        self.bytes.extend_from_slice(&bytes[..length]);
        Ok(length)
    }

    /// Record each flush and optionally fail after a complete line has been accepted.
    fn flush(&mut self) -> io::Result<()> {
        self.flush_count += 1;
        if self.fail_flush {
            Err(io::Error::other("injected flush failure"))
        } else {
            Ok(())
        }
    }
}

/// A nested frame's output is delivered and observed before its later runtime error escapes.
#[test]
fn nested_failure_retains_program_stdout() -> Result<(), Box<dyn std::error::Error>> {
    let module = lower(
        "def child() -> None:\n  println(\"child\")\n  assert false\n\ndef main() -> None:\n  println(\"parent\")\n  child()\n",
    )?;
    let mut stdout = TestWriter::default();
    let mut stderr = Vec::new();
    {
        let mut io = ProgramIo::new(&mut stdout, &mut stderr);
        let result = execute_free_function_with_io(&module, "main", &[], &mut io);
        assert!(matches!(result, Err(ReplacementExecutionError::RuntimeFailure { .. })));
        assert_eq!(io.output().stdout(), b"parent\nchild\n");
        assert!(io.output().stderr().is_empty());
    }
    assert_eq!(stdout.bytes, b"parent\nchild\n");
    assert_eq!(stdout.flush_count, 2);
    Ok(())
}

/// Stream failure stops execution at the print call, before the assertion after it can execute.
#[test]
fn output_failure_precedes_a_later_runtime_failure() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def main() -> None:\n  println(\"hello\")\n  assert false\n";
    let module = lower(source)?;
    let mut stdout = TestWriter {
        fail_after: Some(2),
        ..TestWriter::default()
    };
    let mut stderr = Vec::new();
    {
        let mut io = ProgramIo::new(&mut stdout, &mut stderr);
        let error = execute_free_function_with_io(&module, "main", &[], &mut io)
            .err()
            .ok_or("the injected broken pipe must fail execution")?;
        assert!(matches!(error, ReplacementExecutionError::ProgramIo { .. }), "{error}");
        let span = error
            .primary_span()
            .ok_or("stream failures need the original print span")?;
        assert_eq!(source.get(span.start..span.end), Some("println(\"hello\")"));
        assert_eq!(io.output().stdout(), b"he");
    }
    assert_eq!(stdout.bytes, b"he");
    assert_eq!(stdout.flush_count, 0);
    Ok(())
}

/// A flush failure retains accepted bytes but cannot produce a successful execution receipt input.
#[test]
fn flush_failure_preserves_accepted_output() -> Result<(), Box<dyn std::error::Error>> {
    let module = lower("def main() -> None:\n  println(\"line\")\n")?;
    let mut stdout = TestWriter {
        fail_flush: true,
        ..TestWriter::default()
    };
    let mut stderr = Vec::new();
    let mut io = ProgramIo::new(&mut stdout, &mut stderr);
    let error = execute_free_function_with_io(&module, "main", &[], &mut io)
        .err()
        .ok_or("flush failure must not produce success")?;
    assert!(matches!(error, ReplacementExecutionError::ProgramIo { .. }), "{error}");
    assert_eq!(io.output().stdout(), b"line\n");
    Ok(())
}

/// Receipt identity depends on observed bytes, not a particular writer's short-write boundaries.
#[test]
fn short_writes_preserve_output_and_identity() -> Result<(), Box<dyn std::error::Error>> {
    let module = lower("def main() -> int:\n  println(\"a\\nb\")\n  println(\"c\", 2)\n  return 42\n")?;
    let mut full = TestWriter::default();
    let mut short = TestWriter {
        max_chunk: Some(1),
        ..TestWriter::default()
    };
    let mut stderr_full = Vec::new();
    let mut stderr_short = Vec::new();
    let first = execute_free_function_with_io(&module, "main", &[], &mut ProgramIo::new(&mut full, &mut stderr_full))?;
    let second =
        execute_free_function_with_io(&module, "main", &[], &mut ProgramIo::new(&mut short, &mut stderr_short))?;
    assert_eq!(full.bytes, b"a\nb\nc 2\n");
    assert_eq!(short.bytes, full.bytes);
    assert_eq!(first.output_identity, second.output_identity);
    assert_eq!(first.emitted_output(), ["a\nb", "c 2"]);
    Ok(())
}

/// Zero progress fails at the source print call without observing bytes or continuing execution.
#[test]
fn zero_progress_refuses_at_the_original_print_span() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def main() -> None:\n  println(\"hello\")\n  assert false\n";
    let module = lower(source)?;
    let mut stdout = TestWriter {
        max_chunk: Some(0),
        ..TestWriter::default()
    };
    let mut stderr = Vec::new();
    let mut io = ProgramIo::new(&mut stdout, &mut stderr);
    let error = execute_free_function_with_io(&module, "main", &[], &mut io)
        .err()
        .ok_or("zero progress must stop execution")?;
    let ReplacementExecutionError::ProgramIo {
        error: stream_error,
        span,
        ..
    } = error
    else {
        return Err(format!("expected stream failure, got {error}").into());
    };
    assert_eq!(stream_error.stream, ProgramStream::Stdout);
    assert_eq!(stream_error.operation, ProgramIoOperation::Write);
    assert_eq!(stream_error.source.kind(), io::ErrorKind::WriteZero);
    assert_eq!(source.get(span.start..span.end), Some("println(\"hello\")"));
    assert!(io.output().stdout().is_empty());
    Ok(())
}

/// Interrupted writes retry and preserve arbitrary bytes independently in stdout and stderr.
#[test]
fn interrupted_writes_preserve_exact_independent_streams() -> Result<(), Box<dyn std::error::Error>> {
    let mut stdout = TestWriter {
        interruptions_remaining: 2,
        max_chunk: Some(1),
        ..TestWriter::default()
    };
    let mut stderr = TestWriter {
        interruptions_remaining: 1,
        max_chunk: Some(1),
        ..TestWriter::default()
    };
    {
        let mut io = ProgramIo::new(&mut stdout, &mut stderr);
        io.write(ProgramStream::Stdout, b"out\n")?;
        io.write(ProgramStream::Stderr, &[0xff, 0, b'\n'])?;
        io.flush(ProgramStream::Stdout)?;
        io.flush(ProgramStream::Stderr)?;
        assert_eq!(io.output().stdout(), b"out\n");
        assert_eq!(io.output().stderr(), &[0xff, 0, b'\n']);
    }
    assert_eq!(stdout.bytes, b"out\n");
    assert_eq!(stderr.bytes, [0xff, 0, b'\n']);
    assert_eq!((stdout.flush_count, stderr.flush_count), (1, 1));
    Ok(())
}

/// A reused writer keeps its full history without binding earlier failed runs into a later success identity.
#[test]
fn reused_program_io_isolates_each_successful_execution() -> Result<(), Box<dyn std::error::Error>> {
    let failure = lower("def main() -> None:\n  println(\"before failure\")\n  assert false\n")?;
    let success = lower("def main() -> int:\n  println(\"success\")\n  return 42\n")?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut io = ProgramIo::new(&mut stdout, &mut stderr);
    io.write(ProgramStream::Stderr, b"earlier diagnostic")?;
    assert!(execute_free_function_with_io(&failure, "main", &[], &mut io).is_err());
    let first = execute_free_function_with_io(&success, "main", &[], &mut io)?;
    let second = execute_free_function_with_io(&success, "main", &[], &mut io)?;
    assert_eq!(first.output.stdout(), b"success\n");
    assert!(first.output.stderr().is_empty());
    assert_eq!(first.emitted_output(), ["success"]);
    assert_eq!(first.output_identity, second.output_identity);
    assert_eq!(io.output().stdout(), b"before failure\nsuccess\nsuccess\n");
    assert_eq!(io.output().stderr(), b"earlier diagnostic");
    let mut fresh_stdout = Vec::new();
    let mut fresh_stderr = Vec::new();
    let fresh = execute_free_function_with_io(
        &success,
        "main",
        &[],
        &mut ProgramIo::new(&mut fresh_stdout, &mut fresh_stderr),
    )?;
    assert_eq!(first.output_identity, fresh.output_identity);
    Ok(())
}
