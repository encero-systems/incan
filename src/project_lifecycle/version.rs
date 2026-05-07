//! SemVer policy for project version changes.
//!
//! RFC 015 defines `incan version` as a project-version operation over the `incan.toml` metadata version. This module
//! keeps that policy independent from CLI parsing and manifest writes: callers provide the current version and an
//! already-decided operation, and receive the old/new versions to persist or display.

use semver::{BuildMetadata, Prerelease, Version};
use thiserror::Error;

/// A requested project-version operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionRequest {
    /// Apply an RFC 015 SemVer bump to the current version.
    Bump {
        /// The release-core or prerelease bump to apply.
        bump: VersionBump,
        /// Preserve the existing prerelease component for release-core bumps.
        keep_prerelease: bool,
    },
    /// Replace the current version with an explicit SemVer value from `--set`.
    Set {
        /// The complete SemVer version string to validate and use as-is.
        version: String,
    },
}

/// Release-core and prerelease bump kinds accepted by `incan version`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionBump {
    /// Increment the major component and reset minor and patch to zero.
    Major,
    /// Increment the minor component and reset patch to zero.
    Minor,
    /// Increment the patch component.
    Patch,
    /// Move to or advance the `alpha` prerelease channel.
    Alpha,
    /// Move to or advance the `beta` prerelease channel.
    Beta,
    /// Move to or advance the `rc` prerelease channel.
    Rc,
    /// Move to or advance the `dev` prerelease channel.
    Dev,
}

/// A prerelease channel supported by RFC 015.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrereleaseTag {
    /// The `alpha` prerelease channel.
    Alpha,
    /// The `beta` prerelease channel.
    Beta,
    /// The `rc` prerelease channel.
    Rc,
    /// The `dev` prerelease channel.
    Dev,
}

/// The result of a pure project-version change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionChange {
    /// The parsed version before applying the request.
    pub old_version: Version,
    /// The parsed version after applying the request.
    pub new_version: Version,
}

/// Errors emitted while validating or applying a project-version request.
#[derive(Debug, Error)]
pub enum VersionError {
    /// The current project version is not valid SemVer.
    #[error("invalid current project version `{version}`: {source}")]
    InvalidCurrentVersion {
        /// The invalid version string supplied by the caller.
        version: String,
        /// The SemVer parser error.
        source: semver::Error,
    },
    /// The explicit `--set` value is not valid SemVer.
    #[error("invalid explicit project version `{version}`: {source}")]
    InvalidExplicitVersion {
        /// The invalid explicit version string supplied by the caller.
        version: String,
        /// The SemVer parser error.
        source: semver::Error,
    },
    /// A prerelease bump found the same channel but not the expected `<tag>.<number>` form.
    #[error("cannot advance prerelease `{prerelease}` for `{tag}` bump; expected `{tag}.<numeric suffix>`")]
    UnsupportedPrereleaseShape {
        /// The prerelease value on the current version.
        prerelease: String,
        /// The prerelease channel being advanced.
        tag: &'static str,
    },
    /// Internal construction of a known-good prerelease failed.
    #[error("failed to construct prerelease `{prerelease}`: {source}")]
    InvalidGeneratedPrerelease {
        /// The generated prerelease value.
        prerelease: String,
        /// The SemVer parser error.
        source: semver::Error,
    },
    /// A release-core bump would exceed the SemVer numeric component range.
    #[error("cannot bump `{component}` component because it is already at the maximum SemVer value")]
    VersionCoreOverflow {
        /// The release-core component that could not be incremented.
        component: &'static str,
    },
}

impl VersionRequest {
    /// Apply this request to a current project version string.
    ///
    /// `Set` validates the explicit value as a complete SemVer version and does not reinterpret prerelease or build
    /// metadata. `Bump` follows the RFC 015 bump rules and clears build metadata because build metadata describes a
    /// specific build artifact, not the next logical project version.
    pub fn apply(&self, current_version: &str) -> Result<VersionChange, VersionError> {
        match self {
            Self::Bump { bump, keep_prerelease } => bump_project_version(current_version, *bump, *keep_prerelease),
            Self::Set { version } => set_project_version(current_version, version),
        }
    }
}

impl VersionBump {
    /// Return the prerelease tag associated with this bump, if it is a prerelease bump.
    #[must_use]
    pub fn prerelease_tag(self) -> Option<PrereleaseTag> {
        match self {
            Self::Major | Self::Minor | Self::Patch => None,
            Self::Alpha => Some(PrereleaseTag::Alpha),
            Self::Beta => Some(PrereleaseTag::Beta),
            Self::Rc => Some(PrereleaseTag::Rc),
            Self::Dev => Some(PrereleaseTag::Dev),
        }
    }
}

