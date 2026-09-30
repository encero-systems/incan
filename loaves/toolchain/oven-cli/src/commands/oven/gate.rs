//! Data-driven release gates over checkout pins, semantic consumer locks, and artifact-equivalence evidence.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path};
use std::process::Command;

use oven_model::digest::digest_bytes;
use oven_model::lock::{IncanLock, RegistryRecord, SemanticLockState};
use oven_model::manifest::is_sha256_identity;
use serde::Deserialize;

use super::{CliError, CliResult, ExitCode};

/// Exact schema-1 checkout and lock pin consumed by a release gate.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryPin {
    schema: u64,
    revision: String,
    lock: RegistryLockPin,
}

/// Checkout-relative lock identity in a release gate pin.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryLockPin {
    path: String,
    digest: String,
}

/// Data-driven consumer-graph requirements for one release gate.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GateExpectations {
    schema: u64,
    consumer_only_packages: Vec<String>,
    publisher_only_packages: Vec<String>,
    forbidden_semantic_strings: Vec<String>,
    required_exact_packages: Vec<ExactPackageVersion>,
}

/// One package coordinate that equivalence evidence must select exactly.
#[derive(Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
struct ExactPackageVersion {
    package: String,
    version: String,
}

/// Validate one checkout and committed lock against an exact registry pin.
pub fn oven_gate_registry_pin(pin: &Path, checkout: &Path) -> CliResult<ExitCode> {
    verify_registry_pin(pin, checkout).map_err(gate_error)?;
    Ok(ExitCode::SUCCESS)
}

/// Validate a semantic consumer graph against data-driven expectations and artifact-equivalence evidence.
pub fn oven_gate_consumer_graph(
    expectations: &Path,
    locks: &[impl AsRef<Path>],
    equivalence_report: &Path,
) -> CliResult<ExitCode> {
    verify_consumer_graph(expectations, locks, equivalence_report).map_err(gate_error)?;
    Ok(ExitCode::SUCCESS)
}

/// Check pin schema, checkout revision and cleanliness, and the pinned lock bytes.
fn verify_registry_pin(pin_path: &Path, checkout: &Path) -> Result<(), String> {
    let pin_bytes = fs::read(pin_path).map_err(|error| format!("could not read {}: {error}", pin_path.display()))?;
    let pin: RegistryPin = serde_json::from_slice(&pin_bytes).map_err(|error| {
        format!(
            "{}: expected exact schema-1 registry pin fields: {error}",
            pin_path.display()
        )
    })?;
    if pin.schema != 1 {
        return Err(format!(
            "{}: expected exact schema-1 registry pin fields",
            pin_path.display()
        ));
    }
    if !is_lowercase_commit_id(&pin.revision) {
        return Err(format!(
            "{}: revision must be one lowercase full commit id",
            pin_path.display()
        ));
    }
    if !is_normalized_relative_path(&pin.lock.path) {
        return Err(format!(
            "{}: lock path must be normalized and checkout-relative",
            pin_path.display()
        ));
    }
    if !is_sha256_identity(&pin.lock.digest) {
        return Err(format!(
            "{}: lock digest must be one lowercase sha256 identity",
            pin_path.display()
        ));
    }
    let actual_revision = git_output(checkout, &["rev-parse", "HEAD"])?;
    if actual_revision != pin.revision {
        return Err(format!(
            "checkout revision is {actual_revision}, expected pinned {}",
            pin.revision
        ));
    }
    let dirty = git_output(checkout, &["status", "--porcelain"])?;
    if !dirty.is_empty() {
        return Err(format!(
            "the pinned checkout has tracked or untracked changes:\n{dirty}"
        ));
    }
    let lock_path = checkout.join(&pin.lock.path);
    let lock_bytes = fs::read(&lock_path).map_err(|error| {
        format!(
            "pinned lock is missing or unreadable at {}: {error}",
            lock_path.display()
        )
    })?;
    let actual_digest = digest_bytes(&lock_bytes);
    if actual_digest != pin.lock.digest {
        return Err(format!(
            "lock digest is {actual_digest}, expected pinned {}",
            pin.lock.digest
        ));
    }
    Ok(())
}

/// Run one read-only git query and return its trimmed standard output.
fn git_output(checkout: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(arguments)
        .output()
        .map_err(|error| format!("checkout is not a readable git worktree: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!(
            "checkout is not a readable git worktree: {}",
            if stderr.is_empty() { "git failed" } else { &stderr }
        ));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_string())
        .map_err(|error| format!("git returned non-UTF-8 output: {error}"))
}

