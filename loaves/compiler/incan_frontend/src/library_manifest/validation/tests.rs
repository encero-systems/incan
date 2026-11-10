use super::sha256_digest;
use crate::library_manifest::LibraryManifest;
use incan_semantics_core::record_identity::RecordPackageIdentity;

/// Digest validation and producer namespace round trips share the canonical lowercase spelling.
#[test]
fn digests_accept_only_the_lowercase_form_writers_emit() -> Result<(), Box<dyn std::error::Error>> {
    let lower = "a".repeat(64);
    let upper = "A".repeat(64);
    assert!(sha256_digest("rust_abi.cache_digest", &format!("sha256:{lower}"), true).is_ok());
    assert!(sha256_digest("vocab desugarer_artifact.sha256", &lower, false).is_ok());
    let refused = sha256_digest(
        "provider dependency `p` artifact digest",
        &format!("sha256:{upper}"),
        true,
    );
    assert_eq!(
        refused,
        Err(
            "provider dependency `p` artifact digest must be a SHA-256 digest: `sha256:` and 64 lowercase \
                 hexadecimal characters"
                .to_owned()
        )
    );
    assert!(sha256_digest("rust_abi.cache_digest", &lower, true).is_err());
    let mut manifest = LibraryManifest::new("example/catalog", "1.0.0");
    let absent = manifest.to_json_string()?;
    assert!(!absent.contains("record_package"));
    assert!(
        LibraryManifest::from_json_str(&absent)?
            .contract_metadata
            .record_package
            .is_none()
    );
    let package = RecordPackageIdentity {
        registry_id: "sha256:3d7781b11f6b80f950bab17238cffa7c1f77b5e1f0e0dbbaa656149d9ffbffb0".to_owned(),
        scoped_name: "example/catalog".to_owned(),
    };
    manifest.contract_metadata.record_package = Some(package.clone());
    let encoded = manifest.to_json_string()?;
    assert_eq!(
        LibraryManifest::from_json_str(&encoded)?
            .contract_metadata
            .record_package,
        Some(package.clone())
    );
    let retained = manifest.encode_retained()?;
    assert_eq!(
        LibraryManifest::decode_retained(&retained)?
            .contract_metadata
            .record_package,
        Some(package)
    );
    assert!(LibraryManifest::from_json_str(&encoded.replace("sha256:3d7781", "sha256:3D7781")).is_err());
    manifest.name = "consumer/alias".to_owned();
    assert!(
        manifest.to_json_string().is_err(),
        "consumer aliases cannot replace producer identity"
    );
    Ok(())
}
