//! Binding-level closure inventory generation from typed Oven harvest proposals and Cargo lock authority.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use oven_cargo_compat::{
    HARVEST_PROPOSAL_FILE, HarvestEffectClass, HarvestProposal, HarvestRefusal, HarvestRefusalReason,
    cargo_lock_package_graph, cargo_registry_checksums,
};
use oven_model::digest::digest_bytes;
use serde::{Deserialize, Serialize};

use super::{CliError, CliResult, ExitCode, OvenInventoryCommandOptions};

/// Stable schema version of the binding-level closure inventory.
const INVENTORY_SCHEMA: u64 = 1;

/// One inventory source and its exact lock authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InventorySource {
    /// Source label, currently `incan` or `incql`.
    source: String,
    /// Digest of the exact Cargo lock joined to the harvest.
    lock_digest: String,
    /// Number of unique registry package records in that lock.
    registry_package_count: usize,
}

/// One selected registry binding and the effect classes its harvest observed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InventoryBinding {
    /// Source label owning the binding.
    source: String,
    /// Registry package name.
    package: String,
    /// Exact registry package version.
    version: String,
    /// Canonical registry archive checksum.
    checksum: String,
    /// Sorted, duplicate-free selected features.
    features: Vec<String>,
    /// Host triple that observed the fact.
    host: String,
    /// Target triple the fact binds.
    target: String,
    /// Build profile the fact binds.
    profile: String,
    /// Sorted effect classes, including `empty` when the fact has no effects.
    effects: Vec<HarvestEffectClass>,
}

/// Canonical binding-level closure inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClosureInventory {
    /// Inventory schema version.
    schema: u64,
    /// Lock authorities contributing selected bindings.
    sources: Vec<InventorySource>,
    /// All selected bindings in deterministic order.
    bindings: Vec<InventoryBinding>,
}

/// A source inventory before its bindings are merged into the top-level list.
struct SourceInventory {
    source: InventorySource,
    bindings: Vec<InventoryBinding>,
}