/// Return whether text is one lowercase 40-digit hexadecimal commit id.
fn is_lowercase_commit_id(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Return whether text is a normalized, nonempty, checkout-relative path.
fn is_normalized_relative_path(value: &str) -> bool {
    if value.is_empty() || value.contains('\\') {
        return false;
    }
    let path = Path::new(value);
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        && path.to_string_lossy() == value
}

/// Join selected equivalence units to exact attested registry records from consumer locks.
fn verify_consumer_graph(
    expectations_path: &Path,
    locks: &[impl AsRef<Path>],
    equivalence_report: &Path,
) -> Result<(), String> {
    let expectations = load_expectations(expectations_path)?;
    let mut records: BTreeMap<(String, String), RegistryRecord> = BTreeMap::new();
    for lock_path in locks {
        let path = lock_path.as_ref();
        if !path.is_file() {
            return Err(format!("consumer lock does not exist: {}", path.display()));
        }
        let lock = IncanLock::load(path).map_err(|error| error.to_string())?;
        refuse_forbidden_semantic_strings(path, &lock.semantic, &expectations.forbidden_semantic_strings)?;
        for record in validate_registry_records(path, &lock.semantic.registry_records)? {
            let key = (record.package.clone(), record.version.clone());
            if let Some(previous) = records.insert(key.clone(), record.clone())
                && previous != record
            {
                return Err(format!(
                    "consumer locks disagree about registry record {} {}",
                    key.0, key.1
                ));
            }
        }
    }
    for package in &expectations.publisher_only_packages {
        if records.keys().any(|(record_package, _)| record_package == package) {
            return Err(format!(
                "publisher-only package {package} remains in the consumer graph"
            ));
        }
    }
    let selected = load_equivalence(equivalence_report, &expectations)?;
    for (package, version) in selected {
        if !expectations.publisher_only_packages.contains(&package)
            && !records.contains_key(&(package.clone(), version.clone()))
        {
            return Err(format!(
                "selected unit {package} {version} has no attested registry record"
            ));
        }
    }
    Ok(())
}

/// Decode and validate one schema-1 consumer-graph expectation file.
fn load_expectations(path: &Path) -> Result<GateExpectations, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let expectations: GateExpectations =
        toml::from_str(&text).map_err(|error| format!("could not decode {}: {error}", path.display()))?;
    if expectations.schema != 1 {
        return Err(format!("{}: expected schema-1 gate expectations", path.display()));
    }
    validate_unique_nonempty_strings(path, "consumer_only_packages", &expectations.consumer_only_packages)?;
    validate_unique_nonempty_strings(path, "publisher_only_packages", &expectations.publisher_only_packages)?;
    validate_unique_nonempty_strings(
        path,
        "forbidden_semantic_strings",
        &expectations.forbidden_semantic_strings,
    )?;
    let mut exact = BTreeSet::new();
    for coordinate in &expectations.required_exact_packages {
        if coordinate.package.is_empty() || coordinate.version.is_empty() {
            return Err(format!(
                "{}: required_exact_packages entries need nonempty package and version fields",
                path.display()
            ));
        }
        if !exact.insert(coordinate) {
            return Err(format!(
                "{}: required_exact_packages repeats {} {}",
                path.display(),
                coordinate.package,
                coordinate.version
            ));
        }
    }
    Ok(expectations)
}

/// Require one expectation list to contain distinct nonempty strings.
fn validate_unique_nonempty_strings(path: &Path, field: &str, values: &[String]) -> Result<(), String> {
    let mut unique = BTreeSet::new();
    for value in values {
        if value.is_empty() {
            return Err(format!("{}: {field} contains an empty string", path.display()));
        }
        if !unique.insert(value) {
            return Err(format!("{}: {field} repeats {value}", path.display()));
        }
    }
    Ok(())
}

