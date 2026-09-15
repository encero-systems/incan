//! Oven's content-address rendering: SHA-256 over canonical bytes, written `sha256:<hex>` everywhere an identity
//! is stored or compared.

use sha2::{Digest, Sha256};

/// Hash canonical text with Oven's stable `sha256:` rendering.
pub fn digest_content(content: &str) -> String {
    digest_bytes(content.as_bytes())
}

/// Hash arbitrary canonical identity bytes with Oven's stable `sha256:` rendering.
pub fn digest_bytes(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    format!("sha256:{}", hex::encode(hasher.finalize()))
}