/// Generate or check a typed binding-level harvest closure inventory.
pub fn oven_inventory(options: OvenInventoryCommandOptions) -> CliResult<ExitCode> {
    let inventory = generate_inventory(&options).map_err(inventory_error)?;
    let encoded = encode_inventory(&inventory).map_err(inventory_error)?;
    match (&options.output, &options.check) {
        (Some(output), None) => fs::write(output, encoded).map_err(|error| {
            CliError::failure(format!(
                "oven closure inventory: could not write {}: {error}",
                output.display()
            ))
        })?,
        (None, Some(check)) => check_inventory(check, &inventory)?,
        _ => {
            return Err(CliError::failure(
                "oven closure inventory: exactly one of --output or --check is required".to_string(),
            ));
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Generate the complete inventory requested by command options.
fn generate_inventory(options: &OvenInventoryCommandOptions) -> Result<ClosureInventory, String> {
    if options.incql_lock.is_some() != options.incql_harvest.is_some() {
        return Err("--incql-lock and --incql-harvest must be supplied together".to_string());
    }
    let mut inventories = vec![source_inventory("incan", &options.incan_lock, &options.incan_harvest)?];
    if let (Some(lock), Some(harvest)) = (&options.incql_lock, &options.incql_harvest) {
        inventories.push(source_inventory("incql", lock, harvest)?);
    }
    let mut sources = Vec::new();
    let mut bindings = Vec::new();
    for inventory in inventories {
        sources.push(inventory.source);
        bindings.extend(inventory.bindings);
    }
    bindings.sort();
    Ok(ClosureInventory {
        schema: INVENTORY_SCHEMA,
        sources,
        bindings,
    })
}

/// Join one harvest directory to exact registry identities from its supplied Cargo lock.
fn source_inventory(label: &str, lock: &Path, harvest: &Path) -> Result<SourceInventory, String> {
    let lock_bytes = fs::read(lock).map_err(|error| format!("could not read {}: {error}", lock.display()))?;
    cargo_lock_package_graph(&lock_bytes, "closure inventory Cargo.lock").map_err(|error| error.to_string())?;
    let lock_records = cargo_registry_checksums(&lock_bytes).map_err(|error| error.to_string())?;
    let mut packages = BTreeMap::new();
    for ((name, version, _registry), checksum) in lock_records {
        let checksum = canonical_checksum(&checksum)?;
        let key = (name.clone(), version.clone(), checksum.clone());
        if packages.insert(key, ()).is_some() {
            return Err(format!("lock repeats registry package {name} {version} {checksum}"));
        }
    }
    let proposal_paths = proposal_paths(harvest)?;
    if proposal_paths.is_empty() {
        return Err(format!("{label} harvest contains no proposals"));
    }
    let unresolved = unresolved_refusals(harvest)?;
    if !unresolved.is_empty() {
        return Err(format!(
            "{label} harvest contains unresolved selected bindings: {}",
            unresolved.join(", ")
        ));
    }
    let lock_digest = digest_bytes(&lock_bytes);
    let mut bindings = BTreeMap::new();
    for path in proposal_paths {
        let proposal_bytes = fs::read(&path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
        let proposal: HarvestProposal = serde_json::from_slice(&proposal_bytes)
            .map_err(|error| format!("could not decode {}: {error}", path.display()))?;
        if proposal.evidence.cargo_lock_digest != lock_digest {
            return Err(format!(
                "{} binds Cargo.lock {}, not supplied lock {lock_digest}",
                path.display(),
                proposal.evidence.cargo_lock_digest
            ));
        }
        let package_key = (
            proposal.project.name.clone(),
            proposal.project.version.clone(),
            canonical_checksum(&proposal.source.checksum)?,
        );
        if !packages.contains_key(&package_key) {
            return Err(format!(
                "{} observes package absent from lock: {} {} {}",
                path.display(),
                package_key.0,
                package_key.1,
                package_key.2
            ));
        }
        let [fact] = proposal.rust.facts.as_slice() else {
            return Err(format!("{} must contain exactly one fact", path.display()));
        };
        if fact.features.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(format!("{} features are not sorted and duplicate-free", path.display()));
        }
        let binding = InventoryBinding {
            source: label.to_string(),
            package: package_key.0.clone(),
            version: package_key.1.clone(),
            checksum: package_key.2,
            features: fact.features.clone(),
            host: proposal.evidence.host,
            target: fact.target.clone(),
            profile: fact.profile.clone(),
            effects: fact.effect_classes(),
        };
        let key = (
            binding.source.clone(),
            binding.package.clone(),
            binding.version.clone(),
            binding.checksum.clone(),
            binding.features.clone(),
            binding.host.clone(),
            binding.target.clone(),
            binding.profile.clone(),
        );
        if let Some(previous) = bindings.insert(key, binding.clone()) {
            let kind = if previous == binding {
                "duplicate"
            } else {
                "conflicting"
            };
            return Err(format!(
                "{kind} selected binding in {}: {} {}",
                path.display(),
                binding.package,
                binding.version
            ));
        }
    }
    Ok(SourceInventory {
        source: InventorySource {
            source: label.to_string(),
            lock_digest,
            registry_package_count: packages.len(),
        },
        bindings: bindings.into_values().collect(),
    })
}

/// Return proposal files in directory-name order without following unrelated entries.
fn proposal_paths(root: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(root).map_err(|error| format!("could not read {}: {error}", root.display()))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("could not read {} entry: {error}", root.display()))?;
        let path = entry.path().join(HARVEST_PROPOSAL_FILE);
        if path.is_file() {
            paths.push(path);
        }
    }
    paths.sort_by(|left, right| left.parent().cmp(&right.parent()));
    Ok(paths)
}

/// Return non-bookkeeping selected-unit refusals from every harvest refusal report.
fn unresolved_refusals(root: &Path) -> Result<Vec<String>, String> {
    let entries = fs::read_dir(root).map_err(|error| format!("could not read {}: {error}", root.display()))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("could not read {} entry: {error}", root.display()))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("refusals-") && name.ends_with(".json") {
            paths.push(entry.path());
        }
    }
    paths.sort();
    let mut unresolved = Vec::new();
    for path in paths {
        let bytes = fs::read(&path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
        let refusals: Vec<HarvestRefusal> =
            serde_json::from_slice(&bytes).map_err(|error| format!("could not decode {}: {error}", path.display()))?;
        for refusal in refusals {
            if !matches!(
                refusal.reason,
                HarvestRefusalReason::BuildScriptUnit | HarvestRefusalReason::NotRegistryBacked
            ) {
                let reason = serde_json::to_value(refusal.reason)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_string))
                    .unwrap_or_else(|| format!("{:?}", refusal.reason));
                unresolved.push(format!(
                    "{} {} ({reason}: {})",
                    refusal.package, refusal.version, refusal.detail
                ));
            }
        }
    }
    Ok(unresolved)
}

/// Canonicalize a bare or prefixed lowercase SHA-256 checksum.
fn canonical_checksum(checksum: &str) -> Result<String, String> {
    let canonical = if checksum.starts_with("sha256:") {
        checksum.to_string()
    } else {
        format!("sha256:{checksum}")
    };
    let Some(digest) = canonical.strip_prefix("sha256:") else {
        return Err("checksum is not a SHA-256 identity".to_string());
    };
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(format!("checksum is not a lowercase SHA-256 identity: {checksum}"));
    }
    Ok(canonical)
}

