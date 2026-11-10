//! Native and file names for admitted Loaf package identities, shared with Oven through Rust interop.

/// Derive the native crate name while leaving the semantic package identity unchanged.
///
/// Scoped names use reversible unpadded lowercase base32 of their complete UTF-8 bytes after `__incan_loaf_`.
/// Admitted bare Loaf names start with a letter or digit, so their legacy projection cannot enter that namespace.
/// Base32 also keeps a maximal 64/64-byte scoped name and publication suffixes within a 255-byte filename.
/// Unscoped names preserve the existing lowercase, underscore and leading-digit projection.
pub fn native_crate_name(package: &str) -> String {
    if package.contains('/') {
        return scoped_name(package);
    }
    let mut name = package
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    if name.is_empty() || name.starts_with(|character: char| character.is_ascii_digit()) {
        name.insert(0, '_');
    }
    name
}

/// Name the manifest, body, graph and native-closure files of a Loaf, preserving unscoped filenames.
/// Scoped files share the native crate's exact encoding; the original name stays inside the manifest.
pub fn artifact_stem(package: &str) -> String {
    if package.contains('/') {
        native_crate_name(package)
    } else {
        package.to_owned()
    }
}

/// Encode a scoped identity losslessly with the RFC 4648 alphabet, omitting padding and using lowercase letters.
fn scoped_name(package: &str) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut result = String::from("__incan_loaf_");
    let mut pending: usize = 0;
    let mut bits: usize = 0;
    for byte in package.bytes() {
        pending = (pending << 8) | usize::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            result.push(char::from(ALPHABET[(pending >> bits) & 31]));
        }
        pending &= (1 << bits) - 1;
    }
    if bits != 0 {
        result.push(char::from(ALPHABET[(pending << (5 - bits)) & 31]));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{artifact_stem, native_crate_name};

    /// Exact vectors make the encoding portable, and collisions from punctuation folding remain distinct.
    #[test]
    fn scoped_names_preserve_every_byte() {
        assert_eq!(
            native_crate_name("example/catalog"),
            "__incan_loaf_mv4gc3lqnrss6y3borqwy33h"
        );
        for (left, right) in [
            ("alpha/catalog", "beta/catalog"),
            ("scope-a/catalog", "scope/a-catalog"),
            ("crates-io/a_b", "crates-io/a-b"),
        ] {
            assert_ne!(native_crate_name(left), native_crate_name(right));
            assert_eq!(artifact_stem(left), native_crate_name(left));
        }
    }

    /// Existing bare provider filenames remain readable and a scoped name never enters their namespace.
    #[test]
    fn bare_names_keep_legacy_spelling() {
        assert_eq!(native_crate_name("sample-lib"), "sample_lib");
        assert_eq!(artifact_stem("sample-lib"), "sample-lib");
        assert_eq!(native_crate_name("sample_lib"), "sample_lib");
        assert_eq!(native_crate_name("2d"), "_2d");
        assert_ne!(native_crate_name("scope/name"), native_crate_name("scope_name"));
    }

    /// The longest admitted package still fits a single filename with the longest publication witness suffix.
    #[test]
    fn maximal_scoped_name_fits_publication_files() {
        let package = format!("{}/{}", "a".repeat(64), "b".repeat(64));
        let name = native_crate_name(&package);
        assert!(name.len() + ".native.json.oven-unit".len() <= 255);
        assert!(
            name.bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        );
    }
}