impl PrereleaseTag {
    /// Return the canonical SemVer identifier for this prerelease channel.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Alpha => "alpha",
            Self::Beta => "beta",
            Self::Rc => "rc",
            Self::Dev => "dev",
        }
    }
}

/// Apply an RFC 015 bump to a project version string.
pub fn bump_project_version(
    current_version: &str,
    bump: VersionBump,
    keep_prerelease: bool,
) -> Result<VersionChange, VersionError> {
    let old_version = parse_current_version(current_version)?;
    let mut new_version = old_version.clone();

    if let Some(tag) = bump.prerelease_tag() {
        bump_prerelease(&mut new_version, tag)?;
    } else {
        bump_release_core(&mut new_version, bump, keep_prerelease)?;
    }

    new_version.build = BuildMetadata::EMPTY;

    Ok(VersionChange {
        old_version,
        new_version,
    })
}

/// Validate and apply an explicit `--set <version>` project version.
///
/// This function intentionally accepts every SemVer value the `semver` crate accepts, including prerelease and build
/// metadata. It does not coerce partial versions such as `1.2`; callers must pass a complete SemVer string.
pub fn set_project_version(current_version: &str, explicit_version: &str) -> Result<VersionChange, VersionError> {
    let old_version = parse_current_version(current_version)?;
    let new_version = validate_explicit_version(explicit_version)?;

    Ok(VersionChange {
        old_version,
        new_version,
    })
}

/// Validate an explicit `--set` version without applying it to a current version.
pub fn validate_explicit_version(explicit_version: &str) -> Result<Version, VersionError> {
    Version::parse(explicit_version).map_err(|source| VersionError::InvalidExplicitVersion {
        version: explicit_version.to_string(),
        source,
    })
}

/// Parse the current project version and label parse failures for manifest-facing callers.
fn parse_current_version(current_version: &str) -> Result<Version, VersionError> {
    Version::parse(current_version).map_err(|source| VersionError::InvalidCurrentVersion {
        version: current_version.to_string(),
        source,
    })
}

/// Apply a release-core bump.
///
/// RFC 015 says these operate on the release core and clear prerelease metadata unless `--keep-prerelease` is present.
fn bump_release_core(version: &mut Version, bump: VersionBump, keep_prerelease: bool) -> Result<(), VersionError> {
    match bump {
        VersionBump::Major => {
            version.major = increment_core_component(version.major, "major")?;
            version.minor = 0;
            version.patch = 0;
        }
        VersionBump::Minor => {
            version.minor = increment_core_component(version.minor, "minor")?;
            version.patch = 0;
        }
        VersionBump::Patch => {
            version.patch = increment_core_component(version.patch, "patch")?;
        }
        VersionBump::Alpha | VersionBump::Beta | VersionBump::Rc | VersionBump::Dev => {}
    }

    if !keep_prerelease {
        version.pre = Prerelease::EMPTY;
    }

    Ok(())
}

/// Increment a SemVer release-core component without panicking or wrapping.
fn increment_core_component(value: u64, component: &'static str) -> Result<u64, VersionError> {
    value
        .checked_add(1)
        .ok_or(VersionError::VersionCoreOverflow { component })
}

/// Apply a prerelease-channel bump without changing the release core.
fn bump_prerelease(version: &mut Version, tag: PrereleaseTag) -> Result<(), VersionError> {
    let tag_value = tag.as_str();
    let next_prerelease = if version.pre.is_empty() {
        format!("{tag_value}.1")
    } else {
        next_prerelease_value(version.pre.as_str(), tag_value)?
    };

    version.pre = Prerelease::new(&next_prerelease).map_err(|source| VersionError::InvalidGeneratedPrerelease {
        prerelease: next_prerelease,
        source,
    })?;
    Ok(())
}

/// Derive the next prerelease value from an existing SemVer prerelease.
fn next_prerelease_value(prerelease: &str, tag: &'static str) -> Result<String, VersionError> {
    let mut parts = prerelease.split('.');
    let Some(existing_tag) = parts.next() else {
        return Ok(format!("{tag}.1"));
    };

    if existing_tag != tag {
        return Ok(format!("{tag}.1"));
    }

    let Some(existing_suffix) = parts.next() else {
        return Err(VersionError::UnsupportedPrereleaseShape {
            prerelease: prerelease.to_string(),
            tag,
        });
    };
    if parts.next().is_some() || !is_decimal_identifier(existing_suffix) {
        return Err(VersionError::UnsupportedPrereleaseShape {
            prerelease: prerelease.to_string(),
            tag,
        });
    }

    Ok(format!("{tag}.{}", increment_decimal_identifier(existing_suffix)))
}