/// Serialize an inventory with canonical key ordering and a trailing newline.
fn encode_inventory(inventory: &ClosureInventory) -> Result<Vec<u8>, String> {
    let value = serde_json::to_value(inventory).map_err(|error| error.to_string())?;
    let mut encoded = serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?;
    encoded.push('\n');
    Ok(encoded.into_bytes())
}

/// Compare a generated inventory with a canonicalized checked fixture.
fn check_inventory(path: &Path, generated: &ClosureInventory) -> CliResult<()> {
    let bytes = fs::read(path).map_err(|error| {
        CliError::failure(format!(
            "oven closure inventory: could not read {}: {error}",
            path.display()
        ))
    })?;
    let expected: ClosureInventory = serde_json::from_slice(&bytes).map_err(|error| {
        CliError::failure(format!(
            "oven closure inventory: could not decode {}: {error}",
            path.display()
        ))
    })?;
    if expected.sources.iter().any(|source| source.source == "incql") && generated.sources.len() == 1 {
        return Err(CliError::failure(
            "oven closure inventory: combined inventory checking requires --incql-lock and --incql-harvest".to_string(),
        ));
    }
    if expected != *generated {
        return Err(CliError::failure(format!(
            "oven closure inventory: generated closure inventory differs from {}",
            path.display()
        )));
    }
    Ok(())
}

