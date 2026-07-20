//! Pure validation for Cargo-owned projections of canonical Incan lock payloads.

use std::collections::BTreeSet;
use std::io;

use toml::Value;

/// Canonical Cargo lock seed resolved onto one generated caller-local manifest under the caller's network policy.
#[derive(Debug, Clone)]
pub(crate) struct CargoLockProjection {
    canonical_payload: String,
    canonical_root_name: String,
}

/// One Cargo-owned attempt to move a regenerated package back onto a canonical coordinate.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
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
        Ok(Self {
            canonical_payload,
            canonical_root_name,
        })
    }

    /// Return the exact canonical payload that must seed Cargo's first resolution pass.
    pub(crate) fn seed_payload(&self) -> &str {
        &self.canonical_payload
    }

    /// Return a graph-derived upper bound for successful reconciliation steps.
    ///
    /// Each successful step must retire one previously unseen non-canonical package selection. The sum of canonical
    /// and projected packages is deliberately conservative while remaining proportional to the graph Cargo rendered.
    pub(crate) fn reconciliation_pass_limit(&self, projected_payload: &str) -> io::Result<usize> {
        let canonical = parse_lock(&self.canonical_payload, "canonical")?;
        let projected = parse_lock(projected_payload, "projected")?;
        Ok(package_tables(&canonical)
            .len()
            .saturating_add(package_tables(&projected).len())
            .max(1))
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
        let generated_root = projected_packages
            .iter()
            .find(|package| {
                package_name(package) == Some(generated_package_name)
                    && package_version(package) == Some(generated_package_version)
                    && package.get("source").is_none()
            })
            .ok_or_else(|| io::Error::other("projected Cargo.lock generated root disappeared"))?;
        let canonical_root = canonical_packages
            .iter()
            .find(|package| {
                package_name(package) == Some(self.canonical_root_name.as_str())
                    && package_version(package) == Some(crate::version::INCAN_VERSION)
                    && package.get("source").is_none()
            })
            .ok_or_else(|| io::Error::other("canonical Cargo.lock source root disappeared"))?;
        validate_dependency_edge_subset(
            generated_root,
            &projected_packages,
            canonical_root,
            &canonical_packages,
            "generated root",
        )?;

        let mut skipped_generated_root = false;
        for package in &projected_packages {
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
            validate_dependency_edge_subset(
                package,
                &projected_packages,
                canonical_package,
                &canonical_packages,
                &format!("package `{}`", coordinate.render()),
            )?;
        }
        validate_dependency_references(&projected, "projected")
    }

    /// Require a second Cargo pass to reproduce the first projected lock byte-for-byte.
    pub(crate) fn validate_convergence(&self, first: &str, second: &str) -> io::Result<()> {
        if first == second {
            Ok(())
        } else {
            Err(io::Error::other(
                "Cargo lock projection did not converge after a second resolution pass",
            ))
        }
    }
}

/// Report a Cargo-selected coordinate that canonical lock authority cannot identify uniquely.
fn noncanonical_coordinate_error(coordinate: &PackageCoordinate<'_>, matches: usize) -> io::Error {
    io::Error::other(format!(
        "Cargo selected non-canonical package coordinate `{}` (found {matches} canonical matches)",
        coordinate.render()
    ))
}

/// Return whether two Cargo sources can address the same package in an exact update request.
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

/// Derive Cargo's `--precise` value for one registry or Git package source.
fn precise_for_source(source: &str, version: &str) -> Option<String> {
    if source.starts_with("registry+") || source.starts_with("sparse+") {
        return Some(version.to_string());
    }
    source
        .starts_with("git+")
        .then(|| source.rsplit_once('#').map(|(_, revision)| revision.to_string()))
        .flatten()
}

/// Render a source-qualified package specification for Cargo's update command.
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

/// Sort and deduplicate update candidates so reconciliation attempts are deterministic.
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

/// Parse a Cargo lock payload and retain its authority role in any diagnostic.
fn parse_lock(payload: &str, role: &str) -> io::Result<Value> {
    toml::from_str(payload)
        .map_err(|error| io::Error::other(format!("failed to parse {role} Cargo.lock payload: {error}")))
}

/// Return every package table from a parsed Cargo lock document.
fn package_tables(document: &Value) -> Vec<&toml::Table> {
    document
        .get("package")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_table)
        .collect()
}

/// Read a package table's name when it is a string.
fn package_name(package: &toml::Table) -> Option<&str> {
    package.get("name").and_then(Value::as_str)
}

/// Read a package table's version when it is a string.
fn package_version(package: &toml::Table) -> Option<&str> {
    package.get("version").and_then(Value::as_str)
}

/// Read a package table's checksum when it is a string.
fn package_checksum(package: &toml::Table) -> Option<&str> {
    package.get("checksum").and_then(Value::as_str)
}

#[derive(Debug, PartialEq, Eq)]
struct PackageCoordinate<'a> {
    name: &'a str,
    version: &'a str,
    source: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct OwnedPackageCoordinate {
    name: String,
    version: String,
    source: Option<String>,
}

