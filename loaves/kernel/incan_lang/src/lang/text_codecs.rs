//! Text codec contract shared by the builtin `str.encode` and `bytes.decode` methods (#1668).
//!
//! The 0.6 builtin text return trip supports UTF-8 only. The typechecker uses these tables to reject an unsupported
//! literal label at compile time, and the emitter uses the same tables to build the runtime guard for a label that is
//! only known at run time, so the two stages cannot drift apart on what "UTF-8" is spelled like. Other codecs live in
//! `std.fs` (`read_text` / `write_text`) and `std.encoding`, which resolve WHATWG labels through `encoding_rs`.

/// Canonical spelling of the only encoding the builtin text codec surface supports.
pub const UTF8_ENCODING_LABEL: &str = "utf-8";

/// Every normalized spelling that names UTF-8 (see [`normalize_encoding_label`]).
pub const UTF8_ENCODING_LABELS: &[&str] = &["utf-8", "utf8"];

/// Normalize an encoding label the way Python's codec lookup does before matching: ASCII case is folded and `_` is
/// spelled `-`, so `UTF_8`, `Utf-8`, and `utf8` all resolve to a UTF-8 spelling in [`UTF8_ENCODING_LABELS`].
pub fn normalize_encoding_label(label: &str) -> String {
    label.trim().to_ascii_lowercase().replace('_', "-")
}

/// Return whether `label` names UTF-8 after normalization.
pub fn is_utf8_encoding_label(label: &str) -> bool {
    UTF8_ENCODING_LABELS.contains(&normalize_encoding_label(label).as_str())
}

/// How `bytes.decode` treats malformed input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecodeErrorsPolicy {
    /// Malformed input raises `ValueError` (Python's `UnicodeDecodeError` is a `ValueError`).
    Strict,
    /// Malformed sequences become U+FFFD replacement characters, so decoding never fails.
    Replace,
}

impl DecodeErrorsPolicy {
    /// The policy `bytes.decode` applies when the `errors` argument is omitted.
    pub const DEFAULT: Self = Self::Strict;

    /// Resolve an `errors` label; the spelling is exact because Python accepts no aliases here either.
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "strict" => Some(Self::Strict),
            "replace" => Some(Self::Replace),
            _ => None,
        }
    }

    /// Return the source spelling of this policy.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Replace => "replace",
        }
    }

    /// Every accepted `errors` spelling, in documentation order.
    pub const LABELS: &[&'static str] = &["strict", "replace"];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_labels_normalize_case_and_underscores() {
        for label in ["utf-8", "UTF-8", "Utf_8", "utf8", " UTF8 "] {
            assert!(is_utf8_encoding_label(label), "{label} should name UTF-8");
        }
        for label in ["latin-1", "ascii", "utf-16", "", "utf-88"] {
            assert!(!is_utf8_encoding_label(label), "{label} must not name UTF-8");
        }
    }

    #[test]
    fn decode_error_policies_are_exact_spellings() {
        assert_eq!(
            DecodeErrorsPolicy::from_label("strict"),
            Some(DecodeErrorsPolicy::Strict)
        );
        assert_eq!(
            DecodeErrorsPolicy::from_label("replace"),
            Some(DecodeErrorsPolicy::Replace)
        );
        assert_eq!(DecodeErrorsPolicy::from_label("Strict"), None);
        assert_eq!(DecodeErrorsPolicy::from_label("ignore"), None);
        assert_eq!(DecodeErrorsPolicy::DEFAULT.as_str(), "strict");
        assert_eq!(
            DecodeErrorsPolicy::LABELS,
            &[
                DecodeErrorsPolicy::Strict.as_str(),
                DecodeErrorsPolicy::Replace.as_str()
            ]
        );
    }
}
