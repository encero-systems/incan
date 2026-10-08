//! Shared semantic-core coverage for Incan string length.

use incan_core::strings::str_len;

/// String length counts Unicode scalar values without normalizing the source text.
#[test]
fn string_length_counts_unicode_scalars() {
    for (value, expected) in [("", 0), ("abc", 3), ("é", 1), ("😀", 1), ("e\u{301}", 2)] {
        assert_eq!(str_len(value), expected, "{value:?}");
    }
}