/// Return whether a prerelease identifier is an unsigned decimal number.
fn is_decimal_identifier(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// Increment a non-empty decimal string without imposing an integer-width limit.
fn increment_decimal_identifier(value: &str) -> String {
    let mut carry = true;
    let mut reversed_digits = Vec::with_capacity(value.len() + 1);

    for byte in value.bytes().rev() {
        let next_digit = if carry {
            if byte == b'9' {
                b'0'
            } else {
                carry = false;
                byte + 1
            }
        } else {
            byte
        };
        reversed_digits.push(next_digit);
    }

    if carry {
        reversed_digits.push(b'1');
    }

    reversed_digits.reverse();
    reversed_digits.into_iter().map(char::from).collect()
}

#[cfg(test)]
mod tests {
    use super::{
        VersionBump, VersionError, VersionRequest, bump_project_version, set_project_version, validate_explicit_version,
    };

    #[test]
    fn release_bumps_clear_prerelease_by_default() -> Result<(), Box<dyn std::error::Error>> {
        let major = bump_project_version("1.2.3-alpha.4", VersionBump::Major, false)?;
        let minor = bump_project_version("1.2.3-alpha.4", VersionBump::Minor, false)?;
        let patch = bump_project_version("1.2.3-alpha.4", VersionBump::Patch, false)?;

        assert_eq!("2.0.0", major.new_version.to_string());
        assert_eq!("1.3.0", minor.new_version.to_string());
        assert_eq!("1.2.4", patch.new_version.to_string());
        Ok(())
    }

    #[test]
    fn release_bumps_can_keep_prerelease() -> Result<(), Box<dyn std::error::Error>> {
        let change = bump_project_version("1.2.3-rc.2", VersionBump::Minor, true)?;

        assert_eq!("1.3.0-rc.2", change.new_version.to_string());
        Ok(())
    }

    #[test]
    fn prerelease_bumps_start_increment_and_switch_channels() -> Result<(), Box<dyn std::error::Error>> {
        let alpha = bump_project_version("1.2.3", VersionBump::Alpha, false)?;
        let beta = bump_project_version("1.2.3-beta.9", VersionBump::Beta, false)?;
        let rc = bump_project_version("1.2.3-beta.9", VersionBump::Rc, false)?;
        let dev = bump_project_version("1.2.3-dev.999", VersionBump::Dev, false)?;

        assert_eq!("1.2.3-alpha.1", alpha.new_version.to_string());
        assert_eq!("1.2.3-beta.10", beta.new_version.to_string());
        assert_eq!("1.2.3-rc.1", rc.new_version.to_string());
        assert_eq!("1.2.3-dev.1000", dev.new_version.to_string());
        Ok(())
    }

    #[test]
    fn explicit_set_accepts_complete_semver_values() -> Result<(), Box<dyn std::error::Error>> {
        let change = set_project_version("0.1.0", "2.0.0-beta.3+build.7")?;

        assert_eq!("0.1.0", change.old_version.to_string());
        assert_eq!("2.0.0-beta.3+build.7", change.new_version.to_string());
        Ok(())
    }

    #[test]
    fn bumps_clear_build_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let release = bump_project_version("1.2.3+build.7", VersionBump::Patch, false)?;
        let prerelease = bump_project_version("1.2.3-alpha.1+build.7", VersionBump::Alpha, false)?;

        assert_eq!("1.2.4", release.new_version.to_string());
        assert_eq!("1.2.3-alpha.2", prerelease.new_version.to_string());
        Ok(())
    }

    #[test]
    fn version_request_applies_bump_or_set() -> Result<(), Box<dyn std::error::Error>> {
        let bumped = VersionRequest::Bump {
            bump: VersionBump::Patch,
            keep_prerelease: false,
        }
        .apply("1.2.3")?;
        let set = VersionRequest::Set {
            version: "3.4.5".to_string(),
        }
        .apply("1.2.3")?;

        assert_eq!("1.2.4", bumped.new_version.to_string());
        assert_eq!("3.4.5", set.new_version.to_string());
        Ok(())
    }

    #[test]
    fn explicit_set_rejects_partial_versions() {
        let error = validate_explicit_version("1.2");

        assert!(matches!(error, Err(VersionError::InvalidExplicitVersion { .. })));
    }

    #[test]
    fn invalid_current_version_is_labeled_separately() {
        let error = bump_project_version("not-semver", VersionBump::Patch, false);

        assert!(matches!(error, Err(VersionError::InvalidCurrentVersion { .. })));
    }

    #[test]
    fn release_core_bumps_reject_overflow() {
        let error = bump_project_version("0.0.18446744073709551615", VersionBump::Patch, false);

        assert!(matches!(error, Err(VersionError::VersionCoreOverflow { .. })));
    }

    #[test]
    fn same_channel_prerelease_requires_numeric_suffix() {
        let error = bump_project_version("1.2.3-alpha.preview", VersionBump::Alpha, false);

        assert!(matches!(error, Err(VersionError::UnsupportedPrereleaseShape { .. })));
    }
}
