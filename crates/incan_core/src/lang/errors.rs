//! Builtin exception vocabulary (Python-like).
//!
//! This registry exists so docs/tooling can treat builtin exception names as part of the language vocabulary,
//! similar to keywords/operators/builtins.

use crate::errors::ErrorKind;

use super::registry::{Example, LangItemInfo, RFC, Since, Stability};

/// Metadata for a builtin exception kind.
pub type ExceptionInfo = LangItemInfo<ErrorKind>;

/// Registry of builtin exception kinds.
pub const EXCEPTIONS: &[ExceptionInfo] = &[
    info(
        ErrorKind::ValueError,
        "ValueError",
        "Raised when an operation receives a value of the right type but an invalid value.",
        RFC::_000,
        Since(0, 1),
        &[
            Example {
                code: r#"def main() -> None:
    print("abc"[0:3:0])  # step 0
"#,
                note: Some("Panics at runtime with `ValueError: slice step cannot be zero`."),
            },
            Example {
                code: r#"def main() -> None:
    _ = int("abc")
"#,
                note: Some("Panics at runtime with `ValueError: cannot convert 'abc' to int`."),
            },
            Example {
                code: r#"def main() -> None:
    _ = float("abc")
"#,
                note: Some("Panics at runtime with `ValueError: cannot convert 'abc' to float`."),
            },
            Example {
                code: r#"def main() -> None:
    # range step cannot be zero (Python-like)
    for i in range(0, 5, 0):
        print(i)
"#,
                note: Some("Panics at runtime with `ValueError: range() arg 3 must not be zero`."),
            },
        ],
    ),
    info(
        ErrorKind::TypeError,
        "TypeError",
        "Raised when an operation receives a value of an inappropriate type.",
        RFC::_000,
        Since(0, 1),
        &[Example {
            code: r#"def main() -> None:
    # Example: JSON serialization failures (e.g. NaN/Inf) raise TypeError
    _ = json_stringify(nan)
"#,
            note: Some("Panics at runtime with a `TypeError: ... is not JSON serializable` message."),
        }],
    ),
    info(
        ErrorKind::ZeroDivisionError,
        "ZeroDivisionError",
        "Raised when dividing or taking modulo by zero (Python-like numeric semantics).",
        RFC::_000,
        Since(0, 1),
        &[Example {
            code: r#"def main() -> None:
    print(1 / 0)
"#,
            note: Some("Panics at runtime with `ZeroDivisionError: float division by zero`."),
        }],
    ),
    info(
        ErrorKind::IndexError,
        "IndexError",
        "Raised when an index is out of bounds (e.g. string/list indexing).",
        RFC::_000,
        Since(0, 1),
        &[
            Example {
                code: r#"def main() -> None:
    print("a"[99])
"#,
                note: Some("Panics at runtime with `IndexError: ...`."),
            },
            Example {
                code: r#"def main() -> None:
    xs: list[int] = [1, 2, 3]
    print(xs[99])
"#,
                note: Some("Panics at runtime with `IndexError: index 99 out of range for list of length 3`."),
            },
        ],
    ),
    info(
        ErrorKind::KeyError,
        "KeyError",
        "Raised when a dict key is missing.",
        RFC::_000,
        Since(0, 1),
        &[Example {
            code: r#"def main() -> None:
    d: Dict[str, int] = {"a": 1}
    print(d["b"])
"#,
            note: Some("Panics at runtime with `KeyError: 'b' not found in dict`."),
        }],
    ),
    info(
        ErrorKind::JsonDecodeError,
        "JSONDecodeError",
        "Raised when parsing JSON fails (Python-like).",
        RFC::_000,
        Since(0, 1),
        &[Example {
            code: r#"@derive(Deserialize)
model User:
    name: str

def main() -> None:
    bad: str = "{"
    match User.from_json(bad):
        case Ok(u): print(u.name)
        case Err(e): print(e)
"#,
            note: Some(
                "`from_json` returns `Result[T, str]`; on failure the error string is prefixed with `JSONDecodeError: ...`.",
            ),
        }],
    ),
];

/// Return the canonical spelling for an exception kind (e.g. `"ValueError"`).
#[inline]
pub fn as_str(kind: ErrorKind) -> &'static str {
    info_for(kind).canonical
}

/// Return the user-facing description for an exception kind.
#[inline]
pub fn description(kind: ErrorKind) -> &'static str {
    info_for(kind).description
}

/// Return the documentation examples for an exception kind.
#[inline]
pub fn examples(kind: ErrorKind) -> &'static [Example] {
    info_for(kind).examples
}

/// Resolve a spelling to an exception kind.
///
/// Matching is case-sensitive.
pub fn from_str(name: &str) -> Option<ErrorKind> {
    if let Some(e) = EXCEPTIONS.iter().find(|e| e.canonical == name) {
        return Some(e.id);
    }
    EXCEPTIONS.iter().find(|e| e.aliases.contains(&name)).map(|e| e.id)
}

/// Return full metadata for an exception kind.
///
/// ## Panics
/// - If the registry is missing an entry for `kind` (programming error).
pub fn info_for(kind: ErrorKind) -> &'static ExceptionInfo {
    EXCEPTIONS
        .iter()
        .find(|e| e.id == kind)
        .expect("INVARIANT: exception info missing")
}

const fn info(
    id: ErrorKind,
    canonical: &'static str,
    description: &'static str,
    introduced_in_rfc: super::registry::RfcId,
    since: Since,
    examples: &'static [Example],
) -> ExceptionInfo {
    LangItemInfo {
        id,
        canonical,
        aliases: &[],
        description,
        introduced_in_rfc,
        since,
        stability: Stability::Stable,
        examples,
    }
}
