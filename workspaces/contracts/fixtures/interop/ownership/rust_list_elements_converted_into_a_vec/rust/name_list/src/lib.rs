//! A function taking a `Vec<Name>` whose element type is built from a `String`.
pub struct Name(String);

impl From<String> for Name {
    /// Wrap the string.
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// The summed length of every name.
pub fn total_length(names: Vec<Name>) -> usize {
    names.iter().map(|name| name.0.len()).sum()
}
