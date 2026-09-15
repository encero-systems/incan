//! Compile-time version compatibility check between the Incan compiler and its stdlib.
//!
//! When the Incan compiler generates a Rust project from user code, that project depends on the `incan_stdlib` crate.
//! If the stdlib the generated code links is not the one the compiler generated for — because a cached stdlib from
//! a previous install is still in use, say — the generated code could break in subtle ways at runtime.
//!
//! This module prevents that by providing a macro that the compiler emits into every generated `main.rs`:
//!
//! ```rust,ignore
//! incan_stdlib::__incan_stdlib_version_check!("X.Y.Z");
//! ```
//!
//! The literal is the version of `incan_stdlib` the compiler was built against. The macro expands into a `const`
//! assertion that compares it with the version of the `incan_stdlib` crate the generated code actually links, and a
//! mismatch becomes a **compile-time error** in the generated Rust code, surfacing the problem before anything runs.
//! The expansion deliberately does not require Cargo environment variables in the consumer, so Oven may invoke
//! `rustc` directly.
//!
//! The stdlib carries its own version line, so the two versions need not be equal: a release stdlib is compatible
//! with the one the compiler was built against under the semver caret rule (same major, a later or equal minor and
//! patch; for `0.x` the minor is the compatibility line), while a prerelease on either side demands exact equality,
//! because a `-dev.N` stdlib promises nothing about the next one.

/// The version of this stdlib crate, read from `Cargo.toml` at compile time.
///
/// The compiler embeds the stdlib version it was built against as a literal in the generated code, and the
/// `__incan_stdlib_version_check!` macro compares the two.
pub const INCAN_STDLIB_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Byte-level string equality usable in `const` contexts.
///
/// Rust nightly currently does not stabilise `PartialEq` as a const trait, so `a == b` on `&str` inside a `const`
/// block is a compiler error. This function works around that by comparing raw `&[u8]` slices element by element, and
/// primitive `u8` equality is const-stable.
#[doc(hidden)]
pub const fn const_str_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// The numeric part of a semantic version, plus whether a prerelease tag followed it.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct SemanticVersion {
    /// The major version.
    pub major: u64,
    /// The minor version.
    pub minor: u64,
    /// The patch version.
    pub patch: u64,
    /// Whether the version carries a `-prerelease` tag, which makes it unstable under semver.
    pub prerelease: bool,
}

/// Parse `MAJOR.MINOR.PATCH[-prerelease][+build]` in a `const` context; `None` when the bytes are not that shape.
///
/// Only the digits and the two separators are interpreted: the prerelease tag's content is irrelevant to the
/// compatibility rule (any prerelease demands exact equality) and build metadata never affects compatibility.
#[doc(hidden)]
pub const fn parse_semantic_version(bytes: &[u8]) -> Option<SemanticVersion> {
    let mut components = [0u64; 3];
    let mut component = 0;
    let mut digits = 0;
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        if byte == b'-' || byte == b'+' {
            break;
        }
        if byte == b'.' {
            if digits == 0 || component == 2 {
                return None;
            }
            component += 1;
            digits = 0;
        } else if byte.is_ascii_digit() {
            components[component] = components[component] * 10 + (byte - b'0') as u64;
            digits += 1;
        } else {
            return None;
        }
        i += 1;
    }
    if component != 2 || digits == 0 {
        return None;
    }
    Some(SemanticVersion {
        major: components[0],
        minor: components[1],
        patch: components[2],
        prerelease: i < bytes.len() && bytes[i] == b'-',
    })
}

/// Whether the `linked` stdlib may serve code the compiler generated against the `built_against` stdlib.
///
/// A prerelease on either side, or a version that does not parse, demands exact equality. Otherwise the semver caret
/// rule applies: the same major, and a minor and patch no older than the compiler was built against — with the minor
/// standing in for the major while the major is `0`.
#[doc(hidden)]
pub const fn stdlib_versions_compatible(built_against: &[u8], linked: &[u8]) -> bool {
    if const_str_eq(built_against, linked) {
        return true;
    }
    let (Some(required), Some(candidate)) = (parse_semantic_version(built_against), parse_semantic_version(linked))
    else {
        return false;
    };
    if required.prerelease || candidate.prerelease || required.major != candidate.major {
        return false;
    }
    if required.major == 0 {
        return required.minor == candidate.minor && candidate.patch >= required.patch;
    }
    candidate.minor > required.minor || (candidate.minor == required.minor && candidate.patch >= required.patch)
}

