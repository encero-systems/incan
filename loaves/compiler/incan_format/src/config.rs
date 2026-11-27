//! Formatting configuration for Incan
//!
//! Based on Ruff/Black conventions with customizations.

/// Formatting configuration
#[derive(Debug, Clone)]
pub struct FormatConfig {
    /// Number of spaces per indentation level
    pub indent_width: usize,
    /// Maximum line length before wrapping
    pub line_length: usize,
    /// Quote style for strings
    pub quote_style: QuoteStyle,
    /// Whether to use trailing commas in multi-line constructs
    pub trailing_commas: bool,
}

/// Quote style for string literals
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteStyle {
    Double,
    Single,
    Preserve,
}

impl Default for FormatConfig {
    fn default() -> Self {
        // Ruff/Black style with 120 char lines (Koheesio style)
        Self {
            indent_width: 4,
            line_length: 120,
            quote_style: QuoteStyle::Double,
            trailing_commas: true,
        }
    }
}

impl FormatConfig {
    /// Create a new config with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the indentation width
    pub fn with_indent_width(mut self, width: usize) -> Self {
        self.indent_width = width;
        self
    }

    /// Set the maximum line length
    pub fn with_line_length(mut self, length: usize) -> Self {
        self.line_length = length;
        self
    }

    /// Set the quote style
    pub fn with_quote_style(mut self, style: QuoteStyle) -> Self {
        self.quote_style = style;
        self
    }

    /// Set whether trailing commas are emitted in multi-line constructs
    pub fn with_trailing_commas(mut self, trailing: bool) -> Self {
        self.trailing_commas = trailing;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A built configuration and the indent width, line length and quote style it must hold.
    struct BuilderCase {
        name: &'static str,
        build: fn() -> FormatConfig,
        indent_width: usize,
        line_length: usize,
        quote_style: QuoteStyle,
    }

    const DEFAULT_INDENT: usize = 4;
    const DEFAULT_LENGTH: usize = 120;

    /// Each row starts from `FormatConfig::new()`; the fields it does not set keep their defaults.
    const BUILDER_CASES: &[BuilderCase] = &[
        BuilderCase {
            name: "test_with_indent_width",
            build: || FormatConfig::new().with_indent_width(2),
            indent_width: 2,
            line_length: DEFAULT_LENGTH,
            quote_style: QuoteStyle::Double,
        },
        BuilderCase {
            name: "test_with_indent_width_zero",
            build: || FormatConfig::new().with_indent_width(0),
            indent_width: 0,
            line_length: DEFAULT_LENGTH,
            quote_style: QuoteStyle::Double,
        },
        BuilderCase {
            name: "test_with_indent_width_large",
            build: || FormatConfig::new().with_indent_width(8),
            indent_width: 8,
            line_length: DEFAULT_LENGTH,
            quote_style: QuoteStyle::Double,
        },
        BuilderCase {
            name: "test_with_line_length",
            build: || FormatConfig::new().with_line_length(80),
            indent_width: DEFAULT_INDENT,
            line_length: 80,
            quote_style: QuoteStyle::Double,
        },
        BuilderCase {
            name: "test_with_line_length_short",
            build: || FormatConfig::new().with_line_length(40),
            indent_width: DEFAULT_INDENT,
            line_length: 40,
            quote_style: QuoteStyle::Double,
        },
        BuilderCase {
            name: "test_with_line_length_long",
            build: || FormatConfig::new().with_line_length(200),
            indent_width: DEFAULT_INDENT,
            line_length: 200,
            quote_style: QuoteStyle::Double,
        },
        BuilderCase {
            name: "test_with_quote_style_single",
            build: || FormatConfig::new().with_quote_style(QuoteStyle::Single),
            indent_width: DEFAULT_INDENT,
            line_length: DEFAULT_LENGTH,
            quote_style: QuoteStyle::Single,
        },
        BuilderCase {
            name: "test_with_quote_style_double",
            build: || FormatConfig::new().with_quote_style(QuoteStyle::Double),
            indent_width: DEFAULT_INDENT,
            line_length: DEFAULT_LENGTH,
            quote_style: QuoteStyle::Double,
        },
        BuilderCase {
            name: "test_with_quote_style_preserve",
            build: || FormatConfig::new().with_quote_style(QuoteStyle::Preserve),
            indent_width: DEFAULT_INDENT,
            line_length: DEFAULT_LENGTH,
            quote_style: QuoteStyle::Preserve,
        },
        BuilderCase {
            name: "test_builder_chain_all",
            build: || {
                FormatConfig::new()
                    .with_indent_width(2)
                    .with_line_length(80)
                    .with_quote_style(QuoteStyle::Single)
            },
            indent_width: 2,
            line_length: 80,
            quote_style: QuoteStyle::Single,
        },
        // This row and the next set the same two fields in opposite orders.
        BuilderCase {
            name: "test_builder_chain_order_independence",
            build: || FormatConfig::new().with_indent_width(2).with_line_length(80),
            indent_width: 2,
            line_length: 80,
            quote_style: QuoteStyle::Double,
        },
        BuilderCase {
            name: "test_builder_chain_order_independence_reversed",
            build: || FormatConfig::new().with_line_length(80).with_indent_width(2),
            indent_width: 2,
            line_length: 80,
            quote_style: QuoteStyle::Double,
        },
        // The last value wins.
        BuilderCase {
            name: "test_builder_override",
            build: || FormatConfig::new().with_indent_width(2).with_indent_width(8),
            indent_width: 8,
            line_length: DEFAULT_LENGTH,
            quote_style: QuoteStyle::Double,
        },
        // A clone holds the same fields.
        BuilderCase {
            name: "test_format_config_clone",
            build: || FormatConfig::new().with_indent_width(2).with_line_length(80).clone(),
            indent_width: 2,
            line_length: 80,
            quote_style: QuoteStyle::Double,
        },
    ];

    #[test]
    fn builder_cases() {
        let failures: Vec<String> = BUILDER_CASES
            .iter()
            .filter_map(|case| {
                let config = (case.build)();
                let held = (config.indent_width, config.line_length, config.quote_style);
                let wanted = (case.indent_width, case.line_length, case.quote_style);
                (held != wanted).then(|| format!("{}: held {held:?}, wanted {wanted:?}", case.name))
            })
            .collect();
        assert!(
            failures.is_empty(),
            "{} builder row(s) failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    #[test]
    fn quote_style_and_config_traits() {
        const STYLES: [(QuoteStyle, &str); 3] = [
            (QuoteStyle::Double, "Double"),
            (QuoteStyle::Single, "Single"),
            (QuoteStyle::Preserve, "Preserve"),
        ];
        for (index, (style, name)) in STYLES.iter().enumerate() {
            let copied: QuoteStyle = *style;
            assert_eq!(*style, copied, "{name} copies to an equal value");
            assert!(format!("{style:?}").contains(name), "{name} debug-prints its name");
            for (other, other_name) in &STYLES[index + 1..] {
                assert_ne!(style, other, "{name} differs from {other_name}");
            }
        }

        let debug_str = format!("{:?}", FormatConfig::default());
        assert!(debug_str.contains("FormatConfig"));
        assert!(debug_str.contains("indent_width"));
    }
}
