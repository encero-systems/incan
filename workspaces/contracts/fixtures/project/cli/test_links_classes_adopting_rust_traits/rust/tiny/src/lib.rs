//! A host that reads through a source the caller lends it.

/// What the host reads from: `read` has no default, `label` has one.
pub trait Source {
    /// Read one key.
    fn read(&self, key: i64) -> String;

    /// Name the source; the default names the host.
    fn label(&self) -> String {
        "host".to_owned()
    }
}

/// Read one key from a source, under the source's label.
pub fn ask<S: Source>(source: &S, key: i64) -> String {
    format!("{} {}", source.label(), source.read(key))
}
