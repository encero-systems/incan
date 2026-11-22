//! Persistent hashed-collection cells for canonical storage slots, one typed table per carrier.

use std::collections::{HashMap, HashSet};

use crate::statics::Cells;

static TEXT_INT_DICTS: Cells<HashMap<String, i64>> = Cells::new();
static INT_SETS: Cells<HashSet<i64>> = Cells::new();

/// A snapshot of a dictionary in the seed's shared slot namespace.
pub fn read_static_text_int_dict(slot: i64, initial: HashMap<String, i64>) -> HashMap<String, i64> {
    TEXT_INT_DICTS.read(None, slot, initial)
}

/// Replace a dictionary in the seed's shared slot namespace.
pub fn write_static_text_int_dict(slot: i64, value: HashMap<String, i64>) {
    TEXT_INT_DICTS.write(None, slot, value);
}

/// A snapshot of a set in the seed's shared slot namespace.
pub fn read_static_int_set(slot: i64, initial: HashSet<i64>) -> HashSet<i64> {
    INT_SETS.read(None, slot, initial)
}

/// Replace a set in the seed's shared slot namespace.
pub fn write_static_int_set(slot: i64, value: HashSet<i64>) {
    INT_SETS.write(None, slot, value);
}

/// A snapshot of the unit's current dictionary, or its declared entries before any write.
pub fn read_static_text_int_dict_in_unit(unit: u64, slot: i64, initial: HashMap<String, i64>) -> HashMap<String, i64> {
    TEXT_INT_DICTS.read(Some(unit), slot, initial)
}

/// Replace the unit's dictionary cell.
pub fn write_static_text_int_dict_in_unit(unit: u64, slot: i64, value: HashMap<String, i64>) {
    TEXT_INT_DICTS.write(Some(unit), slot, value);
}

/// A snapshot of the unit's current set, or its declared members before any write.
pub fn read_static_int_set_in_unit(unit: u64, slot: i64, initial: HashSet<i64>) -> HashSet<i64> {
    INT_SETS.read(Some(unit), slot, initial)
}

/// Replace the unit's set cell.
pub fn write_static_int_set_in_unit(unit: u64, slot: i64, value: HashSet<i64>) {
    INT_SETS.write(Some(unit), slot, value);
}
