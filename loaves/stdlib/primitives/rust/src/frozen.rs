//! Foundational immutable text and byte carriers for baked program data.

use core::borrow::Borrow;
use core::fmt;

/// Represent an immutable string baked into the binary.
///
/// The carrier borrows a program-lifetime string and is suitable for module-level constant initializers.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FrozenStr(&'static str);

impl FrozenStr {
    /// Construct a frozen string from a program-lifetime string slice.
    pub const fn new(value: &'static str) -> Self {
        Self(value)
    }

    /// Return the underlying string slice.
    pub const fn as_str(&self) -> &'static str {
        self.0
    }

    /// Return the string length in bytes.
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// Return whether the string is empty.
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for FrozenStr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, formatter)
    }
}

impl fmt::Display for FrozenStr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl AsRef<str> for FrozenStr {
    fn as_ref(&self) -> &str {
        self.0
    }
}

/// Let a frozen-text key answer a lookup through a borrowed text probe.
impl Borrow<str> for FrozenStr {
    fn borrow(&self) -> &str {
        self.0
    }
}

/// Represent immutable bytes baked into the binary.
///
/// The carrier borrows program-lifetime bytes and is suitable for module-level constant initializers.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FrozenBytes(&'static [u8]);

impl FrozenBytes {
    /// Construct frozen bytes from a program-lifetime byte slice.
    pub const fn new(bytes: &'static [u8]) -> Self {
        Self(bytes)
    }

    /// Return the underlying byte slice.
    pub const fn as_slice(&self) -> &'static [u8] {
        self.0
    }

    /// Return the number of bytes.
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// Return whether the byte slice is empty.
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterate over the bytes.
    pub fn iter(&self) -> core::slice::Iter<'static, u8> {
        self.0.iter()
    }
}

impl fmt::Debug for FrozenBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("FrozenBytes").field(&self.0).finish()
    }
}

impl fmt::Display for FrozenBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("b\"")?;
        for byte in self.0 {
            if *byte == b'\\' || *byte == b'"' {
                write!(formatter, "\\{}", *byte as char)?;
            } else if byte.is_ascii_graphic() || *byte == b' ' {
                write!(formatter, "{}", *byte as char)?;
            } else {
                write!(formatter, "\\x{:02x}", byte)?;
            }
        }
        formatter.write_str("\"")
    }
}

impl AsRef<[u8]> for FrozenBytes {
    fn as_ref(&self) -> &[u8] {
        self.0
    }
}

impl core::ops::Index<usize> for FrozenBytes {
    type Output = u8;

    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}