impl From<PackageCoordinate<'_>> for OwnedPackageCoordinate {
    fn from(coordinate: PackageCoordinate<'_>) -> Self {
        Self {
            name: coordinate.name.to_string(),
            version: coordinate.version.to_string(),
            source: coordinate.source.map(ToOwned::to_owned),
        }
    }
}

impl PackageCoordinate<'_> {
    /// Render this package coordinate for diagnostics.
    fn render(&self) -> String {
        match self.source {
            Some(source) => format!("{} {} ({source})", self.name, self.version),
            None => format!("{} {}", self.name, self.version),
        }
    }
}

/// Decode the canonical coordinate fields required from a Cargo package table.
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

/// Return whether a Cargo dependency reference resolves to the supplied package table.
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

/// Require every dependency reference in one Cargo lock document to resolve exactly once.
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

/// Require every Cargo-produced dependency edge to have been authorized by the matching canonical package.
fn validate_dependency_edge_subset(
    projected_package: &toml::Table,
    projected_packages: &[&toml::Table],
    canonical_package: &toml::Table,
    canonical_packages: &[&toml::Table],
    label: &str,
) -> io::Result<()> {
    let projected = resolved_dependency_coordinates(projected_package, projected_packages, "projected")?;
    let canonical = resolved_dependency_coordinates(canonical_package, canonical_packages, "canonical")?;
    let unauthorized = projected.difference(&canonical).collect::<Vec<_>>();
    if unauthorized.is_empty() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "Cargo projected unauthorized dependency edge(s) from {label}: {unauthorized:?}"
    )))
}

/// Resolve dependency strings to full package coordinates, rejecting ambiguous or dangling references.
fn resolved_dependency_coordinates(
    package: &toml::Table,
    packages: &[&toml::Table],
    role: &str,
) -> io::Result<BTreeSet<OwnedPackageCoordinate>> {
    package
        .get("dependencies")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|reference| {
            let matches = packages
                .iter()
                .filter(|candidate| dependency_reference_matches_package(reference, candidate))
                .collect::<Vec<_>>();
            let [package] = matches.as_slice() else {
                return Err(io::Error::other(format!(
                    "{role} Cargo.lock dependency `{reference}` resolves to {} packages; expected exactly one",
                    matches.len()
                )));
            };
            package_coordinate(package).map(OwnedPackageCoordinate::from)
        })
        .collect()
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
    fn projection_rejects_forged_generated_root_edge_to_unreferenced_canonical_package()
    -> Result<(), Box<dyn std::error::Error>> {
        let canonical = format!(
            r#"version = 4

[[package]]
name = "incan_workspace"
version = "{}"
dependencies = ["foo"]

[[package]]
name = "bar"
version = "1.0.0"

[[package]]
name = "foo"
version = "1.0.0"
"#,
            crate::version::INCAN_VERSION
        );
        let projection = CargoLockProjection::new(canonical, "incan_workspace".to_string())?;
        let projected = format!(
            r#"version = 4

[[package]]
name = "caller"
version = "{}"
dependencies = ["bar", "foo"]

[[package]]
name = "bar"
version = "1.0.0"

[[package]]
name = "foo"
version = "1.0.0"
"#,
            crate::version::INCAN_VERSION
        );

        let error = match projection.validate_projected(&projected, "caller", crate::version::INCAN_VERSION) {
            Ok(()) => return Err("forged generated-root edge was accepted".into()),
            Err(error) => error,
        };
        assert!(error.to_string().contains("unauthorized dependency edge"));
        Ok(())
    }

    #[test]
    fn projection_rejects_forged_transitive_edge_between_canonical_packages() -> Result<(), Box<dyn std::error::Error>>
    {
        let canonical = format!(
            r#"version = 4

[[package]]
name = "incan_workspace"
version = "{}"
dependencies = ["bar", "foo"]

[[package]]
name = "bar"
version = "1.0.0"

[[package]]
name = "foo"
version = "1.0.0"
"#,
            crate::version::INCAN_VERSION
        );
        let projection = CargoLockProjection::new(canonical, "incan_workspace".to_string())?;
        let projected = format!(
            r#"version = 4

[[package]]
name = "caller"
version = "{}"
dependencies = ["foo"]

[[package]]
name = "bar"
version = "1.0.0"

[[package]]
name = "foo"
version = "1.0.0"
dependencies = ["bar"]
"#,
            crate::version::INCAN_VERSION
        );

        let error = match projection.validate_projected(&projected, "caller", crate::version::INCAN_VERSION) {
            Ok(()) => return Err("forged transitive edge was accepted".into()),
            Err(error) => error,
        };
        assert!(error.to_string().contains("unauthorized dependency edge"));
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
    fn reconciliation_pass_limit_is_derived_from_the_canonical_and_projected_graphs()
    -> Result<(), Box<dyn std::error::Error>> {
        let projection = CargoLockProjection::new(canonical_payload(), "incan_workspace".to_string())?;
        let projected = format!(
            r#"version = 4

[[package]]
name = "caller"
version = "{}"

[[package]]
name = "foo"
version = "1.0.0"

[[package]]
name = "extra"
version = "2.0.0"
"#,
            crate::version::INCAN_VERSION
        );

        // Two canonical packages plus three Cargo-projected packages.
        assert_eq!(projection.reconciliation_pass_limit(&projected)?, 5);
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
