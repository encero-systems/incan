//! Construction requires the entire public layout, including exhaustiveness.

/// One readable field does not expose private storage.
pub struct Mixed {
    /// Public output.
    pub code: String,
    hidden: u8,
}

/// A public alias does not grant construction authority for private storage.
pub type MixedAlias = Mixed;

/// Empty calls cannot construct opaque storage either.
pub struct Opaque {
    hidden: u8,
}

/// Public fields do not bypass a non-exhaustive declaration.
#[non_exhaustive]
pub struct Open {
    /// Public output.
    pub code: String,
}
