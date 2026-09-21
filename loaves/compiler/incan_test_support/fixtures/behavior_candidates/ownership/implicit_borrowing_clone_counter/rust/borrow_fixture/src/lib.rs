//! A tree whose every clone is counted, so an Incan program can show how many copies its traversal made.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

static CLONES: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug)]
pub enum Item {
    Table(BTreeMap<String, Item>),
    Integer(i64),
}

impl Item {
    /// The child stored under `key` of a table, lent out without a copy.
    pub fn get(&self, key: &str) -> Option<&Item> {
        match self {
            Self::Table(items) => items.get(key),
            _ => None,
        }
    }

    /// The child stored under `key`, returned as an owned copy (counted).
    pub fn child(&self, key: &str) -> Option<Item> {
        self.get(key).cloned()
    }

    /// Whether this item is an integer leaf.
    pub fn is_integer(&self) -> bool {
        matches!(self, Self::Integer(_))
    }

    /// Whether this item is a table.
    pub fn is_table(&self) -> bool {
        matches!(self, Self::Table(_))
    }

    /// A stand-in source span, always present.
    pub fn span(&self) -> Option<usize> {
        Some(0)
    }
}

impl Clone for Item {
    /// Copy the item and count the copy.
    fn clone(&self) -> Self {
        CLONES.fetch_add(1, Ordering::SeqCst);
        match self {
            Self::Table(items) => Self::Table(items.clone()),
            Self::Integer(value) => Self::Integer(*value),
        }
    }
}

/// A two-level tree: `project.count = 7`.
pub fn document() -> Item {
    Item::Table(BTreeMap::from([(
        "project".into(),
        Item::Table(BTreeMap::from([("count".into(), Item::Integer(7))])),
    )]))
}

/// How many item copies were made so far.
pub fn clones() -> usize {
    CLONES.load(Ordering::SeqCst)
}
