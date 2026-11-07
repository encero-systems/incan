//! Native ownership for placement-neutral lock guards.
//!
//! Incan owns lock acquisition and placement policy. This carrier exists only because 0.6 cannot store a capturing
//! closure in a field (#1987); it type-erases the retained file or in-process anchor until that language gap closes.

use std::fs::File;
use std::sync::Arc;

/// A cloneable lifetime anchor for either an operating-system lock or an in-process lock identity.
#[derive(Clone, Debug)]
pub struct LockGuard {
    resource: Arc<LockResource>,
}

/// The resource whose drop releases one placement-specific guard.
#[derive(Debug)]
enum LockResource {
    File(File),
    Memory,
}

impl LockGuard {
    /// Retain the descriptor that owns an already-acquired operating-system lock.
    pub fn file(file: File) -> Self {
        Self {
            resource: Arc::new(LockResource::File(file)),
        }
    }

    /// Create an in-process anchor whose shared-reference count records live guards.
    pub fn memory() -> Self {
        Self {
            resource: Arc::new(LockResource::Memory),
        }
    }

    /// Return the number of anchors sharing this retained resource.
    pub fn reference_count(&self) -> i64 {
        match i64::try_from(Arc::strong_count(&self.resource)) {
            Ok(count) => count,
            Err(_) => i64::MAX,
        }
    }
}
