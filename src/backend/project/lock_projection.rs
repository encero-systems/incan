//! Pure validation for Cargo-owned projections of canonical Incan lock payloads.

use std::io;

use toml::Value;

/// Canonical Cargo lock seed used to resolve one generated caller-local manifest offline.
#[derive(Debug, Clone)]
pub(crate) struct CargoLockProjection {
    canonical_payload: String,
}

/// One Cargo-owned attempt to move a regenerated package back onto a canonical coordinate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CargoLockUpdate {
    pub(crate) package_spec: String,
    pub(crate) precise: String,
}

impl CargoLockProjection {
    /// Create a projection seed whose canonical root identity must be preserved exactly until Cargo resolves it.
    pub(crate) fn new(canonical_payload: String, canonical_root_name: String) -> io::Result<Self> {
        let canonical = parse_lock(&canonical_payload, "canonical")?;
        let roots = package_tables(&canonical)
            .into_iter()
            .filter(|package| {
                package_name(package) == Some(canonical_root_name.as_str())
                    && package_version(package) == Some(crate::version::INCAN_VERSION)
                    && package.get("source").is_none()
            })
            .count();
        if roots != 1 {
            return Err(io::Error::other(format!(
                "canonical Cargo.lock contains {roots} source-less roots named `{canonical_root_name}` at Incan \
                 version `{}`; expected exactly one",
                crate::version::INCAN_VERSION
            )));
        }
        validate_dependency_references(&canonical, "canonical")?;
        Ok(Self { canonical_payload })
    }

    /// Return the exact canonical payload that must seed Cargo's first offline resolution pass.
    pub(crate) fn seed_payload(&self) -> &str {
        &self.canonical_payload
    }

    /// Return Cargo update candidates for the first regenerated package that is outside the canonical lock.
    ///
    /// Cargo's `generate-lockfile` deliberately chooses the latest compatible cached registry revision even when it
    /// starts from an existing lock. Reconciliation therefore asks Cargo itself to try canonical versions; it never
    /// computes a dependency closure or edits dependency references in Rust.
    pub(crate) fn next_update_candidates(
        &self,
        projected_payload: &str,
        generated_package_name: &str,
        generated_package_version: &str,
    ) -> io::Result<Option<Vec<CargoLockUpdate>>> {
        let canonical = parse_lock(&self.canonical_payload, "canonical")?;
        let projected = parse_lock(projected_payload, "projected")?;
        let canonical_packages = package_tables(&canonical);

        let mut skipped_generated_root = false;
        for package in package_tables(&projected) {
            if !skipped_generated_root
                && package_name(package) == Some(generated_package_name)
                && package_version(package) == Some(generated_package_version)
                && package.get("source").is_none()
            {
                skipped_generated_root = true;
                continue;
            }
            let coordinate = package_coordinate(package)?;
            let exact = canonical_packages
                .iter()
                .filter(|candidate| package_coordinate(candidate).is_ok_and(|candidate| candidate == coordinate))
                .collect::<Vec<_>>();
            if exact.len() == 1 {
                if package_checksum(package) != package_checksum(exact[0]) {
                    return Err(io::Error::other(format!(
                        "Cargo selected a checksum for `{}` that differs from the canonical Incan lock",
                        coordinate.render()
                    )));
                }
                continue;
            }
            if exact.len() > 1 {
                return Err(io::Error::other(format!(
                    "Cargo selected package coordinate `{}` with {} canonical matches; expected exactly one",
                    coordinate.render(),
                    exact.len()
                )));
            }

            let Some(source) = coordinate.source else {
                return Err(noncanonical_coordinate_error(&coordinate, 0));
            };
            let candidates = canonical_packages
                .iter()
                .filter_map(|candidate| {
                    let candidate = package_coordinate(candidate).ok()?;
                    (candidate.name == coordinate.name && sources_share_update_identity(source, candidate.source?))
                        .then_some(candidate)
                })
                .filter_map(|candidate| {
                    precise_for_source(candidate.source?, candidate.version).map(|precise| CargoLockUpdate {
                        package_spec: package_spec_for_coordinate(&coordinate),
                        precise,
                    })
                })
                .collect::<Vec<_>>();
            if candidates.is_empty() {
                return Err(noncanonical_coordinate_error(&coordinate, 0));
            }
            return Ok(Some(sort_update_candidates(candidates)));
        }
        Ok(None)
    }

