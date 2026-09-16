//! Rust keyword vocabulary (for codegen identifier escaping).

/// Reserved + strict keywords in Rust.
pub const RUST_KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in",
    "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "static", "struct", "super", "trait", "true",
    "type", "unsafe", "use", "where", "while", "async", "await", "dyn", "abstract", "become", "box", "do", "final",
    "macro", "override", "priv", "typeof", "unsized", "virtual", "yield", "try",
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
