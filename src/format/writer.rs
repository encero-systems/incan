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
    use crate::format::config::QuoteStyle;

    fn default_writer() -> FormatWriter {
        FormatWriter::new(FormatConfig::default())
    }

    // ========================================
    // Constructor and finish tests
    // ========================================

    #[test]
    fn test_new_writer_empty_output() {
        let writer = default_writer();
        assert_eq!(writer.finish(), "");
    }

    #[test]
    fn test_new_writer_at_line_start() {
        let writer = default_writer();
        assert_eq!(writer.current_indent(), 0);
    }

    #[test]
    fn test_new_writer_with_custom_config() {
        let config = FormatConfig::new().with_indent_width(2);
        let writer = FormatWriter::new(config);
        assert_eq!(writer.config().indent_width, 2);
    }

    // ========================================
    // Write tests
    // ========================================

    #[test]
    fn test_write_simple() {
        let mut writer = default_writer();
        writer.write("hello");
        assert_eq!(writer.finish(), "hello");
    }

    #[test]
    fn test_write_empty_string() {
        let mut writer = default_writer();
        writer.write("");
        assert_eq!(writer.finish(), "");
    }

    #[test]
    fn test_write_multiple() {
        let mut writer = default_writer();
        writer.write("hello");
        writer.write(" ");
        writer.write("world");
        assert_eq!(writer.finish(), "hello world");
    }

    #[test]
    fn test_checkpoint_restore_round_trip() {
        let mut writer = default_writer();
        writer.write("hello");
        let checkpoint = writer.checkpoint();
        writer.write(" world");
        assert!(!writer.output_since_contains_newline(checkpoint));
        writer.restore(checkpoint);
        assert_eq!(writer.finish(), "hello");
    }

    #[test]
    fn test_write_preserves_content() {
        let mut writer = default_writer();
        writer.write("special chars: !@#$%^&*()");
        assert_eq!(writer.finish(), "special chars: !@#$%^&*()");
    }

    // ========================================
    // Writeln tests
    // ========================================

    #[test]
    fn test_writeln_adds_newline() {
        let mut writer = default_writer();
        writer.writeln("hello");
        assert_eq!(writer.finish(), "hello\n");
    }

    #[test]
    fn test_writeln_empty_string() {
        let mut writer = default_writer();
        writer.writeln("");
        assert_eq!(writer.finish(), "\n");
    }

    #[test]
    fn test_writeln_multiple() {
        let mut writer = default_writer();
        writer.writeln("line1");
        writer.writeln("line2");
        assert_eq!(writer.finish(), "line1\nline2\n");
    }

    // ========================================
    // Newline tests
    // ========================================

    #[test]
    fn test_newline() {
        let mut writer = default_writer();
        writer.newline();
        assert_eq!(writer.finish(), "\n");
    }

    #[test]
    fn test_multiple_newlines() {
        let mut writer = default_writer();
        writer.newline();
        writer.newline();
        writer.newline();
        assert_eq!(writer.finish(), "\n\n\n");
    }

    #[test]
    fn test_newline_after_write() {
        let mut writer = default_writer();
        writer.write("text");
        writer.newline();
        assert_eq!(writer.finish(), "text\n");
    }

    // ========================================
    // Indent/dedent tests
    // ========================================

    #[test]
    fn test_indent_increases_level() {
        let mut writer = default_writer();
        assert_eq!(writer.current_indent(), 0);
        writer.indent();
        assert_eq!(writer.current_indent(), 1);
    }

    #[test]
    fn test_dedent_decreases_level() {
        let mut writer = default_writer();
        writer.indent();
        writer.indent();
        assert_eq!(writer.current_indent(), 2);
        writer.dedent();
        assert_eq!(writer.current_indent(), 1);
    }

    #[test]
    fn test_dedent_at_zero_stays_zero() {
        let mut writer = default_writer();
        writer.dedent();
        assert_eq!(writer.current_indent(), 0);
    }

    #[test]
    fn test_indent_affects_output() {
        let mut writer = default_writer();
        writer.indent();
        writer.writeln("indented");
        writer.dedent();
        writer.writeln("not indented");
        let output = writer.finish();
        assert!(output.starts_with("    indented\n"));
        assert!(output.ends_with("not indented\n"));
    }

    #[test]
    fn test_double_indent() {
        let config = FormatConfig::new().with_indent_width(2);
        let mut writer = FormatWriter::new(config);
        writer.indent();
        writer.indent();
        writer.write("text");
        assert_eq!(writer.finish(), "    text"); // 2 * 2 = 4 spaces
    }

    #[test]
    fn test_indent_width_4() {
        let config = FormatConfig::new().with_indent_width(4);
        let mut writer = FormatWriter::new(config);
        writer.indent();
        writer.write("text");
        assert_eq!(writer.finish(), "    text");
    }

    #[test]
    fn test_indent_width_2() {
        let config = FormatConfig::new().with_indent_width(2);
        let mut writer = FormatWriter::new(config);
        writer.indent();
        writer.write("text");
        assert_eq!(writer.finish(), "  text");
    }

    // ========================================
    // Blank lines tests
    // ========================================

    #[test]
    fn test_blank_lines_zero() {
        let mut writer = default_writer();
        writer.write("before");
        writer.blank_lines(0);
        writer.write("after");
        assert_eq!(writer.finish(), "beforeafter");
    }

    #[test]
    fn test_blank_lines_one() {
        let mut writer = default_writer();
        writer.blank_lines(1);
        assert_eq!(writer.finish(), "\n");
    }

    #[test]
    fn test_blank_lines_multiple() {
        let mut writer = default_writer();
        writer.blank_lines(3);
        assert_eq!(writer.finish(), "\n\n\n");
    }

    #[test]
    fn test_blank_lines_between_content() {
        let mut writer = default_writer();
        writer.writeln("line1");
        writer.blank_lines(2);
        writer.writeln("line2");
        assert_eq!(writer.finish(), "line1\n\n\nline2\n");
    }

    // ========================================
    // Space tests
    // ========================================

    #[test]
    fn test_space() {
        let mut writer = default_writer();
        writer.write("a");
        writer.space();
        writer.write("b");
        assert_eq!(writer.finish(), "a b");
    }

    // ========================================
    // Config accessor tests
    // ========================================

    #[test]
    fn test_config_returns_correct_config() {
        let config = FormatConfig::new()
            .with_indent_width(8)
            .with_line_length(100)
            .with_quote_style(QuoteStyle::Single);
        let writer = FormatWriter::new(config);

        assert_eq!(writer.config().indent_width, 8);
        assert_eq!(writer.config().line_length, 100);
        assert_eq!(writer.config().quote_style, QuoteStyle::Single);
    }

    // ========================================
    // Line length tracking tests
    // ========================================

    #[test]
    fn test_would_exceed_line_length_at_start() {
        let config = FormatConfig::new().with_line_length(10);
        let writer = FormatWriter::new(config);
        assert!(!writer.would_exceed_line_length(5));
        assert!(!writer.would_exceed_line_length(10));
        assert!(writer.would_exceed_line_length(11));
    }

    #[test]
    fn test_would_exceed_line_length_after_write() {
        let config = FormatConfig::new().with_line_length(10);
        let mut writer = FormatWriter::new(config);
        writer.write("hello"); // 5 chars
        assert!(!writer.would_exceed_line_length(5)); // 5 + 5 = 10, not exceeded
        assert!(writer.would_exceed_line_length(6)); // 5 + 6 = 11, exceeded
    }

    #[test]
    fn test_would_exceed_with_indent() {
        let config = FormatConfig::new().with_indent_width(4).with_line_length(10);
        let mut writer = FormatWriter::new(config);
        writer.indent(); // Will add 4 spaces when we write
        // At line start with indent, we have 4 (indent) + additional
        assert!(!writer.would_exceed_line_length(6)); // 4 + 6 = 10
        assert!(writer.would_exceed_line_length(7)); // 4 + 7 = 11
    }

    #[test]
    fn test_remaining_line_space_at_start() {
        let config = FormatConfig::new().with_line_length(80);
        let writer = FormatWriter::new(config);
        assert_eq!(writer.remaining_line_space(), 80);
    }

    #[test]
    fn test_remaining_line_space_after_write() {
        let config = FormatConfig::new().with_line_length(80);
        let mut writer = FormatWriter::new(config);
        writer.write("hello"); // 5 chars
        assert_eq!(writer.remaining_line_space(), 75);
    }

    #[test]
    fn test_remaining_line_space_with_indent() {
        let config = FormatConfig::new().with_indent_width(4).with_line_length(80);
        let mut writer = FormatWriter::new(config);
        writer.indent();
        // At line start, remaining should account for indent
        assert_eq!(writer.remaining_line_space(), 76); // 80 - 4
    }

    #[test]
    fn test_line_length_resets_after_newline() {
        let config = FormatConfig::new().with_line_length(80);
        let mut writer = FormatWriter::new(config);
        writer.write("this is a long line that takes up space");
        writer.newline();
        assert_eq!(writer.remaining_line_space(), 80);
    }

    // ========================================
    // Complex scenarios
    // ========================================

    #[test]
    fn test_nested_indentation() {
        let config = FormatConfig::new().with_indent_width(2);
        let mut writer = FormatWriter::new(config);

        writer.writeln("fn main() {");
        writer.indent();
        writer.writeln("if true {");
        writer.indent();
        writer.writeln("println!(\"hello\");");
        writer.dedent();
        writer.writeln("}");
        writer.dedent();
        writer.writeln("}");

        let expected = "fn main() {\n  if true {\n    println!(\"hello\");\n  }\n}\n";
        assert_eq!(writer.finish(), expected);
    }

    #[test]
    fn test_code_block_generation() {
        let mut writer = default_writer();

        writer.writeln("struct Point {");
        writer.indent();
        writer.writeln("x: i32,");
        writer.writeln("y: i32,");
        writer.dedent();
        writer.writeln("}");

        let output = writer.finish();
        assert!(output.contains("struct Point {"));
        assert!(output.contains("    x: i32,"));
        assert!(output.contains("}"));
    }

    #[test]
    fn test_mixed_operations() {
        let mut writer = default_writer();

        writer.write("a");
        writer.space();
        writer.write("=");
        writer.space();
        writer.write("1");
        writer.newline();
        writer.blank_lines(1);
        writer.indent();
        writer.writeln("b = 2");

        let output = writer.finish();
        assert_eq!(output, "a = 1\n\n    b = 2\n");
    }
}
