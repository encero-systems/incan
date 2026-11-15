//! Rust type shapes: parsing and rendering of Rust type display text.

use super::*;

/// Canonical Rust display spelling for the uninhabited never type.
pub const RUST_NEVER_TYPE_DISPLAY: &str = "!";

/// Structured Rust type information used by Incan interop consumers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RustTypeShape {
    /// Rust's uninhabited never type (`!`).
    Never,
    /// Any Rust `bool`.
    Bool,
    /// Any floating-point scalar. Width is intentionally erased at this layer.
    Float,
    /// Any signed or unsigned integer scalar. Width is intentionally erased at this layer.
    Int,
    /// UTF-8 string data such as `str` or `String`.
    Str,
    /// Byte buffers such as `Vec<u8>` or `&[u8]`.
    Bytes,
    /// The unit type `()`.
    Unit,
    /// An `Option<T>`-like wrapper.
    Option(Box<RustTypeShape>),
    /// A `Result<T, E>`-like wrapper.
    Result(Box<RustTypeShape>, Box<RustTypeShape>),
    /// A tuple shape with one entry per element.
    Tuple(Vec<RustTypeShape>),
    /// A shared or mutable reference.
    Ref(Box<RustTypeShape>),
    /// A concrete Rust path plus any generic arguments preserved by the extractor.
    RustPath { path: String, args: Vec<RustTypeShape> },
    /// A generic type parameter such as `T`.
    TypeParam(String),
    /// Metadata recovery could not determine a stable semantic shape.
    Unknown,
}

/// Fallback for a parsed Rust type name after structured primitives, wrappers, and path resolution have failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RustTypeShapePathFallback {
    /// Leave unresolved names as unknown. Use this when an extractor has an authoritative name resolver.
    Unknown,
    /// Preserve unresolved names as Rust paths. Use this for surfaces that already canonicalized their path text.
    RustPath,
}

/// Parse a Rust type display into the shared structural shape used by Rust interop consumers.
///
/// Callers own path resolution because HIR extraction can resolve source-relative names, while textual fallback
/// surfaces have already normalized module paths textually. Keeping the parser here prevents those two routes from
/// reimplementing primitive, wrapper, tuple, reference, and byte-buffer classification independently.
#[must_use]
pub fn parse_rust_type_shape_text<F>(
    text: &str,
    mut resolve_path: F,
    fallback: RustTypeShapePathFallback,
) -> RustTypeShape
where
    F: FnMut(&str) -> Option<String>,
{
    parse_rust_type_shape_text_inner(text, &mut resolve_path, fallback)
}

/// Parse normalized Rust type-display text recursively while preserving the caller's path-resolution policy.
pub(super) fn parse_rust_type_shape_text_inner<F>(
    text: &str,
    resolve_path: &mut F,
    fallback: RustTypeShapePathFallback,
) -> RustTypeShape
where
    F: FnMut(&str) -> Option<String>,
{
    let text = strip_rust_borrow_lifetimes(text).trim().replace(' ', "");
    if text.is_empty() {
        return RustTypeShape::Unknown;
    }
    if text == RUST_NEVER_TYPE_DISPLAY {
        return RustTypeShape::Never;
    }
    match text.as_str() {
        "bool" => return RustTypeShape::Bool,
        "f32" | "f64" => return RustTypeShape::Float,
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128" | "usize" => {
            return RustTypeShape::Int;
        }
        "str" | "String" | "std::string::String" | "alloc::string::String" => return RustTypeShape::Str,
        "()" => return RustTypeShape::Unit,
        "[u8]" => return RustTypeShape::Bytes,
        _ => {}
    }

    if let Some(inner) = text.strip_prefix('&') {
        let inner = inner.strip_prefix("mut").unwrap_or(inner).trim();
        return RustTypeShape::Ref(Box::new(parse_rust_type_shape_text_inner(
            inner,
            resolve_path,
            fallback,
        )));
    }

    if text.starts_with('(') && text.ends_with(')') {
        let inner = &text[1..text.len() - 1];
        if inner.is_empty() {
            return RustTypeShape::Unit;
        }
        return RustTypeShape::Tuple(
            split_top_level_rust_args(inner)
                .into_iter()
                .map(|arg| parse_rust_type_shape_text_inner(arg, resolve_path, fallback))
                .collect(),
        );
    }

    if let Some(start) = text.find('<')
        && text.ends_with('>')
    {
        let raw_base = &text[..start];
        let base = resolve_path(raw_base).unwrap_or_else(|| raw_base.to_string());
        let inner = &text[start + 1..text.len() - 1];
        let args: Vec<RustTypeShape> = split_top_level_rust_args(inner)
            .into_iter()
            .map(|arg| parse_rust_type_shape_text_inner(arg, resolve_path, fallback))
            .collect();
        match base.as_str() {
            "Option" | "std::option::Option" | "core::option::Option" => {
                return RustTypeShape::Option(Box::new(args.into_iter().next().unwrap_or(RustTypeShape::Unknown)));
            }
            "Result" | "std::result::Result" | "core::result::Result" => {
                let mut it = args.into_iter();
                return RustTypeShape::Result(
                    Box::new(it.next().unwrap_or(RustTypeShape::Unknown)),
                    Box::new(it.next().unwrap_or(RustTypeShape::Unknown)),
                );
            }
            "Vec" | "std::vec::Vec" | "alloc::vec::Vec"
                if matches!(args.first(), Some(RustTypeShape::Int)) && text.ends_with("<u8>") =>
            {
                return RustTypeShape::Bytes;
            }
            _ => {}
        }
        return RustTypeShape::RustPath { path: base, args };
    }

    if let Some(path) = resolve_path(text.as_str()) {
        return RustTypeShape::RustPath { path, args: Vec::new() };
    }

    if rust_type_shape_text_looks_like_type_param(text.as_str()) {
        return RustTypeShape::TypeParam(text);
    }

    match fallback {
        RustTypeShapePathFallback::Unknown => RustTypeShape::Unknown,
        RustTypeShapePathFallback::RustPath => RustTypeShape::RustPath {
            path: text,
            args: Vec::new(),
        },
    }
}

