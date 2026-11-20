//! Text and byte observations of the static frozen carriers.

use incan_std_primitives::frozen::{FrozenBytes, FrozenStr};

/// Owned text through the frozen carrier's canonical Display implementation.
pub fn format_frozen(value: FrozenStr) -> String {
    value.to_string()
}

/// Measure static bytes without copying them.
pub fn length_frozen_bytes(value: FrozenBytes) -> i64 {
    value.len() as i64
}

/// Copy immutable byte storage into the owned buffer sequence reads use.
pub fn copy_frozen_bytes(value: FrozenBytes) -> Vec<u8> {
    value.as_slice().to_vec()
}
