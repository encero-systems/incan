//! Output writer with indentation tracking
//!
//! Handles writing formatted code with proper indentation.

use super::config::FormatConfig;

/// Snapshot of a writer position that can be restored after speculative formatting.
///
/// The formatter uses checkpoints when it tries an inline layout first and falls back to a multiline layout if the
/// inline version exceeds the configured line length or emits a newline.
#[derive(Clone, Copy)]
pub struct WriterCheckpoint {
    output_len: usize,
    indent_level: usize,
    at_line_start: bool,
    current_line_length: usize,
}

/// Writer that tracks indentation, line length, and the formatted output buffer.
pub struct FormatWriter {
    /// The output buffer
    output: String,
    /// Current indentation level
    indent_level: usize,
    /// Configuration
    config: FormatConfig,
    /// Whether we're at the start of a line
    at_line_start: bool,
    /// Current line length (for line wrapping decisions)
    current_line_length: usize,
}

impl FormatWriter {
    /// Create a new format writer with the given config
    pub fn new(config: FormatConfig) -> Self {
        Self {
            output: String::new(),
            indent_level: 0,
            config,
            at_line_start: true,
            current_line_length: 0,
        }
    }

    /// Get the formatted output
    pub fn finish(self) -> String {
        self.output
    }

    /// Capture the current output and indentation state.
    pub fn checkpoint(&self) -> WriterCheckpoint {
        WriterCheckpoint {
            output_len: self.output.len(),
            indent_level: self.indent_level,
            at_line_start: self.at_line_start,
            current_line_length: self.current_line_length,
        }
    }

    /// Restore the writer to a previously captured checkpoint.
    pub fn restore(&mut self, checkpoint: WriterCheckpoint) {
        self.output.truncate(checkpoint.output_len);
        self.indent_level = checkpoint.indent_level;
        self.at_line_start = checkpoint.at_line_start;
        self.current_line_length = checkpoint.current_line_length;
    }

    /// Return whether any text written since `checkpoint` contains a newline.
    pub fn output_since_contains_newline(&self, checkpoint: WriterCheckpoint) -> bool {
        self.output
            .get(checkpoint.output_len..)
            .is_some_and(|output| output.contains('\n'))
    }

    /// Return whether the current line is longer than the configured target length.
    pub fn line_length_exceeded(&self) -> bool {
        self.current_line_length > self.config.line_length
    }

    /// Return whether the next write will start a fresh line.
    pub fn is_at_line_start(&self) -> bool {
        self.at_line_start
    }

    /// Increase indentation level
    pub fn indent(&mut self) {
        self.indent_level += 1;
    }

    /// Decrease indentation level
    pub fn dedent(&mut self) {
        if self.indent_level > 0 {
            self.indent_level -= 1;
        }
    }

    /// Write indentation if at line start
    fn write_indent(&mut self) {
        if self.at_line_start {
            let indent = " ".repeat(self.indent_level * self.config.indent_width);
            self.output.push_str(&indent);
            self.current_line_length = indent.len();
            self.at_line_start = false;
        }
    }

    /// Write a string (with auto-indent)
    pub fn write(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.write_indent();
        self.output.push_str(s);
        self.current_line_length += s.len();
    }

    /// Write a string and newline
    pub fn writeln(&mut self, s: &str) {
        self.write(s);
        self.newline();
    }

    /// Write just a newline
    pub fn newline(&mut self) {
        self.output.push('\n');
        self.at_line_start = true;
        self.current_line_length = 0;
    }

    /// Write multiple blank lines (for spacing between declarations)
    pub fn blank_lines(&mut self, count: usize) {
        for _ in 0..count {
            self.newline();
        }
    }

    /// Write a space
    #[allow(dead_code)]
    pub fn space(&mut self) {
        self.write(" ");
    }

    /// Get current indentation level
    #[allow(dead_code)]
    pub fn current_indent(&self) -> usize {
        self.indent_level
    }

    /// Get the configuration
    pub fn config(&self) -> &FormatConfig {
        &self.config
    }

    /// Check if current line would exceed max length with additional text
    pub fn would_exceed_line_length(&self, additional: usize) -> bool {
        let indent_len = if self.at_line_start {
            self.indent_level * self.config.indent_width
        } else {
            0
        };
        self.current_line_length + indent_len + additional > self.config.line_length
    }

