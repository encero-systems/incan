//! Persistent scalar and text cells for canonical storage slots, process-wide as the language's statics are.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};

/// One typed cell table: slot to current value. Each critical section is a single map read or write, so no user code
/// runs under the lock.
pub(crate) struct Cells<T>(LazyLock<Mutex<HashMap<i64, T>>>);

impl<T: Clone> Cells<T> {
    /// An empty table, created on first use.
    pub(crate) const fn new() -> Self {
        Self(LazyLock::new(|| Mutex::new(HashMap::new())))
    }

    fn guard(&self) -> MutexGuard<'_, HashMap<i64, T>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The slot's current value, or its declared initial value before any write.
    pub(crate) fn read(&self, slot: i64, initial: T) -> T {
        match self.guard().get(&slot) {
            Some(value) => value.clone(),
            None => initial,
        }
    }

    /// Replace the slot's value.
    pub(crate) fn write(&self, slot: i64, value: T) {
        self.guard().insert(slot, value);
    }
}

static INTEGER_CELLS: Cells<i64> = Cells::new();
static FLOAT_CELLS: Cells<f64> = Cells::new();
static BOOLEAN_CELLS: Cells<bool> = Cells::new();
static TEXT_CELLS: Cells<String> = Cells::new();

/// Module initialization state by module key: absent before, 1 while its statics initialize, 2 once they have.
static MODULE_INITIALIZATION: Cells<i64> = Cells::new();

/// An owned copy of a text cell.
pub fn read_static_text(slot: i64, initial: String) -> String {
    TEXT_CELLS.read(slot, initial)
}

/// Replace a text cell.
pub fn write_static_text(slot: i64, value: String) {
    TEXT_CELLS.write(slot, value);
}

/// An integer cell, keeping a previously assigned value.
pub fn read_static_int(slot: i64, initial: i64) -> i64 {
    INTEGER_CELLS.read(slot, initial)
}

/// Persist an integer assignment.
pub fn write_static_int(slot: i64, value: i64) {
    INTEGER_CELLS.write(slot, value);
}

/// A float cell, keeping a previously assigned value.
pub fn read_static_float(slot: i64, initial: f64) -> f64 {
    FLOAT_CELLS.read(slot, initial)
}

/// Persist a float assignment.
pub fn write_static_float(slot: i64, value: f64) {
    FLOAT_CELLS.write(slot, value);
}

/// A boolean cell, keeping a previously assigned value.
pub fn read_static_bool(slot: i64, initial: bool) -> bool {
    BOOLEAN_CELLS.read(slot, initial)
}

/// Persist a boolean assignment.
pub fn write_static_bool(slot: i64, value: bool) {
    BOOLEAN_CELLS.write(slot, value);
}

/// Claim one module's static initialization. Only the first call claims it; a call while the module is still
/// initializing (from a function an initializer reaches) returns without claiming, and so does every later call.
pub fn begin_module_initialization(module: i64) -> bool {
    let mut states = MODULE_INITIALIZATION.guard();
    if states.get(&module).copied().unwrap_or(0) != 0 {
        return false;
    }
    states.insert(module, 1);
    true
}

/// Record that one module's statics have all been initialized.
pub fn end_module_initialization(module: i64) {
    MODULE_INITIALIZATION.write(module, 2);
}
