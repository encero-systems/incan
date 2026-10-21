/// A native enum whose named payload slots must keep their own types, even in a reordered pattern.
pub enum Message {
    /// A count and owned text stored in declaration order.
    Record {
        /// The numeric payload.
        count: i64,
        /// The text payload.
        text: String,
    },
    /// A message without payload storage.
    Empty,
}
