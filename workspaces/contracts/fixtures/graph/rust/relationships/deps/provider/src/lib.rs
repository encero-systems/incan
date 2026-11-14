/// A dependency-owned function reached through an alias in the consumer.
pub fn leaf() -> i32 {
    3
}

/// A dependency-owned receiver must have the same identity in its consumer.
pub struct Gadget;

impl Gadget {
    /// A receiver-dependent target crossing a crate boundary.
    pub fn value(&self) -> i32 {
        5
    }
}
