//! Byte owners whose bytes Incan slices: an owned vector, a borrowed slice, and a type that derefs to bytes.
use std::ops::Deref;

/// An owned vector of the bytes `incan bakery`.
pub fn owned() -> Vec<u8> {
    b"incan bakery".to_vec()
}

/// A wrapper whose bytes are reachable only through `Deref`.
pub struct Window(Vec<u8>);

impl Deref for Window {
    type Target = [u8];

    /// The wrapped bytes.
    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl Window {
    /// A window over the bytes `incan bakery`.
    pub fn new() -> Self {
        Self(owned())
    }

    /// The window's bytes as a borrowed slice.
    pub fn view(&self) -> &[u8] {
        &self.0
    }
}