    /// Get remaining space on current line
    #[allow(dead_code)]
    pub fn remaining_line_space(&self) -> usize {
        let current = if self.at_line_start {
            self.indent_level * self.config.indent_width
        } else {
            self.current_line_length
        };
        self.config.line_length.saturating_sub(current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::QuoteStyle;

    /// One step of a writer script: an operation, or a check of the writer's state at that point.
    #[derive(Debug)]
    enum Step {
        Write(&'static str),
        Writeln(&'static str),
        Newline,
        Space,
        Indent,
        Dedent,
        BlankLines(usize),
        IndentIs(usize),
        RemainingIs(usize),
        Fits(usize),
        Exceeds(usize),
    }

    /// A check of the finished output.
    #[derive(Debug)]
    enum Output {
        Is(&'static str),
        StartsWith(&'static str),
        EndsWith(&'static str),
        Contains(&'static str),
    }

    struct WriterCase {
        name: &'static str,
        indent_width: Option<usize>,
        line_length: Option<usize>,
        steps: &'static [Step],
        output: &'static [Output],
    }

    impl WriterCase {
        const DEFAULT: WriterCase = WriterCase {
            name: "",
            indent_width: None,
            line_length: None,
            steps: &[],
            output: &[],
        };

        fn run(&self) -> Result<(), String> {
            let mut config = FormatConfig::default();
            if let Some(width) = self.indent_width {
                config = config.with_indent_width(width);
            }
            if let Some(length) = self.line_length {
                config = config.with_line_length(length);
            }
            let mut writer = FormatWriter::new(config);
            for (index, step) in self.steps.iter().enumerate() {
                let holds = match step {
                    Step::Write(text) => {
                        writer.write(text);
                        true
                    }
                    Step::Writeln(text) => {
                        writer.writeln(text);
                        true
                    }
                    Step::Newline => {
                        writer.newline();
                        true
                    }
                    Step::Space => {
                        writer.space();
                        true
                    }
                    Step::Indent => {
                        writer.indent();
                        true
                    }
                    Step::Dedent => {
                        writer.dedent();
                        true
                    }
                    Step::BlankLines(count) => {
                        writer.blank_lines(*count);
                        true
                    }
                    Step::IndentIs(level) => writer.current_indent() == *level,
                    Step::RemainingIs(columns) => writer.remaining_line_space() == *columns,
                    Step::Fits(columns) => !writer.would_exceed_line_length(*columns),
                    Step::Exceeds(columns) => writer.would_exceed_line_length(*columns),
                };
                if !holds {
                    return Err(format!("step {index} {step:?} does not hold"));
                }
            }
            let finished = writer.finish();
            for check in self.output {
                let holds = match check {
                    Output::Is(text) => finished == *text,
                    Output::StartsWith(text) => finished.starts_with(text),
                    Output::EndsWith(text) => finished.ends_with(text),
                    Output::Contains(text) => finished.contains(text),
                };
                if !holds {
                    return Err(format!("{check:?} does not hold for output {finished:?}"));
                }
            }
            Ok(())
        }
    }

    #[test]
    fn writer_scripts() {
        let failures: Vec<String> = WRITER_CASES
            .iter()
            .filter_map(|case| case.run().err().map(|problem| format!("{}: {problem}", case.name)))
            .collect();
        assert!(
            failures.is_empty(),
            "{} writer row(s) failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    #[test]
    fn test_config_returns_correct_config() {
        let writer = FormatWriter::new(FormatConfig::new().with_indent_width(2));
        assert_eq!(writer.config().indent_width, 2);

        let config = FormatConfig::new()
            .with_indent_width(8)
            .with_line_length(100)
            .with_quote_style(QuoteStyle::Single);
        let writer = FormatWriter::new(config);
        assert_eq!(writer.config().indent_width, 8);
        assert_eq!(writer.config().line_length, 100);
        assert_eq!(writer.config().quote_style, QuoteStyle::Single);
    }

    #[test]
    fn test_checkpoint_restore_round_trip() {
        let mut writer = FormatWriter::new(FormatConfig::default());
        writer.write("hello");
        let checkpoint = writer.checkpoint();
        writer.write(" world");
        assert!(!writer.output_since_contains_newline(checkpoint));
        writer.restore(checkpoint);
        assert_eq!(writer.finish(), "hello");
    }

    use Output::{Contains, EndsWith, Is, StartsWith};
    use Step::*;

    const WRITER_CASES: &[WriterCase] = &[
        WriterCase {
            name: "test_new_writer_empty_output",
            output: &[Is("")],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_new_writer_at_line_start",
            steps: &[IndentIs(0)],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_write_simple",
            steps: &[Write("hello")],
            output: &[Is("hello")],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_write_empty_string",
            steps: &[Write("")],
            output: &[Is("")],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_write_multiple",
            steps: &[Write("hello"), Write(" "), Write("world")],
            output: &[Is("hello world")],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_write_preserves_content",
            steps: &[Write("special chars: !@#$%^&*()")],
            output: &[Is("special chars: !@#$%^&*()")],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_writeln_adds_newline",
            steps: &[Writeln("hello")],
            output: &[Is("hello\n")],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_writeln_empty_string",
            steps: &[Writeln("")],
            output: &[Is("\n")],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_writeln_multiple",
            steps: &[Writeln("line1"), Writeln("line2")],
            output: &[Is("line1\nline2\n")],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_newline",
            steps: &[Newline],
            output: &[Is("\n")],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_multiple_newlines",
            steps: &[Newline, Newline, Newline],
            output: &[Is("\n\n\n")],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_newline_after_write",
            steps: &[Write("text"), Newline],
            output: &[Is("text\n")],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_indent_increases_level",
            steps: &[IndentIs(0), Indent, IndentIs(1)],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_dedent_decreases_level",
            steps: &[Indent, Indent, IndentIs(2), Dedent, IndentIs(1)],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_dedent_at_zero_stays_zero",
            steps: &[Dedent, IndentIs(0)],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_indent_affects_output",
            steps: &[Indent, Writeln("indented"), Dedent, Writeln("not indented")],
            output: &[StartsWith("    indented\n"), EndsWith("not indented\n")],
            ..WriterCase::DEFAULT
        },
        // 2 * 2 = 4 spaces.
        WriterCase {
            name: "test_double_indent",
            indent_width: Some(2),
            steps: &[Indent, Indent, Write("text")],
            output: &[Is("    text")],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_indent_width_4",
            indent_width: Some(4),
            steps: &[Indent, Write("text")],
            output: &[Is("    text")],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_indent_width_2",
            indent_width: Some(2),
            steps: &[Indent, Write("text")],
            output: &[Is("  text")],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_blank_lines_zero",
            steps: &[Write("before"), BlankLines(0), Write("after")],
            output: &[Is("beforeafter")],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_blank_lines_one",
            steps: &[BlankLines(1)],
            output: &[Is("\n")],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_blank_lines_multiple",
            steps: &[BlankLines(3)],
            output: &[Is("\n\n\n")],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_blank_lines_between_content",
            steps: &[Writeln("line1"), BlankLines(2), Writeln("line2")],
            output: &[Is("line1\n\n\nline2\n")],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_space",
            steps: &[Write("a"), Space, Write("b")],
            output: &[Is("a b")],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_would_exceed_line_length_at_start",
            line_length: Some(10),
            steps: &[Fits(5), Fits(10), Exceeds(11)],
            ..WriterCase::DEFAULT
        },
        // 5 + 5 = 10 is not exceeded; 5 + 6 = 11 is.
        WriterCase {
            name: "test_would_exceed_line_length_after_write",
            line_length: Some(10),
            steps: &[Write("hello"), Fits(5), Exceeds(6)],
            ..WriterCase::DEFAULT
        },
        // At line start with an indent, the pending 4 indent columns count: 4 + 6 = 10, 4 + 7 = 11.
        WriterCase {
            name: "test_would_exceed_with_indent",
            indent_width: Some(4),
            line_length: Some(10),
            steps: &[Indent, Fits(6), Exceeds(7)],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_remaining_line_space_at_start",
            line_length: Some(80),
            steps: &[RemainingIs(80)],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_remaining_line_space_after_write",
            line_length: Some(80),
            steps: &[Write("hello"), RemainingIs(75)],
            ..WriterCase::DEFAULT
        },
        // At line start, the remaining space accounts for the indent: 80 - 4.
        WriterCase {
            name: "test_remaining_line_space_with_indent",
            indent_width: Some(4),
            line_length: Some(80),
            steps: &[Indent, RemainingIs(76)],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_line_length_resets_after_newline",
            line_length: Some(80),
            steps: &[
                Write("this is a long line that takes up space"),
                Newline,
                RemainingIs(80),
            ],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_nested_indentation",
            indent_width: Some(2),
            steps: &[
                Writeln("fn main() {"),
                Indent,
                Writeln("if true {"),
                Indent,
                Writeln("println!(\"hello\");"),
                Dedent,
                Writeln("}"),
                Dedent,
                Writeln("}"),
            ],
            output: &[Is("fn main() {\n  if true {\n    println!(\"hello\");\n  }\n}\n")],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_code_block_generation",
            steps: &[
                Writeln("struct Point {"),
                Indent,
                Writeln("x: i32,"),
                Writeln("y: i32,"),
                Dedent,
                Writeln("}"),
            ],
            output: &[Contains("struct Point {"), Contains("    x: i32,"), Contains("}")],
            ..WriterCase::DEFAULT
        },
        WriterCase {
            name: "test_mixed_operations",
            steps: &[
                Write("a"),
                Space,
                Write("="),
                Space,
                Write("1"),
                Newline,
                BlankLines(1),
                Indent,
                Writeln("b = 2"),
            ],
            output: &[Is("a = 1\n\n    b = 2\n")],
            ..WriterCase::DEFAULT
        },
    ];
}
