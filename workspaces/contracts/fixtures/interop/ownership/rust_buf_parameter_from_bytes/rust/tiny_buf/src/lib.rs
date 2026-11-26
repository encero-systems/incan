//! A `Buf`-shaped trait over byte slices, as the `prost` crate spells it.
pub trait Buf {
    /// How many bytes are left to read.
    fn remaining(&self) -> usize;
}

impl Buf for &[u8] {
    /// The slice length.
    fn remaining(&self) -> usize {
        self.len()
    }
}

/// Read the remaining count through the trait.
pub fn remaining(buf: impl Buf) -> usize {
    buf.remaining()
}
