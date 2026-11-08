//! Compiler-owned stored-record member identities shared by checking and persisted Body IR validation.

use incan_lang::lang::conventions::RECORD_MEMBER_IDENTITY_BYTES;
use sha2::{Digest, Sha256};

/// Fixed-width identity derived from a stored field or enum variant's earliest name.
pub type RecordMemberIdentity = [u8; RECORD_MEMBER_IDENTITY_BYTES];

const _: () = assert!(RECORD_MEMBER_IDENTITY_BYTES <= 32);

/// Compute the RFC 165 member identity from the UTF-8 bytes of its earliest name.
pub fn record_member_identity(earliest_name: &str) -> RecordMemberIdentity {
    let digest = Sha256::digest(earliest_name.as_bytes());
    let mut identity = [0; RECORD_MEMBER_IDENTITY_BYTES];
    identity.copy_from_slice(&digest[..RECORD_MEMBER_IDENTITY_BYTES]);
    identity
}

#[cfg(test)]
mod tests {
    use super::record_member_identity;

    #[test]
    fn record_member_identities_use_the_registered_sha256_width() {
        assert_eq!(hex::encode(record_member_identity("email")), "82244417f956");
        assert_eq!(hex::encode(record_member_identity("customer")), "b6c45863875e");
        assert_eq!(
            record_member_identity("fieldbbea15ef9eff"),
            record_member_identity("field3d4624c2d3a2")
        );
    }
}
