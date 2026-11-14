//! Compiler-owned stored-record member identities shared by checking and persisted Body IR validation.

use incan_lang::lang::conventions::RECORD_MEMBER_IDENTITY_BYTES;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Fixed-width identity derived from a stored field or enum variant's earliest name.
pub type RecordMemberIdentity = [u8; RECORD_MEMBER_IDENTITY_BYTES];

const _: () = assert!(RECORD_MEMBER_IDENTITY_BYTES <= 32);

/// The owning package's declared persistent-record namespace, independent of compiler symbol ownership.
///
/// Absence means no persistent nominal identity can be derived. This is a declared fact, not authentication or a
/// registry response; consumers retain the producer's pair without substituting their dependency aliases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordPackageIdentity {
    /// SHA-256 identity of the declared registry root key, including its `sha256:` prefix.
    pub registry_id: String,
    /// The producer's scoped manifest name, never the consumer's import binding.
    pub scoped_name: String,
}

impl RecordPackageIdentity {
    /// Check the persisted namespace before a unit or library may derive stored declaration identities from it.
    pub fn validate(&self) -> Result<(), &'static str> {
        validate_record_registry_id(&self.registry_id)?;
        let Some((scope, name)) = self.scoped_name.split_once('/') else {
            return Err("record package identity requires a scoped package name");
        };
        for (part, underscores) in [(scope, false), (name, scope == "crates-io")] {
            if part.is_empty()
                || part.len() > 64
                || !part.as_bytes()[0].is_ascii_alphanumeric()
                || !part.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || (underscores && byte == b'_')
                })
            {
                return Err("record package identity requires a canonical scoped package name");
            }
        }
        Ok(())
    }
}

/// Refuse noncanonical registry spellings without performing registry access or inventing a default root key.
pub fn validate_record_registry_id(identity: &str) -> Result<(), &'static str> {
    let Some(hex) = identity.strip_prefix("sha256:") else {
        return Err("registry-id must be sha256: followed by 64 lowercase hexadecimal digits");
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("registry-id must be sha256: followed by 64 lowercase hexadecimal digits");
    }
    Ok(())
}

/// Compute the RFC 165 member identity from the UTF-8 bytes of its earliest name.
pub fn record_member_identity(earliest_name: &str) -> RecordMemberIdentity {
    let digest = Sha256::digest(earliest_name.as_bytes());
    let mut identity = [0; RECORD_MEMBER_IDENTITY_BYTES];
    identity.copy_from_slice(&digest[..RECORD_MEMBER_IDENTITY_BYTES]);
    identity
}

#[cfg(test)]
mod tests {
    use super::{RecordPackageIdentity, record_member_identity, validate_record_registry_id};

    /// Keep member hashes fixed while admitting only canonical persistent package namespaces.
    #[test]
    fn record_member_identities_use_the_registered_sha256_width() {
        let registry = "sha256:3d7781b11f6b80f950bab17238cffa7c1f77b5e1f0e0dbbaa656149d9ffbffb0";
        assert!(validate_record_registry_id(registry).is_ok());
        for invalid in [
            "sha256:00".to_owned(),
            registry.to_uppercase(),
            format!("{registry}0"),
            registry[7..].to_owned(),
        ] {
            assert!(validate_record_registry_id(&invalid).is_err());
        }
        for scoped_name in ["example/catalog", "crates-io/rust_library"] {
            assert!(
                RecordPackageIdentity {
                    registry_id: registry.to_owned(),
                    scoped_name: scoped_name.to_owned()
                }
                .validate()
                .is_ok()
            );
        }
        for scoped_name in [
            "catalog",
            "example/",
            "/catalog",
            "example/catalog/more",
            "Example/catalog",
            "example/cat\0alog",
        ] {
            assert!(
                RecordPackageIdentity {
                    registry_id: registry.to_owned(),
                    scoped_name: scoped_name.to_owned()
                }
                .validate()
                .is_err()
            );
        }
        assert_eq!(hex::encode(record_member_identity("email")), "82244417f956");
        assert_eq!(hex::encode(record_member_identity("customer")), "b6c45863875e");
        assert_eq!(
            record_member_identity("fieldbbea15ef9eff"),
            record_member_identity("field3d4624c2d3a2")
        );
    }
}
