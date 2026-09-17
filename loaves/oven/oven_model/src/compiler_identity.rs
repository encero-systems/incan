//! The identity of the compiler driving Oven, as data the compiler hands over rather than a constant Oven reads.
//!
//! Oven stamps release domains, Loaf provenance and build-unit inputs with the compiler's version and the revision of
//! its generated SDK provider code. Neither is Oven's to know: the ring must build without the compiler crates, so
//! the driver constructs one of these from its own `version` module and passes it wherever a store, a Loaf bake or a
//! compiler-suite publication needs it.

use std::fmt;

/// The domain-name prefix under which one compiler release's artifacts live in a store.
pub const RELEASE_DOMAIN_PREFIX: &str = "incan-release-";

/// The compiler version and generated-provider revision one Oven operation runs under.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CompilerIdentity {
    /// The compiler's own version string, exactly as it reports it.
    pub version: String,
    /// The compatibility revision of the Rust the compiler generates for SDK providers; a change requires every
    /// provider to be regenerated, so it is part of every identity that folds providers in.
    pub sdk_provider_codegen_revision: u32,
}

impl CompilerIdentity {
    /// Construct an identity from the two facts the compiler owns.
    pub fn new(version: impl Into<String>, sdk_provider_codegen_revision: u32) -> Self {
        Self {
            version: version.into(),
            sdk_provider_codegen_revision,
        }
    }

    /// The store domain this release's artifacts live in.
    pub fn release_domain(&self) -> String {
        format!("{RELEASE_DOMAIN_PREFIX}{}", self.version)
    }
}

impl fmt::Display for CompilerIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (provider codegen revision {})",
            self.version, self.sdk_provider_codegen_revision
        )
    }
}
