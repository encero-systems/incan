//! Version constants for vocab metadata compatibility.
//!
//! These constants describe the serialized contract exposed by `incan_vocab`. They are separate from the crate's own
//! package version so the metadata shape can evolve deliberately and independently.

/// Current serialized `VocabMetadata` contract version.
pub const VOCAB_METADATA_VERSION: u32 = 1;

/// Public `std.vocab` syntax and native worker request/response schema version.
pub const VOCAB_SYNTAX_VERSION: u32 = 3;