    /// Validate one Cargo-produced caller projection against the canonical package authority.
    pub(crate) fn validate_projected(
        &self,
        projected_payload: &str,
        generated_package_name: &str,
        generated_package_version: &str,
    ) -> io::Result<()> {
        let canonical = parse_lock(&self.canonical_payload, "canonical")?;
        let projected = parse_lock(projected_payload, "projected")?;
        let canonical_packages = package_tables(&canonical);
        let projected_packages = package_tables(&projected);
        let generated_roots = projected_packages
            .iter()
            .filter(|package| {
                package_name(package) == Some(generated_package_name)
                    && package_version(package) == Some(generated_package_version)
                    && package.get("source").is_none()
            })
            .count();
        if generated_roots != 1 {
            return Err(io::Error::other(format!(
                "projected Cargo.lock contains {generated_roots} source-less generated roots named \
                 `{generated_package_name}` at version `{generated_package_version}`; expected exactly one"
            )));
        }

        let mut skipped_generated_root = false;
        for package in projected_packages {
            if !skipped_generated_root
                && package_name(package) == Some(generated_package_name)
                && package_version(package) == Some(generated_package_version)
                && package.get("source").is_none()
            {
                skipped_generated_root = true;
                continue;
            }
            let coordinate = package_coordinate(package)?;
            let matches = canonical_packages
                .iter()
                .filter(|candidate| package_coordinate(candidate).is_ok_and(|candidate| candidate == coordinate))
                .collect::<Vec<_>>();
            let [canonical_package] = matches.as_slice() else {
                return Err(io::Error::other(format!(
                    "Cargo selected non-canonical package coordinate `{}` (found {} canonical matches)",
                    coordinate.render(),
                    matches.len()
                )));
            };
            if package_checksum(package) != package_checksum(canonical_package) {
                return Err(io::Error::other(format!(
                    "Cargo selected a checksum for `{}` that differs from the canonical Incan lock",
                    coordinate.render()
                )));
            }
        }
        validate_dependency_references(&projected, "projected")
    }

    /// Require a second Cargo pass to reproduce the first projected lock byte-for-byte.
    pub(crate) fn validate_convergence(&self, first: &str, second: &str) -> io::Result<()> {
        if first == second {
            Ok(())
        } else {
            Err(io::Error::other(
                "Cargo lock projection did not converge after a second offline resolution pass",
            ))
        }
    }
}

fn noncanonical_coordinate_error(coordinate: &PackageCoordinate<'_>, matches: usize) -> io::Error {
    io::Error::other(format!(
        "Cargo selected non-canonical package coordinate `{}` (found {matches} canonical matches)",
        coordinate.render()
    ))
}

fn sources_share_update_identity(left: &str, right: &str) -> bool {
    if left.starts_with("registry+") || left.starts_with("sparse+") {
        return left == right;
    }
    if left.starts_with("git+") && right.starts_with("git+") {
        return left.split_once('#').map_or(left, |(base, _)| base)
            == right.split_once('#').map_or(right, |(base, _)| base);
    }
    false
}

fn precise_for_source(source: &str, version: &str) -> Option<String> {
    if source.starts_with("registry+") || source.starts_with("sparse+") {
        return Some(version.to_string());
    }
    source
        .starts_with("git+")
        .then(|| source.rsplit_once('#').map(|(_, revision)| revision.to_string()))
        .flatten()
}

fn package_spec_for_coordinate(coordinate: &PackageCoordinate<'_>) -> String {
    let Some(source) = coordinate.source else {
        return format!("{}@{}", coordinate.name, coordinate.version);
    };
    let source = if source.starts_with("git+") {
        source.split_once('#').map_or(source, |(base, _)| base)
    } else {
        source
    };
    format!("{source}#{}@{}", coordinate.name, coordinate.version)
}