/// Return whether unresolved type text has the shape of a Rust generic type parameter.
pub(super) fn rust_type_shape_text_looks_like_type_param(text: &str) -> bool {
    !text.is_empty()
        && !text.contains("::")
        && !text.contains(['<', '>', '(', ')', '[', ']', '&', ' '])
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && text.chars().next().is_some_and(|ch| ch.is_ascii_uppercase())
}

/// Render `path` with generic arguments as `path<A, B, ...>` for stable Rust-like display.
#[must_use]
pub fn render_rust_type_shape_path(path: &str, args: &[RustTypeShape]) -> String {
    if args.is_empty() {
        return path.to_string();
    }
    let rendered_args: Vec<String> = args.iter().map(render_rust_type_shape).collect();
    format!("{path}<{}>", rendered_args.join(", "))
}

/// Pretty-print a [`RustTypeShape`] as a stable Rust-like type string.
#[must_use]
pub fn render_rust_type_shape(shape: &RustTypeShape) -> String {
    match shape {
        RustTypeShape::Never => RUST_NEVER_TYPE_DISPLAY.to_string(),
        RustTypeShape::Bool => "bool".to_string(),
        RustTypeShape::Float => "f64".to_string(),
        RustTypeShape::Int => "i64".to_string(),
        RustTypeShape::Str => "String".to_string(),
        RustTypeShape::Bytes => "Vec<u8>".to_string(),
        RustTypeShape::Unit => "()".to_string(),
        RustTypeShape::Option(inner) => format!("Option<{}>", render_rust_type_shape(inner)),
        RustTypeShape::Result(ok, err) => {
            format!(
                "Result<{}, {}>",
                render_rust_type_shape(ok),
                render_rust_type_shape(err)
            )
        }
        RustTypeShape::Tuple(items) => {
            let rendered: Vec<String> = items.iter().map(render_rust_type_shape).collect();
            format!("({})", rendered.join(", "))
        }
        RustTypeShape::Ref(inner) => format!("&{}", render_rust_type_shape(inner)),
        RustTypeShape::RustPath { path, args } => render_rust_type_shape_path(path, args),
        RustTypeShape::TypeParam(name) => name.clone(),
        RustTypeShape::Unknown => "?".to_string(),
    }
}

/// Remove Rust lifetime labels that decorate borrowed display types.
#[must_use]
pub fn strip_rust_borrow_lifetimes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        out.push(ch);
        if ch != '&' {
            continue;
        }
        while matches!(chars.peek(), Some(next) if next.is_whitespace()) {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        }
        if !matches!(chars.peek(), Some('\'')) {
            continue;
        }
        chars.next();
        while matches!(chars.peek(), Some(next) if next.is_ascii_alphanumeric() || *next == '_') {
            chars.next();
        }
        while matches!(chars.peek(), Some(next) if next.is_whitespace()) {
            chars.next();
        }
    }
    out
}

/// Split a comma-separated Rust generic/tuple argument list without splitting inside nested generic, tuple, or slice
/// delimiters. A function return arrow does not close an enclosing generic argument list.
#[must_use]
pub fn split_top_level_rust_args(text: &str) -> Vec<&str> {
    let mut args = Vec::new();
    let mut start = 0usize;
    let mut angle = 0usize;
    let mut paren = 0usize;
    let mut bracket = 0usize;
    for (idx, ch) in text.char_indices() {
        match ch {
            '<' => angle += 1,
            '>' if !text[..idx].ends_with('-') => angle = angle.saturating_sub(1),
            '(' => paren += 1,
            ')' => paren = paren.saturating_sub(1),
            '[' => bracket += 1,
            ']' => bracket = bracket.saturating_sub(1),
            ',' if angle == 0 && paren == 0 && bracket == 0 => {
                args.push(text[start..idx].trim());
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        args.push(tail);
    }
    args
}