/// Compile-time assertion that the linked stdlib is compatible with the one the compiler was built against.
///
/// Emitted by the Incan compiler into every generated `main.rs` with the stdlib version the compiler was built
/// against. Expands to a `const _: () = { ... }` block that panics, which becomes a compile error, when the linked
/// stdlib cannot serve that code under the rule in [`stdlib_versions_compatible`]. Example output on mismatch:
///
/// ```text
/// Incan stdlib version mismatch: the compiler was built against incan_stdlib X.Y.Z, and the linked incan_stdlib is not compatible with it
/// ```
#[doc(hidden)]
#[macro_export]
macro_rules! __incan_stdlib_version_check {
    ($built_against:literal) => {
        const _: () = {
            if !$crate::version::stdlib_versions_compatible(
                $built_against.as_bytes(),
                $crate::version::INCAN_STDLIB_VERSION.as_bytes(),
            ) {
                panic!(concat!(
                    "Incan stdlib version mismatch: the compiler was built against incan_stdlib ",
                    $built_against,
                    ", and the linked incan_stdlib is not compatible with it"
                ));
            }
        };
    };
}

#[cfg(test)]
mod tests {
    use super::{parse_semantic_version, stdlib_versions_compatible};

    /// Shorthand so the table below reads as version pairs.
    fn compatible(built_against: &str, linked: &str) -> bool {
        stdlib_versions_compatible(built_against.as_bytes(), linked.as_bytes())
    }

    #[test]
    fn the_linked_stdlib_the_compiler_was_built_against_is_always_compatible() {
        assert!(compatible("0.6.0", "0.6.0"));
        assert!(compatible("0.6.0-dev.4", "0.6.0-dev.4"));
        assert!(compatible("1.2.3+build.7", "1.2.3+build.7"));
    }

    #[test]
    fn a_prerelease_on_either_side_demands_exact_equality() {
        assert!(!compatible("0.6.0-dev.4", "0.6.0-dev.5"));
        assert!(!compatible("0.6.0-dev.5", "0.6.0-dev.4"));
        assert!(!compatible("0.6.0-dev.4", "0.6.0"));
        assert!(!compatible("0.6.0", "0.6.0-dev.4"));
        assert!(!compatible("0.6.0", "0.6.1-rc.1"));
    }

    #[test]
    fn a_zero_major_release_is_compatible_within_its_minor_and_never_older() {
        assert!(compatible("0.6.0", "0.6.1"));
        assert!(compatible("0.6.2", "0.6.9"));
        assert!(!compatible("0.6.2", "0.6.1"));
        assert!(!compatible("0.6.0", "0.7.0"));
        assert!(!compatible("0.7.0", "0.6.3"));
    }

    #[test]
    fn a_release_after_one_point_zero_follows_the_caret_rule() {
        assert!(compatible("1.2.3", "1.2.3"));
        assert!(compatible("1.2.3", "1.2.4"));
        assert!(compatible("1.2.3", "1.3.0"));
        assert!(!compatible("1.2.3", "1.2.2"));
        assert!(!compatible("1.3.0", "1.2.9"));
        assert!(!compatible("1.2.3", "2.0.0"));
    }

    #[test]
    fn build_metadata_never_affects_compatibility() {
        assert!(compatible("1.2.3", "1.2.3+sha.abc"));
        assert!(compatible("0.6.0+a", "0.6.1+b"));
    }

    #[test]
    fn a_version_that_does_not_parse_is_only_compatible_with_itself() {
        assert!(parse_semantic_version(b"").is_none());
        assert!(parse_semantic_version(b"0.6").is_none());
        assert!(parse_semantic_version(b"0.6.").is_none());
        assert!(parse_semantic_version(b"0.6.0.1").is_none());
        assert!(parse_semantic_version(b"v0.6.0").is_none());
        assert!(parse_semantic_version(b"0..0").is_none());
        assert!(compatible("weird", "weird"));
        assert!(!compatible("weird", "0.6.0"));
        assert!(!compatible("0.6.0", "weird"));
    }

    #[test]
    fn parsing_reads_the_three_components_and_the_prerelease_marker() -> Result<(), String> {
        let version = parse_semantic_version(b"10.20.30-dev.4+build").ok_or("did not parse")?;
        assert_eq!((version.major, version.minor, version.patch), (10, 20, 30));
        assert!(version.prerelease);
        let version = parse_semantic_version(b"1.0.0+build").ok_or("did not parse")?;
        assert!(!version.prerelease);
        Ok(())
    }
}
