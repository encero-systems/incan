//! Immutable module-owned interface tables with a consumer's resolved import bindings layered over them.

use std::collections::HashMap;
use std::sync::Arc;

/// A source module's direct declarations and the import bindings resolved for its current dependency context.
/// Imports retain the old last-inserted precedence without copying or mutating the module's finalized direct table.
#[derive(Debug)]
pub struct DependencyMembers<T> {
    pub(super) direct: Arc<HashMap<String, T>>,
    imported: HashMap<String, T>,
}

impl<T> Default for DependencyMembers<T> {
    /// Start an unshared empty shell table without requiring a default declaration value.
    fn default() -> Self {
        Self::from_direct(Arc::default())
    }
}

impl<T> DependencyMembers<T> {
    /// Share finalized module declarations while starting a fresh import projection.
    pub(super) fn from_direct(direct: Arc<HashMap<String, T>>) -> Self {
        Self {
            direct,
            imported: HashMap::new(),
        }
    }

    /// Read the resolved import when it replaces a direct spelling, otherwise the original module declaration.
    pub fn get(&self, name: &str) -> Option<&T> {
        self.imported.get(name).or_else(|| self.direct.get(name))
    }

    /// Return whether this module exposes a direct or imported binding under the requested spelling.
    pub fn contains_key(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Replace only this projection's import binding, preserving the shared declaration owner.
    pub(super) fn insert(&mut self, name: String, value: T) {
        self.imported.insert(name, value);
    }
}
