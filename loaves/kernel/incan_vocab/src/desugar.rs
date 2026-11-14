//! Desugarer descriptors carried in a vocabulary registration.
//!
//! Desugarers are authored in Incan through `std.vocab` and run as native workers; these types only describe the
//! worker a registration requests.

/// Desugarer runtime artifact format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum DesugarerArtifactKind {
    /// Host-native process speaking the versioned public syntax protocol.
    #[default]
    NativeWorker,
    /// Historical metadata tag, retained only so consumers can issue the required rebuild diagnostic.
    WasmModule,
}

/// Serializable metadata describing the desugarer a registration requests.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DesugarerMetadata {
    /// Artifact kind to produce and package.
    pub artifact_kind: DesugarerArtifactKind,
    /// Public syntax schema version for the native worker protocol.
    pub abi_version: u32,
}

impl Default for DesugarerMetadata {
    /// Describe a host-native worker using the current public syntax schema.
    fn default() -> Self {
        Self {
            artifact_kind: DesugarerArtifactKind::NativeWorker,
            abi_version: crate::VOCAB_SYNTAX_VERSION,
        }
    }
}
