use super::sha256_digest;

#[test]
fn digests_accept_only_the_lowercase_form_writers_emit() {
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
}
