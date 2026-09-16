//! Integration tests for `incan_std_core::errors`.
//!
//! These tests exist to lock in **canonical, user-facing error string formatting** that the compiler/runtime rely on
//! (e.g. `Kind: message` prefixes).
//!
//! Note: `serde_json` lives in the `data` component's facet, not here, so this file uses a small `Display`-only fake
//! error to test the formatting logic the JSON runtime feeds through this helper.

use incan_std_core::errors::json_decode_error_string;
use std::fmt;

struct FakeJsonError(&'static str);

impl fmt::Display for FakeJsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

#[test]
/// `json_decode_error_string` must always prefix with `JSONDecodeError: ` and preserve the
/// underlying error text.
fn json_decode_error_string_is_prefixed() {
    let err = FakeJsonError("expected value at line 1 column 1");
    let formatted = json_decode_error_string(&err);

    assert_eq!(formatted, format!("JSONDecodeError: {}", err));
    assert!(
        formatted.contains("line") && formatted.contains("column"),
        "expected line/column info in JSON decode error: {formatted}"
    );
}
