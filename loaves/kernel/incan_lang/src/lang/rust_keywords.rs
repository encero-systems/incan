//! Rust keyword vocabulary (for codegen identifier escaping).

/// The strict and reserved keywords of Rust 2024, the edition generated code is compiled with.
///
/// `self`, `Self`, `crate` and `super` are strict keywords too, but cannot be raw identifiers, so the escaping helpers
/// leave them alone. `gen` is reserved from the 2024 edition on (#1561).
pub const RUST_KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in",
    "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "static", "struct", "super", "trait", "true",
    "type", "unsafe", "use", "where", "while", "async", "await", "dyn", "abstract", "become", "box", "do", "final",
    "macro", "override", "priv", "typeof", "unsized", "virtual", "yield", "try", "gen",
];

/// Check whether an identifier is a Rust keyword.
pub fn is_keyword(name: &str) -> bool {
    RUST_KEYWORDS.contains(&name)
}

/// Escape a Rust keyword by prepending `r#`.
///
/// Returns the name unchanged if it is not a keyword. `self` and `Self` are never escaped since they cannot be used as
/// raw identifiers in Rust.
pub fn escape_keyword(name: &str) -> String {
    if matches!(name, "self" | "Self") {
        return name.to_string();
    }
    if is_keyword(name) {
        return format!("r#{}", name);
    }
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::{escape_keyword, is_keyword};

    /// #1561: `gen` is reserved in Rust 2024, so a generated identifier spelled `gen` is a raw identifier.
    #[test]
    fn rust_2024_reserved_gen_is_escaped() {
        assert!(is_keyword("gen"));
        assert_eq!(escape_keyword("gen"), "r#gen");
        assert_eq!(escape_keyword("generator"), "generator");
        assert_eq!(escape_keyword("self"), "self");
    }
}