/// Prefix an inventory validation failure with the command's stable diagnostic label.
fn inventory_error(error: String) -> CliError {
    CliError::failure(format!("oven closure inventory: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Write an exact two-version registry lock used by inventory tests.
    fn write_lock(root: &Path) -> TestResult {
        fs::write(
            root.join("Cargo.lock"),
            format!(
                "version = 4\n\n[[package]]\nname = \"same\"\nversion = \"1.0.0\"\nsource = \"registry+https://example.invalid/index\"\nchecksum = \"{}\"\n\n[[package]]\nname = \"same\"\nversion = \"2.0.0\"\nsource = \"registry+https://example.invalid/index\"\nchecksum = \"{}\"\n",
                "1".repeat(64),
                "2".repeat(64)
            ),
        )?;
        Ok(())
    }

    /// Write one typed harvest proposal with the requested selection and effects.
    // Each argument is an independent selected-unit fixture dimension kept visible at its four local call sites.
    #[allow(clippy::too_many_arguments)]
    fn write_proposal(
        harvest: &Path,
        lock_digest: &str,
        version: &str,
        checksum: char,
        features: &[&str],
        cfg: &[&str],
        profile: &str,
        suffix: &str,
    ) -> TestResult {
        let directory = harvest.join(format!("same-{version}-{profile}-{suffix}"));
        fs::create_dir_all(&directory)?;
        let proposal = serde_json::json!({
            "project": {"name": "same", "version": version},
            "source": {
                "registry": "https://example.invalid/index",
                "checksum": format!("sha256:{}", checksum.to_string().repeat(64))
            },
            "rust": {"facts": [{
                "toolchain": "rustc fixture",
                "target": "fixture-target",
                "profile": profile,
                "features": features,
                "cfg": cfg,
                "out": []
            }]},
            "evidence": {
                "method": "compatibility publisher observation",
                "rustc_identity": format!("sha256:{}", "a".repeat(64)),
                "host": "fixture-host",
                "hazards": [],
                "cargo_version": "cargo fixture",
                "cargo_lock_digest": lock_digest,
                "cargo_manifest_digest": format!("sha256:{}", "b".repeat(64))
            }
        });
        fs::write(directory.join(HARVEST_PROPOSAL_FILE), serde_json::to_vec(&proposal)?)?;
        Ok(())
    }

    /// Create the common inventory fixture and return its root, harvest, and lock digest.
    fn fixture() -> Result<(tempfile::TempDir, PathBuf, String), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        write_lock(root.path())?;
        let harvest = root.path().join("harvest");
        fs::create_dir(&harvest)?;
        let lock_digest = digest_bytes(&fs::read(root.path().join("Cargo.lock"))?);
        write_proposal(&harvest, &lock_digest, "2.0.0", '2', &["z"], &["cfg"], "release", "a")?;
        write_proposal(&harvest, &lock_digest, "1.0.0", '1', &[], &[], "debug", "b")?;
        Ok((root, harvest, lock_digest))
    }

    /// Build options for the common Incan-only fixture.
    fn options(root: &Path, harvest: &Path) -> OvenInventoryCommandOptions {
        OvenInventoryCommandOptions {
            incan_lock: root.join("Cargo.lock"),
            incan_harvest: harvest.to_path_buf(),
            incql_lock: None,
            incql_harvest: None,
            output: Some(root.join("inventory.json")),
            check: None,
        }
    }

    #[test]
    fn inventory_is_sorted_version_sensitive_and_keeps_empty_effects() -> TestResult {
        let (root, harvest, _) = fixture()?;
        let inventory = generate_inventory(&options(root.path(), &harvest))?;
        assert_eq!(
            inventory
                .bindings
                .iter()
                .map(|binding| binding.version.as_str())
                .collect::<Vec<_>>(),
            vec!["1.0.0", "2.0.0"]
        );
        assert_eq!(inventory.bindings[0].effects, vec![HarvestEffectClass::Empty]);
        assert_eq!(inventory.bindings[1].effects, vec![HarvestEffectClass::Cfg]);
        Ok(())
    }

    #[test]
    fn stale_lock_unknown_package_and_effect_drift_refuse() -> TestResult {
        let (root, harvest, lock_digest) = fixture()?;
        let generated = generate_inventory(&options(root.path(), &harvest))?;
        let expected = root.path().join("expected.json");
        fs::write(&expected, encode_inventory(&generated)?)?;

        let proposal = harvest.join("same-2.0.0-release-a/proposal.json");
        let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&proposal)?)?;
        value["rust"]["facts"][0]["cfg"] = serde_json::json!([]);
        fs::write(&proposal, serde_json::to_vec(&value)?)?;
        let changed = generate_inventory(&options(root.path(), &harvest))?;
        assert!(check_inventory(&expected, &changed).is_err());

        value["rust"]["facts"][0]["cfg"] = serde_json::json!(["cfg"]);
        value["evidence"]["cargo_lock_digest"] = serde_json::json!(format!("sha256:{}", "9".repeat(64)));
        fs::write(&proposal, serde_json::to_vec(&value)?)?;
        let stale = generate_inventory(&options(root.path(), &harvest))
            .err()
            .ok_or("stale lock was accepted")?;
        assert!(stale.contains("not supplied lock"), "{stale}");

        value["evidence"]["cargo_lock_digest"] = serde_json::json!(lock_digest);
        fs::write(&proposal, serde_json::to_vec(&value)?)?;
        write_proposal(&harvest, &lock_digest, "3.0.0", '3', &[], &[], "release", "missing")?;
        let unknown = generate_inventory(&options(root.path(), &harvest))
            .err()
            .ok_or("unknown package was accepted")?;
        assert!(unknown.contains("absent from lock"), "{unknown}");
        Ok(())
    }

    #[test]
    fn duplicate_bindings_and_unresolved_refusals_refuse() -> TestResult {
        let (root, harvest, lock_digest) = fixture()?;
        write_proposal(&harvest, &lock_digest, "1.0.0", '1', &[], &[], "debug", "duplicate")?;
        let duplicate = generate_inventory(&options(root.path(), &harvest))
            .err()
            .ok_or("duplicate binding was accepted")?;
        assert!(duplicate.contains("duplicate selected binding"), "{duplicate}");
        fs::remove_dir_all(harvest.join("same-1.0.0-debug-duplicate"))?;

        let refusal = serde_json::json!([{
            "package": "missing",
            "version": "1.0.0",
            "reason": "environment-observed",
            "detail": "SECRET"
        }]);
        fs::write(harvest.join("refusals-release.json"), serde_json::to_vec(&refusal)?)?;
        let unresolved = generate_inventory(&options(root.path(), &harvest))
            .err()
            .ok_or("unresolved refusal was accepted")?;
        assert!(unresolved.contains("unresolved selected bindings"), "{unresolved}");
        Ok(())
    }

    #[test]
    fn combined_inputs_are_inseparable_and_required_for_combined_check() -> TestResult {
        let (root, harvest, _) = fixture()?;
        let mut missing_pair = options(root.path(), &harvest);
        missing_pair.incql_lock = Some(root.path().join("Cargo.lock"));
        assert!(generate_inventory(&missing_pair).is_err());

        let mut combined_options = options(root.path(), &harvest);
        combined_options.incql_lock = Some(root.path().join("Cargo.lock"));
        combined_options.incql_harvest = Some(harvest.clone());
        let combined = generate_inventory(&combined_options)?;
        let expected = root.path().join("combined.json");
        fs::write(&expected, encode_inventory(&combined)?)?;
        let incan_only = generate_inventory(&options(root.path(), &harvest))?;
        let refused = check_inventory(&expected, &incan_only)
            .err()
            .ok_or("combined check was accepted")?;
        assert!(
            refused.to_string().contains("combined inventory checking requires"),
            "{refused}"
        );
        Ok(())
    }
}