/// Refuse configured strings anywhere in typed semantic lock state.
fn refuse_forbidden_semantic_strings(
    path: &Path,
    semantic: &SemanticLockState,
    forbidden_strings: &[String],
) -> Result<(), String> {
    let value = serde_json::to_value(semantic)
        .map_err(|error| format!("could not inspect semantic state in {}: {error}", path.display()))?;
    let mut forbidden = BTreeSet::new();
    collect_forbidden_strings(&value, forbidden_strings, &mut forbidden);
    if !forbidden.is_empty() {
        return Err(format!(
            "{}: consumer semantic graph retains forbidden semantic strings: {}",
            path.display(),
            forbidden.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    Ok(())
}

/// Recursively collect configured tokens from JSON-shaped semantic state.
fn collect_forbidden_strings(
    value: &serde_json::Value,
    forbidden_strings: &[String],
    forbidden: &mut BTreeSet<String>,
) {
    match value {
        serde_json::Value::String(text) => {
            for token in forbidden_strings {
                if contains_semantic_token(text, token) {
                    forbidden.insert(token.clone());
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_forbidden_strings(item, forbidden_strings, forbidden);
            }
        }
        serde_json::Value::Object(items) => {
            for item in items.values() {
                collect_forbidden_strings(item, forbidden_strings, forbidden);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

/// Match one token using ASCII package-name boundaries.
fn contains_semantic_token(text: &str, token: &str) -> bool {
    let lowercase = text.to_ascii_lowercase();
    let token = token.to_ascii_lowercase();
    lowercase.match_indices(&token).any(|(index, matched)| {
        let before = lowercase[..index].bytes().next_back();
        let after = lowercase[index + matched.len()..].bytes().next();
        before.is_none_or(|byte| !is_package_character(byte)) && after.is_none_or(|byte| !is_package_character(byte))
    })
}

/// Return whether one byte belongs to a package/tool token.
fn is_package_character(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
}

/// Validate exact attested registry identities and collapse identical duplicates.
fn validate_registry_records(path: &Path, records: &[RegistryRecord]) -> Result<Vec<RegistryRecord>, String> {
    let mut validated: BTreeMap<(String, String), RegistryRecord> = BTreeMap::new();
    for record in records {
        if record.package.is_empty() || record.version.is_empty() {
            return Err(format!(
                "{}: registry record has no package/version identity",
                path.display()
            ));
        }
        if !is_sha256_identity(&record.checksum) {
            return Err(format!(
                "{}: {} {} has a malformed source checksum",
                path.display(),
                record.package,
                record.version
            ));
        }
        if !is_sha256_identity(&record.index_line_digest) {
            return Err(format!(
                "{}: {} {} has a malformed pinned index-line identity",
                path.display(),
                record.package,
                record.version
            ));
        }
        if record.status != "attested" {
            return Err(format!(
                "{}: {} {} registry status is {:?}, not 'attested'",
                path.display(),
                record.package,
                record.version,
                record.status
            ));
        }
        let key = (record.package.clone(), record.version.clone());
        if let Some(previous) = validated.insert(key.clone(), record.clone())
            && previous != *record
        {
            return Err(format!(
                "{}: {} {} has conflicting registry records",
                path.display(),
                key.0,
                key.1
            ));
        }
    }
    Ok(validated.into_values().collect())
}

/// Validate equivalence evidence against configured coverage and return its selected package/version bindings.
fn load_equivalence(path: &Path, expectations: &GateExpectations) -> Result<Vec<(String, String)>, String> {
    let bytes = fs::read(path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let report: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| format!("could not decode {}: {error}", path.display()))?;
    if report.get("schema") != Some(&serde_json::json!(1))
        || report.get("event") != Some(&serde_json::json!("attest"))
        || report.get("status") != Some(&serde_json::json!("attested"))
    {
        return Err(format!(
            "{}: equivalence report is not a schema-1 attestation",
            path.display()
        ));
    }
    let expected_oven = serde_json::json!({"producer": "oven-publisher", "cargo_free": true});
    if report.pointer("/captures/oven") != Some(&expected_oven) {
        return Err(format!(
            "{}: equivalence report has no Cargo-free Oven publisher capture",
            path.display()
        ));
    }
    let bindings = report
        .get("bindings")
        .and_then(serde_json::Value::as_array)
        .filter(|bindings| !bindings.is_empty())
        .ok_or_else(|| format!("{}: equivalence report has no selected bindings", path.display()))?;
    let mut selected = Vec::new();
    let mut binding_names = BTreeSet::new();
    let mut identities = BTreeMap::new();
    for (index, binding) in bindings.iter().enumerate() {
        let object = binding
            .as_object()
            .ok_or_else(|| format!("{}: bindings[{index}] is not an object", path.display()))?;
        let label = required_string(object.get("binding"), path, index)?;
        let package = required_string(object.get("package"), path, index)?;
        let version = required_string(object.get("version"), path, index)?;
        let identity = object
            .get("unit_identity")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("{}: {label} has a malformed unit identity", path.display()))?;
        if !is_sha256_identity(identity) {
            return Err(format!("{}: {label} has a malformed unit identity", path.display()));
        }
        if !binding_names.insert(label.clone()) {
            return Err(format!("{}: repeats binding {label}", path.display()));
        }
        if let Some(previous) = identities.insert(identity.to_string(), label.clone()) {
            return Err(format!(
                "{}: {label} repeats unit identity {identity} from {previous}",
                path.display()
            ));
        }
        selected.push((package, version));
    }
    let present: BTreeSet<_> = selected.iter().map(|(package, _)| package.as_str()).collect();
    let missing: Vec<_> = expectations
        .consumer_only_packages
        .iter()
        .map(String::as_str)
        .filter(|package| !present.contains(package))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "{}: equivalence report is missing consumer-only packages: {}",
            path.display(),
            missing.join(", ")
        ));
    }
    let selected_coordinates: BTreeSet<_> = selected
        .iter()
        .map(|(package, version)| (package.as_str(), version.as_str()))
        .collect();
    let missing_exact: Vec<_> = expectations
        .required_exact_packages
        .iter()
        .filter(|coordinate| {
            !selected_coordinates.contains(&(coordinate.package.as_str(), coordinate.version.as_str()))
        })
        .map(|coordinate| format!("{} {}", coordinate.package, coordinate.version))
        .collect();
    if !missing_exact.is_empty() {
        return Err(format!(
            "{}: equivalence report is missing exact package versions: {}",
            path.display(),
            missing_exact.join(", ")
        ));
    }
    Ok(selected)
}

/// Read one required nonempty string from an equivalence binding.
fn required_string(value: Option<&serde_json::Value>, path: &Path, index: usize) -> Result<String, String> {
    value
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            format!(
                "{}: bindings[{index}] has no binding/package/version identity",
                path.display()
            )
        })
}

/// Prefix a release-gate validation failure with the stable diagnostic label.
fn gate_error(error: String) -> CliError {
    CliError::failure(format!("Oven gate: {error}"))
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;

    use oven_model::lock::{CargoFeatureSelection, LockedProvider};

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Run a successful fixture command and return its trimmed standard output.
    fn fixture_command(program: &str, arguments: &[&str], directory: &Path) -> Result<String, String> {
        let output = Command::new(program)
            .args(arguments)
            .current_dir(directory)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("could not run {program}: {error}"))?;
        if !output.status.success() {
            return Err(format!("{program} failed: {}", String::from_utf8_lossy(&output.stderr)));
        }
        String::from_utf8(output.stdout)
            .map(|value| value.trim().to_string())
            .map_err(|error| error.to_string())
    }

    /// Build one clean committed checkout and its exact schema-1 pin.
    fn pinned_checkout(root: &Path) -> TestResult {
        let checkout = root.join("consumer");
        fs::create_dir(&checkout)?;
        fs::write(checkout.join("incan.lock"), "pinned consumer lock\n")?;
        fixture_command("git", &["init", "-q"], &checkout)?;
        fixture_command("git", &["config", "user.email", "fixture@example.invalid"], &checkout)?;
        fixture_command("git", &["config", "user.name", "Fixture"], &checkout)?;
        fixture_command("git", &["add", "."], &checkout)?;
        fixture_command("git", &["commit", "-qm", "fixture"], &checkout)?;
        let revision = fixture_command("git", &["rev-parse", "HEAD"], &checkout)?;
        let lock_digest = digest_bytes(&fs::read(checkout.join("incan.lock"))?);
        fs::write(
            root.join("pin.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema": 1,
                "revision": revision,
                "lock": {"path": "incan.lock", "digest": lock_digest}
            }))?,
        )?;
        Ok(())
    }

    /// Return the synthetic package/version set in equivalence order.
    fn fixture_packages() -> Vec<(&'static str, &'static str)> {
        vec![
            ("consumer-alpha", "1.2.3"),
            ("consumer-beta", "7.4.2"),
            ("shared-library", "5.0.1"),
            ("publisher-tool", "9.8.7"),
        ]
    }

    /// Write one schema-1 expectation file using only synthetic package names.
    fn write_expectations(path: &Path) -> TestResult {
        fs::write(
            path,
            r#"schema = 1
consumer_only_packages = ["consumer-alpha", "consumer-beta"]
publisher_only_packages = ["publisher-tool"]
forbidden_semantic_strings = ["bootstrap-tool"]

[[required_exact_packages]]
package = "consumer-beta"
version = "7.4.2"
"#,
        )?;
        Ok(())
    }

    /// Write one typed Oven lock containing the requested admitted registry records.
    fn write_lock(path: &Path, omit: Option<&str>, include_publisher_only: bool, forbidden_string: bool) -> TestResult {
        let mut records = Vec::new();
        for (index, (package, version)) in fixture_packages().into_iter().enumerate() {
            if Some(package) == omit || (package == "publisher-tool" && !include_publisher_only) {
                continue;
            }
            records.push(RegistryRecord {
                package: package.to_string(),
                version: version.to_string(),
                checksum: format!("sha256:{index:064x}"),
                index_line_digest: format!("sha256:{:064x}", index + 100),
                status: "attested".to_string(),
            });
        }
        let mut semantic = SemanticLockState {
            registry_records: records,
            ..SemanticLockState::default()
        };
        if forbidden_string {
            semantic.providers.push(LockedProvider {
                identity: "bootstrap-tool fixture".to_string(),
                participation: "private".to_string(),
                namespace_claims: BTreeSet::new(),
                used_modules: BTreeSet::new(),
                implementation_facets: Vec::new(),
                backend_requirements: BTreeSet::new(),
            });
        }
        IncanLock::new_with_semantic(
            "0.6.0",
            "sha256:fixture".to_string(),
            CargoFeatureSelection::default(),
            semantic,
            "version = 4\n".to_string(),
        )
        .write(path)?;
        Ok(())
    }

    /// Write schema-1 equivalence evidence with optional missing, changed, or duplicate binding identity.
    fn write_equivalence(
        path: &Path,
        omit: Option<&str>,
        change_exact_version: bool,
        duplicate_identity: bool,
    ) -> TestResult {
        let mut bindings = Vec::new();
        for (index, (package, fixture_version)) in fixture_packages().into_iter().enumerate() {
            if Some(package) == omit {
                continue;
            }
            let version = if change_exact_version && package == "consumer-beta" {
                "7.4.1"
            } else {
                fixture_version
            };
            let identity_index = if duplicate_identity && index == 1 { 0 } else { index };
            bindings.push(serde_json::json!({
                "binding": format!("{package} {version} fixture-target release"),
                "package": package,
                "version": version,
                "unit_identity": format!("sha256:{:064x}", identity_index + 1000)
            }));
        }
        fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({
                "schema": 1,
                "event": "attest",
                "status": "attested",
                "captures": {
                    "cargo": {"producer": "cargo-harvest", "cargo_free": false},
                    "oven": {"producer": "oven-publisher", "cargo_free": true}
                },
                "bindings": bindings
            }))?,
        )?;
        Ok(())
    }

    /// Prove registry pins accept exact state and refuse revision, lock-byte, or worktree drift.
    #[test]
    fn registry_pin_accepts_exact_checkout_and_refuses_revision_lock_or_dirty_drift() -> TestResult {
        let root = tempfile::tempdir()?;
        pinned_checkout(root.path())?;
        let checkout = root.path().join("consumer");
        let pin_path = root.path().join("pin.json");
        verify_registry_pin(&pin_path, &checkout)?;

        let original: serde_json::Value = serde_json::from_slice(&fs::read(&pin_path)?)?;
        let mut stale_revision = original.clone();
        stale_revision["revision"] = serde_json::json!("0".repeat(40));
        fs::write(&pin_path, serde_json::to_vec(&stale_revision)?)?;
        let revision_error = verify_registry_pin(&pin_path, &checkout)
            .err()
            .ok_or("stale checkout revision was accepted")?;
        assert!(revision_error.contains("checkout revision"), "{revision_error}");

        let mut stale_lock = original.clone();
        stale_lock["lock"]["digest"] = serde_json::json!(format!("sha256:{}", "0".repeat(64)));
        fs::write(&pin_path, serde_json::to_vec(&stale_lock)?)?;
        let lock_error = verify_registry_pin(&pin_path, &checkout)
            .err()
            .ok_or("stale lock digest was accepted")?;
        assert!(lock_error.contains("lock digest"), "{lock_error}");

        fs::write(&pin_path, serde_json::to_vec(&original)?)?;
        fs::write(checkout.join("untracked"), "dirty")?;
        let dirty_error = verify_registry_pin(&pin_path, &checkout)
            .err()
            .ok_or("dirty checkout was accepted")?;
        assert!(dirty_error.contains("tracked or untracked changes"), "{dirty_error}");
        Ok(())
    }

    /// Prove a complete graph satisfying every synthetic expectation passes.
    #[test]
    fn complete_attested_consumer_graph_passes() -> TestResult {
        let root = tempfile::tempdir()?;
        let expectations = root.path().join("expectations.toml");
        let lock = root.path().join("oven.lock");
        let equivalence = root.path().join("equivalence.json");
        write_expectations(&expectations)?;
        write_lock(&lock, None, false, false)?;
        write_equivalence(&equivalence, None, false, false)?;
        verify_consumer_graph(&expectations, &[lock], &equivalence)?;
        Ok(())
    }

    /// Prove missing records, missing consumer-only packages, and duplicate identities refuse.
    #[test]
    fn incomplete_records_equivalence_and_duplicate_identities_refuse() -> TestResult {
        let root = tempfile::tempdir()?;
        let expectations = root.path().join("expectations.toml");
        let lock = root.path().join("oven.lock");
        let equivalence = root.path().join("equivalence.json");
        write_expectations(&expectations)?;
        write_lock(&lock, Some("consumer-alpha"), false, false)?;
        write_equivalence(&equivalence, None, false, false)?;
        let missing_record = verify_consumer_graph(&expectations, &[&lock], &equivalence)
            .err()
            .ok_or("missing registry record was accepted")?;
        assert!(
            missing_record.contains("has no attested registry record"),
            "{missing_record}"
        );

        write_lock(&lock, None, false, false)?;
        write_equivalence(&equivalence, Some("consumer-alpha"), false, false)?;
        let missing_package = verify_consumer_graph(&expectations, &[&lock], &equivalence)
            .err()
            .ok_or("incomplete equivalence closure was accepted")?;
        assert!(
            missing_package.contains("missing consumer-only packages"),
            "{missing_package}"
        );

        write_equivalence(&equivalence, None, false, true)?;
        let duplicate = verify_consumer_graph(&expectations, &[&lock], &equivalence)
            .err()
            .ok_or("duplicate unit identity was accepted")?;
        assert!(duplicate.contains("repeats unit identity"), "{duplicate}");
        Ok(())
    }

    /// Prove configured forbidden strings and publisher-only registry records refuse.
    #[test]
    fn forbidden_semantic_strings_and_publisher_records_refuse() -> TestResult {
        let root = tempfile::tempdir()?;
        let expectations = root.path().join("expectations.toml");
        let lock = root.path().join("oven.lock");
        let equivalence = root.path().join("equivalence.json");
        write_expectations(&expectations)?;
        write_equivalence(&equivalence, None, false, false)?;

        write_lock(&lock, None, false, true)?;
        let forbidden = verify_consumer_graph(&expectations, &[&lock], &equivalence)
            .err()
            .ok_or("forbidden semantic string was accepted")?;
        assert!(forbidden.contains("forbidden semantic strings"), "{forbidden}");

        write_lock(&lock, None, true, false)?;
        let publisher = verify_consumer_graph(&expectations, &[&lock], &equivalence)
            .err()
            .ok_or("publisher-only record was accepted")?;
        assert!(
            publisher.contains("publisher-only package publisher-tool"),
            "{publisher}"
        );
        Ok(())
    }

    /// Prove an expected exact package version cannot be replaced by another version.
    #[test]
    fn missing_required_exact_package_version_refuses() -> TestResult {
        let root = tempfile::tempdir()?;
        let expectations = root.path().join("expectations.toml");
        let lock = root.path().join("oven.lock");
        let equivalence = root.path().join("equivalence.json");
        write_expectations(&expectations)?;
        write_lock(&lock, None, false, false)?;
        write_equivalence(&equivalence, None, true, false)?;
        let error = verify_consumer_graph(&expectations, &[&lock], &equivalence)
            .err()
            .ok_or("missing exact package version was accepted")?;
        assert!(
            error.contains("missing exact package versions: consumer-beta 7.4.2"),
            "{error}"
        );
        Ok(())
    }
}
