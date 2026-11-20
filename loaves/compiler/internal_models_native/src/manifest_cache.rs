//! Manifest digests, their family-specific cached values and the admitted manifest inventory.

/// The public manifest digest and its optional, independently checked, family-specific value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestCacheValue {
    pub digest: Vec<u8>,
    pub payload: Vec<u8>,
}

/// The exact named and global facts admitted from a manifest before demand loading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestInventory {
    pub name: String,
    pub version: String,
    pub features: Vec<String>,
    pub vocabulary: bool,
    pub operations: bool,
    pub native_unions: bool,
    pub rust_paths: Vec<String>,
    pub declares_abi_cache: bool,
    pub has_abi_cache: bool,
    pub abi_digest: Option<String>,
}
