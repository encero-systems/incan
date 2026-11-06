//! Mutation-owned revisions for the checker's nominal inventories.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

/// An identity token that changes on mutation and cannot collide with a live earlier snapshot.
#[derive(Clone, Debug, Default)]
pub(crate) struct InventoryRevision(Arc<()>);

impl PartialEq for InventoryRevision {
    /// Compare revision identities rather than the empty values they own.
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for InventoryRevision {}

/// A nominal inventory whose mutation authority also invalidates its derived lookup facts.
/// Clones preserve a revision only while neither copy changes; mutable access replaces the token first.
#[derive(Clone, Debug)]
pub struct NominalInventory<T> {
    entries: HashMap<String, T>,
    revision: InventoryRevision,
}

impl<T> Default for NominalInventory<T> {
    /// Start an empty independently versioned inventory without requiring a default value type.
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            revision: InventoryRevision::default(),
        }
    }
}

impl<T> NominalInventory<T> {
    /// Retain a constant-size witness for the current inventory, including in-place target mutations.
    pub(crate) fn revision(&self) -> InventoryRevision {
        self.revision.clone()
    }
}

impl<T> Deref for NominalInventory<T> {
    type Target = HashMap<String, T>;

    /// Borrow entries without invalidating any derived facts.
    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl<T> DerefMut for NominalInventory<T> {
    /// Invalidate before granting any mutable access, including entries retained under an unchanged binding ID.
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.revision = InventoryRevision::default();
        &mut self.entries
    }
}

impl<'a, T> IntoIterator for &'a NominalInventory<T> {
    type Item = (&'a String, &'a T);
    type IntoIter = std::collections::hash_map::Iter<'a, String, T>;

    /// Iterate the current entries without transferring their ownership.
    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}
