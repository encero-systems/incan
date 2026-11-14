//! Native input with an explicit tag that cannot be interpreted as a declaration ordinal.

/// A Rust enum whose first variant's discriminant differs from its index.
#[repr(isize)]
pub enum Message {
    /// A record with one integer field.
    Record {
        /// The selected integer value.
        value: i64,
    } = 7,
    /// The empty alternative.
    Empty = 9,
}

/// Construct the explicitly tagged record.
pub fn message() -> Message {
    Message::Record { value: 7 }
}
