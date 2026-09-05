//! Generated-Rust runtime coverage for Incan string length.

/// The runtime wrapper delegates to the shared Unicode-scalar contract for owned and borrowed strings.
#[test]
fn runtime_string_length_counts_unicode_scalars() {
    let owned = "😀".to_string();
    assert_eq!(incan_stdlib::strings::str_len(&owned), 1);
    assert_eq!(incan_stdlib::strings::str_len("e\u{301}"), 2);
}
