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

    // ========================================
    // Default config tests
    // ========================================

    #[test]
    fn test_default_config_indent_width() {
        let config = FormatConfig::default();
        assert_eq!(config.indent_width, 4);
    }

    #[test]
    fn test_default_config_line_length() {
        let config = FormatConfig::default();
        assert_eq!(config.line_length, 120);
    }

    #[test]
    fn test_default_config_quote_style() {
        let config = FormatConfig::default();
        assert_eq!(config.quote_style, QuoteStyle::Double);
    }

    #[test]
    fn test_default_config_trailing_commas() {
        let config = FormatConfig::default();
        assert!(config.trailing_commas);
    }

    // ========================================
    // Constructor tests
    // ========================================

    #[test]
    fn test_new_equals_default() {
        let new_config = FormatConfig::new();
        let default_config = FormatConfig::default();
        assert_eq!(new_config.indent_width, default_config.indent_width);
        assert_eq!(new_config.line_length, default_config.line_length);
        assert_eq!(new_config.quote_style, default_config.quote_style);
        assert_eq!(new_config.trailing_commas, default_config.trailing_commas);
    }

    // ========================================
    // Builder method tests
    // ========================================

    #[test]
    fn test_with_indent_width() {
        let config = FormatConfig::new().with_indent_width(2);
        assert_eq!(config.indent_width, 2);
        // Other fields unchanged
        assert_eq!(config.line_length, 120);
    }

    #[test]
    fn test_with_indent_width_zero() {
        let config = FormatConfig::new().with_indent_width(0);
        assert_eq!(config.indent_width, 0);
    }

    #[test]
    fn test_with_indent_width_large() {
        let config = FormatConfig::new().with_indent_width(8);
        assert_eq!(config.indent_width, 8);
    }

    #[test]
    fn test_with_line_length() {
        let config = FormatConfig::new().with_line_length(80);
        assert_eq!(config.line_length, 80);
        // Other fields unchanged
        assert_eq!(config.indent_width, 4);
    }

    #[test]
    fn test_with_line_length_short() {
        let config = FormatConfig::new().with_line_length(40);
        assert_eq!(config.line_length, 40);
    }

    #[test]
    fn test_with_line_length_long() {
        let config = FormatConfig::new().with_line_length(200);
        assert_eq!(config.line_length, 200);
    }

    #[test]
    fn test_with_quote_style_single() {
        let config = FormatConfig::new().with_quote_style(QuoteStyle::Single);
        assert_eq!(config.quote_style, QuoteStyle::Single);
    }

    #[test]
    fn test_with_quote_style_double() {
        let config = FormatConfig::new().with_quote_style(QuoteStyle::Double);
        assert_eq!(config.quote_style, QuoteStyle::Double);
    }

    #[test]
    fn test_with_quote_style_preserve() {
        let config = FormatConfig::new().with_quote_style(QuoteStyle::Preserve);
        assert_eq!(config.quote_style, QuoteStyle::Preserve);
    }

    // ========================================
    // Builder chaining tests
    // ========================================

    #[test]
    fn test_builder_chain_all() {
        let config = FormatConfig::new()
            .with_indent_width(2)
            .with_line_length(80)
            .with_quote_style(QuoteStyle::Single);

        assert_eq!(config.indent_width, 2);
        assert_eq!(config.line_length, 80);
        assert_eq!(config.quote_style, QuoteStyle::Single);
    }

    #[test]
    fn test_builder_chain_order_independence() {
        let config1 = FormatConfig::new().with_indent_width(2).with_line_length(80);

        let config2 = FormatConfig::new().with_line_length(80).with_indent_width(2);

        assert_eq!(config1.indent_width, config2.indent_width);
        assert_eq!(config1.line_length, config2.line_length);
    }

    #[test]
    fn test_builder_override() {
        let config = FormatConfig::new().with_indent_width(2).with_indent_width(8);

        assert_eq!(config.indent_width, 8); // Last value wins
    }

    // ========================================
    // QuoteStyle tests
    // ========================================

    #[test]
    fn test_quote_style_equality() {
        assert_eq!(QuoteStyle::Double, QuoteStyle::Double);
        assert_eq!(QuoteStyle::Single, QuoteStyle::Single);
        assert_eq!(QuoteStyle::Preserve, QuoteStyle::Preserve);
    }

    #[test]
    fn test_quote_style_inequality() {
        assert_ne!(QuoteStyle::Double, QuoteStyle::Single);
        assert_ne!(QuoteStyle::Single, QuoteStyle::Preserve);
        assert_ne!(QuoteStyle::Double, QuoteStyle::Preserve);
    }

    #[test]
    fn test_quote_style_clone() {
        let style = QuoteStyle::Double;
        let cloned = style;
        assert_eq!(style, cloned);
    }

    #[test]
    fn test_quote_style_copy() {
        let style = QuoteStyle::Single;
        let copied: QuoteStyle = style; // Copy
        assert_eq!(style, copied);
    }

    // ========================================
    // Debug trait tests
    // ========================================

    #[test]
    fn test_format_config_debug() {
        let config = FormatConfig::default();
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("FormatConfig"));
        assert!(debug_str.contains("indent_width"));
    }

    #[test]
    fn test_quote_style_debug() {
        assert!(format!("{:?}", QuoteStyle::Double).contains("Double"));
        assert!(format!("{:?}", QuoteStyle::Single).contains("Single"));
        assert!(format!("{:?}", QuoteStyle::Preserve).contains("Preserve"));
    }

    // ========================================
    // Clone trait tests
    // ========================================

    #[test]
    fn test_format_config_clone() {
        let config = FormatConfig::new().with_indent_width(2).with_line_length(80);

        let cloned = config.clone();
        assert_eq!(config.indent_width, cloned.indent_width);
        assert_eq!(config.line_length, cloned.line_length);
        assert_eq!(config.quote_style, cloned.quote_style);
    }
}
