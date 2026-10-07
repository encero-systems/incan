//! Resolving publisher-declared compile-time environment inputs.

use super::{BTreeMap, OvenRustcError, Path, PathBuf};

/// Validate the small compile-time environment envelope that direct-rustc consumers may restore after ambient Cargo
/// state is cleared.
pub(super) fn validated_compile_environment(
    environment: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, OvenRustcError> {
    for (name, value) in environment {
        let binary_name = name.strip_prefix("CARGO_BIN_EXE_");
        let allowed = matches!(
            name.as_str(),
            "CARGO_MANIFEST_DIR" | "CARGO_MANIFEST_PATH" | "CARGO_CRATE_NAME" | "CARGO_PRIMARY_PACKAGE"
        ) || name.starts_with("CARGO_PKG_")
            || binary_name.is_some_and(|name| !name.is_empty());
        if !allowed {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest compile environment",
                message: format!("does not permit `{name}`"),
            });
        }
        if (value.is_empty() && !name.starts_with("CARGO_PKG_")) || value.contains('\0') {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest compile environment",
                message: format!("has an invalid value for `{name}`"),
            });
        }
        if binary_name.is_some() && !Path::new(value).is_absolute() {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest compile environment",
                message: format!("`{name}` must name an absolute caller-owned binary output"),
            });
        }
        if value == "@oven-source-root" || value.starts_with("@oven-source-ancestor:") {
            if name != "CARGO_MANIFEST_DIR" {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest compile environment",
                    message: "source-relative tokens are permitted only for CARGO_MANIFEST_DIR".to_string(),
                });
            }
            if value.starts_with("@oven-source-ancestor:") {
                let Some(distance) = value
                    .strip_prefix("@oven-source-ancestor:")
                    .and_then(|value| value.parse::<usize>().ok())
                else {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest compile environment",
                        message: "source-relative ancestor token must end in a positive integer".to_string(),
                    });
                };
                if !(1..=16).contains(&distance) {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest compile environment",
                        message: "source-relative ancestor distance must be between 1 and 16".to_string(),
                    });
                }
            }
        }
    }
    Ok(environment.clone())
}

/// Resolve a portable caller-relative compile environment token after the source has been receipt-authorized.
pub fn resolve_compile_environment_value(name: &str, value: &str, source: &Path) -> Result<PathBuf, OvenRustcError> {
    let distance = if value == "@oven-source-root" {
        2
    } else if let Some(distance) = value
        .strip_prefix("@oven-source-ancestor:")
        .and_then(|value| value.parse::<usize>().ok())
    {
        distance
    } else {
        return Ok(PathBuf::from(value));
    };
    let mut ancestor = source.parent().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "source",
        message: format!("{} has no parent directory for {name}", source.display()),
    })?;
    for _ in 1..distance {
        ancestor = ancestor.parent().ok_or_else(|| OvenRustcError::InvalidInput {
            field: "source",
            message: format!(
                "{} cannot derive source ancestor {distance} for {name}",
                source.display()
            ),
        })?;
    }
    Ok(ancestor.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::{BTreeMap, OvenRustcError, validated_compile_environment};

    /// Cargo represents missing optional package fields and a release version's prerelease field as empty strings.
    #[test]
    fn package_envelope_accepts_empty_metadata_and_crate_coordinates() -> Result<(), OvenRustcError> {
        let values = BTreeMap::from([
            ("CARGO_PKG_VERSION_PRE".into(), String::new()),
            ("CARGO_PKG_AUTHORS".into(), String::new()),
            ("CARGO_MANIFEST_PATH".into(), "/source/Cargo.toml".into()),
            ("CARGO_CRATE_NAME".into(), "example".into()),
            ("CARGO_PRIMARY_PACKAGE".into(), "1".into()),
        ]);
        assert_eq!(validated_compile_environment(&values)?, values);
        assert!(validated_compile_environment(&BTreeMap::from([("CARGO_CRATE_NAME".into(), String::new())])).is_err());
        assert!(
            validated_compile_environment(&BTreeMap::from([("CARGO_PKG_NAME".into(), "bad\0value".into())])).is_err()
        );
        Ok(())
    }
}