fn sort_update_candidates(mut candidates: Vec<CargoLockUpdate>) -> Vec<CargoLockUpdate> {
    candidates.sort_by(|left, right| {
        match (
            semver::Version::parse(&left.precise),
            semver::Version::parse(&right.precise),
        ) {
            (Ok(left), Ok(right)) => right.cmp(&left),
            _ => right.precise.cmp(&left.precise),
        }
    });
    candidates.dedup();
    candidates
}

fn parse_lock<'a>(payload: &'a str, role: &str) -> io::Result<Value> {
    toml::from_str(payload)
        .map_err(|error| io::Error::other(format!("failed to parse {role} Cargo.lock payload: {error}")))
}

fn package_tables(document: &Value) -> Vec<&toml::Table> {
    document
        .get("package")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_table)
        .collect()
}

fn package_name(package: &toml::Table) -> Option<&str> {
    package.get("name").and_then(Value::as_str)
}

fn package_version(package: &toml::Table) -> Option<&str> {
    package.get("version").and_then(Value::as_str)
}

fn package_checksum(package: &toml::Table) -> Option<&str> {
    package.get("checksum").and_then(Value::as_str)
}

#[derive(Debug, PartialEq, Eq)]
struct PackageCoordinate<'a> {
    name: &'a str,
    version: &'a str,
    source: Option<&'a str>,
}

impl PackageCoordinate<'_> {
    fn render(&self) -> String {
        match self.source {
            Some(source) => format!("{} {} ({source})", self.name, self.version),
            None => format!("{} {}", self.name, self.version),
        }
    }
}

fn package_coordinate(package: &toml::Table) -> io::Result<PackageCoordinate<'_>> {
    let name = package_name(package).ok_or_else(|| io::Error::other("Cargo.lock package has no name"))?;
    let version = package_version(package)
        .ok_or_else(|| io::Error::other(format!("Cargo.lock package `{name}` has no version")))?;
    Ok(PackageCoordinate {
        name,
        version,
        source: package.get("source").and_then(Value::as_str),
    })
}

fn dependency_reference_matches_package(reference: &str, package: &toml::Table) -> bool {
    let Some(name) = package_name(package) else {
        return false;
    };
    if reference == name {
        return true;
    }
    let Some(version) = package_version(package) else {
        return false;
    };
    if reference == format!("{name} {version}") {
        return true;
    }
    package
        .get("source")
        .and_then(Value::as_str)
        .is_some_and(|source| reference == format!("{name} {version} ({source})"))
}

