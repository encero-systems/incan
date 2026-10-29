//! Test-only clone observation for #2426: each clone of a `Counted` value increments a process-wide count.

use std::sync::atomic::{AtomicI64, Ordering};

static CLONES: AtomicI64 = AtomicI64::new(0);

/// A value whose every clone is counted.
#[derive(Debug, PartialEq)]
pub struct Counted {
    pub id: i64,
}

impl Counted {
    /// Create one counted value; creation itself is not a clone.
    pub fn new(id: i64) -> Self {
        Self { id }
    }
}

impl Clone for Counted {
    fn clone(&self) -> Self {
        CLONES.fetch_add(1, Ordering::SeqCst);
        Self { id: self.id }
    }
}

/// Read how many `Counted` values this process has cloned so far.
pub fn clone_count() -> i64 {
    CLONES.load(Ordering::SeqCst)
}
