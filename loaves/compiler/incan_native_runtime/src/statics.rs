//! Persistent scalar and text cells for canonical storage slots, process-wide as the language's statics are.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};

/// `None` is the shared seed namespace. `Some(unit)` is an exact compiled-crate ID, including zero.
type CellKey = (Option<u64>, i64);

/// One typed cell table, keyed by unit and slot in that unit. No user code runs under the lock.
pub(crate) struct Cells<T>(LazyLock<Mutex<HashMap<CellKey, T>>>);

impl<T: Clone> Cells<T> {
    /// An empty table, created on first use.
    pub(crate) const fn new() -> Self {
        Self(LazyLock::new(|| Mutex::new(HashMap::new())))
    }

    /// Lock this table, recovering its contents if a thread panicked while holding it.
    fn guard(&self) -> MutexGuard<'_, HashMap<CellKey, T>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The cell's current value, or its declared initial value before any write.
    pub(crate) fn read(&self, unit: Option<u64>, slot: i64, initial: T) -> T {
        match self.guard().get(&(unit, slot)) {
            Some(value) => value.clone(),
            None => initial,
        }
    }

    /// Replace this unit's value at `slot`.
    pub(crate) fn write(&self, unit: Option<u64>, slot: i64, value: T) {
        self.guard().insert((unit, slot), value);
    }
}

static INTEGER_CELLS: Cells<i64> = Cells::new();
static FLOAT_CELLS: Cells<f64> = Cells::new();
static BOOLEAN_CELLS: Cells<bool> = Cells::new();
static TEXT_CELLS: Cells<String> = Cells::new();

/// Module initialization state by unit and module key. Absent before initialization, 1 during it, and 2 afterward.
static MODULE_INITIALIZATION: Cells<i64> = Cells::new();

/// Copy a text cell in the seed's shared slot namespace.
pub fn read_static_text(slot: i64, initial: String) -> String {
    TEXT_CELLS.read(None, slot, initial)
}

/// Replace a text cell in the seed's shared slot namespace.
pub fn write_static_text(slot: i64, value: String) {
    TEXT_CELLS.write(None, slot, value);
}

/// Read an integer cell in the seed's shared slot namespace.
pub fn read_static_int(slot: i64, initial: i64) -> i64 {
    INTEGER_CELLS.read(None, slot, initial)
}

/// Persist an integer assignment in the seed's shared slot namespace.
pub fn write_static_int(slot: i64, value: i64) {
    INTEGER_CELLS.write(None, slot, value);
}

/// Read a float cell in the seed's shared slot namespace.
pub fn read_static_float(slot: i64, initial: f64) -> f64 {
    FLOAT_CELLS.read(None, slot, initial)
}

/// Persist a float assignment in the seed's shared slot namespace.
pub fn write_static_float(slot: i64, value: f64) {
    FLOAT_CELLS.write(None, slot, value);
}

/// Read a boolean cell in the seed's shared slot namespace.
pub fn read_static_bool(slot: i64, initial: bool) -> bool {
    BOOLEAN_CELLS.read(None, slot, initial)
}

/// Persist a boolean assignment in the seed's shared slot namespace.
pub fn write_static_bool(slot: i64, value: bool) {
    BOOLEAN_CELLS.write(None, slot, value);
}

/// Claim initialization of one module in the seed's shared slot namespace.
pub fn begin_module_initialization(module: i64) -> bool {
    claim_module_initialization(None, module)
}

/// Record completion of initialization in the seed's shared slot namespace.
pub fn end_module_initialization(module: i64) {
    MODULE_INITIALIZATION.write(None, module, 2);
}

/// Copy the unit's current text value or return its declared initial value.
pub fn read_static_text_in_unit(unit: u64, slot: i64, initial: String) -> String {
    TEXT_CELLS.read(Some(unit), slot, initial)
}

/// Replace the unit's text cell.
pub fn write_static_text_in_unit(unit: u64, slot: i64, value: String) {
    TEXT_CELLS.write(Some(unit), slot, value);
}

/// Read the unit's current integer value or return its declared initial value.
pub fn read_static_int_in_unit(unit: u64, slot: i64, initial: i64) -> i64 {
    INTEGER_CELLS.read(Some(unit), slot, initial)
}

/// Persist an integer assignment to the unit's cell.
pub fn write_static_int_in_unit(unit: u64, slot: i64, value: i64) {
    INTEGER_CELLS.write(Some(unit), slot, value);
}

/// Read the unit's current float value or return its declared initial value.
pub fn read_static_float_in_unit(unit: u64, slot: i64, initial: f64) -> f64 {
    FLOAT_CELLS.read(Some(unit), slot, initial)
}

/// Persist a float assignment to the unit's cell.
pub fn write_static_float_in_unit(unit: u64, slot: i64, value: f64) {
    FLOAT_CELLS.write(Some(unit), slot, value);
}

/// Read the unit's current boolean value or return its declared initial value.
pub fn read_static_bool_in_unit(unit: u64, slot: i64, initial: bool) -> bool {
    BOOLEAN_CELLS.read(Some(unit), slot, initial)
}

/// Persist a boolean assignment to the unit's cell.
pub fn write_static_bool_in_unit(unit: u64, slot: i64, value: bool) {
    BOOLEAN_CELLS.write(Some(unit), slot, value);
}

/// Claim one module's static initialization within its compiled unit.
pub fn begin_unit_module_initialization(unit: u64, module: i64) -> bool {
    claim_module_initialization(Some(unit), module)
}

/// Record that one module in this unit has initialized its statics.
pub fn end_unit_module_initialization(unit: u64, module: i64) {
    MODULE_INITIALIZATION.write(Some(unit), module, 2);
}

/// Only the first call claims a module. Reentrant calls and every call after completion return false.
fn claim_module_initialization(unit: Option<u64>, module: i64) -> bool {
    let mut states = MODULE_INITIALIZATION.guard();
    let Entry::Vacant(entry) = states.entry((unit, module)) else {
        return false;
    };
    entry.insert(1);
    true
}