fn validate_dependency_references(document: &Value, role: &str) -> io::Result<()> {
    let packages = package_tables(document);
    for package in &packages {
        for reference in package
            .get("dependencies")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let matches = packages
                .iter()
                .filter(|candidate| dependency_reference_matches_package(reference, candidate))
                .count();
            if matches != 1 {
                return Err(io::Error::other(format!(
                    "{role} Cargo.lock dependency `{reference}` resolves to {matches} packages; expected exactly one"
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical_payload() -> String {
        format!(
            r#"version = 4

[[package]]
name = "incan_workspace"
version = "{}"
dependencies = ["foo 1.0.0"]

[[package]]
name = "foo"
version = "1.0.0"
"#,
            crate::version::INCAN_VERSION
        )
    }

    #[test]
    fn projection_selects_exact_canonical_root_when_generated_root_collides_with_path_package()
    -> Result<(), Box<dyn std::error::Error>> {
        let projection = CargoLockProjection::new(canonical_payload(), "incan_workspace".to_string())?;
        let projected = format!(
            r#"version = 4

[[package]]
name = "foo"
version = "{}"
dependencies = ["foo 1.0.0"]

[[package]]
name = "foo"
version = "1.0.0"
"#,
            crate::version::INCAN_VERSION
        );

        projection.validate_projected(&projected, "foo", crate::version::INCAN_VERSION)?;
        Ok(())
    }

    #[test]
    fn projection_rejects_noncanonical_checksum() -> Result<(), Box<dyn std::error::Error>> {
        let canonical = format!(
            r#"version = 4

[[package]]
name = "incan_workspace"
version = "{}"
dependencies = ["dep 1.0.0 (registry+https://github.com/rust-lang/crates.io-index)"]

[[package]]
name = "dep"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "canonical"
"#,
            crate::version::INCAN_VERSION
        );
        let projection = CargoLockProjection::new(canonical, "incan_workspace".to_string())?;
        let projected = format!(
            r#"version = 4

[[package]]
name = "caller"
version = "{}"
dependencies = ["dep 1.0.0 (registry+https://github.com/rust-lang/crates.io-index)"]

[[package]]
name = "dep"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "different"
"#,
            crate::version::INCAN_VERSION
        );

        assert!(
            projection
                .validate_projected(&projected, "caller", crate::version::INCAN_VERSION)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn projection_rejects_missing_dependency_reference_target() -> Result<(), Box<dyn std::error::Error>> {
        let projection = CargoLockProjection::new(canonical_payload(), "incan_workspace".to_string())?;
        let projected = format!(
            r#"version = 4

[[package]]
name = "caller"
version = "{}"
dependencies = ["missing 1.0.0"]
"#,
            crate::version::INCAN_VERSION
        );

        assert!(
            projection
                .validate_projected(&projected, "caller", crate::version::INCAN_VERSION)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn projection_requires_byte_convergence() -> Result<(), Box<dyn std::error::Error>> {
        let projection = CargoLockProjection::new(canonical_payload(), "incan_workspace".to_string())?;
        projection.validate_convergence("stable", "stable")?;
        assert!(projection.validate_convergence("first", "second").is_err());
        Ok(())
    }

    #[test]
    fn projection_offers_canonical_registry_versions_to_cargo() -> Result<(), Box<dyn std::error::Error>> {
        let canonical = format!(
            r#"version = 4

[[package]]
name = "incan_workspace"
version = "{}"
dependencies = ["bitflags 1.3.2", "bitflags 2.11.0"]

[[package]]
name = "bitflags"
version = "1.3.2"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "bitflags"
version = "2.11.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#,
            crate::version::INCAN_VERSION
        );
        let projection = CargoLockProjection::new(canonical, "incan_workspace".to_string())?;
        let projected = format!(
            r#"version = 4

[[package]]
name = "bitflags"
version = "2.13.1"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "leaf"
version = "{}"
dependencies = ["bitflags"]
"#,
            crate::version::INCAN_VERSION
        );

        assert_eq!(
            projection.next_update_candidates(&projected, "leaf", crate::version::INCAN_VERSION)?,
            Some(vec![
                CargoLockUpdate {
                    package_spec: "registry+https://github.com/rust-lang/crates.io-index#bitflags@2.13.1".to_string(),
                    precise: "2.11.0".to_string(),
                },
                CargoLockUpdate {
                    package_spec: "registry+https://github.com/rust-lang/crates.io-index#bitflags@2.13.1".to_string(),
                    precise: "1.3.2".to_string(),
                },
            ])
        );
        Ok(())
    }

    #[test]
    fn projection_updates_are_qualified_by_the_projected_source() -> Result<(), Box<dyn std::error::Error>> {
        let canonical = format!(
            r#"version = 4

[[package]]
name = "incan_workspace"
version = "{}"
dependencies = ["dep 1.0.0 (registry+https://first.example/index)"]

[[package]]
name = "dep"
version = "1.0.0"
source = "registry+https://first.example/index"
"#,
            crate::version::INCAN_VERSION
        );
        let projection = CargoLockProjection::new(canonical, "incan_workspace".to_string())?;
        let projected = format!(
            r#"version = 4

[[package]]
name = "caller"
version = "{}"
dependencies = ["dep 2.0.0 (registry+https://first.example/index)"]

[[package]]
name = "dep"
version = "2.0.0"
source = "registry+https://first.example/index"

[[package]]
name = "dep"
version = "2.0.0"
source = "registry+https://second.example/index"
"#,
            crate::version::INCAN_VERSION
        );

        let updates = projection
            .next_update_candidates(&projected, "caller", crate::version::INCAN_VERSION)?
            .ok_or("expected canonical update candidates")?;
        assert_eq!(updates.len(), 1);
        assert_eq!(
            updates[0].package_spec,
            "registry+https://first.example/index#dep@2.0.0"
        );
        Ok(())
    }
}
