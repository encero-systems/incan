//! Version constants for vocab metadata compatibility.
//!
//! These constants describe the serialized contract exposed by `incan_vocab`. They are separate from the crate's own
//! package version so the metadata shape can evolve deliberately and independently.

/// Current serialized `VocabMetadata` contract version.
pub const VOCAB_METADATA_VERSION: u32 = 1;

/// Current serialized WASM desugarer ABI contract version.
///
/// This version controls how request/response payloads are encoded across the compiler/desugarer
/// boundary. Companion crates and compiler tooling must agree on this value.
pub const WASM_DESUGAR_ABI_VERSION: u32 = 1;
